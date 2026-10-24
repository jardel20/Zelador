//! Parsers e backends exercitados com saídas reais capturadas em `tests/fixtures/`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use common::{FixtureRunner, fixture, parse_ls_links};
use zelador_core::backends::{
    CliFlatpak, CliGit, CliNix, FlatpakBackend, GitBackend, NixBackend, PathInfoOptions,
};
use zelador_core::diff::summarize;
use zelador_core::flatpak::Installation;
use zelador_core::gcroots::{self, RootCandidate};
use zelador_core::generations::{GenerationKind, list_generations};
use zelador_core::progress::{ProgressTracker, extract_build_failure, parse_line};
use zelador_core::runner::SystemRunner;

const FLAKE: &str = "/etc/nixos";
const SYSTEM_446: &str =
    "/nix/store/ild1z1rj4mmi4nkv41l6drgsy9gn39cc-nixos-system-nixos-26.11.20261020.02c087f";

#[test]
fn inputs_do_flake_real() {
    let runner = FixtureRunner::new().with_fixture(
        "nix flake metadata /etc/nixos --json",
        "flake-metadata.json",
    );
    let meta = CliNix::new(runner)
        .flake_metadata(Path::new(FLAKE))
        .unwrap();
    let names: Vec<_> = meta.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "home-manager",
            "niri",
            "nirinit",
            "nix-flatpak",
            "nixpkgs",
            "nixpkgs-quarto",
            "nixpkgs-r"
        ]
    );
    let nixpkgs = meta.inputs.iter().find(|i| i.name == "nixpkgs").unwrap();
    assert_eq!(nixpkgs.kind, "github");
    assert_eq!(nixpkgs.reference.as_deref(), Some("nixos-unstable"));
    // Sem ref pedida, o rótulo mostra de onde o input vem.
    let niri = meta.inputs.iter().find(|i| i.name == "niri").unwrap();
    assert!(
        niri.source.as_deref().is_some_and(|s| s.contains('/')),
        "{:?}",
        niri.source
    );
    assert!(niri.label().contains(" · "));
    assert_eq!(nixpkgs.short_rev().map(str::len), Some(7));
    assert!(nixpkgs.last_modified.is_some());
    assert!(meta.dirty_revision.is_some_and(|r| r.ends_with("-dirty")));
}

#[test]
fn tamanho_da_closure_atual() {
    let runner = FixtureRunner::new().with_fixture(
        "nix path-info --json --closure-size /run/current-system",
        "path-info-current-system.json",
    );
    let infos = CliNix::new(runner)
        .path_info(
            &["/run/current-system".to_owned()],
            PathInfoOptions {
                closure_size: true,
                recursive: false,
            },
        )
        .unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].path, SYSTEM_446);
    assert_eq!(infos[0].closure_size, Some(31_396_752_552));
    assert!(!infos[0].references.is_empty());
}

#[test]
fn total_do_store_a_partir_da_amostra() {
    let runner =
        FixtureRunner::new().with_fixture("nix path-info --json --all", "path-info-sample.json");
    let total = CliNix::new(runner).store_total_bytes().unwrap();
    assert!(total > 1_000_000_000, "total = {total}");
}

#[test]
fn caminhos_mortos_reais() {
    let runner =
        FixtureRunner::new().with_fixture("nix-store --gc --print-dead", "gc-print-dead.txt");
    let dead = CliNix::new(runner).dead_paths().unwrap();
    assert!(dead.len() > 500, "{}", dead.len());
    assert!(dead.iter().all(|p| p.starts_with("/nix/store/")));
    assert_eq!(dead.iter().collect::<HashSet<_>>().len(), dead.len());
}

/// Backend que devolve os caminhos mortos reais e um tamanho fixo por caminho.
struct DeadPathsFixture;

