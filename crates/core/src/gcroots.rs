//! GC roots esquecidos dentro de `$HOME` (`result`, `.direnv`).
//!
//! Lê `/nix/var/nix/gcroots/auto` diretamente em vez de rodar
//! `nix-store --gc --print-roots`, que apaga links obsoletos como efeito colateral.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::{Error, Result};
use crate::flake::SECONDS_PER_DAY;

pub const AUTO_GCROOTS_DIR: &str = "/nix/var/nix/gcroots/auto";

/// Pastas de `$HOME` cujos links o próprio Nix e o Home Manager gerenciam.
const MANAGED_DIRS: [&str; 4] = [".local/state", ".cache", ".nix-profile", ".nix-defexpr"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootCandidate {
    /// Link no `$HOME` do usuário que o GC enxerga como raiz.
    pub link: PathBuf,
    pub store_path: PathBuf,
    /// Epoch, em segundos.
    pub mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgottenRoot {
    /// O próprio link, ou a pasta `.direnv` que agrupa vários.
    pub path: PathBuf,
    pub store_paths: Vec<PathBuf>,
    pub age_days: i64,
    /// Desmarcado por padrão quando criado há menos de 1 dia.
    pub selected_by_default: bool,
}

/// Filtra e agrupa os candidatos. `.direnv` vira uma linha só, com a idade do link mais novo.
pub fn forgotten_roots(candidates: &[RootCandidate], home: &Path, now: i64) -> Vec<ForgottenRoot> {
    let mut groups: BTreeMap<PathBuf, (Vec<PathBuf>, i64)> = BTreeMap::new();
    for candidate in candidates {
        let Ok(relative) = candidate.link.strip_prefix(home) else {
            continue;
        };
        if MANAGED_DIRS.iter().any(|dir| relative.starts_with(dir)) {
            continue;
        }
        let key = direnv_dir(&candidate.link).unwrap_or_else(|| candidate.link.clone());
        let entry = groups.entry(key).or_insert((Vec::new(), i64::MIN));
        entry.0.push(candidate.store_path.clone());
        entry.1 = entry.1.max(candidate.mtime);
    }
    groups
        .into_iter()
        .map(|(path, (store_paths, newest))| {
            let age = (now - newest).max(0);
            ForgottenRoot {
                path,
                store_paths,
                age_days: age / SECONDS_PER_DAY,
                selected_by_default: age >= SECONDS_PER_DAY,
            }
        })
        .collect()
}

fn direnv_dir(link: &Path) -> Option<PathBuf> {
    let mut dir = PathBuf::new();
    for component in link.components() {
        dir.push(component);
        if component.as_os_str() == ".direnv" {
            return Some(dir);
        }
    }
    None
}

/// Lê os links automáticos do Nix. Entradas que apontam para fora do store ou
/// para arquivos que não existem mais são ignoradas.
pub fn read_auto_roots(dir: &Path) -> Result<Vec<RootCandidate>> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let Ok(link) = fs::read_link(entry.path()) else {
            continue;
        };
        let Ok(store_path) = fs::read_link(&link) else {
            continue;
        };
        if !store_path.starts_with("/nix/store") {
            continue;
        }
        let Ok(modified) = fs::symlink_metadata(&link).and_then(|m| m.modified()) else {
            continue;
        };
        let mtime = modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        candidates.push(RootCandidate {
            link,
            store_path,
            mtime,
        });
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(link: &str, mtime: i64) -> RootCandidate {
        RootCandidate {
            link: link.into(),
            store_path: format!("/nix/store/h-{}", link.len()).into(),
            mtime,
        }
    }

    #[test]
    fn agrupa_direnv_e_marca_recentes() {
        let now = 100 * SECONDS_PER_DAY;
        let candidates = [
            candidate("/home/u/proj/result", now - 74 * SECONDS_PER_DAY),
            candidate(
                "/home/u/src/app/.direnv/flake-profile-a",
                now - 41 * SECONDS_PER_DAY,
            ),
            candidate(
                "/home/u/src/app/.direnv/flake-profile-b",
                now - 40 * SECONDS_PER_DAY,
            ),
            candidate("/home/u/nixos-config/result", now - 60),
        ];
        let roots = forgotten_roots(&candidates, Path::new("/home/u"), now);
        assert_eq!(roots.len(), 3);
        let direnv = roots
            .iter()
            .find(|r| r.path == Path::new("/home/u/src/app/.direnv"))
            .unwrap();
        assert_eq!(direnv.store_paths.len(), 2);
        assert_eq!(direnv.age_days, 40);
        let recent = roots
            .iter()
            .find(|r| r.path == Path::new("/home/u/nixos-config/result"))
            .unwrap();
        assert!(!recent.selected_by_default);
        assert!(roots.iter().filter(|r| r.selected_by_default).count() == 2);
    }

    #[test]
    fn ignora_links_gerenciados_e_de_fora_do_home() {
        let candidates = [
            candidate("/home/u/.local/state/nix/profiles/home-manager-1-link", 0),
            candidate("/home/u/.cache/nix/flake-registry.json", 0),
            candidate("/home/outro/result", 0),
            candidate("/tmp/nh-x/result", 0),
        ];
        assert!(forgotten_roots(&candidates, Path::new("/home/u"), 10).is_empty());
    }
}
