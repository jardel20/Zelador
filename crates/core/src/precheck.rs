//! Antes de construir: quanto vai ser baixado e se cabe no disco.

use crate::backends::{BuildRequest, RebuildBackend};
use crate::diff::strip_ansi;
use crate::error::Result;
use crate::progress::BuildFailure;
use crate::runner::CommandRunner;
use crate::units::parse_iec_size;

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Margem fixa para o que o build gera além do download (logs, temporários, o resultado).
pub const MARGIN_BASE: u64 = 2 * GIB;
/// Cada derivação construída localmente soma esta margem...
pub const MARGIN_PER_DERIVATION: u64 = 512 * MIB;
/// ...até este limite: o tamanho de um build local não dá para prever.
pub const MARGIN_EXTRA_MAX: u64 = 8 * GIB;

/// O que o Nix diz que vai acontecer, lido do `nix build --dry-run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DownloadEstimate {
    pub paths_to_fetch: u64,
    pub download_bytes: u64,
    pub unpacked_bytes: u64,
    pub derivations_to_build: u64,
}

/// Lê as linhas `these 24 paths will be fetched (221.8 MiB download, 750.9 MiB unpacked):` e
/// `these 3 derivations will be built:`. Sem elas (tudo já está no store), tudo zero.
pub fn parse_dry_run(text: &str) -> DownloadEstimate {
    let mut estimate = DownloadEstimate::default();
    for line in strip_ansi(text).lines().map(str::trim) {
        if line.contains("will be fetched") {
            estimate.paths_to_fetch += leading_count(line);
            if let Some((download, unpacked)) = sizes_in_parentheses(line) {
                estimate.download_bytes += download;
                estimate.unpacked_bytes += unpacked;
            }
        } else if line.contains("will be built") {
            estimate.derivations_to_build += leading_count(line);
        }
    }
    estimate
}

/// `these 24 paths ...` → 24; `this path ...` → 1.
fn leading_count(line: &str) -> u64 {
    line.strip_prefix("these ")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
}

