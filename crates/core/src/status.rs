//! Reúne os dados da tela "Seu sistema" a partir dos backends.
//!
//! Cada seção é coletada de forma independente: se uma ferramenta falhar, o erro vira um
//! aviso e as demais seções continuam.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backends::{AutoMaintenance, FlatpakBackend, GitBackend, NixBackend, PathInfoOptions};
use crate::flake::{DEFAULT_STALE_DAYS, FlakeInput};
use crate::flatpak::FlatpakSummary;
use crate::gcroots::{self, AUTO_GCROOTS_DIR, ForgottenRoot};
use crate::generations::{self, Generation, GenerationKind, SYSTEM_PROFILES_DIR};
use crate::git::GitStatus;
use crate::store::GcEstimate;

#[derive(Debug, Clone)]
pub struct StatusEnv {
    pub flake: PathBuf,
    pub host: String,
    /// Epoch atual, em segundos.
    pub now: i64,
    pub home: PathBuf,
    pub system_profiles_dir: PathBuf,
    pub home_manager_profiles_dir: PathBuf,
    pub current_system: PathBuf,
    /// Link do Home Manager em uso (`~/.local/state/home-manager/gcroots/current-home`).
    pub current_home: PathBuf,
    pub auto_gcroots_dir: PathBuf,
    pub stale_days: i64,
}

