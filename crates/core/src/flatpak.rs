//! Flatpak: runtimes, apps e o que está sem uso.

use crate::units::parse_si_size;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installation {
    System,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatpakRef {
    pub id: String,
    pub branch: String,
    pub size_bytes: Option<u64>,
    pub installation: Installation,
}

/// Saída de `flatpak list --columns=application,branch,size,installation` com `LC_ALL=C`.
/// Colunas separadas por tabulação.
pub fn parse_list(text: &str) -> Vec<FlatpakRef> {
    text.lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let id = cols.next()?.trim();
            let branch = cols.next()?.trim();
            let size = cols.next()?.trim();
            let installation = match cols.next()?.trim() {
                "system" => Installation::System,
                "user" => Installation::User,
                _ => return None,
            };
            (!id.is_empty()).then(|| FlatpakRef {
                id: id.to_owned(),
                branch: branch.to_owned(),
                size_bytes: parse_si_size(size),
                installation,
            })
        })
        .collect()
}

/// Tabela de `flatpak uninstall --unused` (respondida com `n`):
/// ` 1.\t   \torg.gnome.Platform\t49\tr`. Devolve `(id, branch)`.
pub fn parse_unused(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut cols = line.split('\t').map(str::trim);
            let index = cols.next()?;
            index.strip_suffix('.')?.trim().parse::<u32>().ok()?;
            cols.next()?;
            let id = cols.next()?;
            let branch = cols.next()?;
            (!id.is_empty()).then(|| (id.to_owned(), branch.to_owned()))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatpakSummary {
    pub apps: Vec<FlatpakRef>,
    pub runtimes: Vec<FlatpakRef>,
    pub unused: Vec<(String, String)>,
    /// Instalação que contém os runtimes sem uso; a UI só usa o helper se for `System`.
    pub installation: Installation,
    pub total_bytes: u64,
    /// Soma só dos sem-uso com tamanho conhecido (runtimes `.Locale` não aparecem no `list`).
    pub unused_bytes: u64,
    pub unused_bytes_partial: bool,
}

pub fn summarize(
    apps: Vec<FlatpakRef>,
    runtimes: Vec<FlatpakRef>,
    unused: Vec<(String, String)>,
) -> FlatpakSummary {
    let total_bytes = apps
        .iter()
        .chain(&runtimes)
        .filter_map(|r| r.size_bytes)
        .sum();
    let mut unused_bytes = 0;
    let mut partial = false;
    for (id, branch) in &unused {
        match runtimes
            .iter()
            .find(|r| &r.id == id && &r.branch == branch)
            .and_then(|r| r.size_bytes)
        {
            Some(size) => unused_bytes += size,
            None => partial = true,
        }
    }
    let installation = if apps
        .iter()
        .chain(&runtimes)
        .any(|r| r.installation == Installation::System)
        || (apps.is_empty() && runtimes.is_empty())
    {
        Installation::System
    } else {
        Installation::User
    };
    FlatpakSummary {
        apps,
        runtimes,
        unused,
        installation,
        total_bytes,
        unused_bytes,
        unused_bytes_partial: partial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lista() {
        let refs = parse_list("org.gnome.Platform\t49\t1.1 GB\tsystem\nsem\tcolunas\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].size_bytes, Some(1_100_000_000));
        assert_eq!(refs[0].installation, Installation::System);
    }

    #[test]
    fn sem_uso() {
        let text = "\n\n 1.\t   \torg.gnome.Platform\t49\tr\n10.\t   \tx.Locale\t50\tr\n\nProceed? [Y/n]: n\n";
        assert_eq!(
            parse_unused(text),
            [
                ("org.gnome.Platform".into(), "49".into()),
                ("x.Locale".into(), "50".into())
            ]
        );
    }
}
