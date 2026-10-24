//! Execução de processos externos atrás de um trait, para trocar por fixtures nos testes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
