//! Verificações periódicas (timer de usuário): avisam, nunca alteram nada.

use crate::error::Result;
use crate::flake::{FlakeInput, SECONDS_PER_DAY};
use crate::history::History;
use crate::runner::{CommandRunner, CommandSpec};
use crate::units::format_bytes;

const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckConfig {
    /// Inputs mais velhos que isto geram aviso (o mesmo limiar da tela "Seu sistema").
    pub stale_days: i64,
    /// Aviso quando o espaço livre cai abaixo disto.
    pub min_free_bytes: u64,
    /// Aviso quando pode sair mais que isto do store. `0` desliga (evita rodar `--print-dead`).
    pub reclaimable_bytes: u64,
    /// Dias mínimos entre dois avisos da mesma condição.
    pub notice_interval_days: i64,
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            stale_days: 14,
            min_free_bytes: 10 * GIB,
            reclaimable_bytes: 20 * GIB,
            notice_interval_days: 7,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    StaleInputs,
    LowDisk,
    Reclaimable,
}

impl Condition {
    /// Identificador gravado no banco (não mude: é a chave do anti-spam).
    pub fn id(self) -> &'static str {
        match self {
            Condition::StaleInputs => "stale-inputs",
            Condition::LowDisk => "low-disk",
            Condition::Reclaimable => "reclaimable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub condition: Condition,
    pub title: String,
    pub body: String,
}

/// O que foi observado. `None` = não foi possível medir (e não gera aviso).
pub struct Observations<'a> {
    pub inputs: &'a [FlakeInput],
    pub now: i64,
    pub free_bytes: Option<u64>,
    pub reclaimable_bytes: Option<u64>,
}

pub fn evaluate(obs: &Observations<'_>, config: &CheckConfig) -> Vec<Finding> {
    let mut findings = Vec::new();

    let stale: Vec<&str> = obs
        .inputs
        .iter()
        .filter(|input| input.is_stale(obs.now, config.stale_days))
        .map(|input| input.name.as_str())
        .collect();
    if !stale.is_empty() {
        let shown = stale.iter().take(4).copied().collect::<Vec<_>>().join(", ");
        let more = stale.len().saturating_sub(4);
        let list = if more > 0 {
            format!("{shown} e mais {more}")
        } else {
            shown
        };
        let count = if stale.len() == 1 {
            "1 input tem".to_owned()
        } else {
            format!("{} inputs têm", stale.len())
        };
        findings.push(Finding {
            condition: Condition::StaleInputs,
            title: "Inputs do flake desatualizados".to_owned(),
            body: format!("{count} mais de {} dias: {list}.", config.stale_days),
        });
    }

    if let Some(free) = obs.free_bytes.filter(|free| *free < config.min_free_bytes) {
        findings.push(Finding {
            condition: Condition::LowDisk,
            title: "Pouco espaço em disco".to_owned(),
            body: format!(
                "Restam {} livres onde fica o Nix store (mínimo configurado: {}).",
                format_bytes(free),
                format_bytes(config.min_free_bytes)
            ),
        });
    }

    if let Some(bytes) = obs
        .reclaimable_bytes
        .filter(|bytes| config.reclaimable_bytes > 0 && *bytes >= config.reclaimable_bytes)
    {
        findings.push(Finding {
            condition: Condition::Reclaimable,
            title: "Há espaço para liberar".to_owned(),
            body: format!(
                "Cerca de {} no Nix store podem ser liberados. Abra o Zelador, em Liberar espaço.",
                format_bytes(bytes)
            ),
        });
    }
    findings
}

