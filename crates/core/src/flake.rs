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
    /// Origem legível quando não há ref: `owner/repo`, ou a URL.
    pub source: Option<String>,
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
        let reference = self
            .reference
            .as_deref()
            .or(self.source.as_deref())
            .unwrap_or(&self.kind);
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
    owner: Option<String>,
    repo: Option<String>,
    url: Option<String>,
}

pub fn parse_flake_metadata(json: &str) -> Result<FlakeMetadata> {
    let raw: RawMetadata =
        serde_json::from_str(json).map_err(|e| Error::json("flake metadata", e))?;
    Ok(FlakeMetadata {
        inputs: inputs_from_locks(&raw.locks)?,
        revision: raw.revision,
        dirty_revision: raw.dirty_revision,
    })
}

/// Inputs de um arquivo `flake.lock` (o formato do arquivo, sem o envelope do `flake metadata`).
pub fn parse_flake_lock(json: &str) -> Result<Vec<FlakeInput>> {
    let locks: RawLocks = serde_json::from_str(json).map_err(|e| Error::json("flake.lock", e))?;
    inputs_from_locks(&locks)
}

fn inputs_from_locks(locks: &RawLocks) -> Result<Vec<FlakeInput>> {
    let root_name = locks.root.as_deref().unwrap_or("root");
    let root = locks
        .nodes
        .get(root_name)
        .ok_or_else(|| Error::parse("flake.lock", "nó raiz ausente em nodes"))?;

    let mut inputs = Vec::new();
    for (name, target) in &root.inputs {
        let Some(node) = resolve(&locks.nodes, root_name, target, 0) else {
            continue;
        };
        let locked = node.locked.as_ref();
        inputs.push(FlakeInput {
            name: name.clone(),
            kind: locked
                .and_then(|l| l.kind.clone())
                .unwrap_or_else(|| "desconhecido".to_owned()),
            reference: node.original.as_ref().and_then(|o| o.reference.clone()),
            source: node
                .original
                .as_ref()
                .and_then(|o| match (&o.owner, &o.repo) {
                    (Some(owner), Some(repo)) => Some(format!("{owner}/{repo}")),
                    _ => o.url.clone(),
                }),
            rev: locked.and_then(|l| l.rev.clone()),
            last_modified: locked.and_then(|l| l.last_modified),
        });
    }
    Ok(inputs)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputChange {
    pub name: String,
    pub old_rev: Option<String>,
    pub new_rev: Option<String>,
    pub old_last_modified: Option<i64>,
    pub new_last_modified: Option<i64>,
}

impl InputChange {
    fn short(rev: &Option<String>) -> String {
        rev.as_deref()
            .map_or_else(|| "?".to_owned(), |r| r.get(..7).unwrap_or(r).to_owned())
    }

    /// `8f3c1a2 → 02c087f`.
    pub fn label(&self) -> String {
        format!(
            "{} → {}",
            Self::short(&self.old_rev),
            Self::short(&self.new_rev)
        )
    }
}

/// Inputs cuja revisão mudou entre dois locks (ou que apareceram ou sumiram).
pub fn diff_inputs(before: &[FlakeInput], after: &[FlakeInput]) -> Vec<InputChange> {
    let mut changes = Vec::new();
    for new in after {
        let old = before.iter().find(|i| i.name == new.name);
        if old.is_none_or(|old| old.rev != new.rev) {
            changes.push(InputChange {
                name: new.name.clone(),
                old_rev: old.and_then(|o| o.rev.clone()),
                new_rev: new.rev.clone(),
                old_last_modified: old.and_then(|o| o.last_modified),
                new_last_modified: new.last_modified,
            });
        }
    }
    changes
}

/// Referência de flake que fixa `input` em `rev`, para `--override-input`.
/// `None` quando o tipo do input não permite (ex.: `path`, `tarball`) ou a revisão é inválida.
pub fn pin_reference(input: &FlakeInput, rev: &str) -> Option<String> {
    // Só hexadecimal completo: a revisão entra numa referência de flake.
    if rev.len() != 40 || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let source = input.source.as_deref()?;
    match input.kind.as_str() {
        kind @ ("github" | "gitlab" | "sourcehut")
            if source.contains('/') && !source.contains("://") =>
        {
            Some(format!("{kind}:{source}/{rev}"))
        }
        "git" => {
            let url = source.strip_prefix("git+").unwrap_or(source);
            (url.contains("://") && !url.contains(['?', '#', ' ']))
                .then(|| format!("git+{url}?rev={rev}"))
        }
        _ => None,
    }
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

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn locks_reais_antes_depois_do_update_e_do_pin() {
        let before = parse_flake_lock(&fixture("lock-before.json")).unwrap();
        let after = parse_flake_lock(&fixture("lock-after-update.json")).unwrap();
        let pinned = parse_flake_lock(&fixture("lock-after-pin.json")).unwrap();

        // O update real mexeu só em dois inputs.
        let changes = diff_inputs(&before, &after);
        let names: Vec<_> = changes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["home-manager", "nixpkgs"]);
        let nixpkgs = changes.iter().find(|c| c.name == "nixpkgs").unwrap();
        assert_eq!(nixpkgs.label(), "02c087f → 3ec6363");
        assert!(nixpkgs.new_last_modified > nixpkgs.old_last_modified);

        // Fixar o nixpkgs na revisão antiga o tira da lista de mudanças; o resto continua.
        let after_pin = diff_inputs(&before, &pinned);
        assert_eq!(
            after_pin
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["home-manager"]
        );
        // E nada muda de um lock para ele mesmo.
        assert!(diff_inputs(&before, &before).is_empty());
    }

    #[test]
    fn referencia_de_fixacao() {
        let github = FlakeInput {
            name: "nixpkgs".into(),
            kind: "github".into(),
            reference: Some("nixos-unstable".into()),
            source: Some("NixOS/nixpkgs".into()),
            rev: None,
            last_modified: None,
        };
        let rev = "02c087f3fe673c9e317f206a627cb6b910336dbe";
        assert_eq!(
            pin_reference(&github, rev).as_deref(),
            Some("github:NixOS/nixpkgs/02c087f3fe673c9e317f206a627cb6b910336dbe")
        );
        // Revisão curta, com lixo ou com injeção de referência: recusada.
        for bad in [
            "02c087f",
            "",
            &rev.replace('0', "g"),
            &format!("{rev}?dir=x"),
            "../../x",
        ] {
            assert_eq!(pin_reference(&github, bad), None, "{bad:?}");
        }
        let git = FlakeInput {
            kind: "git".into(),
            source: Some("git+https://example.org/r.git".into()),
            ..github.clone()
        };
        assert_eq!(
            pin_reference(&git, rev).as_deref(),
            Some("git+https://example.org/r.git?rev=02c087f3fe673c9e317f206a627cb6b910336dbe")
        );
        let path = FlakeInput {
            kind: "path".into(),
            source: Some("/etc/x".into()),
            ..github
        };
        assert_eq!(pin_reference(&path, rev), None);
    }

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