impl NixBackend for DeadPathsFixture {
    fn flake_metadata(&self, _: &Path) -> zelador_core::Result<zelador_core::flake::FlakeMetadata> {
        unreachable!()
    }
    fn path_info(
        &self,
        paths: &[String],
        _: PathInfoOptions,
    ) -> zelador_core::Result<Vec<zelador_core::store::PathInfo>> {
        Ok(paths
            .iter()
            .map(|path| zelador_core::store::PathInfo {
                path: path.clone(),
                nar_size: 1_000,
                closure_size: None,
                references: Vec::new(),
            })
            .collect())
    }
    fn store_total_bytes(&self) -> zelador_core::Result<u64> {
        unreachable!()
    }
    fn path_sizes(&self) -> zelador_core::Result<std::collections::HashMap<String, u64>> {
        unreachable!()
    }
    fn closure_paths(&self, _: &str) -> zelador_core::Result<Vec<String>> {
        unreachable!()
    }
    fn references(&self, _: &str) -> zelador_core::Result<Vec<String>> {
        unreachable!()
    }
    fn gc_dry_run(&self) -> zelador_core::Result<u64> {
        unreachable!("a estimativa não deve rodar o dry-run")
    }
    fn dead_paths(&self) -> zelador_core::Result<Vec<String>> {
        Ok(zelador_core::store::parse_dead_paths(&fixture(
            "gc-print-dead.txt",
        )))
    }
    fn diff_closures(
        &self,
        _: &str,
        _: &str,
    ) -> zelador_core::Result<Vec<zelador_core::diff::DiffEntry>> {
        unreachable!()
    }
    fn why_depends(&self, _: &str, _: &str) -> zelador_core::Result<Vec<String>> {
        unreachable!()
    }
    fn auto_maintenance(
        &self,
        _: &Path,
        _: &str,
    ) -> zelador_core::Result<zelador_core::backends::AutoMaintenance> {
        unreachable!()
    }
    fn flatpak_uninstall_unused(&self, _: &Path, _: &str) -> zelador_core::Result<Option<bool>> {
        unreachable!()
    }
    fn delete_user_generations(&self, _: &Path, _: &[u64]) -> zelador_core::Result<()> {
        unreachable!()
    }
}

#[test]
fn estimativa_do_gc_soma_os_caminhos_mortos() {
    let estimate = zelador_core::status::gc_estimate(&DeadPathsFixture).unwrap();
    assert_eq!(estimate.bytes, estimate.paths * 1_000);
    assert!(estimate.paths > 500);
}

#[test]
fn simulacao_do_gc() {
    let runner = FixtureRunner::new().with_fixture("nix store gc --dry-run", "gc-dry-run.txt");
    assert_eq!(CliNix::new(runner).gc_dry_run().unwrap(), 807);
}

#[test]
fn diff_de_geracoes() {
    let cmd = "nix store diff-closures /nix/var/nix/profiles/system-444-link /nix/var/nix/profiles/system-446-link";
    let runner = FixtureRunner::new().with_fixture(cmd, "diff-closures-444-446.txt");
    let entries = CliNix::new(runner)
        .diff_closures(
            "/nix/var/nix/profiles/system-444-link",
            "/nix/var/nix/profiles/system-446-link",
        )
        .unwrap();
    let find = |name: &str| entries.iter().find(|e| e.name == name).unwrap();
    assert!(find("aalib").is_removed());
    assert!(find("aws-c-auth").is_added());
    assert_eq!(
        find("bash").new.as_deref(),
        Some(&["5.3p20".to_owned()][..])
    );
    assert_eq!(find("claude-code").size_delta, Some(18 * 1024 * 1024));
    let summary = summarize(&entries);
    assert!(summary.updated > 10 && summary.added > 0 && summary.removed > 0);
    assert!(entries.iter().all(|e| !e.name.contains('\u{1b}')));
}

#[test]
fn cadeia_do_why_depends() {
    let fw = "/nix/store/4q35kbgs7jdk5q4da755sl1zhz459cyk-linux-firmware-20261019-zstd";
    let runner = FixtureRunner::new().with_fixture(
        &format!("nix why-depends /run/current-system {fw}"),
        "why-depends.txt",
    );
    let chain = CliNix::new(runner)
        .why_depends("/run/current-system", fw)
        .unwrap();
    assert_eq!(chain.first().map(String::as_str), Some(SYSTEM_446));
    assert_eq!(chain.last().map(String::as_str), Some(fw));
}