/// Já passou tempo suficiente desde o último aviso desta condição?
pub fn is_due(
    history: &History,
    condition: Condition,
    now: i64,
    config: &CheckConfig,
) -> Result<bool> {
    Ok(history
        .last_notice(condition.id())?
        .is_none_or(|last| now - last >= config.notice_interval_days * SECONDS_PER_DAY))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyOutcome {
    /// O usuário clicou na ação "Abrir".
    pub clicked: bool,
}

pub trait Notifier {
    fn notify(&self, finding: &Finding) -> Result<NotifyOutcome>;
}

/// Notifica pelo `notify-send` (libnotify), com a ação "Abrir". O daemon da sessão decide como
/// mostrar; `--wait` faz o programa esperar o clique ou o fim do prazo (`--expire-time`).
pub struct NotifySend<R> {
    runner: R,
    program: String,
}

impl<R: CommandRunner> NotifySend<R> {
    pub fn new(runner: R, program: impl Into<String>) -> Self {
        Self {
            runner,
            program: program.into(),
        }
    }

    pub fn command(&self, finding: &Finding) -> CommandSpec {
        CommandSpec::new(self.program.clone())
            .args([
                "--app-name=Zelador",
                "--icon=br.dev.zelador",
                "--expire-time=60000",
                "--hint=string:desktop-entry:br.dev.zelador",
                "--action=open=Abrir o Zelador",
                "--wait",
                "--",
            ])
            .arg(finding.title.clone())
            .arg(finding.body.clone())
    }
}

impl<R: CommandRunner> Notifier for NotifySend<R> {
    fn notify(&self, finding: &Finding) -> Result<NotifyOutcome> {
        let spec = self.command(finding);
        let stdout = self.runner.run(&spec)?.into_stdout(&spec)?;
        Ok(NotifyOutcome {
            clicked: stdout.trim() == "open",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Output;
    use std::cell::RefCell;

    const DAY: i64 = SECONDS_PER_DAY;

    fn input(name: &str, age_days: i64, now: i64) -> FlakeInput {
        FlakeInput {
            name: name.into(),
            kind: "github".into(),
            reference: None,
            source: None,
            rev: None,
            last_modified: Some(now - age_days * DAY),
        }
    }

    #[test]
    fn avisa_so_o_que_passou_do_limite() {
        let now = 1_000 * DAY;
        let inputs = [
            input("a", 20, now),
            input("b", 14, now),
            input("c", 15, now),
            input("d", 1, now),
        ];
        let obs = Observations {
            inputs: &inputs,
            now,
            free_bytes: Some(50 * GIB),
            reclaimable_bytes: Some(5 * GIB),
        };
        let findings = evaluate(&obs, &CheckConfig::default());
        // `b` tem exatamente 14 dias: não passou do limite.
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].condition, Condition::StaleInputs);
        assert_eq!(findings[0].body, "2 inputs têm mais de 14 dias: a, c.");
    }

    #[test]
    fn disco_baixo_e_lixo_grande() {
        let obs = Observations {
            inputs: &[],
            now: 0,
            free_bytes: Some(3 * GIB),
            reclaimable_bytes: Some(25 * GIB),
        };
        let findings = evaluate(&obs, &CheckConfig::default());
        let conditions: Vec<_> = findings.iter().map(|f| f.condition).collect();
        assert_eq!(conditions, [Condition::LowDisk, Condition::Reclaimable]);
        assert!(findings[0].body.contains("3,2 GB"), "{}", findings[0].body);
    }

    #[test]
    fn o_que_nao_foi_medido_nao_avisa_e_zero_desliga_o_lixo() {
        let obs = Observations {
            inputs: &[],
            now: 0,
            free_bytes: None,
            reclaimable_bytes: None,
        };
        assert!(evaluate(&obs, &CheckConfig::default()).is_empty());
        let off = CheckConfig {
            reclaimable_bytes: 0,
            ..CheckConfig::default()
        };
        let obs = Observations {
            inputs: &[],
            now: 0,
            free_bytes: None,
            reclaimable_bytes: Some(1_000 * GIB),
        };
        assert!(evaluate(&obs, &off).is_empty());
    }

    #[test]
    fn muitos_inputs_velhos_resumem_a_lista() {
        let now = 100 * DAY;
        let inputs: Vec<_> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|n| input(n, 30, now))
            .collect();
        let obs = Observations {
            inputs: &inputs,
            now,
            free_bytes: None,
            reclaimable_bytes: None,
        };
        let body = evaluate(&obs, &CheckConfig::default())[0].body.clone();
        assert_eq!(body, "6 inputs têm mais de 14 dias: a, b, c, d e mais 2.");
        let one = [input("x", 30, now)];
        let obs = Observations {
            inputs: &one,
            now,
            free_bytes: None,
            reclaimable_bytes: None,
        };
        assert_eq!(
            evaluate(&obs, &CheckConfig::default())[0].body,
            "1 input tem mais de 14 dias: x."
        );
    }

    #[test]
    fn anti_spam_um_aviso_por_condicao_por_intervalo() {
        let history = History::in_memory().unwrap();
        let config = CheckConfig::default();
        assert!(is_due(&history, Condition::LowDisk, 1_000 * DAY, &config).unwrap());
        history
            .set_notice(Condition::LowDisk.id(), 1_000 * DAY)
            .unwrap();
        assert!(!is_due(&history, Condition::LowDisk, 1_003 * DAY, &config).unwrap());
        assert!(is_due(&history, Condition::LowDisk, 1_007 * DAY, &config).unwrap());
        // Outra condição não é afetada.
        assert!(is_due(&history, Condition::StaleInputs, 1_001 * DAY, &config).unwrap());
    }

    struct Fake(RefCell<Vec<CommandSpec>>, &'static str);
    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<Output> {
            self.0.borrow_mut().push(spec.clone());
            Ok(Output {
                stdout: self.1.into(),
                stderr: String::new(),
                code: Some(0),
            })
        }
    }

    #[test]
    fn notify_send_leva_titulo_corpo_e_acao_abrir() {
        let finding = Finding {
            condition: Condition::LowDisk,
            title: "Pouco espaço".into(),
            body: "Restam 3 GB".into(),
        };
        let fake = Fake(RefCell::default(), "open\n");
        let notifier = NotifySend::new(&fake, "/n/notify-send");
        let outcome = notifier.notify(&finding).unwrap();
        assert!(outcome.clicked);
        let spec = fake.0.borrow()[0].clone();
        assert_eq!(spec.program, "/n/notify-send");
        assert!(
            spec.args
                .contains(&"--action=open=Abrir o Zelador".to_owned())
        );
        assert!(spec.args.contains(&"--wait".to_owned()));
        // Título e corpo vão depois do `--`, para um texto com `-` nunca virar opção.
        let n = spec.args.len();
        assert_eq!(&spec.args[n - 3..], ["--", "Pouco espaço", "Restam 3 GB"]);
        // Sem clique, a saída vem vazia.
        let quiet = Fake(RefCell::default(), "");
        assert!(
            !NotifySend::new(&quiet, "n")
                .notify(&finding)
                .unwrap()
                .clicked
        );
    }
}
