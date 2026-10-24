//! CLI de debug do `zelador-core`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use zelador_core::backends::{CliFlatpak, CliGit, CliNix};
use zelador_core::check::CheckConfig;
use zelador_core::cleanup::Policy;
use zelador_core::flake::DEFAULT_STALE_DAYS;
use zelador_core::history::{self, NewOperation, OperationKind, Outcome};
use zelador_core::runner::SystemRunner;
use zelador_core::status::{self, PendingChanges, StatusEnv, SystemStatus};
use zelador_core::units::format_bytes;

const GIB: u64 = 1024 * 1024 * 1024;

const USAGE: &str = "uso: zelador-core <comando> [--flake <pasta>] [--host <nome>] [opções]

Comandos (todos só leem, exceto onde indicado):
  status      geração em uso, commits pendentes, inputs do flake, Flatpak
  space       uso do /nix/store por categoria
  clean       simula a limpeza; com --execute --yes, executa (pede senha uma vez)
              [--keep-system N] [--keep-hm N] [--keep-days N] [--no-generations]
              [--no-links] [--flatpak] [--optimise]
  update      constrói a atualização sem aplicar [--input NOME]... [--pin NOME=REV]
              [--state-dir PASTA]
  activate    ativa um sistema já construído: activate <caminho-do-store> <switch|boot|test>
  check       verifica inputs velhos, pouco espaço e lixo grande [--notify]
              [--stale-days N] [--min-free-gib N] [--reclaimable-gib N]
              [--notify-send PROGRAMA] [--open PROGRAMA]
  history     últimas operações registradas";

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
        Some("status" | "space" | "clean" | "update" | "activate" | "check" | "history") => {}
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => bail!("subcomando desconhecido: {other}\n{USAGE}"),
    }
    let mut flake = PathBuf::from("/etc/nixos");
    let mut host = None;
    let mut clean = CleanArgs::default();
    let mut update = UpdateArgs::default();
    let mut check = CheckArgs::default();
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = args.next() {
        let number =
            |args: &mut dyn Iterator<Item = String>, name: &str| -> anyhow::Result<usize> {
                args.next()
                    .with_context(|| format!("{name} pede um número"))?
                    .parse()
                    .with_context(|| format!("{name}: número inválido"))
            };
        match arg.as_str() {
            "--flake" => flake = args.next().context("--flake pede uma pasta")?.into(),
            "--host" => host = Some(args.next().context("--host pede um nome")?),
            "--keep-system" if command.as_deref() == Some("clean") => {
                clean.policy.keep_system = number(&mut args, "--keep-system")?;
            }
            "--keep-hm" if command.as_deref() == Some("clean") => {
                clean.policy.keep_home_manager = number(&mut args, "--keep-hm")?;
            }
            "--keep-days" if command.as_deref() == Some("clean") => {
                clean.policy.keep_days = u32::try_from(number(&mut args, "--keep-days")?)
                    .context("--keep-days: valor grande demais")?;
            }
            "--no-generations" if command.as_deref() == Some("clean") => clean.generations = false,
            "--no-links" if command.as_deref() == Some("clean") => clean.links = false,
            "--flatpak" if command.as_deref() == Some("clean") => clean.flatpak = true,
            "--optimise" if command.as_deref() == Some("clean") => clean.optimise = true,
            "--execute" if command.as_deref() == Some("clean") => clean.execute = true,
            "--yes" if command.as_deref() == Some("clean") => clean.yes = true,
            "--input" if command.as_deref() == Some("update") => {
                update
                    .inputs
                    .push(args.next().context("--input pede um nome")?);
            }
            "--pin" if command.as_deref() == Some("update") => {
                update
                    .pins
                    .push(args.next().context("--pin pede NOME=REV")?);
            }
            "--notify" if command.as_deref() == Some("check") => check.notify = true,
            "--notify-send" if command.as_deref() == Some("check") => {
                check.notify_send = args.next().context("--notify-send pede um programa")?;
            }
            "--open" if command.as_deref() == Some("check") => {
                check.open = Some(args.next().context("--open pede um programa")?);
            }
            "--stale-days" if command.as_deref() == Some("check") => {
                check.config.stale_days = i64::try_from(number(&mut args, "--stale-days")?)?;
            }
            "--min-free-gib" if command.as_deref() == Some("check") => {
                check.config.min_free_bytes = number(&mut args, "--min-free-gib")? as u64 * GIB;
            }
            "--reclaimable-gib" if command.as_deref() == Some("check") => {
                check.config.reclaimable_bytes =
                    number(&mut args, "--reclaimable-gib")? as u64 * GIB;
            }
            "--build" if command.as_deref() == Some("update") => update.build = true,
            "--state-dir" if command.as_deref() == Some("update") => {
                update.state_dir = Some(args.next().context("--state-dir pede uma pasta")?.into());
            }
            other if command.as_deref() == Some("activate") && !other.starts_with("--") => {
                positional.push(other.to_owned());
            }
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
    if command.as_deref() == Some("clean") {
        return run_clean(&env, &nix, &git, &flatpak, &clean);
    }
    if command.as_deref() == Some("update") {
        return run_update(&env, &nix, &update);
    }
    if command.as_deref() == Some("check") {
        return run_check(&env, &nix, &check);
    }
    if command.as_deref() == Some("history") {
        return run_history(&env);
    }
    if command.as_deref() == Some("activate") {
        return run_activate(&env, &positional);
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
    })
    .scaled(zelador_core::cleanup::disk_ratio(
        &sizes,
        zelador_core::disk::used_bytes(std::path::Path::new("/nix")).unwrap_or(u64::MAX),
    ));
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

struct CleanArgs {
    policy: Policy,
    generations: bool,
    links: bool,
    flatpak: bool,
    optimise: bool,
    execute: bool,
    yes: bool,
}

impl Default for CleanArgs {
    fn default() -> Self {
        Self {
            policy: Policy::default(),
            generations: true,
            links: true,
            flatpak: false,
            optimise: false,
            execute: false,
            yes: false,
        }
    }
}

/// Simula (padrão) ou executa a limpeza. Executar exige `--execute --yes` e o helper instalado.
fn run_clean(
    env: &StatusEnv,
    nix: &CliNix<SystemRunner>,
    git: &CliGit<SystemRunner>,
    flatpak: &CliFlatpak<SystemRunner>,
    args: &CleanArgs,
) -> anyhow::Result<()> {
    use zelador_core::backends::NixBackend;
    use zelador_core::cleanup::{
        Cleaner, HelperClient, Selection, StepKind, build_plan, closure_index, dry_run, estimate,
    };
    use zelador_core::disk::free_bytes;

    let status = status::collect_overview(env, nix, git, flatpak);
    let selection = Selection {
        generations: args.generations,
        links: if args.links {
            status
                .forgotten_roots
                .iter()
                .filter(|root| root.selected_by_default)
                .map(|root| root.path.clone())
                .collect()
        } else {
            Vec::new()
        },
        flatpak: args.flatpak,
        optimise: args.optimise,
    };
    let plan = build_plan(&status, args.policy, &selection, env.now);
    println!(
        "Encontradas: {} gerações do sistema, {} do Home Manager, {} links esquecidos",
        status.generations.len(),
        status.home_manager_generations.len(),
        status.forgotten_roots.len()
    );
    for warning in &status.warnings {
        println!("  aviso: {warning}");
    }
    println!("Plano:");
    println!(
        "  Gerações do sistema a excluir: {:?}",
        plan.system_generations
    );
    println!(
        "  Gerações do Home Manager a excluir: {:?}",
        plan.home_manager_generations
    );
    for root in &plan.links {
        println!("  Link esquecido: {}", root.path.display());
    }
    println!(
        "  Runtimes Flatpak sem uso: {}",
        if plan.flatpak { "sim" } else { "não" }
    );
    println!(
        "  Deduplicar o store: {}",
        if plan.optimise { "sim" } else { "não" }
    );
    if plan.is_empty() {
        println!("Nada a fazer.");
        return Ok(());
    }

    if !args.execute {
        let sizes = nix.path_sizes().context("tamanhos do store")?;
        let dead: std::collections::HashSet<String> = nix
            .dead_paths()
            .context("caminhos mortos")?
            .into_iter()
            .collect();
        let index = closure_index(nix, env, &status).context("closures")?;
        let flatpak_bytes = status.flatpak.as_ref().map_or(0, |f| f.unused_bytes);
        let result = estimate(&index, &sizes, &dead, &plan, flatpak_bytes);
        println!("Estimativa (simulação, nada foi alterado):");
        println!("  Gerações {}", format_bytes(result.generations));
        println!("  Links {}", format_bytes(result.links));
        println!("  Sem referência {}", format_bytes(result.unreferenced));
        println!("  Flatpak {}", format_bytes(result.flatpak));
        println!("  Total {}", format_bytes(result.total()));
        if let Some(paths) = dry_run(nix, &plan).context("simulação do GC")?.gc_paths {
            println!("  O GC apagaria hoje {paths} caminhos (nix store gc --dry-run)");
        }
        return Ok(());
    }

    if !args.yes {
        bail!("--execute altera o sistema: confirme com --yes");
    }
    let helper = HelperClient::production(SystemRunner);
    if let Err(issue) = helper.check() {
        bail!("o helper privilegiado não está disponível: {issue:?}");
    }
    let cleaner = Cleaner {
        nix,
        flatpak,
        helper: &helper,
        home: &env.home,
        home_manager_profile: &env.home_manager_profiles_dir.join("home-manager"),
    };
    let before = free_bytes(std::path::Path::new("/nix")).unwrap_or(0);
    let started = std::time::Instant::now();
    let result: anyhow::Result<()> = (|| {
        for step in plan.steps() {
            let name = match step.kind {
                StepKind::RemoveLinks => "removendo links esquecidos",
                StepKind::HomeManagerGenerations => "excluindo gerações do Home Manager",
                StepKind::FlatpakUser => "removendo runtimes Flatpak sem uso (usuário)",
                StepKind::Privileged => {
                    "etapa privilegiada: excluindo gerações do sistema, coletando lixo do store e deduplicando"
                }
            };
            println!("→ {name}…");
            cleaner.run(step, &plan).with_context(|| name.to_owned())?;
        }
        Ok(())
    })();
    let after = free_bytes(std::path::Path::new("/nix")).unwrap_or(0);
    record_history(
        env,
        OperationKind::Clean,
        started,
        &result,
        None,
        result.is_ok().then(|| after.saturating_sub(before)),
    );
    result?;
    println!(
        "Concluído. Espaço livre: {} → {}",
        format_bytes(before),
        format_bytes(after)
    );
    Ok(())
}

/// Grava a operação no histórico, sem nunca atrapalhar o resultado.
fn record_history(
    env: &StatusEnv,
    kind: OperationKind,
    started: std::time::Instant,
    result: &anyhow::Result<()>,
    generation: Option<u64>,
    bytes_freed: Option<u64>,
) {
    let detail = result.as_ref().err().map(|e| {
        let text = format!("{e:#}");
        text.chars().take(200).collect::<String>()
    });
    history::record_best_effort(
        env,
        &NewOperation {
            kind,
            started_at: env.now,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            outcome: if result.is_ok() {
                Outcome::Success
            } else {
                Outcome::Failure
            },
            generation,
            bytes_freed,
            detail,
        },
    );
}

#[derive(Default)]
struct UpdateArgs {
    inputs: Vec<String>,
    pins: Vec<String>,
    build: bool,
    state_dir: Option<PathBuf>,
}

/// Prepara o lock novo (e opcionalmente constrói). Nunca altera o `flake.lock`.
fn run_update(
    env: &StatusEnv,
    nix: &CliNix<SystemRunner>,
    args: &UpdateArgs,
) -> anyhow::Result<()> {
    use zelador_core::backends::{BuildRequest, NixBuild};
    use zelador_core::build::{BuildOutcome, run_build};
    use zelador_core::flake::{parse_flake_lock, pin_reference};
    use zelador_core::runner::CancelToken;
    use zelador_core::update::{Pin, UpdateWorkspace, prepare_lock, summarize_update};

    let workspace = args
        .state_dir
        .as_ref()
        .map_or_else(|| UpdateWorkspace::default_for(env), UpdateWorkspace::new);
    let current = std::fs::read_to_string(env.flake.join("flake.lock")).context("flake.lock")?;
    let inputs = parse_flake_lock(&current)?;
    let mut pins = Vec::new();
    for pin in &args.pins {
        let (name, rev) = pin.split_once('=').context("--pin pede NOME=REV")?;
        let input = inputs
            .iter()
            .find(|i| i.name == name)
            .with_context(|| format!("input `{name}` não existe"))?;
        let flake_ref = pin_reference(input, rev).with_context(|| {
            format!("não dá para fixar `{name}` nessa revisão (use os 40 caracteres)")
        })?;
        pins.push(Pin {
            input: name.to_owned(),
            flake_ref,
        });
    }
    let selected = if args.inputs.is_empty() && pins.is_empty() {
        inputs.iter().map(|i| i.name.clone()).collect()
    } else {
        args.inputs.clone()
    };
    let prepared =
        prepare_lock(nix, &env.flake, &workspace, &selected, &pins).context("preparar o lock")?;
    println!(
        "Lock novo em {} (o flake.lock não foi tocado)",
        workspace.new_lock().display()
    );
    if prepared.changes.is_empty() {
        println!("Nenhum input mudou de revisão.");
    }
    for change in &prepared.changes {
        println!("  {:<16} {}", change.name, change.label());
    }
    if !args.build {
        return Ok(());
    }

    let request = BuildRequest {
        flake: env.flake.clone(),
        host: env.host.clone(),
        out_link: workspace.out_link(),
        reference_lock: Some(workspace.new_lock()),
    };
    let cancel = CancelToken::new();
    println!("Construindo...");
    let mut last = String::new();
    let outcome = run_build(
        &SystemRunner,
        &NixBuild,
        &request,
        &cancel,
        &mut |progress| {
            let line = format!(
                "  {:?} · builds {}/{} · downloads {}/{}",
                progress.phase,
                progress.builds.done,
                progress.builds.expected,
                progress.downloads.done,
                progress.downloads.expected
            );
            if line != last {
                println!("{line}");
                last = line;
            }
        },
    )?;
    match outcome {
        BuildOutcome::Built { out_path } => {
            println!("Construído: {}", out_path.display());
            let summary = summarize_update(
                nix,
                &env.current_system
                    .canonicalize()
                    .unwrap_or(env.current_system.clone()),
                &out_path,
                &prepared.changes,
            )?;
            println!(
                "Mudanças: {} atualizados, {} novos, {} removidos; tamanho {:+} bytes; kernel mudou: {:?}",
                summary.totals.updated,
                summary.totals.added,
                summary.totals.removed,
                summary.size_delta.unwrap_or(0),
                summary.kernel_changed
            );
            Ok(())
        }
        BuildOutcome::Cancelled => bail!("cancelado"),
        BuildOutcome::Failed(failure) => {
            println!("O build falhou: {}", failure.headline);
            println!("{}", failure.details);
            bail!("build falhou")
        }
    }
}

fn run_activate(env: &StatusEnv, positional: &[String]) -> anyhow::Result<()> {
    use zelador_core::apply::{ActivationEvent, ApplyMode};
    use zelador_core::cleanup::HelperClient;

    let [path, mode] = positional else {
        bail!("uso: zelador-core activate <caminho-do-store> <boot|switch|test>");
    };
    let mode = match mode.as_str() {
        "boot" => ApplyMode::Boot,
        "switch" => ApplyMode::Switch,
        "test" => ApplyMode::Test,
        other => bail!("modo inválido: {other}"),
    };
    let helper = HelperClient::production(SystemRunner);
    if let Err(issue) = helper.check() {
        bail!("o helper privilegiado não está disponível: {issue:?}");
    }
    let started = std::time::Instant::now();
    let result = helper
        .activate(std::path::Path::new(path), mode, &mut |event| match event {
            ActivationEvent::Step(marker) => {
                println!("etapa {} de {}: {}", marker.step, marker.of, marker.program)
            }
            ActivationEvent::Stage(stage) => println!("fase: {stage:?}"),
            ActivationEvent::Log(line) => println!("  {line}"),
        })
        .context("ativação");
    let generation = history::current_system_generation(&env.system_profiles_dir);
    record_history(
        env,
        OperationKind::Update,
        started,
        &result,
        generation,
        None,
    );
    result?;
    println!("Concluído.");
    Ok(())
}

struct CheckArgs {
    config: CheckConfig,
    notify: bool,
    notify_send: String,
    open: Option<String>,
}

impl Default for CheckArgs {
    fn default() -> Self {
        Self {
            config: CheckConfig::default(),
            notify: false,
            notify_send: "notify-send".to_owned(),
            open: None,
        }
    }
}

/// Mede, decide e (com `--notify`) avisa. Só lê o sistema: nada é alterado.
fn run_check(env: &StatusEnv, nix: &CliNix<SystemRunner>, args: &CheckArgs) -> anyhow::Result<()> {
    use zelador_core::check::{Notifier, NotifySend, Observations, evaluate, is_due};
    use zelador_core::disk::free_bytes;
    use zelador_core::flake::parse_flake_lock;
    use zelador_core::history::{History, default_path};

    let inputs = std::fs::read_to_string(env.flake.join("flake.lock"))
        .map_err(anyhow::Error::from)
        .and_then(|text| Ok(parse_flake_lock(&text)?))
        .unwrap_or_else(|error| {
            eprintln!("aviso: não foi possível ler os inputs do flake: {error}");
            Vec::new()
        });
    let free = free_bytes(std::path::Path::new("/nix")).ok();
    // `--print-dead` leva alguns segundos: só roda se o limiar estiver ligado.
    let reclaimable = if args.config.reclaimable_bytes > 0 {
        let ratio = zelador_core::backends::NixBackend::store_total_bytes(nix)
            .ok()
            .zip(zelador_core::disk::used_bytes(std::path::Path::new("/nix")).ok())
            .map_or(1.0, |(total, used)| {
                zelador_core::disk::real_ratio(total, used)
            });
        status::gc_estimate(nix)
            .ok()
            .map(|estimate| zelador_core::disk::scale_bytes(estimate.bytes, ratio))
    } else {
        None
    };
    let findings = evaluate(
        &Observations {
            inputs: &inputs,
            now: env.now,
            free_bytes: free,
            reclaimable_bytes: reclaimable,
        },
        &args.config,
    );
    if findings.is_empty() {
        println!("Tudo certo: nada a avisar.");
        return Ok(());
    }
    let history = if args.notify {
        Some(History::open(&default_path(env)).context("histórico")?)
    } else {
        None
    };
    let notifier = NotifySend::new(SystemRunner, args.notify_send.clone());
    for finding in &findings {
        println!("{}: {}", finding.title, finding.body);
        let Some(history) = &history else { continue };
        if !is_due(history, finding.condition, env.now, &args.config)? {
            println!(
                "  (já avisado há menos de {} dias)",
                args.config.notice_interval_days
            );
            continue;
        }
        match notifier.notify(finding) {
            Ok(outcome) => {
                history.set_notice(finding.condition.id(), env.now)?;
                if outcome.clicked
                    && let Some(open) = &args.open
                {
                    // Abre o app sem esperar por ele.
                    if let Err(error) = std::process::Command::new(open).spawn() {
                        eprintln!("aviso: não foi possível abrir {open}: {error}");
                    }
                }
            }
            Err(error) => eprintln!("aviso: não foi possível notificar: {error}"),
        }
    }
    Ok(())
}

/// As últimas operações registradas, da mais nova para a mais antiga.
fn run_history(env: &StatusEnv) -> anyhow::Result<()> {
    let history = history::History::open(&history::default_path(env)).context("histórico")?;
    let operations = history.recent(20)?;
    if operations.is_empty() {
        println!("Nenhuma operação registrada.");
    }
    for op in operations {
        println!(
            "{} {} {} {}ms geração={} liberou={} {}",
            op.started_at,
            op.kind.as_str(),
            op.outcome.as_str(),
            op.duration_ms,
            op.generation.map_or("-".to_owned(), |n| n.to_string()),
            op.bytes_freed.map_or("-".to_owned(), |n| n.to_string()),
            op.detail.unwrap_or_default()
        );
    }
    Ok(())
}