#[test]
fn gc_e_optimise_automaticos() {
    let cmd = "nix eval --json /etc/nixos#nixosConfigurations.nixos.config.nix --apply c: { gc = c.gc.automatic; optimise = c.optimise.automatic; }";
    let runner = FixtureRunner::new().with_fixture(cmd, "nix-gc-optimise.json");
    let auto = CliNix::new(runner)
        .auto_maintenance(Path::new(FLAKE), "nixos")
        .unwrap();
    assert!(auto.gc);
    assert!(!auto.optimise);
}

#[test]
fn opcao_ausente_do_nix_flatpak_vira_none() {
    let runner = FixtureRunner::new(); // qualquer comando sem fixture sai com 127
    let value = CliNix::new(runner)
        .flatpak_uninstall_unused(Path::new(FLAKE), "nixos")
        .unwrap();
    assert_eq!(value, None);
}

#[test]
fn git_status_real_e_arquivos_fora_do_git() {
    let runner = FixtureRunner::new().with_fixture(
        "git -C /etc/nixos status --porcelain=v2 --branch",
        "git-status-v2.txt",
    );
    let status = CliGit::new(runner).status(Path::new(FLAKE)).unwrap();
    assert_eq!(status.branch.as_deref(), Some("master"));
    assert_eq!(
        status.modified,
        [
            "flake.lock",
            "modules/home.nix",
            "modules/noctalia-config.toml"
        ]
    );
    assert!(status.untracked.is_empty());
    assert!(status.is_dirty());
    assert!(fixture("git-untracked.txt").is_empty());
}

#[test]
fn flatpak_real() {
    let runner = FixtureRunner::new()
        .with_fixture(
            "flatpak list --app --columns=application,branch,size,installation",
            "flatpak-apps.txt",
        )
        .with_fixture(
            "flatpak list --runtime --columns=application,branch,size,installation",
            "flatpak-runtimes.txt",
        )
        // A fixture real tem exit 1: a resposta "n" ao prompt.
        .with_output(
            "flatpak uninstall --unused",
            &fixture("flatpak-unused.txt"),
            1,
        );
    let summary = CliFlatpak::new(runner).summary().unwrap();
    assert_eq!(summary.apps.len(), 1);
    assert_eq!(summary.runtimes.len(), 13);
    assert_eq!(summary.unused.len(), 11);
    assert_eq!(summary.installation, Installation::System);
    // Platform.Locale (24.08, 49 e 50) não aparece no `list` sem `--all`.
    assert!(summary.unused_bytes_partial);
    assert!(
        summary.unused_bytes > 3_000_000_000,
        "{}",
        summary.unused_bytes
    );
    assert!(summary.total_bytes > summary.unused_bytes);
}

#[test]
fn flatpak_responde_n_ao_prompt_e_fixa_o_idioma() {
    let columns = "--columns=application,branch,size,installation";
    let runner = FixtureRunner::new()
        .with_output(&format!("flatpak list --app {columns}"), "", 0)
        .with_output(&format!("flatpak list --runtime {columns}"), "", 0)
        .with_output("flatpak uninstall --unused", "", 1);
    CliFlatpak::new(&runner).summary().unwrap();
    let calls = runner.calls.borrow();
    let uninstall = calls
        .iter()
        .find(|c| c.args.first().map(String::as_str) == Some("uninstall"))
        .unwrap();
    assert_eq!(uninstall.stdin.as_deref(), Some("n\n"));
    assert!(
        calls
            .iter()
            .all(|c| c.env == [("LC_ALL".to_owned(), "C".to_owned())])
    );
}

#[test]
fn geracoes_do_sistema_a_partir_dos_links_reais() {
    let dir = tempfile::tempdir().unwrap();
    let links = parse_ls_links(&fixture("profiles-system.txt"));
    make_links(dir.path(), &links);
    let current = Path::new(SYSTEM_446);
    let generations =
        list_generations(dir.path(), "system", GenerationKind::System, Some(current)).unwrap();
    let numbers: Vec<_> = generations.iter().map(|g| g.number).collect();
    assert_eq!(numbers, [446, 445, 444]);
    assert!(generations[0].is_current && generations[0].is_boot_default);
    assert!(
        generations[1..]
            .iter()
            .all(|g| !g.is_current && !g.is_boot_default)
    );
    // 2026-10-24 12:47:39 -0300
    assert_eq!(generations[0].created, 1_792_856_859);
}

