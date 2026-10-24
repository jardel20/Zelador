//! Gerações do sistema e do Home Manager, lidas dos links de perfil.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::{Error, Result};

pub const SYSTEM_PROFILES_DIR: &str = "/nix/var/nix/profiles";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationKind {
    System,
    HomeManager,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub kind: GenerationKind,
    pub number: u64,
    pub store_path: PathBuf,
    /// Epoch, em segundos (mtime do link, que é quando a geração foi registrada).
    pub created: i64,
    /// Versão do kernel; só existe para gerações do sistema.
    pub kernel: Option<String>,
    pub is_current: bool,
    pub is_boot_default: bool,
}

/// `system-446-link` com prefixo `system` → `Some(446)`.
pub fn parse_generation_number(file_name: &str, profile: &str) -> Option<u64> {
    file_name
        .strip_prefix(profile)?
        .strip_prefix('-')?
        .strip_suffix("-link")?
        .parse()
        .ok()
}

/// `/nix/store/<hash>-linux-6.18.57/bzImage` → `6.18.57`.
pub fn kernel_version_from_path(path: &Path) -> Option<String> {
    path.components().find_map(|component| {
        let name = component.as_os_str().to_str()?;
        let rest = name.split_once("-linux-")?.1;
        rest.starts_with(|c: char| c.is_ascii_digit())
            .then(|| rest.to_owned())
    })
}

/// Lista as gerações de `profile` (`system` ou `home-manager`) em `dir`,
/// da mais nova para a mais antiga.
///
/// `current` é o caminho do store em uso agora (já resolvido).
pub fn list_generations(
    dir: &Path,
    profile: &str,
    kind: GenerationKind,
    current: Option<&Path>,
) -> Result<Vec<Generation>> {
    let boot_default = match kind {
        GenerationKind::System => fs::read_link(dir.join(profile))
            .ok()
            .and_then(|target| parse_generation_number(target.file_name()?.to_str()?, profile)),
        GenerationKind::HomeManager => None,
    };

    let mut generations = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let file_name = entry.file_name();
        let Some(number) = file_name
            .to_str()
            .and_then(|name| parse_generation_number(name, profile))
        else {
            continue;
        };
        let link = entry.path();
        let store_path = fs::read_link(&link).map_err(|e| Error::io(&link, e))?;
        let created = fs::symlink_metadata(&link)
            .and_then(|meta| meta.modified())
            .map_err(|e| Error::io(&link, e))?
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        let kernel = match kind {
            GenerationKind::System => fs::read_link(store_path.join("kernel"))
                .ok()
                .and_then(|k| kernel_version_from_path(&k)),
            GenerationKind::HomeManager => None,
        };
        generations.push(Generation {
            kind,
            number,
            is_current: current == Some(store_path.as_path()),
            is_boot_default: boot_default == Some(number),
            store_path,
            created,
            kernel,
        });
    }
    generations.sort_by_key(|g| std::cmp::Reverse(g.number));
    Ok(generations)
}

/// Tamanho incremental de cada geração: bytes dos caminhos que ela tem e que
/// nenhuma geração mais nova tem. `closures` vem da mais nova para a mais antiga.
/// Caminhos sem tamanho conhecido contam como zero.
pub fn incremental_sizes(
    closures: &[(u64, HashSet<String>)],
    sizes: &HashMap<String, u64>,
) -> Vec<(u64, u64)> {
    let mut seen: HashSet<&str> = HashSet::new();
    closures
        .iter()
        .map(|(number, paths)| {
            let added: u64 = paths
                .iter()
                .filter(|path| seen.insert(path.as_str()))
                .map(|path| sizes.get(path).copied().unwrap_or(0))
                .sum();
            (*number, added)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numero_da_geracao() {
        assert_eq!(
            parse_generation_number("system-446-link", "system"),
            Some(446)
        );
        assert_eq!(parse_generation_number("system", "system"), None);
        assert_eq!(parse_generation_number("system-x-link", "system"), None);
        assert_eq!(
            parse_generation_number("home-manager-5-link", "home-manager"),
            Some(5)
        );
        assert_eq!(parse_generation_number("profile-3-link", "system"), None);
    }

    #[test]
    fn versao_do_kernel() {
        let path = Path::new("/nix/store/maq0fxv0r4p9bac4haqy45c4vhlc33ag-linux-6.18.57/bzImage");
        assert_eq!(kernel_version_from_path(path).as_deref(), Some("6.18.57"));
        let path = Path::new("/nix/store/abc-linux-firmware-2026/x");
        assert_eq!(kernel_version_from_path(path), None);
    }

    #[test]
    fn incremental_conta_so_o_que_e_novo() {
        let set = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect();
        let closures = vec![
            (3, set(&["a", "b"])),
            (2, set(&["b", "c"])),
            (1, set(&["c", "d"])),
        ];
        let sizes = HashMap::from([
            ("a".to_owned(), 10),
            ("b".to_owned(), 20),
            ("c".to_owned(), 30),
            ("d".to_owned(), 40),
        ]);
        assert_eq!(
            incremental_sizes(&closures, &sizes),
            [(3, 30), (2, 30), (1, 40)]
        );
    }
}
