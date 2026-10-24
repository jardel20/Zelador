//! Conversão de tamanhos entre o texto das CLIs e bytes.

/// Tamanho do `flatpak list` (`691.3 MB`, `789.5 kB`): base 1000, sempre com ponto decimal.
pub fn parse_si_size(text: &str) -> Option<u64> {
    let (value, unit) = split_number_unit(text)?;
    let factor = match unit {
        "bytes" | "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    to_bytes(value * factor)
}

/// Tamanho do `nix store diff-closures` (`-111.7 KiB`): base 1024, com sinal opcional.
/// Devolve o valor com sinal.
pub fn parse_iec_size(text: &str) -> Option<i64> {
    let (value, unit) = split_number_unit(text)?;
    let factor = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0_f64.powi(4),
        _ => return None,
    };
    let scaled = value * factor;
    if !scaled.is_finite() {
        return None;
    }
    // Arredondar é intencional: o texto original tem uma casa decimal.
    Some(scaled.round() as i64)
}

fn split_number_unit(text: &str) -> Option<(f64, &str)> {
    // O flatpak separa número e unidade com NBSP (U+00A0), que `char::is_whitespace` cobre.
    let (number, unit) = text.trim().split_once(char::is_whitespace)?;
    Some((number.parse().ok()?, unit.trim()))
}

fn to_bytes(value: f64) -> Option<u64> {
    (value.is_finite() && value >= 0.0).then(|| value.round() as u64)
}

/// Formata bytes como na interface (`14,6 GB`): base 1000 em pt-BR.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn si() {
        assert_eq!(parse_si_size("691.3 MB"), Some(691_300_000));
        assert_eq!(parse_si_size("789.5 kB"), Some(789_500));
        assert_eq!(parse_si_size("1.1 GB"), Some(1_100_000_000));
        assert_eq!(parse_si_size("691.3\u{a0}MB"), Some(691_300_000));
        assert_eq!(parse_si_size("lixo"), None);
    }

    #[test]
    fn iec() {
        assert_eq!(parse_iec_size("-111.7 KiB"), Some(-114_381));
        assert_eq!(parse_iec_size("7.0 MiB"), Some(7_340_032));
        assert_eq!(parse_iec_size("3 furlongs"), None);
    }

    #[test]
    fn formato() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(14_600_000_000), "14,6 GB");
    }
}
