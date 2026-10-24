//! Execução de um [`Plan`]. A validação já aconteceu; aqui só se roda.

use std::io;
use std::process::{Command, Stdio};

use crate::plan::{Plan, Step};

/// `PATH` mínimo para os filhos; as ferramentas em si são chamadas por caminho absoluto.
const CHILD_PATH: &str = "/run/current-system/sw/bin";

pub trait Executor {
    /// Devolve o código de saída do processo.
    fn run(&mut self, step: &Step) -> io::Result<i32>;
}

/// Roda de verdade, com ambiente limpo e sem stdin. stdout e stderr são herdados, então a
/// UI recebe o progresso do filho.
#[derive(Debug, Default)]
pub struct ProcessExecutor;

impl Executor for ProcessExecutor {
    fn run(&mut self, step: &Step) -> io::Result<i32> {
        let status = Command::new(&step.program)
            .args(&step.args)
            .env_clear()
            .env("PATH", CHILD_PATH)
            .env("HOME", "/root")
            .envs(step.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .status()?;
        // Morte por sinal não tem código; 1 evita confundir com sucesso.
        Ok(status.code().unwrap_or(1))
    }
}

/// Antes de cada passo, escreve `@zelador {"step":i,"of":n,"program":"..."}` em `out` (o stderr),
/// para o app mostrar o progresso sem interpretar a saída dos programas. Não muda o que roda.
pub struct AnnouncingExecutor<E, W> {
    inner: E,
    out: W,
    total: usize,
    done: usize,
}

impl<E: Executor, W: std::io::Write> AnnouncingExecutor<E, W> {
    pub fn new(inner: E, out: W, total: usize) -> Self {
        Self {
            inner,
            out,
            total,
            done: 0,
        }
    }
}

impl<E: Executor, W: std::io::Write> Executor for AnnouncingExecutor<E, W> {
    fn run(&mut self, step: &Step) -> io::Result<i32> {
        self.done += 1;
        let program = step
            .program
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // O nome vem de caminhos fixos do helper; mesmo assim escapa o que o JSON exige.
        let program: String = program
            .chars()
            .flat_map(|c| match c {
                '"' | '\\' => vec!['\\', c],
                c if c.is_control() => vec![],
                c => vec![c],
            })
            .collect();
        // Falha ao anunciar não deve impedir o passo: o marcador é só informação.
        let _ = writeln!(
            self.out,
            "@zelador {{\"step\":{},\"of\":{},\"program\":\"{program}\"}}",
            self.done, self.total
        );
        let _ = self.out.flush();
        self.inner.run(step)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// O processo não pôde ser iniciado.
    Spawn { step: usize, message: String },
    /// O processo terminou com código diferente de zero.
    Exit { step: usize, code: i32 },
}

/// Executa os passos em ordem e para no primeiro que falhar.
pub fn run_plan(plan: &Plan, executor: &mut dyn Executor) -> Result<(), Failure> {
    for (index, step) in plan.steps.iter().enumerate() {
        match executor.run(step) {
            Ok(0) => {}
            Ok(code) => return Err(Failure::Exit { step: index, code }),
            Err(error) => {
                return Err(Failure::Spawn {
                    step: index,
                    message: format!("{}: {error}", step.program.display()),
                });
            }
        }
    }
    Ok(())
}