#[test]
fn geracoes_do_home_manager_a_partir_dos_links_reais() {
    let dir = tempfile::tempdir().unwrap();
    make_links(dir.path(), &parse_ls_links(&fixture("profiles-home.txt")));
    let generations = list_generations(
        dir.path(),
        "home-manager",
        GenerationKind::HomeManager,
        None,
    )
    .unwrap();
    assert_eq!(
        generations.iter().map(|g| g.number).collect::<Vec<_>>(),
        [5, 4, 3, 2, 1]
    );
    assert!(
        generations
            .iter()
            .all(|g| !g.is_boot_default && g.kernel.is_none())
    );
}

fn make_links(dir: &Path, links: &[common::LsLink]) {
    for link in links {
        let path = dir.join(&link.name);
        std::os::unix::fs::symlink(&link.target, &path).unwrap();
        let time = filetime::FileTime::from_unix_time(link.epoch, 0);
        filetime::set_symlink_file_times(&path, time, time).unwrap();
    }
}

fn candidates_from_roots_fixture() -> Vec<RootCandidate> {
    fixture("gc-roots.txt")
        .lines()
        .filter_map(|line| {
            let (link, target) = line.split_once(" -> ")?;
            Some(RootCandidate {
                link: link.trim_matches('"').into(),
                store_path: target.into(),
                mtime: 0,
            })
        })
        .collect()
}

#[test]
fn roots_reais_do_home_nao_sao_esquecidos() {
    // Os únicos roots do $HOME nesta máquina são do Home Manager e do cache do Nix.
    let candidates = candidates_from_roots_fixture();
    assert!(candidates.iter().any(|c| c.link.starts_with("/home/user")));
    assert!(gcroots::forgotten_roots(&candidates, Path::new("/home/user"), 10_000_000).is_empty());
}

#[test]
fn le_links_automaticos_do_gc() {
    let dir = tempfile::tempdir().unwrap();
    let auto = dir.path().join("auto");
    let project = dir.path().join("proj");
    fs::create_dir_all(&auto).unwrap();
    fs::create_dir_all(&project).unwrap();
    let result = project.join("result");
    std::os::unix::fs::symlink("/nix/store/aaa-sistema", &result).unwrap();
    std::os::unix::fs::symlink(&result, auto.join("h1")).unwrap();
    // Aponta para fora do store e para um link que já não existe: ambos ignorados.
    let other = project.join("outro");
    std::os::unix::fs::symlink("/tmp/qualquer", &other).unwrap();
    std::os::unix::fs::symlink(&other, auto.join("h2")).unwrap();
    std::os::unix::fs::symlink(project.join("sumiu"), auto.join("h3")).unwrap();

    let roots = gcroots::read_auto_roots(&auto).unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].link, result);
    let forgotten = gcroots::forgotten_roots(&roots, dir.path(), roots[0].mtime + 3 * 86_400);
    assert_eq!(forgotten.len(), 1);
    assert_eq!(forgotten[0].age_days, 3);
    assert!(forgotten[0].selected_by_default);
}

#[test]
fn falha_de_build_real() {
    let events: Vec<_> = fixture("build-fail-internal-json.txt")
        .lines()
        .filter_map(parse_line)
        .collect();
    assert!(events.len() > 60, "{}", events.len());
    let mut tracker = ProgressTracker::default();
    events.iter().for_each(|e| tracker.push(e));
    let failure = extract_build_failure(&events, tracker.log_tail()).unwrap();
    assert!(
        failure.headline.starts_with("error: Cannot build"),
        "{}",
        failure.headline
    );
    assert!(failure.details.contains("builder failed with exit code 1"));
    assert!(failure.details.contains("boom"));
    assert!(!failure.details.contains('\u{1b}'));
}

#[test]
fn incremental_de_geracoes_reais_usa_referencias_do_store() {
    // Fecha o ciclo path-info → conjuntos de caminhos → tamanho incremental.
    let infos =
        zelador_core::store::parse_path_info(&fixture("path-info-current-system.json")).unwrap();
    let paths: HashSet<String> = infos
        .iter()
        .flat_map(|i| std::iter::once(i.path.clone()).chain(i.references.clone()))
        .collect();
    let sizes = infos.iter().map(|i| (i.path.clone(), i.nar_size)).collect();
    let result = zelador_core::generations::incremental_sizes(&[(446, paths)], &sizes);
    assert_eq!(result, [(446, infos[0].nar_size)]);
}

