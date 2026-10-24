//! Para onde vai o espaço do Nix store: em uso (sistema, Home Manager, ambientes) e o que
//! pode sair (gerações antigas, links esquecidos, caminhos sem referência).
//!
//! É uma estimativa: o Nix não diz quem mantém cada caminho vivo, então as categorias saem de
//! conjuntos de closures e de qual categoria "vence" quando um caminho está em mais de uma.

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::backends::{NixBackend, PathInfoOptions};
use crate::generations::Generation;
use crate::status::{StatusEnv, SystemStatus};
use crate::store::store_name;

/// Conjuntos de caminhos do store de cada categoria. Os de cima vencem os de baixo.
/// O Home Manager vem antes do sistema: quando ele é um módulo do NixOS, a closure dele está
/// dentro da do sistema e, ao contrário, ficaria sempre em 0.
#[derive(Debug)]
pub struct SpaceInputs<'a> {
    /// `narSize` de todos os caminhos do store.
    pub sizes: &'a HashMap<String, u64>,
    /// Caminhos que o GC apagaria agora.
    pub dead: &'a HashSet<String>,
    /// Closure do sistema em uso e do padrão do boot.
    pub system: &'a HashSet<String>,
    /// Closure do Home Manager em uso.
    pub home_manager: &'a HashSet<String>,
    /// Closures das demais gerações (sistema e Home Manager).
    pub old_generations: &'a HashSet<String>,
    /// Closures dos links `result` e `.direnv` esquecidos.
    pub forgotten_roots: &'a HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpaceBreakdown {
    pub total: u64,
    // Em uso
    pub system: u64,
    pub home_manager: u64,
    pub dev_environments: u64,
    // Pode sair
    pub old_generations: u64,
    pub forgotten_roots: u64,
    pub unreferenced: u64,
}

impl SpaceBreakdown {
    pub fn in_use(&self) -> u64 {
        self.system + self.home_manager + self.dev_environments
    }

    pub fn reclaimable(&self) -> u64 {
        self.old_generations + self.forgotten_roots + self.unreferenced
    }
}

pub fn breakdown(inputs: &SpaceInputs<'_>) -> SpaceBreakdown {
    let mut result = SpaceBreakdown::default();
    for (path, size) in inputs.sizes {
        result.total += size;
        let bucket = if inputs.dead.contains(path) {
            &mut result.unreferenced
        } else if inputs.home_manager.contains(path) {
            &mut result.home_manager
        } else if inputs.system.contains(path) {
            &mut result.system
        } else if inputs.old_generations.contains(path) {
            &mut result.old_generations
        } else if inputs.forgotten_roots.contains(path) {
            &mut result.forgotten_roots
        } else {
            &mut result.dev_environments
        };
        *bucket += size;
    }
    result
}

/// Caminhos do store de onde saem as closures de cada categoria.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootPaths {
    /// Sistema em uso e padrão do boot.
    pub system: Vec<String>,
    pub home_manager: Vec<String>,
    /// Gerações do sistema e do Home Manager que não estão em uso.
    pub other_generations: Vec<String>,
    pub forgotten_roots: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClosureSets {
    pub system: HashSet<String>,
    pub home_manager: HashSet<String>,
    pub old_generations: HashSet<String>,
    pub forgotten_roots: HashSet<String>,
}

/// Raízes de cada categoria, a partir do estado já coletado.
///
/// O Home Manager em uso vem do link `current-home`: quando ele é ativado como módulo do NixOS
/// não gera `home-manager-N-link`, e as gerações listadas são só as de um uso standalone antigo.
pub fn root_paths(env: &StatusEnv, status: &SystemStatus) -> RootPaths {
    let path = |g: &Generation| g.store_path.to_string_lossy().into_owned();
    let canonical = |link: &std::path::Path| {
        fs::canonicalize(link)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    };

    let mut system: Vec<String> = status
        .generations
        .iter()
        .filter(|g| g.is_current || g.is_boot_default)
        .map(path)
        .collect();
    system.extend(canonical(&env.current_system));
    let mut home_manager: Vec<String> = status
        .home_manager_generations
        .iter()
        .filter(|g| g.is_current)
        .map(path)
        .collect();
    home_manager.extend(canonical(&env.current_home));
    for list in [&mut system, &mut home_manager] {
        list.sort();
        list.dedup();
    }

    let other_generations = status
        .generations
        .iter()
        .filter(|g| !g.is_current && !g.is_boot_default)
        .chain(
            status
                .home_manager_generations
                .iter()
                .filter(|g| !g.is_current),
        )
        .map(path)
        .filter(|p| !home_manager.contains(p))
        .collect();
    let forgotten_roots = status
        .forgotten_roots
        .iter()
        .flat_map(|root| root.store_paths.iter())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    RootPaths {
        system,
        home_manager,
        other_generations,
        forgotten_roots,
    }
}

/// União das closures de cada grupo de raízes (uma consulta por raiz, décimos de segundo).
pub fn compute_sets(nix: &dyn NixBackend, roots: &RootPaths) -> crate::Result<ClosureSets> {
    let union = |paths: &[String]| -> crate::Result<HashSet<String>> {
        let mut set = HashSet::new();
        for path in paths {
            set.extend(nix.closure_paths(path)?);
        }
        Ok(set)
    };
    Ok(ClosureSets {
        system: union(&roots.system)?,
        home_manager: union(&roots.home_manager)?,
        old_generations: union(&roots.other_generations)?,
        forgotten_roots: union(&roots.forgotten_roots)?,
    })
}

