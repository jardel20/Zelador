//! Linhas `@nix {...}` do stderr com `--log-format internal-json`.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::diff::strip_ansi;

/// Tipos de atividade (`ActivityType` em `libutil/logging.hh` do Nix).
pub mod activity {
    pub const FILE_TRANSFER: u32 = 101;
    pub const BUILDS: u32 = 104;
    pub const BUILD: u32 = 105;
    pub const SUBSTITUTE: u32 = 108;
}

/// Tipos de resultado (`ResultType`).
pub mod result {
    pub const BUILD_LOG_LINE: u32 = 101;
    pub const SET_PHASE: u32 = 104;
    pub const PROGRESS: u32 = 105;
}

#[derive(Debug, Clone, PartialEq)]
pub enum NixEvent {
    Start {
        id: u64,
        kind: u32,
        parent: u64,
        level: u8,
        text: String,
        fields: Vec<Value>,
    },
    Stop {
        id: u64,
    },
    Result {
        id: u64,
        kind: u32,
        fields: Vec<Value>,
    },
    Msg {
        level: u8,
        msg: String,
        file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum RawEvent {
    Start {
        id: u64,
        #[serde(rename = "type")]
        kind: u32,
        #[serde(default)]
        parent: u64,
        #[serde(default)]
        level: u8,
        #[serde(default)]
        text: String,
        #[serde(default)]
        fields: Vec<Value>,
    },
    Stop {
        id: u64,
    },
    Result {
        id: u64,
        #[serde(rename = "type")]
        kind: u32,
        #[serde(default)]
        fields: Vec<Value>,
    },
    Msg {
        #[serde(default)]
        level: u8,
        #[serde(default)]
        msg: String,
        file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
}

/// Interpreta uma linha do stderr. Linhas sem o prefixo `@nix ` (saída comum) viram `None`,
/// assim como eventos de ação desconhecida: o Nix pode acrescentar ações novas.
pub fn parse_line(line: &str) -> Option<NixEvent> {
    let json = line.strip_prefix("@nix ")?;
    Some(match serde_json::from_str::<RawEvent>(json).ok()? {
        RawEvent::Start {
            id,
            kind,
            parent,
            level,
            text,
            fields,
        } => NixEvent::Start {
            id,
            kind,
            parent,
            level,
            text,
            fields,
        },
        RawEvent::Stop { id } => NixEvent::Stop { id },
        RawEvent::Result { id, kind, fields } => NixEvent::Result { id, kind, fields },
        RawEvent::Msg {
            level,
            msg,
            file,
            line,
            column,
        } => NixEvent::Msg {
            level,
            msg,
            file,
            line,
            column,
        },
    })
}

/// Contadores de `resProgress` da atividade `Builds`: construções e transferências.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counter {
    pub done: u64,
    pub expected: u64,
    pub running: u64,
    pub failed: u64,
}

impl Counter {
    pub fn fraction(&self) -> Option<f64> {
        (self.expected > 0).then(|| (self.done as f64 / self.expected as f64).min(1.0))
    }
}

/// Acumula eventos e mantém o progresso e as últimas linhas de log de cada build.
#[derive(Debug, Default)]
pub struct ProgressTracker {
    kinds: HashMap<u64, u32>,
    pub builds: Counter,
    pub downloads: Counter,
    log_tail: Vec<String>,
}

const LOG_TAIL_LINES: usize = 20;

impl ProgressTracker {
    pub fn push(&mut self, event: &NixEvent) {
        match event {
            NixEvent::Start { id, kind, .. } => {
                self.kinds.insert(*id, *kind);
            }
            NixEvent::Stop { id } => {
                self.kinds.remove(id);
            }
            NixEvent::Result { id, kind, fields } => match *kind {
                result::PROGRESS => {
                    let counter = fields_to_counter(fields);
                    match self.kinds.get(id) {
                        Some(&activity::BUILDS) => self.builds = counter,
                        Some(&activity::FILE_TRANSFER | &activity::SUBSTITUTE) => {
                            self.downloads = counter;
                        }
                        _ => {}
                    }
                }
                result::BUILD_LOG_LINE => {
                    if let Some(line) = fields.first().and_then(Value::as_str) {
                        self.log_tail.push(strip_ansi(line));
                        if self.log_tail.len() > LOG_TAIL_LINES {
                            self.log_tail.remove(0);
                        }
                    }
                }
                _ => {}
            },
            NixEvent::Msg { .. } => {}
        }
    }

    pub fn log_tail(&self) -> &[String] {
        &self.log_tail
    }
}

fn fields_to_counter(fields: &[Value]) -> Counter {
    let at = |i: usize| fields.get(i).and_then(Value::as_u64).unwrap_or(0);
    Counter {
        done: at(0),
        expected: at(1),
        running: at(2),
        failed: at(3),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildFailure {
    /// Primeira linha, sem cor: `error: Cannot build '...drv'.`
    pub headline: String,
    /// Mensagem completa, sem cor.
    pub details: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// Últimas linhas do log do builder.
    pub log_tail: Vec<String>,
}

/// Extrai o erro relevante: a última mensagem de nível 0 (erro). O arquivo e a linha vêm do
/// evento ou, se ausentes, de um trecho `at /caminho:linha:coluna` no texto.
pub fn extract_build_failure(events: &[NixEvent], log_tail: &[String]) -> Option<BuildFailure> {
    let (msg, file, line) = events.iter().rev().find_map(|event| match event {
        NixEvent::Msg {
            level: 0,
            msg,
            file,
            line,
            ..
        } => Some((msg, file, line)),
        _ => None,
    })?;
    let details = strip_ansi(msg);
    let (found_file, found_line) = find_location(&details);
    Some(BuildFailure {
        headline: details.lines().next().unwrap_or_default().trim().to_owned(),
        file: file.clone().or(found_file),
        line: line.or(found_line),
        log_tail: log_tail.to_vec(),
        details,
    })
}

/// Procura `at /caminho/arquivo.nix:12:3` e devolve `(arquivo, linha)`.
fn find_location(text: &str) -> (Option<String>, Option<u32>) {
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("at ") else {
            continue;
        };
        let token = rest.trim_end_matches(':');
        let mut parts = token.rsplitn(3, ':');
        let (last, middle, head) = (parts.next(), parts.next(), parts.next());
        // `caminho:linha:coluna` ou `caminho:linha`.
        let candidate = match (head, middle, last) {
            (Some(path), Some(line), Some(col))
                if line.parse::<u32>().is_ok() && col.parse::<u32>().is_ok() =>
            {
                Some((path, line))
            }
            (None, Some(path), Some(line)) if line.parse::<u32>().is_ok() => Some((path, line)),
            _ => None,
        };
        if let Some((path, line)) = candidate
            && (path.starts_with('/') || path.starts_with('«'))
        {
            return (Some(path.to_owned()), line.parse().ok());
        }
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignora_linhas_comuns() {
        assert_eq!(parse_line("building..."), None);
        assert_eq!(parse_line("@nix {\"action\":\"nova-acao\"}"), None);
        assert_eq!(parse_line("@nix lixo"), None);
    }

    #[test]
    fn progresso_dos_builds() {
        let mut tracker = ProgressTracker::default();
        for line in [
            r#"@nix {"action":"start","id":1,"level":0,"parent":0,"text":"","type":104}"#,
            r#"@nix {"action":"result","id":1,"type":105,"fields":[3,10,2,0]}"#,
        ] {
            tracker.push(&parse_line(line).unwrap());
        }
        assert_eq!(tracker.builds.fraction(), Some(0.3));
        assert_eq!(tracker.builds.running, 2);
    }

    #[test]
    fn localizacao_no_texto() {
        let text = "error: boom\n       at /etc/nixos/modules/a.nix:12:3:\n          11|";
        assert_eq!(
            find_location(text),
            (Some("/etc/nixos/modules/a.nix".to_owned()), Some(12))
        );
        assert_eq!(find_location("sem nada"), (None, None));
    }
}
