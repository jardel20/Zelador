//! Estado do repositório do flake, lido de `git status --porcelain=v2 --branch`.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitStatus {
    pub head_oid: Option<String>,
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Arquivos rastreados com alterações.
    pub modified: Vec<String>,
    /// Arquivos que o flake ignora por não estarem no git.
    pub untracked: Vec<String>,
}

impl GitStatus {
    pub fn is_dirty(&self) -> bool {
        !self.modified.is_empty() || !self.untracked.is_empty()
    }
}

pub fn parse_status_v2(text: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head_oid = Some(value.to_owned()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_owned()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
        } else if let Some(path) = line.strip_prefix("? ") {
            status.untracked.push(path.to_owned());
        } else if let Some(rest) = line.strip_prefix("1 ") {
            // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
            if let Some(path) = rest.splitn(8, ' ').nth(7) {
                status.modified.push(path.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("2 ") {
            // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\t<origPath>
            if let Some(paths) = rest.splitn(9, ' ').nth(8) {
                let path = paths.split('\t').next().unwrap_or(paths);
                status.modified.push(path.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("u ") {
            // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
            if let Some(path) = rest.splitn(10, ' ').nth(9) {
                status.modified.push(path.to_owned());
            }
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cabecalho_e_arquivos() {
        let text = "# branch.oid abc\n# branch.head master\n# branch.upstream origin/master\n\
                    # branch.ab +2 -1\n\
                    1 .M N... 100644 100644 100644 aaa bbb modules/a b.nix\n\
                    2 R. N... 100644 100644 100644 aaa bbb R100 novo.nix\tvelho.nix\n\
                    ? modules/audio/alc287.nix\n";
        let status = parse_status_v2(text);
        assert_eq!(status.head_oid.as_deref(), Some("abc"));
        assert_eq!(status.branch.as_deref(), Some("master"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.modified, ["modules/a b.nix", "novo.nix"]);
        assert_eq!(status.untracked, ["modules/audio/alc287.nix"]);
        assert!(status.is_dirty());
    }

    #[test]
    fn repositorio_sem_commits() {
        let status = parse_status_v2("# branch.oid (initial)\n# branch.head (detached)\n");
        assert_eq!(status, GitStatus::default());
        assert!(!status.is_dirty());
    }
}