/// Maiores pacotes por tamanho de closure, entre os que o sistema instala em `sw`.
pub fn top_packages(
    nix: &dyn NixBackend,
    env: &StatusEnv,
    limit: usize,
) -> crate::Result<Vec<PackageSize>> {
    let sw = fs::canonicalize(env.current_system.join("sw"))
        .map_err(|e| crate::Error::io(env.current_system.join("sw"), e))?;
    let refs = nix.references(&sw.to_string_lossy())?;
    let infos = nix.path_info(
        &refs,
        PathInfoOptions {
            closure_size: true,
            recursive: false,
        },
    )?;
    Ok(top_by_closure(&infos, limit))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSize {
    pub name: String,
    pub path: String,
    pub closure_bytes: u64,
}

/// Os `limit` maiores por tamanho de closure. Desempata pelo nome, para a ordem ser estável.
pub fn top_by_closure(infos: &[crate::store::PathInfo], limit: usize) -> Vec<PackageSize> {
    let mut packages: Vec<PackageSize> = infos
        .iter()
        .filter_map(|info| {
            Some(PackageSize {
                name: store_name(&info.path).to_owned(),
                path: info.path.clone(),
                closure_bytes: info.closure_size?,
            })
        })
        .collect();
    packages.sort_by(|a, b| {
        b.closure_bytes
            .cmp(&a.closure_bytes)
            .then_with(|| a.name.cmp(&b.name))
    });
    packages.truncate(limit);
    packages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PathInfo;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn cada_caminho_cai_numa_so_categoria() {
        let sizes: HashMap<String, u64> = [
            ("sys", 100),
            ("sys+hm", 10),
            ("hm", 20),
            ("old", 30),
            ("link", 40),
            ("dev", 50),
            ("dead", 60),
            ("old+link", 5),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let dead = set(&["dead"]);
        let system = set(&["sys", "sys+hm"]);
        let hm = set(&["sys+hm", "hm"]);
        let old = set(&["old", "old+link", "sys"]);
        let forgotten = set(&["link", "old+link"]);
        let result = breakdown(&SpaceInputs {
            sizes: &sizes,
            dead: &dead,
            system: &system,
            home_manager: &hm,
            old_generations: &old,
            forgotten_roots: &forgotten,
        });
        assert_eq!(result.system, 100);
        assert_eq!(result.home_manager, 30);
        assert_eq!(result.old_generations, 35);
        assert_eq!(result.forgotten_roots, 40);
        assert_eq!(result.dev_environments, 50);
        assert_eq!(result.unreferenced, 60);
        assert_eq!(result.total, sizes.values().sum::<u64>());
        assert_eq!(result.in_use() + result.reclaimable(), result.total);
    }

    #[test]
    fn raizes_usam_o_current_home_mesmo_sem_geracoes_do_home_manager() {
        use crate::generations::{Generation, GenerationKind};
        use crate::status::{StatusEnv, SystemStatus};
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let store = |name: &str| {
            let path = root.join(name);
            std::fs::create_dir(&path).unwrap();
            path
        };
        let (system, home, old, old_hm) = (store("sys"), store("hm"), store("old"), store("oldhm"));
        symlink(&system, root.join("current-system")).unwrap();
        symlink(&home, root.join("current-home")).unwrap();
        let generation = |kind, number, path: &std::path::Path, current, boot| Generation {
            kind,
            number,
            store_path: path.to_owned(),
            created: 0,
            kernel: None,
            is_current: current,
            is_boot_default: boot,
        };
        let status = SystemStatus {
            generations: vec![
                generation(GenerationKind::System, 2, &system, true, true),
                generation(GenerationKind::System, 1, &old, false, false),
            ],
            // Geração standalone antiga do Home Manager: não é a que está em uso.
            home_manager_generations: vec![generation(
                GenerationKind::HomeManager,
                5,
                &old_hm,
                false,
                false,
            )],
            ..SystemStatus::default()
        };
        let mut env = StatusEnv::detect(root.clone(), Some("h".into()));
        env.current_system = root.join("current-system");
        env.current_home = root.join("current-home");

        let roots = root_paths(&env, &status);
        let text = |p: &std::path::Path| p.to_string_lossy().into_owned();
        assert_eq!(roots.system, [text(&system)]);
        assert_eq!(roots.home_manager, [text(&home)]);
        assert_eq!(roots.other_generations, [text(&old), text(&old_hm)]);
        assert!(roots.forgotten_roots.is_empty());
    }

    #[test]
    fn maiores_pacotes() {
        let hash = "0123456789abcdfghijklmnpqrsvwxyz";
        let info = |name: &str, size: Option<u64>| PathInfo {
            path: format!("/nix/store/{hash}-{name}"),
            nar_size: 1,
            closure_size: size,
            references: vec![],
        };
        let infos = [
            info("b", Some(5)),
            info("a", Some(5)),
            info("c", Some(9)),
            info("sem", None),
        ];
        let top = top_by_closure(&infos, 2);
        assert_eq!(
            top.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["c", "a"]
        );
        assert_eq!(top[0].closure_bytes, 9);
    }
}
