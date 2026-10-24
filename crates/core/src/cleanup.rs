//! "Liberar espaço": política de gerações, plano, estimativa e execução em passos.
//!
//! A estimativa sai de conjuntos de closures (sem tocar em nada). A execução usa o helper
//! privilegiado para o que exige root e roda o resto como usuário.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::backends::{FlatpakBackend, NixBackend};
use crate::error::{Error, Result};
use crate::flake::SECONDS_PER_DAY;
use crate::flatpak::Installation;
use crate::gcroots::ForgottenRoot;
use crate::generations::{Generation, GenerationKind};
use crate::runner::{CommandRunner, CommandSpec};
use crate::status::{StatusEnv, SystemStatus};

pub const DEFAULT_KEEP_SYSTEM: usize = 5;
pub const DEFAULT_KEEP_DAYS: u32 = 7;
pub const DEFAULT_KEEP_HOME_MANAGER: usize = 3;

/// Quantas gerações manter. Uma geração fica se estiver entre as `keep_last` mais novas **ou**
/// for dos últimos `keep_days` dias (mesma regra do `nh clean`). A em uso e a padrão do boot
/// nunca saem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub keep_system: usize,
    pub keep_days: u32,
    pub keep_home_manager: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            keep_system: DEFAULT_KEEP_SYSTEM,
            keep_days: DEFAULT_KEEP_DAYS,
            keep_home_manager: DEFAULT_KEEP_HOME_MANAGER,
        }
    }
}

/// Números das gerações a excluir, em ordem crescente.
pub fn select_for_deletion(
    generations: &[Generation],
    keep_last: usize,
    keep_days: u32,
    now: i64,
) -> Vec<u64> {
    let mut newest_first: Vec<&Generation> = generations.iter().collect();
    newest_first.sort_by_key(|g| std::cmp::Reverse(g.number));
    let cutoff = now - i64::from(keep_days) * SECONDS_PER_DAY;
    let mut selected: Vec<u64> = newest_first
        .iter()
        .enumerate()
        .filter(|(position, g)| {
            *position >= keep_last && g.created <= cutoff && !g.is_current && !g.is_boot_default
        })
        .map(|(_, g)| g.number)
        .collect();
    selected.sort_unstable();
    selected
}

/// O que o usuário marcou na tela.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub generations: bool,
    /// `path` dos links esquecidos marcados.
    pub links: Vec<PathBuf>,
    pub flatpak: bool,
    pub optimise: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupPlan {
    pub system_generations: Vec<u64>,
    pub home_manager_generations: Vec<u64>,
    pub links: Vec<ForgottenRoot>,
    pub flatpak: bool,
    /// O Flatpak sem uso está na instalação de sistema (exige o helper); senão, na do usuário.
    pub flatpak_system: bool,
    pub optimise: bool,
}

impl CleanupPlan {
    /// A coleta de lixo só adianta se algo deixou de ser raiz.
    pub fn runs_gc(&self) -> bool {
        !self.system_generations.is_empty()
            || !self.home_manager_generations.is_empty()
            || !self.links.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        !self.runs_gc() && !self.flatpak && !self.optimise
    }

    /// Passos sem privilégio primeiro; tudo o que exige root vai num único passo do helper
    /// (`clean`), então a senha de administrador é pedida uma vez.
    pub fn steps(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        let mut push = |kind, privileged| steps.push(Step { kind, privileged });
        if !self.links.is_empty() {
            push(StepKind::RemoveLinks, false);
        }
        if !self.home_manager_generations.is_empty() {
            push(StepKind::HomeManagerGenerations, false);
        }
        if self.flatpak && !self.flatpak_system {
            push(StepKind::FlatpakUser, false);
        }
        if self.needs_helper() {
            push(StepKind::Privileged, true);
        }
        steps
    }

    /// Há algo que só o helper pode fazer.
    pub fn needs_helper(&self) -> bool {
        !self.system_generations.is_empty()
            || (self.flatpak && self.flatpak_system)
            || self.runs_gc()
            || self.optimise
    }

