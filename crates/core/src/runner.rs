//! Execução de processos externos atrás de um trait, para trocar por fixtures nos testes.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, mpsc};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<String>,
    pub cwd: Option<PathBuf>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn stdin(mut self, input: impl Into<String>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Linha de comando legível, para logs e mensagens de erro.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Devolve o stdout se o comando terminou com código 0.
    pub fn into_stdout(self, spec: &CommandSpec) -> Result<String> {
        if self.success() {
            Ok(self.stdout)
        } else {
            Err(Error::Command {
                command: spec.display(),
                code: self.code,
                stderr: self.stderr.trim().to_owned(),
            })
        }
    }
}

pub trait CommandRunner {
    fn run(&self, spec: &CommandSpec) -> Result<Output>;
}

impl<R: CommandRunner + ?Sized> CommandRunner for &R {
    fn run(&self, spec: &CommandSpec) -> Result<Output> {
        (**self).run(spec)
    }
}

/// Executa processos de verdade. Código de saída diferente de zero não é erro aqui:
/// quem chama decide, porque algumas ferramentas usam o código como informação.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, spec: &CommandSpec) -> Result<Output> {
        tracing::debug!(command = %spec.display(), "executando");
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        let spawn_err = |source| Error::Spawn {
            program: spec.program.clone(),
            source,
        };
        let mut child = command.spawn().map_err(spawn_err)?;
        if let (Some(input), Some(mut stdin)) = (&spec.stdin, child.stdin.take()) {
            // Um EPIPE aqui só significa que o processo já terminou de ler.
            let _ = stdin.write_all(input.as_bytes());
        }
        let out = child.wait_with_output().map_err(spawn_err)?;
        Ok(Output {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        })
    }
}

// ── Streaming e cancelamento ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

/// Pede para um processo em execução parar. O cancelamento envia SIGINT, que o `nix` trata
/// encerrando as construções de forma limpa; o processo ainda precisa terminar sozinho.
#[derive(Debug, Default)]
pub struct CancelToken {
    cancelled: AtomicBool,
    pid: AtomicI32,
}

impl CancelToken {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if let Some(pid) = rustix::process::Pid::from_raw(pid) {
            // O processo pode já ter terminado: um erro aqui não muda nada.
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::INT);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn attach(&self, pid: u32) {
        self.pid
            .store(i32::try_from(pid).unwrap_or(0), Ordering::SeqCst);
        // Cancelado antes de o processo existir: interrompe assim que ele nasce.
        if self.is_cancelled() {
            self.cancel();
        }
    }
}

/// Roda um processo entregando cada linha de stdout e stderr assim que ela aparece.
pub trait StreamingRunner {
    /// Bloqueia até o processo terminar; chame de uma thread. `Output.stderr` guarda só o
    /// final (até [`STDERR_TAIL_BYTES`]), para não acumular logs enormes.
    fn run_streaming(
        &self,
        spec: &CommandSpec,
        cancel: Option<&CancelToken>,
        on_line: &mut dyn FnMut(StreamKind, &str),
    ) -> Result<Output>;
}

pub const STDERR_TAIL_BYTES: usize = 64 * 1024;

impl<R: StreamingRunner + ?Sized> StreamingRunner for &R {
    fn run_streaming(
        &self,
        spec: &CommandSpec,
        cancel: Option<&CancelToken>,
        on_line: &mut dyn FnMut(StreamKind, &str),
    ) -> Result<Output> {
        (**self).run_streaming(spec, cancel, on_line)
    }
}

impl StreamingRunner for SystemRunner {
    fn run_streaming(
        &self,
        spec: &CommandSpec,
        cancel: Option<&CancelToken>,
        on_line: &mut dyn FnMut(StreamKind, &str),
    ) -> Result<Output> {
        tracing::debug!(command = %spec.display(), "executando (streaming)");
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        let spawn_err = |source| Error::Spawn {
            program: spec.program.clone(),
            source,
        };
        let mut child = command.spawn().map_err(spawn_err)?;
        if let Some(cancel) = cancel {
            cancel.attach(child.id());
        }

        let (tx, rx) = mpsc::channel::<(StreamKind, String)>();
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_reader(stdout, StreamKind::Stdout, tx.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_reader(stderr, StreamKind::Stderr, tx.clone()));
        }
        drop(tx);

