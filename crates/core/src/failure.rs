//! Ajuda para entender uma falha de build: qual arquivo seu é o culpado, qual derivação falhou
//! e o que provavelmente fazer.

use std::path::{Path, PathBuf};

use crate::diff::strip_ansi;

/// Causas comuns que merecem uma orientação própria.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureHint {
    DiskFull,
    Network,
}

pub fn hint_for(text: &str) -> Option<FailureHint> {
    let text = strip_ansi(text).to_lowercase();
    if text.contains("no space left on device") || text.contains("disk quota exceeded") {
        Some(FailureHint::DiskFull)
    } else if [
        "unable to download",
        "could not resolve host",
        "couldn't connect",
        "connection timed out",
        "failed to connect",
        "ssl connect error",
    ]
    .iter()
    .any(|needle| text.contains(needle))
    {
        Some(FailureHint::Network)
    } else {
        None
    }
}

/// A derivação (`.drv`) que falhou, de `Cannot build '/nix/store/...drv'`.
pub fn failed_derivation(text: &str) -> Option<String> {
    let text = strip_ansi(text);
    let after = text.split("Cannot build '").nth(1)?;
    let path = after.split('\'').next()?;
    (path.starts_with("/nix/store/") && path.ends_with(".drv")).then(|| path.to_owned())
}

/// Um arquivo do flake do usuário citado no erro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFile {
    /// Caminho na pasta real do flake (não a cópia do store).
    pub path: PathBuf,
    pub line: Option<u32>,
}

/// Procura no erro os arquivos que estão na cópia do flake no store (`flake_store_path`) e os
/// traduz para a pasta real (`flake_dir`). Sem repetir, na ordem em que aparecem.
pub fn user_files(text: &str, flake_store_path: &str, flake_dir: &Path) -> Vec<UserFile> {
    let text = strip_ansi(text);
    let prefix = format!("{}/", flake_store_path.trim_end_matches('/'));
    let mut found: Vec<UserFile> = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find(&prefix) {
        let after = &rest[start + prefix.len()..];
        let end = after
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '`' | '\'' | '"' | '»' | ')' | '(' | ',')
            })
            .unwrap_or(after.len());
        let token = &after[..end];
        // `modules/boot.nix:12:3` → arquivo e linha; um `:` final (de pontuação) é descartado.
        let mut parts = token.trim_end_matches(':').split(':');
        let relative = parts.next().unwrap_or_default();
        let line = parts.next().and_then(|n| n.parse::<u32>().ok());
        if !relative.is_empty() && !relative.contains("..") {
            let file = UserFile {
                path: flake_dir.join(relative),
                line,
            };
            if !found.iter().any(|known| known.path == file.path) {
                found.push(file);
            }
        }
        rest = &after[end..];
    }
    found
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
    fn arquivo_do_usuario_no_erro_real_de_avaliacao() {
        // Os dois `boot.initrd.*` vêm do mesmo arquivo da cópia do flake no store.
        let text = fixture("build-eval-failure-internal-json.txt");
        let files = user_files(
            &text,
            "/nix/store/dn5kr2iq5hzwhdinyglsvwlq4ih2xgib-source",
            Path::new("/etc/nixos"),
        );
        assert_eq!(
            files,
            [UserFile {
                path: "/etc/nixos/modules/boot.nix".into(),
                line: None
            }]
        );
    }

    #[test]
    fn linha_e_varios_arquivos_sem_repeticao() {
        let store = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-source";
        let text = format!(
            "erro em {store}/a/b.nix:12:3: e também em `{store}/c.nix' e de novo {store}/a/b.nix:12:3"
        );
        let files = user_files(&text, store, Path::new("/home/u/cfg"));
        assert_eq!(
            files,
            [
                UserFile {
                    path: "/home/u/cfg/a/b.nix".into(),
                    line: Some(12)
                },
                UserFile {
                    path: "/home/u/cfg/c.nix".into(),
                    line: None
                },
            ]
        );
    }

    #[test]
    fn nao_sai_da_pasta_do_flake_nem_pega_outro_store() {
        let store = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-source";
        assert!(user_files(&format!("{store}/../etc/passwd"), store, Path::new("/x")).is_empty());
        assert!(user_files("/nix/store/bbbb-source/x.nix", store, Path::new("/x")).is_empty());
        assert!(user_files("sem nada", store, Path::new("/x")).is_empty());
    }

    #[test]
    fn derivacao_que_falhou_vem_do_erro_real() {
        // O texto decodificado, como o código real o recebe (a fixture é JSON com escapes).
        let events: Vec<_> = fixture("build-fail-internal-json.txt")
            .lines()
            .filter_map(crate::progress::parse_line)
            .collect();
        let failure = crate::progress::extract_build_failure(&events, &[]).unwrap();
        let drv = failed_derivation(&failure.details).unwrap();
        assert!(drv.ends_with("-zelador-falha.drv"), "{drv}");
        assert_eq!(failed_derivation("error: outra coisa"), None);
        assert_eq!(failed_derivation("Cannot build '/tmp/x.txt'"), None);
    }

    #[test]
    fn dicas_para_disco_cheio_e_rede() {
        assert_eq!(
            hint_for("error: writing to file: No space left on device"),
            Some(FailureHint::DiskFull)
        );
        assert_eq!(
            hint_for(
                "error: unable to download 'https://cache.nixos.org/x': Could not resolve host"
            ),
            Some(FailureHint::Network)
        );
        assert_eq!(
            hint_for("\u{1b}[31;1merror:\u{1b}[0m Connection timed out"),
            Some(FailureHint::Network)
        );
        assert_eq!(hint_for("error: builder failed with exit code 1"), None);
    }
}
