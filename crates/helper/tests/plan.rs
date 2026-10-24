//! Validação e planos, sobre um "store" temporário. Nada é executado.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zelador_helper::Rejection;
use zelador_helper::exec::{Executor, Failure, run_plan};
use zelador_helper::lock::{Lock, LockError};
use zelador_helper::plan::{Plan, SWITCH_UNIT, Step, validate};
use zelador_helper::request::{Mode, Request};
use zelador_helper::roots::{Roots, Tools};

struct Sandbox {
    _dir: TempDir,
    root: PathBuf,
    roots: Roots,
    tools: Tools,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for sub in ["store", "profiles", "run"] {
            fs::create_dir(root.join(sub)).unwrap();
        }
        let roots = Roots {
            store: root.join("store"),
            profiles: root.join("profiles"),
            current_system: root.join("run/current-system"),
            flatpak_repo: root.join("flatpak/repo"),
        };
        let tools = Tools {
            nix_env: "/t/nix-env".into(),
            nix_store: "/t/nix-store".into(),
            systemd_run: "/t/systemd-run".into(),
            flatpak: "/t/flatpak".into(),
        };
        Self {
            _dir: dir,
            root,
            roots,
            tools,
        }
    }

    /// Cria `store/<name>/bin/switch-to-configuration` executável.
    fn system(&self, name: &str) -> PathBuf {
        let dir = self.roots.store.join(name);
        fs::create_dir_all(dir.join("bin")).unwrap();
        let script = dir.join("bin/switch-to-configuration");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    fn generation(&self, number: u64, system: &Path) {
        symlink(system, self.roots.generation_link(number)).unwrap();
    }

    fn boot_default(&self, number: u64) {
        let _ = fs::remove_file(self.roots.system_profile());
        symlink(format!("system-{number}-link"), self.roots.system_profile()).unwrap();
    }

    fn running(&self, system: &Path) {
        let _ = fs::remove_file(&self.roots.current_system);
        symlink(system, &self.roots.current_system).unwrap();
    }

    fn plan(&self, request: &Request) -> Result<Plan, Rejection> {
        validate(request, &self.roots, &self.tools)
    }

    fn activate(&self, path: &Path, mode: Mode) -> Result<Plan, Rejection> {
        self.plan(&Request::Activate {
            path: path.to_owned(),
            mode,
        })
    }

    /// Três gerações: 1 e 2 antigas, 3 em uso e padrão do boot.
    fn with_generations(&self) -> [PathBuf; 3] {
        let systems = ["aaa-system-1", "bbb-system-2", "ccc-system-3"].map(|n| self.system(n));
        for (i, system) in systems.iter().enumerate() {
            self.generation(i as u64 + 1, system);
        }
        self.running(&systems[2]);
        self.boot_default(3);
        systems
    }
}

fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn os(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

#[test]
fn activate_switch_define_o_perfil_e_ativa_em_unidade_transitoria() {
    let sb = Sandbox::new();
    let system = sb.system("abc-nixos-system");
    let plan = sb.activate(&system, Mode::Switch).unwrap();

    let script = system.join("bin/switch-to-configuration");
    assert_eq!(
        plan.steps,
        [
            Step {
                program: "/t/nix-env".into(),
                args: vec![
                    "-p".into(),
                    os(&sb.roots.profiles.join("system")),
                    "--set".into(),
                    os(&system),
                ],
                env: vec![],
            },
            Step {
                program: "/t/systemd-run".into(),
                args: {
                    let mut a = args(&[
                        "--collect",
                        "--no-ask-password",
                        "--pipe",
                        "--quiet",
                        "--service-type=exec",
                        "--unit",
                        SWITCH_UNIT,
                        "-E",
                        "NIXOS_INSTALL_BOOTLOADER",
                    ]);
                    a.push(os(&script));
                    a.push("switch".into());
                    a
                },
                env: vec![("NIXOS_INSTALL_BOOTLOADER".into(), "0".into())],
            },
        ]
    );
}

#[test]
fn activate_test_nao_mexe_no_perfil() {
    let sb = Sandbox::new();
    let system = sb.system("abc-nixos-system");
    let plan = sb.activate(&system, Mode::Test).unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].program, Path::new("/t/systemd-run"));
    assert_eq!(plan.steps[0].args.last().unwrap(), "test");
}

