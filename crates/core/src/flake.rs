//! Inputs do flake, lidos de `nix flake metadata --json`.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

pub const SECONDS_PER_DAY: i64 = 86_400;
/// Idade a partir da qual um input aparece em cor de aviso (configurável na UI).
pub const DEFAULT_STALE_DAYS: i64 = 14;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlakeInput {
    pub name: String,
    /// Tipo do fetcher (`github`, `git`, `path`...).
    pub kind: String,
    /// Ref pedida no `flake.nix` (`nixos-unstable`, `main`).
    pub reference: Option<String>,
    pub rev: Option<String>,
    /// Epoch, em segundos.
    pub last_modified: Option<i64>,
}

impl FlakeInput {
    pub fn short_rev(&self) -> Option<&str> {
        self.rev.as_deref().map(|rev| rev.get(..7).unwrap_or(rev))
    }

    /// `nixos-unstable · 8f3c1a2`, como na tela "Seu sistema".
    pub fn label(&self) -> String {
        let reference = self.reference.as_deref().unwrap_or(&self.kind);
        match self.short_rev() {
            Some(rev) => format!("{reference} · {rev}"),
            None => reference.to_owned(),
        }
    }

    pub fn age_days(&self, now: i64) -> Option<i64> {
        self.last_modified
            .map(|modified| (now - modified).max(0) / SECONDS_PER_DAY)
    }

    pub fn is_stale(&self, now: i64, threshold_days: i64) -> bool {
        self.age_days(now).is_some_and(|days| days > threshold_days)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlakeMetadata {
    pub inputs: Vec<FlakeInput>,
    /// Revisão git do próprio flake, quando a árvore está limpa.
    pub revision: Option<String>,
    /// Revisão com sufixo `-dirty`, quando há alterações não commitadas.
    pub dirty_revision: Option<String>,
}

#[derive(Deserialize)]
struct RawMetadata {
    #[serde(default)]
    revision: Option<String>,
    #[serde(default, rename = "dirtyRevision")]
    dirty_revision: Option<String>,
    locks: RawLocks,
}

#[derive(Deserialize)]
struct RawLocks {
    root: Option<String>,
    nodes: BTreeMap<String, RawNode>,
}

#[derive(Deserialize, Default)]
struct RawNode {
    #[serde(default)]
    inputs: BTreeMap<String, Value>,
    locked: Option<RawLocked>,
    original: Option<RawOriginal>,
}

#[derive(Deserialize)]
struct RawLocked {
    #[serde(rename = "type")]
    kind: Option<String>,
    rev: Option<String>,
    #[serde(rename = "lastModified")]
    last_modified: Option<i64>,
}

#[derive(Deserialize)]
struct RawOriginal {
    #[serde(rename = "ref")]
    reference: Option<String>,
}

pub fn parse_flake_metadata(json: &str) -> Result<FlakeMetadata> {
    let raw: RawMetadata =
        serde_json::from_str(json).map_err(|e| Error::json("flake metadata", e))?;
    let root_name = raw.locks.root.as_deref().unwrap_or("root");
    let root = raw
        .locks
        .nodes
        .get(root_name)
        .ok_or_else(|| Error::parse("flake metadata", "nó raiz ausente em locks.nodes"))?;

    let mut inputs = Vec::new();
    for (name, target) in &root.inputs {
        let Some(node) = resolve(&raw.locks.nodes, root_name, target, 0) else {
            continue;
        };
        let locked = node.locked.as_ref();
        inputs.push(FlakeInput {
            name: name.clone(),
            kind: locked
                .and_then(|l| l.kind.clone())
                .unwrap_or_else(|| "desconhecido".to_owned()),
            reference: node.original.as_ref().and_then(|o| o.reference.clone()),
            rev: locked.and_then(|l| l.rev.clone()),
            last_modified: locked.and_then(|l| l.last_modified),
        });
    }
    Ok(FlakeMetadata {
        inputs,
        revision: raw.revision,
        dirty_revision: raw.dirty_revision,
    })
}

/// Segue um input até o nó travado. O alvo é o nome de um nó ou um caminho
/// (`follows`) a partir da raiz.
fn resolve<'a>(
    nodes: &'a BTreeMap<String, RawNode>,
    root: &str,
    target: &Value,
    depth: u8,
) -> Option<&'a RawNode> {
    if depth > 16 {
        return None;
    }
    match target {
        Value::String(name) => nodes.get(name),
        Value::Array(path) => {
            let mut current = root.to_owned();
            for step in path {
                let name = step.as_str()?;
                let next = nodes.get(&current)?.inputs.get(name)?;
                match next {
                    Value::String(node) => current = node.clone(),
                    Value::Array(_) => return resolve(nodes, root, next, depth + 1),
                    _ => return None,
                }
            }
            nodes.get(&current)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLLOWS: &str = r#"{
      "locks": {"root":"root","version":7,"nodes":{
        "root":{"inputs":{"a":"a","b":["a","dep"]}},
        "a":{"inputs":{"dep":"dep"},"locked":{"type":"github","rev":"1234567890abcdef","lastModified":1000},"original":{"ref":"main"}},
        "dep":{"locked":{"type":"git","rev":"fedcba9876543210","lastModified":2000},"original":{}}
      }}}"#;

    #[test]
    fn segue_follows_ate_o_no_travado() {
        let meta = parse_flake_metadata(FOLLOWS).unwrap();
        let names: Vec<_> = meta.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(meta.inputs[1].rev.as_deref(), Some("fedcba9876543210"));
        assert_eq!(meta.inputs[0].label(), "main · 1234567");
        assert_eq!(meta.inputs[1].label(), "git · fedcba9");
    }

    #[test]
    fn idade_e_aviso() {
        let meta = parse_flake_metadata(FOLLOWS).unwrap();
        let input = &meta.inputs[0];
        let now = 1000 + 15 * SECONDS_PER_DAY;
        assert_eq!(input.age_days(now), Some(15));
        assert!(input.is_stale(now, DEFAULT_STALE_DAYS));
        assert!(!input.is_stale(1000 + 14 * SECONDS_PER_DAY, DEFAULT_STALE_DAYS));
    }

    #[test]
    fn json_invalido_vira_erro() {
        assert!(parse_flake_metadata("{").is_err());
    }
}