impl StatusEnv {
    /// Valores desta máquina. `flake` é a pasta do flake (padrão `/etc/nixos`).
    pub fn detect(flake: PathBuf, host: Option<String>) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let host = host
            .or_else(|| {
                fs::read_to_string("/proc/sys/kernel/hostname")
                    .ok()
                    .map(|name| name.trim().to_owned())
            })
            .unwrap_or_else(|| "nixos".to_owned());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        let current_home = home.join(".local/state/home-manager/gcroots/current-home");
        Self {
            flake,
            host,
            current_home,
            now,
            system_profiles_dir: SYSTEM_PROFILES_DIR.into(),
            home_manager_profiles_dir: home.join(".local/state/nix/profiles"),
            home,
            current_system: "/run/current-system".into(),
            auto_gcroots_dir: AUTO_GCROOTS_DIR.into(),
            stale_days: DEFAULT_STALE_DAYS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproximationReason {
    /// A geração não registra a revisão do flake (`system.configurationRevision` vazio).
    NoConfigurationRevision,
    /// A geração foi construída de uma árvore com alterações não commitadas.
    DirtyConfigurationRevision,
    /// A revisão registrada não existe mais neste repositório.
    RevisionNotInRepo,
}

/// Commits ainda não aplicados. `Approximate` conta os commits mais novos que a geração em
/// uso; a UI deve avisar que o número pode estar errado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingChanges {
    Exact(u64),
    Approximate {
        commits: u64,
        reasons: Vec<ApproximationReason>,
    },
}

pub fn pending_changes(
    git: &dyn GitBackend,
    repo: &Path,
    deployed_revision: Option<&str>,
    generation_created: i64,
) -> crate::Result<PendingChanges> {
    let reason = match deployed_revision
        .map(str::trim)
        .filter(|rev| !rev.is_empty())
    {
        None => ApproximationReason::NoConfigurationRevision,
        Some(rev) if rev.ends_with("-dirty") => ApproximationReason::DirtyConfigurationRevision,
        Some(rev) => match git.commits_since_rev(repo, rev)? {
            Some(commits) => return Ok(PendingChanges::Exact(commits)),
            None => ApproximationReason::RevisionNotInRepo,
        },
    };
    Ok(PendingChanges::Approximate {
        commits: git.commits_since_time(repo, generation_created)?,
        reasons: vec![reason],
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputStatus {
    pub input: FlakeInput,
    pub age_days: Option<i64>,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreStatus {
    pub total_bytes: u64,
    pub gc: Option<GcEstimate>,
}

#[derive(Debug, Default)]
pub struct SystemStatus {
    pub nixos_version: Option<String>,
    pub generations: Vec<Generation>,
    pub home_manager_generations: Vec<Generation>,
    pub inputs: Vec<InputStatus>,
    pub git: Option<GitStatus>,
    pub pending: Option<PendingChanges>,
    pub store: Option<StoreStatus>,
    pub forgotten_roots: Vec<ForgottenRoot>,
    pub flatpak: Option<FlatpakSummary>,
    pub auto_maintenance: Option<AutoMaintenance>,
    pub flatpak_uninstall_unused: Option<bool>,
    pub warnings: Vec<String>,
}

impl SystemStatus {
    pub fn current_generation(&self) -> Option<&Generation> {
        self.generations.iter().find(|g| g.is_current)
    }
}

/// Tudo de `collect` menos o tamanho do store e a estimativa de limpeza, que levam dezenas
/// de segundos. A UI usa esta parte primeiro e completa o resto depois.
pub fn collect_overview(
    env: &StatusEnv,
    nix: &dyn NixBackend,
    git: &dyn GitBackend,
    flatpak: &dyn FlatpakBackend,
) -> SystemStatus {
    let mut status = SystemStatus::default();
    let mut warnings = Warnings::default();

    status.nixos_version = fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| os_release_value(&text, "VERSION"));

    let current = fs::canonicalize(&env.current_system).ok();
    status.generations = warnings
        .keep(
            "gerações do sistema",
            generations::list_generations(
                &env.system_profiles_dir,
                "system",
                GenerationKind::System,
                current.as_deref(),
            ),
        )
        .unwrap_or_default();

    let hm_current = fs::canonicalize(
        env.home
            .join(".local/state/home-manager/gcroots/current-home"),
    )
    .ok();
    status.home_manager_generations = warnings
        .keep(
            "gerações do Home Manager",
            generations::list_generations(
                &env.home_manager_profiles_dir,
                "home-manager",
                GenerationKind::HomeManager,
                hm_current.as_deref(),
            ),
        )
        .unwrap_or_default();

    if let Some(metadata) = warnings.keep("inputs do flake", nix.flake_metadata(&env.flake)) {
        status.inputs = metadata
            .inputs
            .into_iter()
            .map(|input| InputStatus {
                age_days: input.age_days(env.now),
                stale: input.is_stale(env.now, env.stale_days),
                input,
            })
            .collect();
    }

    status.git = warnings.keep("git do flake", git.status(&env.flake));
    if let Some(created) = status.current_generation().map(|g| g.created) {
        let deployed = fs::read_to_string(env.current_system.join("configuration-revision")).ok();
        status.pending = warnings.keep(
            "commits não aplicados",
            pending_changes(git, &env.flake, deployed.as_deref(), created),
        );
    }

    status.forgotten_roots = warnings
        .keep(
            "links de build esquecidos",
            gcroots::read_auto_roots(&env.auto_gcroots_dir),
        )
        .map(|candidates| gcroots::forgotten_roots(&candidates, &env.home, env.now))
        .unwrap_or_default();

    status.flatpak = warnings.keep("flatpak", flatpak.summary());
    status.auto_maintenance = warnings.keep(
        "nix.gc.automatic",
        nix.auto_maintenance(&env.flake, &env.host),
    );
    status.flatpak_uninstall_unused = warnings
        .keep(
            "services.flatpak.uninstallUnused",
            nix.flatpak_uninstall_unused(&env.flake, &env.host),
        )
        .flatten();

    status.warnings = warnings.0;
    status
}

pub fn collect(
    env: &StatusEnv,
    nix: &dyn NixBackend,
    git: &dyn GitBackend,
    flatpak: &dyn FlatpakBackend,
) -> SystemStatus {
    let mut status = collect_overview(env, nix, git, flatpak);
    let mut warnings = Warnings(std::mem::take(&mut status.warnings));
    let total = warnings.keep("tamanho do store", nix.store_total_bytes());
    let gc = warnings.keep("estimativa de limpeza", gc_estimate(nix));
    status.store = total.map(|total_bytes| StoreStatus { total_bytes, gc });
    status.warnings = warnings.0;
    status
}

/// Acumula falhas de seções como avisos, devolvendo `None` no lugar do valor.
#[derive(Default)]
struct Warnings(Vec<String>);

impl Warnings {
    fn keep<T>(&mut self, what: &str, result: crate::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(%error, secao = what, "falha ao coletar");
                self.0.push(format!("{what}: {error}"));
                None
            }
        }
    }
}

/// Quantos caminhos o GC apagaria e quantos bytes isso libera.
///
/// Usa só `nix-store --gc --print-dead`: a contagem vem do número de caminhos, então não
/// é preciso rodar também `nix store gc --dry-run` (que repetiria a busca de roots).
pub fn gc_estimate(nix: &dyn NixBackend) -> crate::Result<GcEstimate> {
    let dead = nix.dead_paths()?;
    let bytes = if dead.is_empty() {
        0
    } else {
        nix.path_info(&dead, PathInfoOptions::default())?
            .iter()
            .map(|info| info.nar_size)
            .sum()
    };
    Ok(GcEstimate {
        paths: dead.len() as u64,
        bytes,
    })
}

/// Versões mostradas no rodapé da barra lateral: `(NixOS, Nix)`, ex.: `("26.11", "2.34.9")`.
pub fn system_versions(
    runner: &dyn crate::runner::CommandRunner,
) -> (Option<String>, Option<String>) {
    let nixos = fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| os_release_value(&text, "VERSION_ID"));
    let spec = crate::runner::CommandSpec::new("nix").arg("--version");
    let nix = runner
        .run(&spec)
        .and_then(|out| out.into_stdout(&spec))
        .ok()
        .and_then(|text| parse_nix_version(&text));
    (nixos, nix)
}

/// `nix (Nix) 2.34.9` → `2.34.9`.
fn parse_nix_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .last()
        .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_owned)
}