#[test]
fn activate_boot_define_o_perfil_e_usa_o_modo_boot() {
    let sb = Sandbox::new();
    let system = sb.system("abc-nixos-system");
    let plan = sb.activate(&system, Mode::Boot).unwrap();
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[1].args.last().unwrap(), "boot");
}

#[test]
fn activate_aceita_script_que_e_link_para_dentro_do_store() {
    let sb = Sandbox::new();
    let real = sb.system("real-script");
    let linked = sb.roots.store.join("linked-system");
    fs::create_dir_all(linked.join("bin")).unwrap();
    symlink(
        real.join("bin/switch-to-configuration"),
        linked.join("bin/switch-to-configuration"),
    )
    .unwrap();
    assert!(sb.activate(&linked, Mode::Switch).is_ok());
}

#[test]
fn activate_recusa_caminhos_invalidos() {
    let sb = Sandbox::new();
    let good = sb.system("abc-nixos-system");
    let outside = sb.root.join("fora");
    fs::create_dir_all(outside.join("bin")).unwrap();
    fs::write(outside.join("bin/switch-to-configuration"), "").unwrap();
    fs::set_permissions(
        outside.join("bin/switch-to-configuration"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let reject = |path: &Path| sb.activate(path, Mode::Switch).unwrap_err();

    // Fora do store ou não direto no store.
    assert!(matches!(reject(&outside), Rejection::OutsideStore { .. }));
    assert!(matches!(
        reject(&sb.roots.store),
        Rejection::OutsideStore { .. }
    ));
    assert!(matches!(
        reject(&sb.roots.store.join("..").join("fora")),
        Rejection::OutsideStore { .. }
    ));
    assert!(matches!(
        reject(&good.join("..")),
        Rejection::OutsideStore { .. }
    ));
    assert!(matches!(
        reject(&good.join("bin")),
        Rejection::OutsideStore { .. }
    ));
    assert!(matches!(
        reject(&sb.roots.store.join(".oculto")),
        Rejection::OutsideStore { .. }
    ));
    assert!(matches!(
        reject(Path::new("relativo/abc")),
        Rejection::NotAbsolute(_)
    ));

    // Não existe, ou existe mas não é um sistema.
    assert!(matches!(
        reject(&sb.roots.store.join("nao-existe")),
        Rejection::NotFound(_)
    ));
    fs::write(sb.roots.store.join("arquivo"), "").unwrap();
    assert!(matches!(
        reject(&sb.roots.store.join("arquivo")),
        Rejection::NotDirectory(_)
    ));
    fs::create_dir(sb.roots.store.join("vazio")).unwrap();
    assert!(matches!(
        reject(&sb.roots.store.join("vazio")),
        Rejection::NotASystem(_)
    ));
}

#[test]
fn activate_recusa_symlinks() {
    let sb = Sandbox::new();
    let good = sb.system("abc-nixos-system");
    let outside = sb.root.join("fora");
    fs::create_dir_all(outside.join("bin")).unwrap();
    fs::write(outside.join("bin/switch-to-configuration"), "").unwrap();
    fs::set_permissions(
        outside.join("bin/switch-to-configuration"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    // Link no store para outro sistema do store e para fora do store.
    symlink(&good, sb.roots.store.join("link-dentro")).unwrap();
    symlink(&outside, sb.roots.store.join("link-fora")).unwrap();
    for name in ["link-dentro", "link-fora"] {
        let err = sb
            .activate(&sb.roots.store.join(name), Mode::Switch)
            .unwrap_err();
        assert!(matches!(err, Rejection::Symlink(_)), "{name}: {err:?}");
    }

    // Script que é link para fora do store.
    let tricky = sb.roots.store.join("tricky");
    fs::create_dir_all(tricky.join("bin")).unwrap();
    symlink(
        outside.join("bin/switch-to-configuration"),
        tricky.join("bin/switch-to-configuration"),
    )
    .unwrap();
    assert!(matches!(
        sb.activate(&tricky, Mode::Switch).unwrap_err(),
        Rejection::NotASystem(_)
    ));

    // `bin` inteiro apontando para fora.
    let bin_link = sb.roots.store.join("bin-link");
    fs::create_dir(&bin_link).unwrap();
    symlink(outside.join("bin"), bin_link.join("bin")).unwrap();
    assert!(matches!(
        sb.activate(&bin_link, Mode::Switch).unwrap_err(),
        Rejection::NotASystem(_)
    ));
}

#[test]
fn activate_recusa_script_sem_permissao_de_execucao() {
    let sb = Sandbox::new();
    let system = sb.system("sem-exec");
    fs::set_permissions(
        system.join("bin/switch-to-configuration"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(matches!(
        sb.activate(&system, Mode::Boot).unwrap_err(),
        Rejection::NotASystem(_)
    ));
}

#[test]
fn rollback_e_set_boot_default_usam_o_destino_da_geracao() {
    let sb = Sandbox::new();
    let [one, ..] = sb.with_generations();

    let rollback = sb.plan(&Request::Rollback(1)).unwrap();
    assert_eq!(
        rollback.steps[0].args,
        args(&[
            "-p",
            sb.roots.profiles.join("system").to_str().unwrap(),
            "--switch-generation",
            "1"
        ])
    );
    assert_eq!(rollback.steps[1].args.last().unwrap(), "switch");
    assert!(
        rollback.steps[1]
            .args
            .contains(&os(&one.join("bin/switch-to-configuration")))
    );

    let boot = sb.plan(&Request::SetBootDefault(2)).unwrap();
    assert_eq!(boot.steps[0].args.last().unwrap(), "2");
    assert_eq!(boot.steps[1].args.last().unwrap(), "boot");

    assert_eq!(
        sb.plan(&Request::Rollback(9)).unwrap_err(),
        Rejection::GenerationMissing(9)
    );
    assert_eq!(
        sb.plan(&Request::SetBootDefault(9)).unwrap_err(),
        Rejection::GenerationMissing(9)
    );
}

#[test]
fn rollback_recusa_geracao_cujo_destino_nao_e_um_sistema() {
    let sb = Sandbox::new();
    let fake = sb.roots.store.join("sem-script");
    fs::create_dir(&fake).unwrap();
    sb.generation(7, &fake);
    assert!(matches!(
        sb.plan(&Request::Rollback(7)).unwrap_err(),
        Rejection::NotASystem(_)
    ));
}

#[test]
fn delete_generations_exclui_as_antigas_e_regenera_o_boot() {
    let sb = Sandbox::new();
    let [_, _, running] = sb.with_generations();
    let plan = sb.plan(&Request::DeleteGenerations(vec![2, 1])).unwrap();
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].program, Path::new("/t/nix-env"));
    assert_eq!(
        plan.steps[0].args,
        args(&[
            "-p",
            sb.roots.profiles.join("system").to_str().unwrap(),
            "--delete-generations",
            "2",
            "1"
        ])
    );
    // Depois de excluir, regenera as entradas de boot a partir do sistema em uso.
    assert_eq!(plan.steps[1].program, Path::new("/t/systemd-run"));
    assert!(
        plan.steps[1]
            .args
            .contains(&os(&running.join("bin/switch-to-configuration")))
    );
    assert_eq!(plan.steps[1].args.last().unwrap(), "boot");
}

#[test]
fn delete_generations_recusa_a_atual_e_a_padrao_do_boot() {
    let sb = Sandbox::new();
    let [_, two, _] = sb.with_generations();
    // 3 é a atual e a padrão.
    assert_eq!(
        sb.plan(&Request::DeleteGenerations(vec![1, 3]))
            .unwrap_err(),
        Rejection::GenerationProtected(3)
    );
    // Em uso é a 3, mas o boot aponta para a 2.
    sb.boot_default(2);
    assert_eq!(
        sb.plan(&Request::DeleteGenerations(vec![2])).unwrap_err(),
        Rejection::GenerationProtected(2)
    );
    // Em uso é a 2 e o boot aponta para a 3.
    sb.running(&two);
    sb.boot_default(3);
    assert_eq!(
        sb.plan(&Request::DeleteGenerations(vec![2])).unwrap_err(),
        Rejection::GenerationProtected(2)
    );
    assert_eq!(
        sb.plan(&Request::DeleteGenerations(vec![1]))
            .map(|p| p.steps.len()),
        Ok(2)
    );
}

#[test]
fn delete_generations_recusa_geracao_que_nao_existe_e_nao_adivinha_a_atual() {
    let sb = Sandbox::new();
    sb.with_generations();
    assert_eq!(
        sb.plan(&Request::DeleteGenerations(vec![42])).unwrap_err(),
        Rejection::GenerationMissing(42)
    );

    // Sem conseguir resolver a geração em uso, recusa em vez de arriscar.
    fs::remove_file(&sb.roots.current_system).unwrap();
    assert!(matches!(
        sb.plan(&Request::DeleteGenerations(vec![1])).unwrap_err(),
        Rejection::CurrentSystemUnknown(_)
    ));
}

#[test]
fn gc_optimise_e_flatpak() {
    let sb = Sandbox::new();
    assert_eq!(
        sb.plan(&Request::Gc).unwrap().steps,
        [Step {
            program: "/t/nix-store".into(),
            args: args(&["--gc"]),
            env: vec![]
        }]
    );
    assert_eq!(
        sb.plan(&Request::Optimise).unwrap().steps[0].args,
        args(&["--optimise"])
    );

    assert_eq!(
        sb.plan(&Request::FlatpakRemoveUnused).unwrap_err(),
        Rejection::NoSystemFlatpak
    );
    fs::create_dir_all(&sb.roots.flatpak_repo).unwrap();
    let plan = sb.plan(&Request::FlatpakRemoveUnused).unwrap();
    assert_eq!(plan.steps[0].program, Path::new("/t/flatpak"));
    assert_eq!(
        plan.steps[0].args,
        args(&["uninstall", "--system", "--unused", "--noninteractive"])
    );
}

#[derive(Default)]
struct Recorder {
    codes: Vec<io::Result<i32>>,
    seen: Vec<PathBuf>,
}

impl Executor for Recorder {
    fn run(&mut self, step: &Step) -> io::Result<i32> {
        self.seen.push(step.program.clone());
        self.codes.remove(0)
    }
}

#[test]
fn o_plano_para_no_primeiro_passo_que_falha() {
    let sb = Sandbox::new();
    let system = sb.system("abc-nixos-system");
    let plan = sb.activate(&system, Mode::Switch).unwrap();

    let mut ok = Recorder {
        codes: vec![Ok(0), Ok(0)],
        ..Recorder::default()
    };
    assert_eq!(run_plan(&plan, &mut ok), Ok(()));
    assert_eq!(ok.seen.len(), 2);

    // Se `nix-env --set` falha, a ativação nem começa.
    let mut fail = Recorder {
        codes: vec![Ok(3), Ok(0)],
        ..Recorder::default()
    };
    assert_eq!(
        run_plan(&plan, &mut fail),
        Err(Failure::Exit { step: 0, code: 3 })
    );
    assert_eq!(fail.seen, [PathBuf::from("/t/nix-env")]);

    let mut spawn = Recorder {
        codes: vec![Err(io::Error::from(io::ErrorKind::NotFound))],
        ..Recorder::default()
    };
    assert!(matches!(
        run_plan(&plan, &mut spawn),
        Err(Failure::Spawn { step: 0, .. })
    ));
}

#[test]
fn a_trava_impede_duas_operacoes_ao_mesmo_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lock");
    let first = Lock::acquire(&path).unwrap();
    assert!(matches!(Lock::acquire(&path), Err(LockError::Busy)));
    drop(first);
    assert!(Lock::acquire(&path).is_ok());
}
