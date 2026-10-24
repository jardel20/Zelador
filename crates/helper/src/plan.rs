//! Validação contra o sistema de arquivos e montagem do plano de execução.
//!
//! Nada aqui executa processos. O plano é um dado, então os testes comparam os comandos
//! exatos que rodariam como root.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::Rejection;
use crate::request::{Mode, Request};
use crate::roots::{Roots, Tools};

/// Unidade transitória do systemd em que a ativação roda. Sobrevive ao fim do processo que
/// a pediu (a UI pode fechar) e um nome fixo impede duas ativações ao mesmo tempo.
pub const SWITCH_UNIT: &str = "zelador-switch-to-configuration";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
}

pub fn validate(request: &Request, roots: &Roots, tools: &Tools) -> Result<Plan, Rejection> {
    let mut steps = Vec::new();
    match request {
        Request::Activate { path, mode } => {
            let system = validate_system_path(path, roots)?;
            if *mode != Mode::Test {
                steps.push(set_profile(tools, roots, &system));
            }
            steps.push(switch(tools, &system, *mode));
        }
        Request::Rollback(n) => {
            let system = generation_target(roots, *n)?;
            validate_system_path(&system, roots)?;
            steps.push(switch_generation(tools, roots, *n));
            steps.push(switch(tools, &system, Mode::Switch));
        }
        Request::SetBootDefault(n) => {
            let system = generation_target(roots, *n)?;
            validate_system_path(&system, roots)?;
            steps.push(switch_generation(tools, roots, *n));
            steps.push(switch(tools, &system, Mode::Boot));
        }
        Request::DeleteGenerations(numbers) => {
            let protected = protected_paths(roots)?;
            for n in numbers {
                if protected.contains(&generation_target(roots, *n)?) {
                    return Err(Rejection::GenerationProtected(*n));
                }
            }
            let mut args = vec!["-p".into(), roots.system_profile().into_os_string()];
            args.push("--delete-generations".into());
            args.extend(numbers.iter().map(|n| OsString::from(n.to_string())));
            steps.push(step(&tools.nix_env, args));
            // As entradas de boot das gerações apagadas ficariam até o próximo `switch`.
            // `boot` a partir do sistema em uso as regenera sem trocar nada em execução.
            let running = fs::canonicalize(&roots.current_system)
                .map_err(|_| Rejection::CurrentSystemUnknown(roots.current_system.clone()))?;
            let running = validate_system_path(&running, roots)?;
            steps.push(switch(tools, &running, Mode::Boot));
        }
        Request::Gc => steps.push(step(&tools.nix_store, vec!["--gc".into()])),
        Request::Optimise => steps.push(step(&tools.nix_store, vec!["--optimise".into()])),
        Request::FlatpakRemoveUnused => {
            if !roots.flatpak_repo.is_dir() {
                return Err(Rejection::NoSystemFlatpak);
            }
            steps.push(step(
                &tools.flatpak,
                ["uninstall", "--system", "--unused", "--noninteractive"]
                    .map(OsString::from)
                    .to_vec(),
            ));
        }
    }
    Ok(Plan { steps })
}

fn step(program: &Path, args: Vec<OsString>) -> Step {
    Step {
        program: program.to_owned(),
        args,
        env: Vec::new(),
    }
}

fn set_profile(tools: &Tools, roots: &Roots, system: &Path) -> Step {
    step(
        &tools.nix_env,
        vec![
            "-p".into(),
            roots.system_profile().into_os_string(),
            "--set".into(),
            system.as_os_str().to_owned(),
        ],
    )
}

fn switch_generation(tools: &Tools, roots: &Roots, number: u64) -> Step {
    step(
        &tools.nix_env,
        vec![
            "-p".into(),
            roots.system_profile().into_os_string(),
            "--switch-generation".into(),
            number.to_string().into(),
        ],
    )
}

/// Roda `switch-to-configuration` numa unidade transitória, como faz o `nixos-rebuild`.
fn switch(tools: &Tools, system: &Path, mode: Mode) -> Step {
    let script = system.join("bin/switch-to-configuration");
    let mut args: Vec<OsString> = [
        "--collect",
        "--no-ask-password",
        "--pipe",
        "--quiet",
        "--service-type=exec",
        "--unit",
        SWITCH_UNIT,
        "-E",
        "NIXOS_INSTALL_BOOTLOADER",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(script.into_os_string());
    args.push(mode.as_str().into());
    Step {
        program: tools.systemd_run.clone(),
        args,
        env: vec![("NIXOS_INSTALL_BOOTLOADER".into(), "0".into())],
    }
}

/// Confere que `path` é uma closure de sistema NixOS dentro do store e devolve o caminho
/// canônico, que é o que será executado.
///
/// O store é imutável e pertence ao root, então validar e executar o mesmo caminho
/// canônico não deixa janela útil para troca (TOCTOU).
pub fn validate_system_path(path: &Path, roots: &Roots) -> Result<PathBuf, Rejection> {
    if !path.is_absolute() {
        return Err(Rejection::NotAbsolute(path.to_owned()));
    }
    let outside = || Rejection::OutsideStore {
        path: path.to_owned(),
        store: roots.store.clone(),
    };
    // `file_name` é `None` para caminhos terminados em `..`; `parent` deixa `..` no meio.
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(outside());
    };
    if parent != roots.store || name.as_encoded_bytes().starts_with(b".") {
        return Err(outside());
    }

    let meta = fs::symlink_metadata(path).map_err(|_| Rejection::NotFound(path.to_owned()))?;
    if meta.file_type().is_symlink() {
        return Err(Rejection::Symlink(path.to_owned()));
    }
    if !meta.is_dir() {
        return Err(Rejection::NotDirectory(path.to_owned()));
    }

    let canonical_store =
        fs::canonicalize(&roots.store).map_err(|_| Rejection::NotFound(roots.store.clone()))?;
    let canonical = fs::canonicalize(path).map_err(|_| Rejection::NotFound(path.to_owned()))?;
    if canonical != canonical_store.join(name) {
        return Err(Rejection::Symlink(path.to_owned()));
    }

    let not_a_system = || Rejection::NotASystem(path.to_owned());
    let script = fs::canonicalize(canonical.join("bin/switch-to-configuration"))
        .map_err(|_| not_a_system())?;
    if !script.starts_with(&canonical_store) {
        return Err(not_a_system());
    }
    let script_meta = fs::metadata(&script).map_err(|_| not_a_system())?;
    if !script_meta.is_file() || script_meta.permissions().mode() & 0o111 == 0 {
        return Err(not_a_system());
    }
    Ok(canonical)
}

/// Destino canônico de `system-<n>-link`.
fn generation_target(roots: &Roots, number: u64) -> Result<PathBuf, Rejection> {
    let link = roots.generation_link(number);
    let is_link = fs::symlink_metadata(&link)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if !is_link {
        return Err(Rejection::GenerationMissing(number));
    }
    fs::canonicalize(&link).map_err(|_| Rejection::GenerationMissing(number))
}

/// Gerações que não podem ser excluídas: a em uso e a padrão do boot.
fn protected_paths(roots: &Roots) -> Result<Vec<PathBuf>, Rejection> {
    [roots.current_system.clone(), roots.system_profile()]
        .into_iter()
        .map(|link| fs::canonicalize(&link).map_err(|_| Rejection::CurrentSystemUnknown(link)))
        .collect()
}
