//! Ativação do sistema pelo helper, com o progresso que ele anuncia.

use std::path::Path;

use serde::Deserialize;

use crate::cleanup::HelperClient;
use crate::error::Result;
use crate::runner::{CommandSpec, StreamKind, StreamingRunner};

/// Como efetivar o sistema construído. Cada modo é um argumento do `activate` do helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// No próximo reinício.
    Boot,
    /// Agora.
    Switch,
    /// Só até reiniciar (não registra uma geração).
    Test,
}

impl ApplyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ApplyMode::Boot => "boot",
            ApplyMode::Switch => "switch",
            ApplyMode::Test => "test",
        }
    }

    /// Quantos passos o helper anuncia: registrar a geração (exceto em `test`) e ativar.
    pub fn helper_steps(self) -> u32 {
        if self == ApplyMode::Test { 1 } else { 2 }
    }
}

pub fn activate_args(system: &Path, mode: ApplyMode) -> Vec<String> {
    vec![
        "activate".to_owned(),
        system.to_string_lossy().into_owned(),
        mode.as_str().to_owned(),
    ]
}

/// Linha estruturada que o helper escreve no stderr antes de cada passo.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HelperMarker {
    pub step: u32,
    pub of: u32,
    /// Nome do programa do passo (`nix-env`, `systemd-run`...).
    pub program: String,
}

pub fn parse_marker(line: &str) -> Option<HelperMarker> {
    serde_json::from_str(line.strip_prefix("@zelador ")?).ok()
}

/// Fases reconhecíveis na saída do `switch-to-configuration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationStage {
    ActivatingConfiguration,
    StoppingUnits,
    StartingUnits,
    RestartingUnits,
    UpdatingBootMenu,
    InstallingBootLoader,
}