    /// Argumentos do `zelador-helper clean` para a parte privilegiada.
    pub fn helper_args(&self) -> Vec<String> {
        let mut args = vec!["clean".to_owned()];
        if !self.system_generations.is_empty() {
            args.push("--generations".to_owned());
            args.extend(self.system_generations.iter().map(u64::to_string));
        }
        if self.flatpak && self.flatpak_system {
            args.push("--flatpak".to_owned());
        }
        if self.runs_gc() {
            args.push("--gc".to_owned());
        }
        if self.optimise {
            args.push("--optimise".to_owned());
        }
        args
    }
}

pub fn build_plan(
    status: &SystemStatus,
    policy: Policy,
    selection: &Selection,
    now: i64,
) -> CleanupPlan {
    let (system, home_manager) = if selection.generations {
        (
            select_for_deletion(
                &status.generations,
                policy.keep_system,
                policy.keep_days,
                now,
            ),
            select_for_deletion(
                &status.home_manager_generations,
                policy.keep_home_manager,
                policy.keep_days,
                now,
            ),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    CleanupPlan {
        system_generations: system,
        home_manager_generations: home_manager,
        links: status
            .forgotten_roots
            .iter()
            .filter(|root| selection.links.contains(&root.path))
            .cloned()
            .collect(),
        flatpak: selection.flatpak
            && status
                .flatpak
                .as_ref()
                .is_some_and(|flatpak| !flatpak.unused.is_empty()),
        flatpak_system: status
            .flatpak
            .as_ref()
            .is_none_or(|flatpak| flatpak.installation == Installation::System),
        optimise: selection.optimise,
    }
}

// ── Estimativa ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RootId {
    SystemGeneration(u64),
    HomeManagerGeneration(u64),
    /// Em uso agora: nunca é candidato.
    CurrentSystem,
    CurrentHome,
    Link(PathBuf),
}

#[derive(Debug, Clone)]
pub struct IndexedRoot {
    pub id: RootId,
    pub paths: HashSet<String>,
}

/// Closure de cada raiz conhecida, para estimar o que sairia ao descartar algumas delas.
#[derive(Debug, Clone, Default)]
pub struct ClosureIndex {
    pub roots: Vec<IndexedRoot>,
}

pub fn closure_index(
    nix: &dyn NixBackend,
    env: &StatusEnv,
    status: &SystemStatus,
) -> Result<ClosureIndex> {
    let mut roots = Vec::new();
    let mut add = |id: RootId, path: &Path| -> Result<()> {
        let paths = nix.closure_paths(&path.to_string_lossy())?;
        roots.push(IndexedRoot {
            id,
            paths: paths.into_iter().collect(),
        });
        Ok(())
    };
    if let Ok(current) = fs::canonicalize(&env.current_system) {
        add(RootId::CurrentSystem, &current)?;
    }
    if let Ok(current) = fs::canonicalize(&env.current_home) {
        add(RootId::CurrentHome, &current)?;
    }
    for generation in &status.generations {
        add(
            RootId::SystemGeneration(generation.number),
            &generation.store_path,
        )?;
    }
    for generation in &status.home_manager_generations {
        add(
            RootId::HomeManagerGeneration(generation.number),
            &generation.store_path,
        )?;
    }
    for root in &status.forgotten_roots {
        for store_path in &root.store_paths {
            add(RootId::Link(root.path.clone()), store_path)?;
        }
    }
    Ok(ClosureIndex { roots })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Estimate {
    pub generations: u64,
    pub links: u64,
    /// Caminhos sem referência hoje, removidos pela coleta de lixo.
    pub unreferenced: u64,
    pub flatpak: u64,
}

impl Estimate {
    /// Converte o tamanho lógico (soma dos `narSize`) em uma estimativa do que sai do disco.
    /// O Flatpak já vem em bytes reais e não muda.
    pub fn scaled(self, ratio: f64) -> Self {
        let scale = |bytes: u64| crate::disk::scale_bytes(bytes, ratio);
        Self {
            generations: scale(self.generations),
            links: scale(self.links),
            unreferenced: scale(self.unreferenced),
            flatpak: self.flatpak,
        }
    }

    pub fn total(&self) -> u64 {
        self.generations + self.links + self.unreferenced + self.flatpak
    }
}

/// O que sairia com `plan`: caminhos que só as raízes descartadas mantinham.
///
/// É um limite superior: o Nix não diz se outra raiz desconhecida também usa um caminho.
pub fn estimate(
    index: &ClosureIndex,
    sizes: &HashMap<String, u64>,
    dead: &HashSet<String>,
    plan: &CleanupPlan,
    flatpak_bytes: u64,
) -> Estimate {
    let selected = |root: &IndexedRoot| match &root.id {
        RootId::SystemGeneration(n) => plan.system_generations.contains(n),
        RootId::HomeManagerGeneration(n) => plan.home_manager_generations.contains(n),
        RootId::Link(path) => plan.links.iter().any(|link| &link.path == path),
        RootId::CurrentSystem | RootId::CurrentHome => false,
    };
    let kept: HashSet<&String> = index
        .roots
        .iter()
        .filter(|root| !selected(root))
        .flat_map(|root| root.paths.iter())
        .collect();
    let mut generations = HashSet::new();
    let mut links = HashSet::new();
    for root in index.roots.iter().filter(|root| selected(root)) {
        let target = if matches!(root.id, RootId::Link(_)) {
            &mut links
        } else {
            &mut generations
        };
        target.extend(root.paths.iter().filter(|path| !kept.contains(path)));
    }
    // Um caminho em gerações e em links conta uma vez, como geração.
    links.retain(|path| !generations.contains(path));
    let sum = |paths: &HashSet<&String>| -> u64 {
        paths.iter().filter_map(|path| sizes.get(*path)).sum()
    };
    Estimate {
        generations: sum(&generations),
        links: sum(&links),
        unreferenced: if plan.runs_gc() {
            dead.iter().filter_map(|path| sizes.get(path)).sum()
        } else {
            0
        },
        flatpak: if plan.flatpak { flatpak_bytes } else { 0 },
    }
}

/// Razão entre o disco ocupado e o tamanho lógico de todos os caminhos do store
/// (ver [`crate::disk::real_ratio`]).
pub fn disk_ratio(sizes: &HashMap<String, u64>, used_bytes: u64) -> f64 {
    crate::disk::real_ratio(sizes.values().sum(), used_bytes)
}

/// Quanto cada link esquecido libera sozinho (fecho dele menos o das demais raízes).
pub fn link_sizes(index: &ClosureIndex, sizes: &HashMap<String, u64>) -> HashMap<PathBuf, u64> {
    let mut result: HashMap<PathBuf, u64> = HashMap::new();
    for root in &index.roots {
        let RootId::Link(path) = &root.id else {
            continue;
        };
        let others: HashSet<&String> = index
            .roots
            .iter()
            .filter(|other| match &other.id {
                RootId::Link(p) => p != path,
                _ => true,
            })
            .flat_map(|other| other.paths.iter())
            .collect();
        let bytes: u64 = root
            .paths
            .iter()
            .filter(|p| !others.contains(p))
            .filter_map(|p| sizes.get(p))
            .sum();
        *result.entry(path.clone()).or_default() += bytes;
    }
    result
}

// ── Simulação ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRun {
    /// Quantos caminhos a coleta de lixo apagaria agora (`nix store gc --dry-run`).
    pub gc_paths: Option<u64>,
}

pub fn dry_run(nix: &dyn NixBackend, plan: &CleanupPlan) -> Result<DryRun> {
    Ok(DryRun {
        gc_paths: if plan.runs_gc() {
            Some(nix.gc_dry_run()?)
        } else {
            None
        },
    })
}

// ── Helper privilegiado ─────────────────────────────────────────────────────

/// Socket em que o systemd escuta para o helper (`programs.zelador.enable`).
pub const SOCKET_PATH: &str = "/run/zelador/helper.sock";
pub const POLICY_PATH: &str =
    "/run/current-system/sw/share/polkit-1/actions/br.dev.zelador.helper.policy";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperIssue {
    /// O socket do helper não existe: o serviço do systemd não está ativo.
    SocketMissing,
    HelperMissing,
    /// A action do polkit não está instalada (`programs.zelador.enable`).
    PolicyMissing,
}

/// Chama `zelador-helper --call`, que conversa com o serviço do systemd (root, com polkit).
/// Sempre por caminhos absolutos fixos.
#[derive(Debug, Clone)]
pub struct HelperClient<R> {
    runner: R,
    socket: PathBuf,
    helper: PathBuf,
    policy: PathBuf,
}

pub trait PrivilegedHelper {
    fn run(&self, args: &[String]) -> Result<()>;
}

impl<R> HelperClient<R> {
    pub fn new(runner: R, socket: PathBuf, helper: PathBuf, policy: PathBuf) -> Self {
        Self {
            runner,
            socket,
            helper,
            policy,
        }
    }

    /// Caminhos de produção: o helper vem do build (`ZELADOR_HELPER`).
    pub fn production(runner: R) -> Self {
        let helper =
            option_env!("ZELADOR_HELPER").unwrap_or("/run/current-system/sw/bin/zelador-helper");
        Self::new(
            runner,
            SOCKET_PATH.into(),
            helper.into(),
            POLICY_PATH.into(),
        )
    }

    pub fn check(&self) -> std::result::Result<(), HelperIssue> {
        if !self.socket.exists() {
            Err(HelperIssue::SocketMissing)
        } else if !self.helper.exists() {
            Err(HelperIssue::HelperMissing)
        } else if !self.policy.exists() {
            Err(HelperIssue::PolicyMissing)
        } else {
            Ok(())
        }
    }
}

impl<R> HelperClient<R> {
    /// `<helper> --call <args...>`, sem repassar o ambiente do usuário.
    pub fn command(&self, args: &[String]) -> CommandSpec {
        CommandSpec::new(self.helper.to_string_lossy())
            .arg("--call")
            .args(args.iter().cloned())
    }

    pub fn runner(&self) -> &R {
        &self.runner
    }
}

impl<R: CommandRunner> PrivilegedHelper for HelperClient<R> {
    fn run(&self, args: &[String]) -> Result<()> {
        let spec = self.command(args);
        self.runner.run(&spec)?.into_stdout(&spec).map(drop)
    }
}

// ── Execução ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    RemoveLinks,
    HomeManagerGenerations,
    /// Runtimes Flatpak sem uso da instalação do usuário.
    FlatpakUser,
    /// Tudo o que exige root, numa única chamada ao helper.
    Privileged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    /// Pede senha de administrador (uma autorização do polkit por passo).
    pub privileged: bool,
}

