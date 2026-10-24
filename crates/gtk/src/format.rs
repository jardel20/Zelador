//! Formatação de datas, idades e plurais em pt-BR.

const MONTHS: [&str; 12] = [
    "jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.", "dez.",
];

/// `21 de out.`; com o ano quando não é o corrente (`21 de out. de 2025`).
pub fn short_date(year: i32, month: u32, day: u32, current_year: i32) -> String {
    let month = usize::try_from(month.saturating_sub(1))
        .ok()
        .and_then(|index| MONTHS.get(index))
        .copied()
        .unwrap_or("?");
    if year == current_year {
        format!("{day} de {month}")
    } else {
        format!("{day} de {month} de {year}")
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
        i64::MIN..=0 => "hoje".to_owned(),
        1 => "há 1 dia".to_owned(),
        2..=59 => format!("há {days} dias"),
        60..=364 => format!("há {} meses", days / 30),
        _ => {
            let years = days / 365;
            if years == 1 {
                "há 1 ano".to_owned()
            } else {
                format!("há {years} anos")
            }
        }
    }
}

/// `1 commit`, `3 commits`.
pub fn count(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
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

    #[test]
    fn plurais() {
        assert_eq!(count(1, "commit", "commits"), "1 commit");
        assert_eq!(count(0, "commit", "commits"), "0 commits");
        assert_eq!(count(3, "commit", "commits"), "3 commits");
    }
}
