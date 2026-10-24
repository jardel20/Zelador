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