/// Executa um passo do plano. Separado da ordem dos passos para a UI conduzi-los um a um.
pub struct Cleaner<'a> {
    pub nix: &'a dyn NixBackend,
    pub flatpak: &'a dyn FlatpakBackend,
    pub helper: &'a dyn PrivilegedHelper,
    pub home: &'a Path,
    pub home_manager_profile: &'a Path,
}

impl Cleaner<'_> {
    pub fn run(&self, step: Step, plan: &CleanupPlan) -> Result<()> {
        match step.kind {
            StepKind::RemoveLinks => remove_links(&plan.links, self.home),
            StepKind::HomeManagerGenerations => self
                .nix
                .delete_user_generations(self.home_manager_profile, &plan.home_manager_generations),
            StepKind::FlatpakUser => self.flatpak.remove_unused_user(),
            StepKind::Privileged => self.helper.run(&plan.helper_args()),
        }
    }
}

/// Apaga só links simbólicos para o store, dentro do `$HOME`. O que não for isso é ignorado:
/// o plano pode estar velho e o arquivo já ser outra coisa.
pub fn remove_links(links: &[ForgottenRoot], home: &Path) -> Result<()> {
    for root in links {
        for link in &root.links {
            let is_store_link = link.starts_with(home)
                && fs::symlink_metadata(link).is_ok_and(|meta| meta.file_type().is_symlink())
                && fs::read_link(link).is_ok_and(|target| target.starts_with("/nix/store"));
            if !is_store_link {
                tracing::warn!(link = %link.display(), "link ignorado: não é mais um link para o store");
                continue;
            }
            fs::remove_file(link).map_err(|e| Error::io(link, e))?;
        }
    }
    Ok(())
}