// ---- git de verdade, em repositório temporário ----

fn git(dir: &Path, args: &[&str], date: Option<&str>) {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    if let Some(date) = date {
        cmd.env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn commits_pendentes_com_git_real() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"], None);
    git(repo, &["config", "user.email", "t@t"], None);
    git(repo, &["config", "user.name", "t"], None);
    let mut dates = ["1000000000", "1000001000", "1000002000"].into_iter();
    for n in 0..3 {
        fs::write(repo.join("f"), n.to_string()).unwrap();
        git(repo, &["add", "f"], None);
        git(
            repo,
            &["commit", "-q", "-m", &format!("c{n}")],
            dates.next(),
        );
    }
    fs::write(repo.join("novo.nix"), "").unwrap();

    let backend = CliGit::new(SystemRunner);
    assert_eq!(backend.commits_since_time(repo, 1_000_000_500).unwrap(), 2);
    assert_eq!(backend.commits_since_rev(repo, "HEAD~2").unwrap(), Some(2));
    assert_eq!(
        backend
            .commits_since_rev(repo, "0000000000000000000000000000000000000000")
            .unwrap(),
        None
    );
    assert_eq!(backend.status(repo).unwrap().untracked, ["novo.nix"]);

    backend.add(repo, &["novo.nix".to_owned()]).unwrap();
    let status = backend.status(repo).unwrap();
    assert!(status.untracked.is_empty());
    assert_eq!(status.modified, ["novo.nix"]);
}

// ---- tamanho incremental das gerações, via CliNix ----

#[test]
fn tamanho_incremental_das_geracoes_usa_uma_consulta_recursiva_por_geracao() {
    use zelador_core::generations::{Generation, GenerationKind, generation_sizes};
    let generation = |number: u64, path: &str| Generation {
        kind: GenerationKind::System,
        number,
        store_path: path.into(),
        created: 0,
        kernel: None,
        is_current: false,
        is_boot_default: false,
    };
    // Geração 2 (nova): a, b. Geração 1 (antiga): b, c. Só `c` é novo para a 1.
    let two = r#"{"/nix/store/a-x":{"narSize":10},"/nix/store/b-x":{"narSize":20}}"#;
    let one = r#"{"/nix/store/b-x":{"narSize":20},"/nix/store/c-x":{"narSize":40}}"#;
    let runner = FixtureRunner::new()
        .with_output(
            "nix path-info --json --recursive /nix/store/g2-system",
            two,
            0,
        )
        .with_output(
            "nix path-info --json --recursive /nix/store/g1-system",
            one,
            0,
        );
    let nix = CliNix::new(runner);
    let sizes = generation_sizes(
        &nix,
        &[
            generation(1, "/nix/store/g1-system"),
            generation(2, "/nix/store/g2-system"),
        ],
    )
    .unwrap();
    assert_eq!(sizes, [(2, 30), (1, 40)]);
}

#[test]
fn closures_e_referencias_do_store() {
    let runner = FixtureRunner::new()
        .with_output(
            "nix-store --query --requisites /nix/store/a-x",
            "/nix/store/b-y\n/nix/store/a-x\n",
            0,
        )
        .with_output(
            "nix-store --query --references /nix/store/a-x",
            "/nix/store/b-y\n",
            0,
        );
    let nix = CliNix::new(runner);
    assert_eq!(
        nix.closure_paths("/nix/store/a-x").unwrap(),
        ["/nix/store/b-y", "/nix/store/a-x"]
    );
    assert_eq!(
        nix.references("/nix/store/a-x").unwrap(),
        ["/nix/store/b-y"]
    );
}

#[test]
fn tamanhos_de_todo_o_store_sem_alocar_referencias() {
    let runner =
        FixtureRunner::new().with_fixture("nix path-info --json --all", "path-info-sample.json");
    let sizes = CliNix::new(runner).path_sizes().unwrap();
    assert!(sizes.len() >= 300);
    assert!(sizes.values().sum::<u64>() > 1_000_000_000);
}