fn os_release_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .map(|value| value.trim_matches('"').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use std::cell::Cell;

    struct FakeGit {
        by_rev: Option<u64>,
        by_time: u64,
        asked_time: Cell<bool>,
    }

    impl GitBackend for FakeGit {
        fn status(&self, _: &Path) -> Result<GitStatus> {
            Ok(GitStatus::default())
        }
        fn commits_since_rev(&self, _: &Path, _: &str) -> Result<Option<u64>> {
            Ok(self.by_rev)
        }
        fn commits_since_time(&self, _: &Path, _: i64) -> Result<u64> {
            self.asked_time.set(true);
            Ok(self.by_time)
        }
        fn add(&self, _: &Path, _: &[String]) -> Result<()> {
            Ok(())
        }
    }

    fn git(by_rev: Option<u64>) -> FakeGit {
        FakeGit {
            by_rev,
            by_time: 3,
            asked_time: Cell::new(false),
        }
    }

    #[test]
    fn com_revisao_a_contagem_e_exata() {
        let git = git(Some(5));
        let pending = pending_changes(&git, Path::new("/r"), Some("abc\n"), 0).unwrap();
        assert_eq!(pending, PendingChanges::Exact(5));
        assert!(!git.asked_time.get());
    }

    #[test]
    fn sem_revisao_e_aproximada_e_diz_por_que() {
        let pending = pending_changes(&git(None), Path::new("/r"), None, 0).unwrap();
        assert_eq!(
            pending,
            PendingChanges::Approximate {
                commits: 3,
                reasons: vec![ApproximationReason::NoConfigurationRevision]
            }
        );
    }

    #[test]
    fn revisao_suja_ou_ausente_no_repo_tambem_e_aproximada() {
        let dirty = pending_changes(&git(Some(9)), Path::new("/r"), Some("abc-dirty"), 0).unwrap();
        assert!(
            matches!(dirty, PendingChanges::Approximate { ref reasons, .. }
            if reasons == &[ApproximationReason::DirtyConfigurationRevision])
        );
        let gone = pending_changes(&git(None), Path::new("/r"), Some("abc"), 0).unwrap();
        assert!(
            matches!(gone, PendingChanges::Approximate { ref reasons, .. }
            if reasons == &[ApproximationReason::RevisionNotInRepo])
        );
    }

    #[test]
    fn versao_do_nix() {
        assert_eq!(
            parse_nix_version("nix (Nix) 2.34.9\n").as_deref(),
            Some("2.34.9")
        );
        assert_eq!(
            parse_nix_version("nix (Lix, like Nix) 2.93.0").as_deref(),
            Some("2.93.0")
        );
        assert_eq!(parse_nix_version("sem versão"), None);
    }

    #[test]
    fn os_release() {
        let text = "NAME=NixOS\nVERSION=\"26.11 (Zokor)\"\nVERSION_ID=\"26.11\"\n";
        assert_eq!(
            os_release_value(text, "VERSION").as_deref(),
            Some("26.11 (Zokor)")
        );
    }
}
