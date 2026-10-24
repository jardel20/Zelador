//! CLI de debug do `zelador-core`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use zelador_core::backends::{CliFlatpak, CliGit, CliNix};
use zelador_core::flake::DEFAULT_STALE_DAYS;
use zelador_core::runner::SystemRunner;
use zelador_core::status::{self, PendingChanges, StatusEnv, SystemStatus};
use zelador_core::units::format_bytes;

const USAGE: &str = "uso: zelador-core <status|space> [--flake <pasta>] [--host <nome>]";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("erro: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    match command.as_deref() {
        Some("status" | "space") => {}
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => bail!("subcomando desconhecido: {other}\n{USAGE}"),
    }
    let mut flake = PathBuf::from("/etc/nixos");
    let mut host = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--flake" => flake = args.next().context("--flake pede uma pasta")?.into(),
            "--host" => host = Some(args.next().context("--host pede um nome")?),
            other => bail!("argumento desconhecido: {other}\n{USAGE}"),
        }
    }

    let env = StatusEnv::detect(flake, host);
    let nix = CliNix::new(SystemRunner);
    let git = CliGit::new(SystemRunner);
    let flatpak = CliFlatpak::new(SystemRunner);
    if command.as_deref() == Some("space") {
        return print_space(&env, &nix, &git, &flatpak);
    }
    let result = status::collect(&env, &nix, &git, &flatpak);
    print_status(&env, &result);
    Ok(())
}

fn print_status(env: &StatusEnv, status: &SystemStatus) {
    println!("Flake: {} (host {})", env.flake.display(), env.host);
    if let Some(version) = &status.nixos_version {
        println!("NixOS {version}");
    }

    println!("\n== Seu sistema ==");
    match status.current_generation() {
        Some(generation) => {
            let days = (env.now - generation.created).max(0) / 86_400;
            let kernel = generation.kernel.as_deref().unwrap_or("?");
            println!(
                "Geração {} em uso, aplicada há {days} dias (kernel {kernel}); {} geração(ões) do sistema, {} do Home Manager",
                generation.number,
                status.generations.len(),
                status.home_manager_generations.len(),
            );
        }
        None => println!("Geração em uso não encontrada"),
    }
    match &status.pending {
        Some(PendingChanges::Exact(n)) => println!("Commits não aplicados: {n}"),
        Some(PendingChanges::Approximate { commits, reasons }) => {
            println!("Commits não aplicados: cerca de {commits} (aproximado: {reasons:?})")
        }
        None => {}
    }
    if let Some(git) = &status.git {
        if git.untracked.is_empty() {
            println!("Arquivos fora do git: nenhum");
        } else {
            println!("Arquivos fora do git (o flake os ignora):");
            for file in &git.untracked {
                println!("  {file}");
            }
        }
        if !git.modified.is_empty() {
            println!(
                "Arquivos modificados e não commitados: {}",
                git.modified.len()
            );
        }
    }

    println!("\n== Inputs do flake ==");
    for item in &status.inputs {
        let age = item.age_days.map_or_else(
            || "idade desconhecida".to_owned(),
            |d| format!("há {d} dias"),
        );
        let mark = if item.stale { " !" } else { "" };
        println!(
            "  {:<16} {:<32} {age}{mark}",
            item.input.name,
            item.input.label()
        );
    }
    println!("  (! = mais de {DEFAULT_STALE_DAYS} dias)");

    println!("\n== Espaço ==");
    if let Some(store) = &status.store {
        println!("{} no /nix/store", format_bytes(store.total_bytes));
        if let Some(gc) = &store.gc {
            println!(
                "Cerca de {} podem ser liberados ({} caminhos sem referência)",
                format_bytes(gc.bytes),
                gc.paths
            );
        }
    }
    if status.forgotten_roots.is_empty() {
        println!("Links de build esquecidos: nenhum");
    }
    for root in &status.forgotten_roots {
        let mark = if root.selected_by_default { "x" } else { " " };
        println!(
            "  [{mark}] {} ({} dias)",
            root.path.display(),
            root.age_days
        );
    }
    if let Some(flatpak) = &status.flatpak {
        let partial = if flatpak.unused_bytes_partial {
            "+"
        } else {
            ""
        };
        println!(
            "Flatpak: {} no total, {}{partial} em {} runtimes sem uso",
            format_bytes(flatpak.total_bytes),
            format_bytes(flatpak.unused_bytes),
            flatpak.unused.len(),
        );
    }

    println!("\n== Manutenção automática ==");
    if let Some(auto) = &status.auto_maintenance {
        println!("nix.gc.automatic: {}", yes_no(auto.gc));
        println!("nix.optimise.automatic: {}", yes_no(auto.optimise));
    }
    match status.flatpak_uninstall_unused {
        Some(value) => println!("services.flatpak.uninstallUnused: {}", yes_no(value)),
        None => println!("services.flatpak.uninstallUnused: opção ausente"),
    }

    if !status.warnings.is_empty() {
        println!("\n== Avisos ==");
        for warning in &status.warnings {
            println!("  {warning}");
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "ativo" } else { "inativo" }
}

/// Para onde vai o espaço do store, com as raízes de cada categoria (diagnóstico).
fn print_space(
    env: &StatusEnv,
    nix: &CliNix<SystemRunner>,
    git: &CliGit<SystemRunner>,
    flatpak: &CliFlatpak<SystemRunner>,
) -> anyhow::Result<()> {
    use zelador_core::backends::NixBackend;
    use zelador_core::space::{SpaceInputs, breakdown, compute_sets, root_paths};

    let status = status::collect_overview(env, nix, git, flatpak);
    let roots = root_paths(env, &status);
    println!(
        "Raízes: sistema {}, Home Manager {}, outras gerações {}, links esquecidos {}",
        roots.system.len(),
        roots.home_manager.len(),
        roots.other_generations.len(),
        roots.forgotten_roots.len()
    );
    let sets = compute_sets(nix, &roots).context("closures")?;
    let sizes = nix.path_sizes().context("tamanhos do store")?;
    let dead: std::collections::HashSet<String> = nix
        .dead_paths()
        .context("caminhos mortos")?
        .into_iter()
        .collect();
    let result = breakdown(&SpaceInputs {
        sizes: &sizes,
        dead: &dead,
        system: &sets.system,
        home_manager: &sets.home_manager,
        old_generations: &sets.old_generations,
        forgotten_roots: &sets.forgotten_roots,
    });
    println!("Total {}", format_bytes(result.total));
    println!("Em uso {}", format_bytes(result.in_use()));
    println!("  Sistema {}", format_bytes(result.system));
    println!("  Home Manager {}", format_bytes(result.home_manager));
    println!(
        "  Ambientes de desenvolvimento {}",
        format_bytes(result.dev_environments)
    );
    println!("Pode sair {}", format_bytes(result.reclaimable()));
    println!(
        "  Gerações antigas {}",
        format_bytes(result.old_generations)
    );
    println!(
        "  Links esquecidos {}",
        format_bytes(result.forgotten_roots)
    );
    println!("  Sem referência {}", format_bytes(result.unreferenced));
    Ok(())
}
