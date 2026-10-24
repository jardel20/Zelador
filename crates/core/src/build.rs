//! Construção do sistema sem privilégio, com progresso real.

use std::path::PathBuf;

use crate::backends::{BuildRequest, RebuildBackend};
use crate::error::Result;
use crate::progress::{
    BuildFailure, Counter, NixEvent, ProgressTracker, activity, extract_build_failure, parse_line,
};
use crate::runner::{CancelToken, StreamKind, StreamingRunner};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPhase {
    /// Avaliando a configuração, antes de qualquer construção.
    Evaluating,
    /// Construindo e baixando.
    Building,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildProgress {
    pub phase: BuildPhase,
    pub builds: Counter,
    /// Caminhos baixados ou copiados.
    pub downloads: Counter,
    /// Última linha de log de um build, sem cores.
    pub last_log: Option<String>,
}

impl BuildProgress {
    /// Fração concluída de builds e downloads somados; `None` enquanto não há o que contar.
    pub fn fraction(&self) -> Option<f64> {
        let expected = self.builds.expected + self.downloads.expected;
        (expected > 0)
            .then(|| ((self.builds.done + self.downloads.done) as f64 / expected as f64).min(1.0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildOutcome {
    Built { out_path: PathBuf },
    Failed(BuildFailure),
    Cancelled,
}

/// Constrói o sistema e entrega o progresso a `on_progress` a cada mudança. Bloqueia até o fim:
/// chame de uma thread. Cancelar (`cancel`) interrompe o `nix` por SIGINT.
pub fn run_build(
    runner: &dyn StreamingRunner,
    backend: &dyn RebuildBackend,
    request: &BuildRequest,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(&BuildProgress),
) -> Result<BuildOutcome> {
    let spec = backend.build_command(request);
    let mut tracker = ProgressTracker::default();
    let mut messages: Vec<NixEvent> = Vec::new();
    let mut progress = BuildProgress {
        phase: BuildPhase::Evaluating,
        builds: Counter::default(),
        downloads: Counter::default(),
        last_log: None,
    };

    let output = runner.run_streaming(&spec, Some(cancel), &mut |kind, line| {
        if kind != StreamKind::Stderr {
            return;
        }
        let Some(event) = parse_line(line) else {
            return;
        };
        tracker.push(&event);
        match &event {
            NixEvent::Start { kind, .. } if *kind == activity::BUILDS => {
                progress.phase = BuildPhase::Building;
            }
            // Só mensagens importantes entram: o resto é ruído de avaliação (milhares de linhas).
            NixEvent::Msg { level, .. } if *level <= 1 => messages.push(event.clone()),
            _ => {}
        }
        let updated = BuildProgress {
            phase: progress.phase,
            builds: tracker.builds,
            downloads: tracker.downloads,
            last_log: tracker.log_tail().last().cloned(),
        };
        if updated != progress {
            progress = updated;
            on_progress(&progress);
        }
    })?;

    if cancel.is_cancelled() {
        return Ok(BuildOutcome::Cancelled);
    }
    if output.code == Some(0) {
        let out_path = output
            .stdout
            .lines()
            .map(str::trim)
            .rfind(|line| line.starts_with("/nix/store/"));
        return Ok(match out_path {
            Some(path) => BuildOutcome::Built {
                out_path: path.into(),
            },
            None => BuildOutcome::Failed(BuildFailure {
                headline: "O build terminou sem informar o resultado".to_owned(),
                details: output.stdout.trim().to_owned(),
                file: None,
                line: None,
                log_tail: tracker.log_tail().to_vec(),
            }),
        });
    }
    Ok(BuildOutcome::Failed(
        extract_build_failure(&messages, tracker.log_tail()).unwrap_or_else(|| BuildFailure {
            headline: format!("O build falhou (código {:?})", output.code),
            details: output.stderr.trim().to_owned(),
            file: None,
            line: None,
            log_tail: tracker.log_tail().to_vec(),
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::NixBuild;
    use crate::runner::{CancelToken, CommandSpec, Output};

    /// Reproduz um build gravado, linha a linha.
    struct Replay {
        stderr: String,
        stdout: String,
        code: Option<i32>,
        cancel_after_first: bool,
    }

    impl StreamingRunner for Replay {
        fn run_streaming(
            &self,
            _: &CommandSpec,
            cancel: Option<&CancelToken>,
            on_line: &mut dyn FnMut(StreamKind, &str),
        ) -> Result<Output> {
            for (index, line) in self.stderr.lines().enumerate() {
                on_line(StreamKind::Stderr, line);
                if self.cancel_after_first
                    && index == 0
                    && let Some(cancel) = cancel
                {
                    cancel.cancel();
                }
            }
            for line in self.stdout.lines() {
                on_line(StreamKind::Stdout, line);
            }
            Ok(Output {
                stdout: self.stdout.clone(),
                stderr: String::new(),
                code: self.code,
            })
        }
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn request() -> BuildRequest {
        BuildRequest {
            flake: "/etc/nixos".into(),
            host: "nixos".into(),
            out_link: "/s/result".into(),
            reference_lock: None,
        }
    }

    #[test]
    fn build_real_com_sucesso_devolve_o_caminho_e_o_progresso() {
        let runner = Replay {
            stderr: fixture("build-ok-internal-json.txt"),
            stdout: fixture("build-ok.stdout"),
            code: Some(0),
            cancel_after_first: false,
        };
        let mut updates = Vec::new();
        let outcome = run_build(
            &runner,
            &NixBuild,
            &request(),
            &CancelToken::default(),
            &mut |p| updates.push(p.clone()),
        )
        .unwrap();
        let BuildOutcome::Built { out_path } = outcome else {
            panic!("esperava Built")
        };
        assert!(
            out_path.to_string_lossy().ends_with("-zelador-ok"),
            "{out_path:?}"
        );
        assert!(!updates.is_empty());
        assert_eq!(updates.last().map(|p| p.phase), Some(BuildPhase::Building));
        let last = updates.last().unwrap();
        // O contador do próprio Nix termina em 1/2 mesmo num build de uma derivação só (fixture
        // real): a fração é aproximada e a UI marca 100% quando o processo termina com sucesso.
        assert_eq!((last.builds.done, last.builds.expected), (1, 2), "{last:?}");
        assert_eq!(last.fraction(), Some(0.5));
        assert!(
            updates
                .iter()
                .any(|p| p.last_log.as_deref() == Some("construindo"))
        );
    }

    #[test]
    fn erro_de_avaliacao_real_vira_falha_legivel() {
        let runner = Replay {
            stderr: fixture("build-eval-failure-internal-json.txt"),
            stdout: String::new(),
            code: Some(1),
            cancel_after_first: false,
        };
        let outcome = run_build(
            &runner,
            &NixBuild,
            &request(),
            &CancelToken::default(),
            &mut |_| {},
        )
        .unwrap();
        let BuildOutcome::Failed(failure) = outcome else {
            panic!("esperava Failed")
        };
        // A causa, não o traço de avaliação: o título é a primeira opção que deixou de existir
        // no nixpkgs novo e que a configuração ainda usa.
        assert!(
            failure.headline.contains("boot.initrd.systemd.enable"),
            "{}",
            failure.headline
        );
        assert!(failure.details.contains("boot.initrd.verbose"));
        assert!(failure.details.contains("Scripted initrd has been removed"));
        assert!(
            !failure.details.contains("while calling the"),
            "o traço de avaliação fica de fora"
        );
        assert!(!failure.details.contains('\u{1b}'));
    }

    #[test]
    fn cancelar_devolve_cancelled_mesmo_que_o_processo_saia_com_erro() {
        let runner = Replay {
            stderr: fixture("build-ok-internal-json.txt"),
            stdout: String::new(),
            code: None,
            cancel_after_first: true,
        };
        let outcome = run_build(
            &runner,
            &NixBuild,
            &request(),
            &CancelToken::default(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(outcome, BuildOutcome::Cancelled);
    }

    #[test]
    fn falha_sem_mensagem_estruturada_ainda_explica() {
        let runner = Replay {
            stderr: "lixo sem @nix\n".into(),
            stdout: String::new(),
            code: Some(137),
            cancel_after_first: false,
        };
        let outcome = run_build(
            &runner,
            &NixBuild,
            &request(),
            &CancelToken::default(),
            &mut |_| {},
        )
        .unwrap();
        let BuildOutcome::Failed(failure) = outcome else {
            panic!()
        };
        assert!(failure.headline.contains("137"));
    }

    #[test]
    fn fracao_soma_builds_e_downloads() {
        let progress = BuildProgress {
            phase: BuildPhase::Building,
            builds: Counter {
                done: 1,
                expected: 3,
                running: 1,
                failed: 0,
            },
            downloads: Counter {
                done: 5,
                expected: 7,
                running: 0,
                failed: 0,
            },
            last_log: None,
        };
        assert_eq!(progress.fraction(), Some(0.6));
        let nothing = BuildProgress {
            builds: Counter::default(),
            downloads: Counter::default(),
            ..progress
        };
        assert_eq!(nothing.fraction(), None);
    }
}