        let (mut stdout_text, mut stderr_tail) = (String::new(), String::new());
        for (kind, line) in rx {
            on_line(kind, &line);
            match kind {
                StreamKind::Stdout => {
                    stdout_text.push_str(&line);
                    stdout_text.push('\n');
                }
                StreamKind::Stderr => {
                    stderr_tail.push_str(&line);
                    stderr_tail.push('\n');
                    if stderr_tail.len() > STDERR_TAIL_BYTES * 2 {
                        let cut = stderr_tail.len() - STDERR_TAIL_BYTES;
                        let cut = (cut..stderr_tail.len())
                            .find(|i| stderr_tail.is_char_boundary(*i))
                            .unwrap_or(stderr_tail.len());
                        stderr_tail.drain(..cut);
                    }
                }
            }
        }
        for reader in readers {
            // Uma thread de leitura que pânica só perde linhas; o processo já terminou.
            let _ = reader.join();
        }
        let status = child.wait().map_err(spawn_err)?;
        Ok(Output {
            stdout: stdout_text,
            stderr: stderr_tail,
            code: status.code(),
        })
    }
}

fn spawn_reader(
    pipe: impl std::io::Read + Send + 'static,
    kind: StreamKind,
    tx: mpsc::Sender<(StreamKind, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buffer);
                    let line = line.trim_end_matches(['\n', '\r']).to_owned();
                    if tx.send((kind, line)).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn entrega_as_linhas_de_cada_fluxo_e_o_codigo_de_saida() {
        let spec = CommandSpec::new("sh").args(["-c", "echo um; echo dois >&2; echo tres; exit 3"]);
        let mut lines = Vec::new();
        let out = SystemRunner
            .run_streaming(&spec, None, &mut |kind, line| {
                lines.push((kind, line.to_owned()))
            })
            .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, "um\ntres\n");
        assert_eq!(out.stderr, "dois\n");
        assert!(lines.contains(&(StreamKind::Stderr, "dois".to_owned())));
        // A ordem dentro de cada fluxo é preservada.
        let stdout: Vec<_> = lines
            .iter()
            .filter(|(k, _)| *k == StreamKind::Stdout)
            .map(|(_, l)| l.as_str())
            .collect();
        assert_eq!(stdout, ["um", "tres"]);
    }

    #[test]
    fn cancelar_interrompe_o_processo_logo() {
        let token = CancelToken::new();
        let spec = CommandSpec::new("sh").args(["-c", "echo pronto; sleep 30"]);
        let started = Instant::now();
        let t = Arc::clone(&token);
        let mut seen = false;
        let out = SystemRunner
            .run_streaming(&spec, Some(&token), &mut |_, line| {
                if line == "pronto" && !seen {
                    seen = true;
                    // Cancela já depois da primeira linha, de outra thread.
                    let t = Arc::clone(&t);
                    std::thread::spawn(move || t.cancel());
                }
            })
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "não deveria esperar o sleep"
        );
        assert!(token.is_cancelled());
        assert_eq!(out.code, None, "morto por sinal, sem código de saída");
    }

    #[test]
    fn cancelado_antes_de_comecar_ainda_para() {
        let token = CancelToken::new();
        token.cancel();
        let spec = CommandSpec::new("sh").args(["-c", "sleep 30"]);
        let started = Instant::now();
        let out = SystemRunner
            .run_streaming(&spec, Some(&token), &mut |_, _| {})
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_ne!(out.code, Some(0));
    }

    #[test]
    fn programa_inexistente_vira_erro_de_spawn() {
        let spec = CommandSpec::new("/nao/existe/zelador");
        assert!(matches!(
            SystemRunner.run_streaming(&spec, None, &mut |_, _| {}),
            Err(Error::Spawn { .. })
        ));
    }
}
