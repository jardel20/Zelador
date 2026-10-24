//! Formatação de datas, idades e textos do histórico em pt-BR (traduzíveis).

use crate::i18n::{tn, tr, trf};

/// Abreviação do mês (1 a 12). Cada nome é um literal próprio para o gettext extraí-lo.
fn month_abbreviation(month: u32) -> String {
    match month {
        1 => tr("jan."),
        2 => tr("fev."),
        3 => tr("mar."),
        4 => tr("abr."),
        5 => tr("mai."),
        6 => tr("jun."),
        7 => tr("jul."),
        8 => tr("ago."),
        9 => tr("set."),
        10 => tr("out."),
        11 => tr("nov."),
        12 => tr("dez."),
        _ => "?".to_owned(),
    }
}

/// `21 de out.`; com o ano quando não é o corrente (`21 de out. de 2025`).
pub fn short_date(year: i32, month: u32, day: u32, current_year: i32) -> String {
    let month = month_abbreviation(month);
    if year == current_year {
        trf("{0} de {1}", &[day.to_string(), month])
    } else {
        trf(
            "{0} de {1} de {2}",
            &[day.to_string(), month, year.to_string()],
        )
    }
}

/// Data local de um instante em epoch.
pub fn short_date_from_epoch(epoch: i64) -> String {
    use gtk::glib::DateTime;
    let current_year = DateTime::now_local().map_or(0, |now| now.year());
    match DateTime::from_unix_local(epoch) {
        Ok(date) => short_date(
            date.year(),
            u32::try_from(date.month()).unwrap_or(1),
            u32::try_from(date.day_of_month()).unwrap_or(1),
            current_year,
        ),
        Err(_) => String::new(),
    }
}

/// `hoje`, `há 1 dia`, `há 23 dias`, `há 2 meses`, `há 1 ano`.
pub fn relative_age(days: i64) -> String {
    match days {
        i64::MIN..=0 => tr("hoje"),
        1..=59 => tn(
            "há {0} dia",
            "há {0} dias",
            u64::try_from(days).unwrap_or(0),
        ),
        60..=364 => tn(
            "há {0} mês",
            "há {0} meses",
            u64::try_from(days / 30).unwrap_or(0),
        ),
        _ => tn(
            "há {0} ano",
            "há {0} anos",
            u64::try_from(days / 365).unwrap_or(0),
        ),
    }
}

/// `41 pacotes atualizados, 3 novos e 1 removido`.
pub fn package_changes(updated: usize, added: usize, removed: usize) -> String {
    let n = |v: usize| u64::try_from(v).unwrap_or(0);
    trf(
        "{0}, {1} e {2}",
        &[
            tn(
                "{0} pacote atualizado",
                "{0} pacotes atualizados",
                n(updated),
            ),
            tn("{0} novo", "{0} novos", n(added)),
            tn("{0} removido", "{0} removidos", n(removed)),
        ],
    )
}

/// Texto de uma linha do histórico: `(título, quando)`, como nos mockups
/// ("Geração 214 aplicada", "Limpeza liberou 6,1 GB").
pub fn describe_operation(
    op: &zelador_core::history::Operation,
    current_year: i32,
) -> (String, String) {
    use zelador_core::history::{OperationKind, Outcome};
    use zelador_core::units::format_bytes;
    let generation = |template: &str, none: &str| match op.generation {
        Some(n) => trf(template, &[n.to_string()]),
        None => tr(none),
    };
    let title = match (op.kind, op.outcome) {
        (OperationKind::Update, Outcome::Success) => {
            generation("Geração {0} aplicada", "Atualização aplicada")
        }
        (OperationKind::Update, Outcome::Failure) => tr("A atualização falhou"),
        (OperationKind::Update, Outcome::Cancelled) => tr("Atualização cancelada"),
        (OperationKind::Clean, Outcome::Success) => match op.bytes_freed {
            Some(bytes) if bytes > 0 => trf("Limpeza liberou {0}", &[format_bytes(bytes)]),
            _ => tr("Limpeza concluída"),
        },
        (OperationKind::Clean, _) => tr("A limpeza falhou"),
        (OperationKind::DeleteGenerations, Outcome::Success) => op
            .detail
            .clone()
            .unwrap_or_else(|| tr("Gerações excluídas")),
        (OperationKind::DeleteGenerations, _) => tr("A exclusão de gerações falhou"),
        (OperationKind::Rollback, Outcome::Success) => {
            generation("Voltou para a geração {0}", "Voltou para outra geração")
        }
        (OperationKind::Rollback, _) => tr("Não foi possível voltar de geração"),
        (OperationKind::SetBootDefault, Outcome::Success) => {
            generation("Geração {0} é o padrão no boot", "Padrão no boot alterado")
        }
        (OperationKind::SetBootDefault, _) => tr("Não foi possível mudar o padrão no boot"),
    };
    (title, date_and_time(op.started_at, current_year))
}

/// `21 de out., 14:32` (só a data se o horário não puder ser lido).
fn date_and_time(epoch: i64, current_year: i32) -> String {
    use gtk::glib::DateTime;
    match DateTime::from_unix_local(epoch) {
        Ok(date) => {
            let day = short_date(
                date.year(),
                u32::try_from(date.month()).unwrap_or(1),
                u32::try_from(date.day_of_month()).unwrap_or(1),
                current_year,
            );
            trf(
                "{0}, {1}",
                &[day, format!("{:02}:{:02}", date.hour(), date.minute())],
            )
        }
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datas() {
        assert_eq!(short_date(2026, 10, 21, 2026), "21 de out.");
        assert_eq!(short_date(2025, 9, 2, 2026), "2 de set. de 2025");
        assert_eq!(short_date(2026, 13, 1, 2026), "1 de ?");
    }

    #[test]
    fn idades() {
        assert_eq!(relative_age(0), "hoje");
        assert_eq!(relative_age(-3), "hoje");
        assert_eq!(relative_age(1), "há 1 dia");
        assert_eq!(relative_age(23), "há 23 dias");
        assert_eq!(relative_age(60), "há 2 meses");
        assert_eq!(relative_age(242), "há 8 meses");
        assert_eq!(relative_age(365), "há 1 ano");
        assert_eq!(relative_age(800), "há 2 anos");
    }

    fn op(
        kind: zelador_core::history::OperationKind,
        outcome: zelador_core::history::Outcome,
    ) -> zelador_core::history::Operation {
        zelador_core::history::Operation {
            id: 1,
            kind,
            started_at: 0,
            duration_ms: 0,
            outcome,
            generation: Some(214),
            bytes_freed: Some(6_100_000_000),
            detail: None,
        }
    }

    #[test]
    fn resumo_de_pacotes_com_plural() {
        assert_eq!(
            package_changes(41, 3, 1),
            "41 pacotes atualizados, 3 novos e 1 removido"
        );
        assert_eq!(
            package_changes(1, 1, 2),
            "1 pacote atualizado, 1 novo e 2 removidos"
        );
    }

    #[test]
    fn historico_como_nos_mockups() {
        use zelador_core::history::{OperationKind::*, Outcome::*};
        assert_eq!(
            describe_operation(&op(Update, Success), 2026).0,
            "Geração 214 aplicada"
        );
        assert_eq!(
            describe_operation(&op(Clean, Success), 2026).0,
            "Limpeza liberou 6,1 GB"
        );
        assert_eq!(
            describe_operation(&op(Rollback, Success), 2026).0,
            "Voltou para a geração 214"
        );
        assert_eq!(
            describe_operation(&op(Update, Failure), 2026).0,
            "A atualização falhou"
        );
    }
}
