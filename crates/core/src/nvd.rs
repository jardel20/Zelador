//! Comparação de sistemas pelo `nvd`, quando o usuário o tem e o prefere.
//!
//! O `nvd` não informa o tamanho de cada pacote, só o uso de disco total do fecho.

use std::path::PathBuf;

use crate::diff::{DiffEntry, strip_ansi};
use crate::units::parse_iec_size;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvdDiff {
    pub entries: Vec<DiffEntry>,
    /// Variação do uso de disco do fecho, em bytes (`disk usage +625.8MiB`).
    pub disk_usage_delta: Option<i64>,
}

/// Procura um executável nos diretórios do `PATH`.
pub fn find_in_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

/// Lê a saída de `nvd --color never diff <antes> <depois>`.
pub fn parse_nvd(text: &str) -> NvdDiff {
    let text = strip_ansi(text);
    let mut entries = Vec::new();
    let mut section = Section::None;
    let mut disk_usage_delta = None;

    for line in text.lines() {
        let trimmed = line.trim();
        match trimmed {
            "Version changes:" => section = Section::Changes,
            "Added packages:" => section = Section::Added,
            "Removed packages:" => section = Section::Removed,
            _ if trimmed.starts_with("Closure size:") => {
                disk_usage_delta = disk_usage(trimmed);
                section = Section::None;
            }
            _ if trimmed.starts_with('[') => {
                if let Some(entry) = parse_entry(trimmed, section) {
                    entries.push(entry);
                }
            }
            _ => {}
        }
    }
    NvdDiff {
        entries,
        disk_usage_delta,
    }
}

#[derive(Clone, Copy)]
enum Section {
    None,
    Changes,
    Added,
    Removed,
}

/// `[C.]  #003  aws-c-auth   0.9.1 x2 -> 0.9.1, 1.0.0`
fn parse_entry(line: &str, section: Section) -> Option<DiffEntry> {
    // Pula o `[..]` e o `#NNN`.
    let after_status = line.split_once(']')?.1.trim_start();
    let after_number = after_status.split_once(char::is_whitespace)?.1.trim_start();
    let (name, versions) = after_number.split_once(char::is_whitespace)?;
    let versions = versions.trim();
    if name.is_empty() {
        return None;
    }
    let (old, new) = match section {
        Section::Changes => {
            let (old, new) = versions.split_once(" -> ")?;
            (Some(parse_versions(old)), Some(parse_versions(new)))
        }
        Section::Added => (None, Some(parse_versions(versions))),
        Section::Removed => (Some(parse_versions(versions)), None),
        Section::None => return None,
    };
    Some(DiffEntry {
        name: name.to_owned(),
        old,
        new,
        size_delta: None,
    })
}

/// `0.9.1 x2, 1.0.0` → `["0.9.1", "1.0.0"]`; `<none>` → uma versão vazia (o pacote existe).
fn parse_versions(text: &str) -> Vec<String> {
    text.split(", ")
        .map(|version| {
            let version = version.trim();
            // Remove o contador de repetições (` x2`).
            let version = match version.rsplit_once(" x") {
                Some((base, count))
                    if count.chars().all(|c| c.is_ascii_digit()) && !count.is_empty() =>
                {
                    base
                }
                _ => version,
            };
            if version == "<none>" {
                String::new()
            } else {
                version.to_owned()
            }
        })
        .collect()
}

/// `... disk usage +625.8MiB).` → bytes.
fn disk_usage(line: &str) -> Option<i64> {
    let value = line
        .split("disk usage ")
        .nth(1)?
        .trim_end_matches([')', '.']);
    let unit_at = value.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = value.split_at(unit_at);
    parse_iec_size(&format!("{} {unit}", number.trim_start_matches('+'))).map(|bytes| {
        if number.starts_with('-') {
            -bytes.abs()
        } else {
            bytes
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real() -> NvdDiff {
        let text = std::fs::read_to_string(format!(
            "{}/tests/fixtures/nvd-diff-444-446.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        parse_nvd(&text)
    }

    #[test]
    fn saida_real_do_nvd() {
        let diff = real();
        let find = |name: &str| diff.entries.iter().find(|e| e.name == name).unwrap();
        let updated = diff.entries.iter().filter(|e| e.is_updated()).count();
        let added = diff.entries.iter().filter(|e| e.is_added()).count();
        let removed = diff.entries.iter().filter(|e| e.is_removed()).count();
        assert_eq!((added, removed), (6, 21));
        assert!(updated > 150, "{updated}");

        // `[C.]`: várias versões antes e depois.
        let ada = find("ada");
        assert_eq!(
            ada.old.as_deref(),
            Some(&["3.4.4".to_owned(), "4.0.0".into(), "4.0.0-dev".into()][..])
        );
        assert_eq!(
            ada.new.as_deref(),
            Some(&["3.4.4".to_owned(), "4.0.0".into()][..])
        );
        // ` x2` é só contagem.
        assert_eq!(
            find("aws-c-auth").old.as_deref(),
            Some(&["0.9.1".to_owned()][..])
        );
        // `<none>` é "existia, sem versão".
        assert_eq!(
            find("appimage-run").old.as_deref(),
            Some(&[String::new()][..])
        );
        assert!(find("appimage-run").is_updated());
        // Atualização simples e novos/removidos.
        assert_eq!(
            find("vscode").new.as_deref(),
            Some(&["1.142.0".to_owned()][..])
        );
        assert!(find("firefox").is_added() && find("aalib").is_removed());
        // O nvd não dá tamanho por pacote.
        assert!(diff.entries.iter().all(|e| e.size_delta.is_none()));
    }

    #[test]
    fn uso_de_disco_do_fecho() {
        let diff = real();
        // `disk usage +625.8MiB`
        assert_eq!(diff.disk_usage_delta, Some(656_198_861));
        assert_eq!(
            disk_usage(
                "Closure size: 1 -> 2 (1 paths added, 0 removed, delta +1, disk usage -3.0KiB)."
            ),
            Some(-3072)
        );
        assert_eq!(disk_usage("sem nada"), None);
    }

    #[test]
    fn linhas_fora_de_secao_ou_malformadas_sao_ignoradas() {
        let diff = parse_nvd(
            "<<< a\n>>> b\n[U.]  #1  solto  1 -> 2\nVersion changes:\n[U.]  #1  sem-seta  1.0\n[U.]  #2  ok  1 -> 2\n",
        );
        assert_eq!(diff.entries.len(), 1);
        assert_eq!(diff.entries[0].name, "ok");
    }
}
