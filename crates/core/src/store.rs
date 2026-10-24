//! Tamanhos do Nix store e estimativas de coleta de lixo.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathInfo {
    pub path: String,
    pub nar_size: u64,
    /// Só vem quando o comando foi chamado com `--closure-size`.
    pub closure_size: Option<u64>,
    pub references: Vec<String>,
}

#[derive(Deserialize)]
struct RawPathInfo {
    #[serde(default, rename = "narSize")]
    nar_size: u64,
    #[serde(default, rename = "closureSize")]
    closure_size: Option<u64>,
    #[serde(default)]
    references: Vec<String>,
}

/// `nix path-info --json`. O Nix 2.34 devolve um objeto indexado pelo caminho.
/// Caminhos inválidos aparecem com valor `null` e são ignorados.
pub fn parse_path_info(json: &str) -> Result<Vec<PathInfo>> {
    let raw: BTreeMap<String, Option<RawPathInfo>> =
        serde_json::from_str(json).map_err(|e| Error::json("nix path-info", e))?;
    Ok(raw
        .into_iter()
        .filter_map(|(path, info)| {
            let info = info?;
            Some(PathInfo {
                path,
                nar_size: info.nar_size,
                closure_size: info.closure_size,
                references: info.references,
            })
        })
        .collect())
}

/// `807 store paths would be deleted` → 807.
pub fn parse_gc_dry_run(text: &str) -> Result<u64> {
    text.lines()
        .find_map(|line| {
            let line = line.trim();
            line.ends_with("would be deleted")
                .then(|| line.split_whitespace().next()?.parse().ok())
                .flatten()
        })
        .ok_or_else(|| Error::parse("nix store gc --dry-run", text.trim().to_owned()))
}

/// Saída de `nix-store --gc --print-dead`: um caminho por linha.
pub fn parse_dead_paths(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("/nix/store/"))
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcEstimate {
    pub paths: u64,
    pub bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_info_ignora_nulos() {
        let json = r#"{"/nix/store/a-x":{"narSize":10,"closureSize":30,"references":["/nix/store/b-y"]},
                       "/nix/store/c-z":null}"#;
        let infos = parse_path_info(json).unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].closure_size, Some(30));
        assert_eq!(infos[0].references, ["/nix/store/b-y"]);
    }

    #[test]
    fn dry_run() {
        let text = "finding garbage collector roots...\n807 store paths would be deleted\n";
        assert_eq!(parse_gc_dry_run(text).unwrap(), 807);
        assert!(parse_gc_dry_run("nada").is_err());
    }

    #[test]
    fn caminhos_mortos() {
        let text = "finding...\n/nix/store/a-x\n  /nix/store/b-y  \nlixo\n";
        assert_eq!(parse_dead_paths(text), ["/nix/store/a-x", "/nix/store/b-y"]);
    }
}