/// Qual `GenerationKind` o número pertence: só para mensagens.
pub fn kind_label(kind: GenerationKind) -> &'static str {
    match kind {
        GenerationKind::System => "sistema",
        GenerationKind::HomeManager => "Home Manager",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;

    const DAY: i64 = SECONDS_PER_DAY;

    fn generation(number: u64, age_days: i64, current: bool, boot: bool) -> Generation {
        Generation {
            kind: GenerationKind::System,
            number,
            store_path: format!("/nix/store/g{number}").into(),
            created: 100 * DAY - age_days * DAY,
            kernel: None,
            is_current: current,
            is_boot_default: boot,
        }
    }

    #[test]
    fn politica_mantem_as_n_mais_novas_ou_as_dos_ultimos_dias() {
        // 10 é a mais nova (em uso); 6..9 são recentes; 1..5 são antigas.
        let gens = [
            generation(10, 0, true, true),
            generation(9, 1, false, false),
            generation(8, 3, false, false),
            generation(7, 20, false, false),
            generation(6, 30, false, false),
            generation(5, 40, false, false),
        ];
        // Manter 3 ou as dos últimos 7 dias: sobram 7, 6 e 5 fora (10, 9, 8 são as 3 mais novas).
        assert_eq!(select_for_deletion(&gens, 3, 7, 100 * DAY), [5, 6, 7]);
        // Mais dias de proteção: nenhuma fora.
        assert!(select_for_deletion(&gens, 3, 60, 100 * DAY).is_empty());
        // keep 0 e 0 dias: só a em uso/padrão do boot fica.
        assert_eq!(select_for_deletion(&gens, 0, 0, 100 * DAY), [5, 6, 7, 8, 9]);
    }

    #[test]
    fn com_zero_dias_uma_geracao_criada_agora_ja_pode_sair() {
        // Criada no mesmo segundo do corte: antes (`<`) ficava protegida por engano.
        let gens = [generation(2, 0, true, true), generation(1, 0, false, false)];
        assert_eq!(select_for_deletion(&gens, 0, 0, 100 * DAY), [1]);
    }

    #[test]
    fn nunca_exclui_a_em_uso_nem_a_padrao_do_boot() {
        let gens = [
            generation(3, 90, true, false),
            generation(2, 90, false, true),
            generation(1, 90, false, false),
        ];
        assert_eq!(select_for_deletion(&gens, 0, 0, 100 * DAY), [1]);
    }

    fn root(path: &str, store: &str) -> ForgottenRoot {
        ForgottenRoot {
            path: path.into(),
            links: vec![path.into()],
            store_paths: vec![store.into()],
            age_days: 40,
            selected_by_default: true,
        }
    }

    fn status() -> SystemStatus {
        SystemStatus {
            generations: vec![
                generation(2, 0, true, true),
                generation(1, 50, false, false),
            ],
            forgotten_roots: vec![root("/home/u/proj/result", "/nix/store/link")],
            ..SystemStatus::default()
        }
    }

    #[test]
    fn plano_e_passos_na_ordem_certa() {
        let plan = build_plan(
            &status(),
            Policy {
                keep_system: 1,
                keep_days: 0,
                keep_home_manager: 1,
            },
            &Selection {
                generations: true,
                links: vec!["/home/u/proj/result".into()],
                flatpak: false,
                optimise: true,
            },
            100 * DAY,
        );
        assert_eq!(plan.system_generations, [1]);
        assert_eq!(plan.links.len(), 1);
        let kinds: Vec<_> = plan
            .steps()
            .iter()
            .map(|s| (s.kind, s.privileged))
            .collect();
        assert_eq!(
            kinds,
            [
                (StepKind::RemoveLinks, false),
                // Gerações, GC e otimização: um único passo, uma única senha.
                (StepKind::Privileged, true),
            ]
        );
        assert_eq!(
            plan.helper_args(),
            ["clean", "--generations", "1", "--gc", "--optimise"]
        );
    }

    #[test]
    fn so_deduplicar_nao_roda_gc_e_nada_marcado_e_plano_vazio() {
        let only_optimise = build_plan(
            &status(),
            Policy::default(),
            &Selection {
                optimise: true,
                ..Selection::default()
            },
            100 * DAY,
        );
        assert!(!only_optimise.runs_gc());
        assert_eq!(only_optimise.steps().len(), 1);
        let nothing = build_plan(
            &status(),
            Policy::default(),
            &Selection::default(),
            100 * DAY,
        );
        assert!(nothing.is_empty() && nothing.steps().is_empty());
    }

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn index() -> ClosureIndex {
        ClosureIndex {
            roots: vec![
                IndexedRoot {
                    id: RootId::CurrentSystem,
                    paths: set(&["core", "sys2"]),
                },
                IndexedRoot {
                    id: RootId::SystemGeneration(2),
                    paths: set(&["core", "sys2"]),
                },
                IndexedRoot {
                    id: RootId::SystemGeneration(1),
                    paths: set(&["core", "sys1", "shared-with-link"]),
                },
                IndexedRoot {
                    id: RootId::Link("/home/u/proj/result".into()),
                    paths: set(&["shared-with-link", "link-only"]),
                },
            ],
        }
    }

    #[test]
    fn estimativa_conta_so_o_que_as_raizes_mantidas_nao_seguram() {
        let sizes: HashMap<String, u64> = [
            ("core", 100),
            ("sys1", 10),
            ("sys2", 20),
            ("shared-with-link", 5),
            ("link-only", 7),
            ("dead1", 1000),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let dead = set(&["dead1"]);
        let plan = build_plan(
            &status(),
            Policy {
                keep_system: 1,
                keep_days: 0,
                keep_home_manager: 1,
            },
            &Selection {
                generations: true,
                links: vec!["/home/u/proj/result".into()],
                flatpak: true,
                optimise: false,
            },
            100 * DAY,
        );
        let result = estimate(&index(), &sizes, &dead, &plan, 300);
        // A geração 1 só mantinha `sys1`; `shared-with-link` é do link, mas também da geração 1:
        // as duas saem juntas, então conta como geração.
        assert_eq!(result.generations, 10 + 5);
        assert_eq!(result.links, 7);
        assert_eq!(result.unreferenced, 1000);
        // Sem gerações com Flatpak sem uso no `status`, o plano não inclui Flatpak.
        assert_eq!(result.flatpak, 0);
        assert_eq!(result.total(), 15 + 7 + 1000);
    }

    #[test]
    fn caminho_mantido_por_outra_raiz_nao_conta() {
        let sizes: HashMap<String, u64> = [
            ("core", 100),
            ("sys1", 10),
            ("shared-with-link", 5),
            ("link-only", 7),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        // Só exclui a geração 1; o link continua segurando `shared-with-link`.
        let plan = CleanupPlan {
            system_generations: vec![1],
            ..CleanupPlan::default()
        };
        let result = estimate(&index(), &sizes, &HashSet::new(), &plan, 0);
        assert_eq!(result.generations, 10);
        let per_link = link_sizes(&index(), &sizes);
        // `shared-with-link` também está na geração 1: não é só do link.
        assert_eq!(per_link[&PathBuf::from("/home/u/proj/result")], 7);
    }

    #[derive(Default)]
    struct FakeHelper {
        calls: RefCell<Vec<Vec<String>>>,
        fail_on: Option<&'static str>,
    }

    impl PrivilegedHelper for FakeHelper {
        fn run(&self, args: &[String]) -> Result<()> {
            self.calls.borrow_mut().push(args.to_vec());
            if self.fail_on == Some(args[0].as_str()) {
                return Err(Error::Command {
                    command: "helper".into(),
                    code: Some(65),
                    stderr: "recusado".into(),
                });
            }
            Ok(())
        }
    }

    struct FakeNix(RefCell<Vec<(PathBuf, Vec<u64>)>>);
    // Só o que o `Cleaner` usa; o resto nunca é chamado nestes testes.
    impl NixBackend for FakeNix {
        fn delete_user_generations(&self, profile: &Path, numbers: &[u64]) -> Result<()> {
            self.0
                .borrow_mut()
                .push((profile.to_owned(), numbers.to_vec()));
            Ok(())
        }
        fn flake_metadata(&self, _: &Path) -> Result<crate::flake::FlakeMetadata> {
            unreachable!()
        }
        fn path_info(
            &self,
            _: &[String],
            _: crate::backends::PathInfoOptions,
        ) -> Result<Vec<crate::store::PathInfo>> {
            unreachable!()
        }
        fn store_total_bytes(&self) -> Result<u64> {
            unreachable!()
        }
        fn path_sizes(&self) -> Result<HashMap<String, u64>> {
            unreachable!()
        }
        fn closure_paths(&self, _: &str) -> Result<Vec<String>> {
            unreachable!()
        }
        fn references(&self, _: &str) -> Result<Vec<String>> {
            unreachable!()
        }
        fn gc_dry_run(&self) -> Result<u64> {
            Ok(7)
        }
        fn dead_paths(&self) -> Result<Vec<String>> {
            unreachable!()
        }
        fn diff_closures(&self, _: &str, _: &str) -> Result<Vec<crate::diff::DiffEntry>> {
            unreachable!()
        }
        fn why_depends(&self, _: &str, _: &str) -> Result<Vec<String>> {
            unreachable!()
        }
        fn auto_maintenance(&self, _: &Path, _: &str) -> Result<crate::backends::AutoMaintenance> {
            unreachable!()
        }
        fn flatpak_uninstall_unused(&self, _: &Path, _: &str) -> Result<Option<bool>> {
            unreachable!()
        }
        fn flake_update(&self, _: &Path, _: &[String], _: &Path) -> Result<()> {
            unreachable!()
        }
        fn flake_pin(&self, _: &Path, _: &Path, _: &Path, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
    }

    struct FakeFlatpak(RefCell<u32>);
    impl FlatpakBackend for FakeFlatpak {
        fn summary(&self) -> Result<crate::flatpak::FlatpakSummary> {
            unreachable!()
        }
        fn remove_unused_user(&self) -> Result<()> {
            *self.0.borrow_mut() += 1;
            Ok(())
        }
    }

    #[test]
    fn executor_manda_cada_passo_ao_lugar_certo() {
        let nix = FakeNix(RefCell::default());
        let flatpak = FakeFlatpak(RefCell::new(0));
        let helper = FakeHelper::default();
        let plan = CleanupPlan {
            system_generations: vec![1, 2],
            home_manager_generations: vec![3],
            flatpak: true,
            optimise: true,
            ..CleanupPlan::default()
        };
        let cleaner = Cleaner {
            nix: &nix,
            flatpak: &flatpak,
            helper: &helper,
            home: Path::new("/home/u"),
            home_manager_profile: Path::new("/home/u/.local/state/nix/profiles/home-manager"),
        };
        for step in plan.steps() {
            cleaner.run(step, &plan).unwrap();
        }
        assert_eq!(nix.0.borrow()[0].1, [3]);
        assert_eq!(*flatpak.0.borrow(), 1);
        let calls = helper.calls.borrow();
        assert_eq!(
            calls.as_slice(),
            [["clean", "--generations", "1", "2", "--gc", "--optimise"]],
            "uma só chamada ao helper, na ordem fixa"
        );
    }

    #[test]
    fn flatpak_de_sistema_vai_no_passo_privilegiado_e_falha_do_helper_chega_a_quem_chamou() {
        let (nix, flatpak) = (FakeNix(RefCell::default()), FakeFlatpak(RefCell::new(0)));
        let helper = FakeHelper {
            fail_on: Some("clean"),
            ..FakeHelper::default()
        };
        let cleaner = Cleaner {
            nix: &nix,
            flatpak: &flatpak,
            helper: &helper,
            home: Path::new("/home/u"),
            home_manager_profile: Path::new("/p"),
        };
        let plan = CleanupPlan {
            flatpak: true,
            flatpak_system: true,
            ..CleanupPlan::default()
        };
        assert_eq!(plan.helper_args(), ["clean", "--flatpak"]);
        let err = cleaner.run(plan.steps()[0], &plan).unwrap_err();
        assert!(err.to_string().contains("recusado"));
        assert_eq!(
            *flatpak.0.borrow(),
            0,
            "a instalação de sistema nunca usa o flatpak direto"
        );
    }

    #[test]
    fn simulacao_so_consulta_o_gc_quando_ha_o_que_coletar() {
        let nix = FakeNix(RefCell::default());
        let with_gc = CleanupPlan {
            system_generations: vec![1],
            ..CleanupPlan::default()
        };
        assert_eq!(dry_run(&nix, &with_gc).unwrap().gc_paths, Some(7));
        let without = CleanupPlan {
            optimise: true,
            ..CleanupPlan::default()
        };
        assert_eq!(dry_run(&nix, &without).unwrap().gc_paths, None);
    }

    #[test]
    fn remover_links_so_apaga_link_para_o_store_dentro_do_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(dir.path()).unwrap();
        let good = home.join("result");
        symlink("/nix/store/aaa-x", &good).unwrap();
        let elsewhere = home.join("outro");
        symlink("/tmp/qualquer", &elsewhere).unwrap();
        let regular = home.join("arquivo");
        fs::write(&regular, "x").unwrap();
        let outside = tempfile::tempdir().unwrap();
        let foreign = outside.path().join("result");
        symlink("/nix/store/bbb-y", &foreign).unwrap();

        let mk = |path: &Path| ForgottenRoot {
            path: path.to_owned(),
            links: vec![path.to_owned()],
            store_paths: vec![],
            age_days: 5,
            selected_by_default: true,
        };
        remove_links(
            &[mk(&good), mk(&elsewhere), mk(&regular), mk(&foreign)],
            &home,
        )
        .unwrap();
        assert!(
            !good.exists() && good.symlink_metadata().is_err(),
            "o link para o store sai"
        );
        assert!(
            elsewhere.symlink_metadata().is_ok(),
            "link que não aponta ao store fica"
        );
        assert!(regular.exists(), "arquivo comum fica");
        assert!(foreign.symlink_metadata().is_ok(), "fora do HOME fica");
    }

    #[test]
    fn disponibilidade_do_helper() {
        use crate::runner::SystemRunner;
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let p = dir.path().join(name);
            fs::write(&p, "").unwrap();
            p
        };
        let (socket, helper, policy) = (file("socket"), file("helper"), file("policy"));
        let missing = dir.path().join("nada");
        let client = |p: &Path, h: &Path, a: &Path| {
            HelperClient::new(SystemRunner, p.into(), h.into(), a.into())
        };
        assert_eq!(client(&socket, &helper, &policy).check(), Ok(()));
        assert_eq!(
            client(&missing, &helper, &policy).check(),
            Err(HelperIssue::SocketMissing)
        );
        assert_eq!(
            client(&socket, &missing, &policy).check(),
            Err(HelperIssue::HelperMissing)
        );
        assert_eq!(
            client(&socket, &helper, &missing).check(),
            Err(HelperIssue::PolicyMissing)
        );
    }

    #[test]
    fn cliente_do_helper_monta_helper_call_e_argumentos() {
        use crate::runner::Output;
        struct Capture(RefCell<Vec<CommandSpec>>);
        impl CommandRunner for Capture {
            fn run(&self, spec: &CommandSpec) -> Result<Output> {
                self.0.borrow_mut().push(spec.clone());
                Ok(Output {
                    stdout: String::new(),
                    stderr: String::new(),
                    code: Some(0),
                })
            }
        }
        let capture = Capture(RefCell::default());
        let client = HelperClient::new(
            &capture,
            "/w/socket".into(),
            "/s/helper".into(),
            "/p".into(),
        );
        client
            .run(&["delete-generations".into(), "4".into()])
            .unwrap();
        let spec = capture.0.borrow()[0].clone();
        assert_eq!(spec.program, "/s/helper");
        assert_eq!(spec.args, ["--call", "delete-generations", "4"]);
        assert!(spec.env.is_empty(), "o ambiente do usuário não é repassado");
    }

    #[test]
    fn razao_do_disco_so_reduz_quando_a_soma_logica_passa_do_disco() {
        let sizes = HashMap::from([("a".to_owned(), 300), ("b".to_owned(), 100)]);
        assert_eq!(disk_ratio(&sizes, 1000), 1.0);
        assert_eq!(disk_ratio(&sizes, 200), 0.5);
        assert_eq!(disk_ratio(&HashMap::new(), 200), 1.0);
        let estimate = Estimate {
            generations: 100,
            links: 40,
            unreferenced: 400,
            flatpak: 9,
        };
        let scaled = estimate.scaled(0.5);
        assert_eq!(
            (
                scaled.generations,
                scaled.links,
                scaled.unreferenced,
                scaled.flatpak
            ),
            (50, 20, 200, 9)
        );
    }
}
