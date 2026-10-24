//! Resumo de mudanças entre duas closures, de `nix store diff-closures` (ou `nvd`).

use crate::units::parse_iec_size;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    pub name: String,
    /// Versões antes. `None` = o pacote não existia.
    pub old: Option<Vec<String>>,
    /// Versões depois. `None` = o pacote foi removido.
    pub new: Option<Vec<String>>,
    /// Variação de tamanho em bytes, quando o Nix informa.
    pub size_delta: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffSummary {
    pub updated: usize,
    pub added: usize,
    pub removed: usize,
    pub size_delta: i64,
}

impl DiffEntry {
    pub fn is_added(&self) -> bool {
        self.old.is_none() && self.new.is_some()
    }

    pub fn is_removed(&self) -> bool {
        self.old.is_some() && self.new.is_none()
    }

    pub fn is_updated(&self) -> bool {
        matches!((&self.old, &self.new), (Some(old), Some(new)) if old != new)
    }
}

pub fn summarize(entries: &[DiffEntry]) -> DiffSummary {
    DiffSummary {
        updated: entries.iter().filter(|e| e.is_updated()).count(),
        added: entries.iter().filter(|e| e.is_added()).count(),
        removed: entries.iter().filter(|e| e.is_removed()).count(),
        size_delta: entries.iter().filter_map(|e| e.size_delta).sum(),
    }
}

/// Remove sequências ANSI (`ESC [ ... letra`).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn parse_diff_closures(text: &str) -> Vec<DiffEntry> {
    strip_ansi(text).lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<DiffEntry> {
    let (name, rest) = line.split_once(": ")?;
    let name = name.trim();
    if name.is_empty() || name.contains(' ') {
        return None;
    }
    let (versions, size_delta) = match rest.rsplit_once(", ") {
        Some((head, tail)) => match parse_iec_size(tail) {
            Some(size) => (head, Some(size)),
            None => (rest, None),
        },
        None => match parse_iec_size(rest) {
            Some(size) => ("", Some(size)),
            None => (rest, None),
        },
    };
    let (old, new) = match versions.split_once(" → ") {
        Some((old, new)) => (parse_versions(old), parse_versions(new)),
        // Só o tamanho mudou: o pacote existe dos dois lados.
        None if versions.is_empty() => (Some(Vec::new()), Some(Vec::new())),
        None => return None,
    };
    Some(DiffEntry {
        name: name.to_owned(),
        old,
        new,
        size_delta,
    })
}

/// `∅` = ausente; `ε` = presente com versão vazia.
fn parse_versions(text: &str) -> Option<Vec<String>> {
    let text = text.trim();
    if text == "∅" {
        return None;
    }
    Some(
        text.split(", ")
            .map(|v| {
                if v == "ε" {
                    String::new()
                } else {
                    v.to_owned()
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
aalib: 1.4rc5 → ∅, \u{1b}[32;1m-111.7 KiB\u{1b}[0m
ada: \u{1b}[32;1m-448.1 KiB\u{1b}[0m
appimage-run: ε → 0, 0-fhsenv, \u{1b}[31;1m7.0 MiB\u{1b}[0m
aws-c-auth: ∅ → 1.0.0, \u{1b}[31;1m46.6 KiB\u{1b}[0m
bash: 5.3p15 → 5.3p20
nome com espaço: lixo
";

    #[test]
    fn linhas() {
        let entries = parse_diff_closures(SAMPLE);
        assert_eq!(entries.len(), 5);
        assert!(entries[0].is_removed());
        assert_eq!(entries[0].size_delta, Some(-114_381));
        assert!(!entries[1].is_updated() && entries[1].old.is_some());
        assert_eq!(entries[2].new, Some(vec!["0".into(), "0-fhsenv".into()]));
        assert!(entries[2].is_updated());
        assert!(entries[3].is_added());
        assert_eq!(entries[4].size_delta, None);
    }

    #[test]
    fn resumo() {
        let summary = summarize(&parse_diff_closures(SAMPLE));
        assert_eq!((summary.updated, summary.added, summary.removed), (2, 1, 1));
    }
}