/// `(221.8 MiB download, 750.9 MiB unpacked)` → `(download, unpacked)` em bytes.
fn sizes_in_parentheses(line: &str) -> Option<(u64, u64)> {
    let inner = line.split_once('(')?.1.split_once(')')?.0;
    let mut parts = inner.split(", ");
    let size = |part: &str, suffix: &str| -> Option<u64> {
        let value = part.trim().strip_suffix(suffix)?.trim();
        u64::try_from(parse_iec_size(value)?).ok()
    };
    Some((
        size(parts.next()?, "download")?,
        size(parts.next()?, "unpacked")?,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskCheck {
    Enough {
        free: u64,
        needed: u64,
    },
    Insufficient {
        free: u64,
        needed: u64,
        margin: u64,
        estimate: DownloadEstimate,
    },
}

/// Espaço necessário: o que será descompactado mais a margem. Devolve `(total, margem)`.
pub fn required_bytes(estimate: &DownloadEstimate) -> (u64, u64) {
    let extra = estimate
        .derivations_to_build
        .saturating_mul(MARGIN_PER_DERIVATION)
        .min(MARGIN_EXTRA_MAX);
    let margin = MARGIN_BASE + extra;
    (estimate.unpacked_bytes.saturating_add(margin), margin)
}

pub fn check_disk(estimate: DownloadEstimate, free: u64) -> DiskCheck {
    let (needed, margin) = required_bytes(&estimate);
    if free >= needed {
        DiskCheck::Enough { free, needed }
    } else {
        DiskCheck::Insufficient {
            free,
            needed,
            margin,
            estimate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DryRun {
    Estimate(DownloadEstimate),
    /// A avaliação falhou antes de qualquer download: já é o erro do build.
    Failed(BuildFailure),
}

/// Roda `nix build --dry-run` (avalia a configuração, sem baixar nem construir).
pub fn run_dry_run(
    runner: &dyn CommandRunner,
    backend: &dyn RebuildBackend,
    request: &BuildRequest,
) -> Result<DryRun> {
    let spec = backend.dry_run_command(request);
    let output = runner.run(&spec)?;
    // O Nix escreve o relatório no stderr, em texto (sem `--log-format`).
    if output.code == Some(0) {
        return Ok(DryRun::Estimate(parse_dry_run(&output.stderr)));
    }
    let text = strip_ansi(&output.stderr);
    let (headline, details) = crate::progress::focus_on_cause(&text);
    Ok(DryRun::Failed(BuildFailure {
        headline,
        details,
        file: None,
        line: None,
        log_tail: Vec::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::NixBuild;
    use crate::runner::{CommandSpec, Output};
    use std::path::Path;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn download_real_do_dry_run() {
        let estimate = parse_dry_run(&fixture("dry-run-fetch.txt"));
        assert_eq!(estimate.paths_to_fetch, 24);
        assert_eq!(estimate.derivations_to_build, 0);
        assert_eq!(estimate.download_bytes, 232_574_157); // 221,8 MiB
        assert_eq!(estimate.unpacked_bytes, 787_375_718); // 750,9 MiB
    }

    #[test]
    fn linha_internal_json_tambem_e_lida() {
        // A fixture de internal-json é uma linha `@nix {"msg": "..."}`.
        let line = fixture("dry-run-fetch-internal-json.txt");
        let msg = line
            .split("\"msg\":\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(parse_dry_run(msg).paths_to_fetch, 24);
    }

    #[test]
    fn derivacao_a_construir() {
        let estimate = parse_dry_run(&fixture("dry-run-build.txt"));
        assert_eq!(estimate.derivations_to_build, 1);
        assert_eq!(estimate.download_bytes, 0);
        assert_eq!(
            parse_dry_run("these 3 derivations will be built:\n  /nix/store/a.drv\n")
                .derivations_to_build,
            3
        );
    }

    #[test]
    fn nada_a_fazer_e_tudo_zero() {
        assert_eq!(parse_dry_run(""), DownloadEstimate::default());
        assert_eq!(
            parse_dry_run("warning: Git tree is dirty\n"),
            DownloadEstimate::default()
        );
    }

    #[test]
    fn download_e_construcao_juntos() {
        let text = "this path will be fetched (1.0 MiB download, 3.0 MiB unpacked):\n  /nix/store/x\nthese 2 derivations will be built:\n  /nix/store/a.drv\n  /nix/store/b.drv\n";
        let estimate = parse_dry_run(text);
        assert_eq!(
            (estimate.paths_to_fetch, estimate.derivations_to_build),
            (1, 2)
        );
        assert_eq!(estimate.unpacked_bytes, 3 * MIB);
    }

    #[test]
    fn margem_cresce_com_as_derivacoes_ate_o_limite() {
        let none = DownloadEstimate {
            unpacked_bytes: GIB,
            ..DownloadEstimate::default()
        };
        assert_eq!(required_bytes(&none), (GIB + MARGIN_BASE, MARGIN_BASE));
        let four = DownloadEstimate {
            derivations_to_build: 4,
            ..none
        };
        assert_eq!(required_bytes(&four).1, MARGIN_BASE + 2 * GIB);
        let many = DownloadEstimate {
            derivations_to_build: 10_000,
            ..none
        };
        assert_eq!(required_bytes(&many).1, MARGIN_BASE + MARGIN_EXTRA_MAX);
    }

    #[test]
    fn bloqueia_quando_nao_cabe_e_libera_quando_cabe() {
        let estimate = DownloadEstimate {
            unpacked_bytes: 10 * GIB,
            ..DownloadEstimate::default()
        };
        // Precisa de 12 GiB (10 + 2 de margem).
        assert!(
            matches!(check_disk(estimate, 11 * GIB), DiskCheck::Insufficient { needed, margin, .. } if needed == 12 * GIB && margin == 2 * GIB)
        );
        assert!(matches!(
            check_disk(estimate, 12 * GIB),
            DiskCheck::Enough { .. }
        ));
        // Sem nada a baixar, só a margem fixa conta.
        assert!(matches!(
            check_disk(DownloadEstimate::default(), 3 * GIB),
            DiskCheck::Enough { .. }
        ));
        assert!(matches!(
            check_disk(DownloadEstimate::default(), GIB),
            DiskCheck::Insufficient { .. }
        ));
    }

    struct Fixed(Output);
    impl CommandRunner for Fixed {
        fn run(&self, spec: &CommandSpec) -> Result<Output> {
            assert!(
                spec.args.iter().any(|a| a == "--dry-run")
                    && spec.args.iter().any(|a| a == "--no-link")
            );
            assert!(
                !spec.args.iter().any(|a| a == "--out-link"),
                "um dry-run não cria link"
            );
            Ok(self.0.clone())
        }
    }

    fn request() -> BuildRequest {
        BuildRequest {
            flake: "/etc/nixos".into(),
            host: "h".into(),
            out_link: Path::new("/s/result").into(),
            reference_lock: Some("/s/novo.lock".into()),
        }
    }

    #[test]
    fn dry_run_ok_devolve_a_estimativa() {
        let runner = Fixed(Output {
            stdout: String::new(),
            stderr: fixture("dry-run-fetch.txt"),
            code: Some(0),
        });
        let DryRun::Estimate(estimate) = run_dry_run(&runner, &NixBuild, &request()).unwrap()
        else {
            panic!()
        };
        assert_eq!(estimate.paths_to_fetch, 24);
    }

    #[test]
    fn dry_run_com_erro_de_avaliacao_ja_vira_a_falha_do_build() {
        let stderr = "error:\n       … while calling 'head'\n\n       error:\n       Failed assertions:\n       - A opção `a.b' foi removida.\n";
        let runner = Fixed(Output {
            stdout: String::new(),
            stderr: stderr.into(),
            code: Some(1),
        });
        let DryRun::Failed(failure) = run_dry_run(&runner, &NixBuild, &request()).unwrap() else {
            panic!()
        };
        assert_eq!(failure.headline, "A opção `a.b' foi removida.");
        assert!(!failure.details.contains("while calling"));
    }
}
