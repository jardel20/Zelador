//! Runner de testes: devolve o conteúdo das fixtures capturadas na máquina real.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use zelador_core::error::Result;
use zelador_core::runner::{CommandRunner, CommandSpec, Output};

pub fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn fixture(name: &str) -> String {
    fs::read_to_string(fixture_path(name)).unwrap_or_else(|e| panic!("fixture {name} ausente: {e}"))
}

/// Mapeia a linha de comando (`programa arg arg`) para a saída gravada.
#[derive(Default)]
pub struct FixtureRunner {
    outputs: HashMap<String, Output>,
    pub calls: RefCell<Vec<CommandSpec>>,
}

impl FixtureRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Saída vinda de um arquivo em `tests/fixtures`, com código 0.
    pub fn with_fixture(self, command: &str, file: &str) -> Self {
        self.with_output(command, &fixture(file), 0)
    }

    /// Saída escrita à mão. Use só para o que ainda não tem fixture real.
    pub fn with_output(mut self, command: &str, stdout: &str, code: i32) -> Self {
        self.outputs.insert(
            command.to_owned(),
            Output {
                stdout: stdout.to_owned(),
                stderr: String::new(),
                code: Some(code),
            },
        );
        self
    }
}

impl CommandRunner for FixtureRunner {
    fn run(&self, spec: &CommandSpec) -> Result<Output> {
        self.calls.borrow_mut().push(spec.clone());
        Ok(self
            .outputs
            .get(&spec.display())
            .cloned()
            .unwrap_or(Output {
                stdout: String::new(),
                stderr: format!("sem fixture para `{}`", spec.display()),
                code: Some(127),
            }))
    }
}

/// Linha de `ls -l --time-style=full-iso` sobre um link simbólico.
pub struct LsLink {
    pub name: String,
    pub target: String,
    pub epoch: i64,
}

pub fn parse_ls_links(text: &str) -> Vec<LsLink> {
    text.lines()
        .filter(|l| l.starts_with('l'))
        .map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            // perm size owner date time tz name -> target
            LsLink {
                name: f[6].to_owned(),
                target: f[8].to_owned(),
                epoch: epoch(f[3], f[4], f[5]),
            }
        })
        .collect()
}

fn epoch(date: &str, time: &str, tz: &str) -> i64 {
    let d: Vec<i64> = date.split('-').map(|n| n.parse().unwrap()).collect();
    let t: Vec<&str> = time.split(':').collect();
    let (h, m) = (t[0].parse::<i64>().unwrap(), t[1].parse::<i64>().unwrap());
    let s = t[2].split('.').next().unwrap().parse::<i64>().unwrap();
    // Dias desde 1970 (algoritmo de Howard Hinnant).
    let (y, mo, da) = (if d[1] <= 2 { d[0] - 1 } else { d[0] }, d[1], d[2]);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let sign = if tz.starts_with('-') { -1 } else { 1 };
    let off =
        sign * (tz[1..3].parse::<i64>().unwrap() * 3600 + tz[3..5].parse::<i64>().unwrap() * 60);
    days * 86_400 + h * 3600 + m * 60 + s - off
}