/// Reconhece a fase a partir de uma linha do log. Linhas que não casam não são erro: o log
/// completo continua disponível.
pub fn stage_from_log(line: &str) -> Option<ActivationStage> {
    let text = line.trim().to_lowercase();
    if text.starts_with("activating the configuration") {
        Some(ActivationStage::ActivatingConfiguration)
    } else if text.starts_with("stopping the following units") {
        Some(ActivationStage::StoppingUnits)
    } else if text.starts_with("starting the following units") {
        Some(ActivationStage::StartingUnits)
    } else if text.starts_with("restarting the following units") {
        Some(ActivationStage::RestartingUnits)
    } else if text.starts_with("updating grub") || text.starts_with("updating systemd-boot") {
        Some(ActivationStage::UpdatingBootMenu)
    } else if text.starts_with("installing the") && text.contains("boot loader")
        || text.starts_with("installing systemd-boot")
    {
        Some(ActivationStage::InstallingBootLoader)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationEvent {
    /// O helper começou um passo.
    Step(HelperMarker),
    Stage(ActivationStage),
    /// Qualquer outra linha, para o log.
    Log(String),
}

impl<R: StreamingRunner> HelperClient<R> {
    /// Ativa `system` e entrega os eventos conforme chegam. Bloqueia até o fim: chame de uma
    /// thread. A ativação não é cancelável depois de iniciada.
    pub fn activate(
        &self,
        system: &Path,
        mode: ApplyMode,
        on_event: &mut dyn FnMut(ActivationEvent),
    ) -> Result<()> {
        let args = activate_args(system, mode);
        let spec: CommandSpec = self.command(&args);
        let output = self
            .runner()
            .run_streaming(&spec, None, &mut |kind, line| {
                if kind == StreamKind::Stderr
                    && let Some(marker) = parse_marker(line)
                {
                    return on_event(ActivationEvent::Step(marker));
                }
                if let Some(stage) = stage_from_log(line) {
                    on_event(ActivationEvent::Stage(stage));
                }
                on_event(ActivationEvent::Log(line.to_owned()));
            })?;
        output.into_stdout(&spec).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{CancelToken, Output};
    use std::path::PathBuf;

    #[test]
    fn modos_e_argumentos_do_helper() {
        assert_eq!(
            activate_args(Path::new("/nix/store/x-sys"), ApplyMode::Boot),
            ["activate", "/nix/store/x-sys", "boot"]
        );
        assert_eq!(ApplyMode::Test.as_str(), "test");
        assert_eq!(
            (
                ApplyMode::Test.helper_steps(),
                ApplyMode::Switch.helper_steps()
            ),
            (1, 2)
        );
    }

    #[test]
    fn marcador_do_helper() {
        assert_eq!(
            parse_marker(r#"@zelador {"step":1,"of":2,"program":"nix-env"}"#),
            Some(HelperMarker {
                step: 1,
                of: 2,
                program: "nix-env".into()
            })
        );
        for bad in [
            "",
            "@zelador",
            "@zelador {",
            "@nix {\"step\":1}",
            "updating GRUB 2 menu...",
            r#"@zelador {"step":"x"}"#,
        ] {
            assert_eq!(parse_marker(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn fases_do_switch_to_configuration() {
        use ActivationStage::*;
        for (line, stage) in [
            ("activating the configuration...", ActivatingConfiguration),
            ("stopping the following units: foo.service", StoppingUnits),
            ("starting the following units: bar.service", StartingUnits),
            (
                "restarting the following units: baz.service",
                RestartingUnits,
            ),
            ("updating GRUB 2 menu...", UpdatingBootMenu),
            (
                "installing the GRUB 2 boot loader on /dev/disk/by-id/virtio-root...",
                InstallingBootLoader,
            ),
        ] {
            assert_eq!(stage_from_log(line), Some(stage), "{line:?}");
        }
        for other in [
            "",
            "Not checking switch inhibitors (action = boot)",
            "Installing for i386-pc platform.",
        ] {
            assert_eq!(stage_from_log(other), None, "{other:?}");
        }
    }

    struct Replay(&'static str);

    impl StreamingRunner for Replay {
        fn run_streaming(
            &self,
            _: &CommandSpec,
            _: Option<&CancelToken>,
            on_line: &mut dyn FnMut(StreamKind, &str),
        ) -> Result<Output> {
            for line in self.0.lines() {
                on_line(StreamKind::Stderr, line);
            }
            Ok(Output {
                stdout: String::new(),
                stderr: String::new(),
                code: Some(0),
            })
        }
    }

    #[test]
    fn ativacao_entrega_passos_fases_e_log_na_ordem() {
        let log = "@zelador {\"step\":1,\"of\":2,\"program\":\"nix-env\"}\n@zelador {\"step\":2,\"of\":2,\"program\":\"systemd-run\"}\nNot checking switch inhibitors (action = boot)\nupdating GRUB 2 menu...\ninstalling the GRUB 2 boot loader on /dev/x...\n";
        let client = HelperClient::new(
            Replay(log),
            PathBuf::from("/w/pkexec"),
            PathBuf::from("/s/helper"),
            PathBuf::from("/p"),
        );
        let mut events = Vec::new();
        client
            .activate(Path::new("/nix/store/x-sys"), ApplyMode::Boot, &mut |e| {
                events.push(e)
            })
            .unwrap();
        assert!(matches!(&events[0], ActivationEvent::Step(m) if m.step == 1 && m.of == 2));
        assert!(matches!(&events[1], ActivationEvent::Step(m) if m.step == 2));
        assert!(events.contains(&ActivationEvent::Stage(ActivationStage::UpdatingBootMenu)));
        assert!(events.contains(&ActivationEvent::Stage(
            ActivationStage::InstallingBootLoader
        )));
        // Os marcadores não vão para o log; o resto vai.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ActivationEvent::Log(l) if l.starts_with("@zelador")))
        );
        assert!(events.contains(&ActivationEvent::Log("updating GRUB 2 menu...".into())));
    }
}
