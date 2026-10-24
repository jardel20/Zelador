//! Atualização do sistema: preparar o lock novo, resumir as mudanças e, só ao aplicar,
//! gravá-lo no `flake.lock`.
//!
//! O `flake.lock` do usuário não é tocado até a ativação dar certo: o lock novo vive na pasta
//! de estado do app e o build o usa por `--reference-lock-file`. "Descartar" apaga tudo isso.

use std::fs;
use std::path::{Path, PathBuf};

use crate::backends::{NixBackend, PathInfoOptions};
use crate::diff::{DiffEntry, DiffSummary, summarize};
use crate::error::{Error, Result};
use crate::flake::{InputChange, diff_inputs, parse_flake_lock};
use crate::generations::kernel_version_from_path;
use crate::status::StatusEnv;

/// Pasta de estado da atualização em andamento (`~/.local/state/zelador/update`).
#[derive(Debug, Clone)]
pub struct UpdateWorkspace {
    dir: PathBuf,
}

impl UpdateWorkspace {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_for(env: &StatusEnv) -> Self {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| env.home.join(".local/state"));
        Self::new(base.join("zelador/update"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// O lock novo, usado pelo build.
    pub fn new_lock(&self) -> PathBuf {
        self.dir.join("flake.lock.novo")
    }

    fn staging_lock(&self) -> PathBuf {
        self.dir.join("flake.lock.parcial")
    }

    /// Cópia do `flake.lock` original, guardada ao gravar o novo.
    pub fn backup_lock(&self) -> PathBuf {
        self.dir.join("flake.lock.anterior")
    }

    /// O link do build: uma raiz do GC enquanto a atualização existir.
    pub fn out_link(&self) -> PathBuf {
        self.dir.join("result")
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))
    }

    /// Apaga o lock novo e o link do build. Não toca no `flake.lock` do usuário.
    pub fn discard(&self) -> Result<()> {
        for path in [self.new_lock(), self.staging_lock(), self.out_link()] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(path, e)),
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub input: String,
    /// Referência de flake que fixa o input (ver [`crate::flake::pin_reference`]).
    pub flake_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockPreparation {
    /// Inputs que mudaram de revisão, em relação ao `flake.lock` atual.
    pub changes: Vec<InputChange>,
}

/// Atualiza `inputs` e aplica as fixações sobre o resultado, tudo em `workspace`. Inputs
/// fixados não são atualizados. O `flake.lock` do flake fica intacto.
pub fn prepare_lock(
    nix: &dyn NixBackend,
    flake: &Path,
    workspace: &UpdateWorkspace,
    inputs: &[String],
    pins: &[Pin],
) -> Result<LockPreparation> {
    let base = flake.join("flake.lock");
    let to_update: Vec<String> = inputs
        .iter()
        .filter(|name| !pins.iter().any(|pin| &pin.input == *name))
        .cloned()
        .collect();
    if to_update.is_empty() && pins.is_empty() {
        return Err(Error::parse("atualização", "nenhum input para atualizar"));
    }
    let before = fs::read_to_string(&base).map_err(|e| Error::io(&base, e))?;
    workspace.ensure()?;
    workspace.discard()?;

    let new_lock = workspace.new_lock();
    let mut reference = base;
    if !to_update.is_empty() {
        nix.flake_update(flake, &to_update, &new_lock)?;
        reference = new_lock.clone();
    }
    for pin in pins {
        let staging = workspace.staging_lock();
        nix.flake_pin(flake, &reference, &staging, &pin.input, &pin.flake_ref)?;
        fs::rename(&staging, &new_lock).map_err(|e| Error::io(&new_lock, e))?;
        reference = new_lock.clone();
    }

    let after = fs::read_to_string(&new_lock).map_err(|e| Error::io(&new_lock, e))?;
    Ok(LockPreparation {
        changes: diff_inputs(&parse_flake_lock(&before)?, &parse_flake_lock(&after)?),
    })
}

/// Grava o lock novo no `flake.lock`, guardando antes uma cópia do original. Escreve num
/// arquivo temporário ao lado e renomeia, então ninguém vê um lock pela metade.
pub fn commit_lock(flake: &Path, workspace: &UpdateWorkspace) -> Result<()> {
    let target = flake.join("flake.lock");
    let new_lock = workspace.new_lock();
    let contents = fs::read(&new_lock).map_err(|e| Error::io(&new_lock, e))?;
    workspace.ensure()?;
    fs::copy(&target, workspace.backup_lock()).map_err(|e| Error::io(&target, e))?;

    let temp = flake.join(".flake.lock.zelador-tmp");
    fs::write(&temp, &contents).map_err(|e| Error::io(&temp, e))?;
    if let Ok(meta) = fs::metadata(&target) {
        // Mantém as permissões do arquivo original.
        let _ = fs::set_permissions(&temp, meta.permissions());
    }
    fs::rename(&temp, &target).map_err(|e| Error::io(&target, e))
}

/// O kernel muda entre os dois sistemas? `None` se não der para saber.
pub fn kernel_changed(current: &Path, new: &Path) -> Option<bool> {
    let version =
        |system: &Path| kernel_version_from_path(&fs::read_link(system.join("kernel")).ok()?);
    Some(version(current)? != version(new)?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSummary {
    pub totals: DiffSummary,
    pub entries: Vec<DiffEntry>,
    /// Quanto a closure cresce (ou diminui) em bytes.
    pub size_delta: Option<i64>,
    pub kernel_changed: Option<bool>,
    pub changed_inputs: Vec<String>,
}

pub fn summarize_update(
    nix: &dyn NixBackend,
    current: &Path,
    new: &Path,
    changes: &[InputChange],
) -> Result<UpdateSummary> {
    let (current_text, new_text) = (
        current.to_string_lossy().into_owned(),
        new.to_string_lossy().into_owned(),
    );
    let entries = nix.diff_closures(&current_text, &new_text)?;
    let infos = nix.path_info(
        &[current_text.clone(), new_text.clone()],
        PathInfoOptions {
            closure_size: true,
            recursive: false,
        },
    )?;
    let closure = |path: &str| {
        infos
            .iter()
            .find(|i| i.path == path)
            .and_then(|i| i.closure_size)
    };
    let size_delta = match (closure(&current_text), closure(&new_text)) {
        (Some(old), Some(new)) => {
            Some(i64::try_from(new).unwrap_or(i64::MAX) - i64::try_from(old).unwrap_or(i64::MAX))
        }
        _ => None,
    };
    Ok(UpdateSummary {
        totals: summarize(&entries),
        entries,
        size_delta,
        kernel_changed: kernel_changed(current, new),
        changed_inputs: changes.iter().map(|c| c.name.clone()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;

    fn fixture(name: &str) -> String {
        fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    /// `NixBackend` que grava os locks de fixtures reais onde o Nix gravaria.
    struct LockWriter {
        update_with: &'static str,
        pin_with: &'static str,
        calls: RefCell<Vec<String>>,
    }

    impl NixBackend for LockWriter {
        fn flake_update(&self, _: &Path, inputs: &[String], out: &Path) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("update {}", inputs.join(",")));
            fs::write(out, fixture(self.update_with)).unwrap();
            Ok(())
        }
        fn flake_pin(
            &self,
            _: &Path,
            reference: &Path,
            out: &Path,
            input: &str,
            flake_ref: &str,
        ) -> Result<()> {
            self.calls.borrow_mut().push(format!(
                "pin {input}={flake_ref} sobre {}",
                reference.file_name().unwrap().to_string_lossy()
            ));
            fs::write(out, fixture(self.pin_with)).unwrap();
            Ok(())
        }
        fn flake_metadata(&self, _: &Path) -> Result<crate::flake::FlakeMetadata> {
            unreachable!()
        }
        fn path_info(
            &self,
            paths: &[String],
            _: PathInfoOptions,
        ) -> Result<Vec<crate::store::PathInfo>> {
            Ok(paths
                .iter()
                .enumerate()
                .map(|(i, p)| crate::store::PathInfo {
                    path: p.clone(),
                    nar_size: 1,
                    closure_size: Some(1000 + 500 * i as u64),
                    references: vec![],
                })
                .collect())
        }
        fn store_total_bytes(&self) -> Result<u64> {
            unreachable!()
        }
        fn path_sizes(&self) -> Result<std::collections::HashMap<String, u64>> {
            unreachable!()
        }
        fn closure_paths(&self, _: &str) -> Result<Vec<String>> {
            unreachable!()
        }
        fn references(&self, _: &str) -> Result<Vec<String>> {
            unreachable!()
        }
        fn gc_dry_run(&self) -> Result<u64> {
            unreachable!()
        }
        fn dead_paths(&self) -> Result<Vec<String>> {
            unreachable!()
        }
        fn diff_closures(&self, _: &str, _: &str) -> Result<Vec<DiffEntry>> {
            Ok(crate::diff::parse_diff_closures(
                "bash: 5.3p15 → 5.3p20\nnovo: ∅ → 1.0, 5.0 KiB\nvelho: 2 → ∅, -3.0 KiB\n",
            ))
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
        fn delete_user_generations(&self, _: &Path, _: &[u64]) -> Result<()> {
            unreachable!()
        }
    }

    fn flake_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("flake.lock"), fixture("lock-before.json")).unwrap();
        dir
    }

    fn writer() -> LockWriter {
        LockWriter {
            update_with: "lock-after-update.json",
            pin_with: "lock-after-pin.json",
            calls: RefCell::default(),
        }
    }

    #[test]
    fn atualizar_nao_toca_no_flake_lock_e_resume_o_que_mudou() {
        let flake = flake_dir();
        let state = tempfile::tempdir().unwrap();
        let ws = UpdateWorkspace::new(state.path().join("update"));
        let nix = writer();
        let before = fs::read(flake.path().join("flake.lock")).unwrap();

        let prep = prepare_lock(
            &nix,
            flake.path(),
            &ws,
            &["nixpkgs".into(), "home-manager".into()],
            &[],
        )
        .unwrap();
        assert_eq!(
            prep.changes
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["home-manager", "nixpkgs"]
        );
        assert_eq!(
            fs::read(flake.path().join("flake.lock")).unwrap(),
            before,
            "o lock do usuário fica intacto"
        );
        assert!(ws.new_lock().exists());
        assert_eq!(
            nix.calls.borrow().as_slice(),
            ["update nixpkgs,home-manager"]
        );
    }

    #[test]
    fn fixar_exclui_o_input_do_update_e_parte_do_lock_novo() {
        let flake = flake_dir();
        let state = tempfile::tempdir().unwrap();
        let ws = UpdateWorkspace::new(state.path());
        let nix = writer();
        let pin = Pin {
            input: "nixpkgs".into(),
            flake_ref: "github:NixOS/nixpkgs/02c087f3fe673c9e317f206a627cb6b910336dbe".into(),
        };
        let prep = prepare_lock(
            &nix,
            flake.path(),
            &ws,
            &["nixpkgs".into(), "home-manager".into()],
            &[pin],
        )
        .unwrap();
        // O nixpkgs fixado não é atualizado; só o home-manager.
        assert_eq!(nix.calls.borrow()[0], "update home-manager");
        assert!(nix.calls.borrow()[1].starts_with("pin nixpkgs=github:NixOS/nixpkgs/02c087f"));
        assert!(
            nix.calls.borrow()[1].ends_with("sobre flake.lock.novo"),
            "a fixação parte do lock já atualizado"
        );
        assert_eq!(
            prep.changes
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["home-manager"]
        );
    }

    #[test]
    fn so_fixar_parte_do_lock_atual_do_usuario() {
        let flake = flake_dir();
        let state = tempfile::tempdir().unwrap();
        let ws = UpdateWorkspace::new(state.path());
        let nix = writer();
        let pin = Pin {
            input: "nixpkgs".into(),
            flake_ref: "github:NixOS/nixpkgs/02c087f3fe673c9e317f206a627cb6b910336dbe".into(),
        };
        prepare_lock(&nix, flake.path(), &ws, &[], &[pin]).unwrap();
        assert!(
            nix.calls.borrow()[0].ends_with("sobre flake.lock"),
            "{:?}",
            nix.calls.borrow()
        );
    }

    #[test]
    fn nada_a_fazer_e_erro() {
        let flake = flake_dir();
        let state = tempfile::tempdir().unwrap();
        let err = prepare_lock(
            &writer(),
            flake.path(),
            &UpdateWorkspace::new(state.path()),
            &[],
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("nenhum input"));
    }

    #[test]
    fn gravar_o_lock_guarda_o_original_e_descartar_nao_toca_no_flake() {
        let flake = flake_dir();
        let state = tempfile::tempdir().unwrap();
        let ws = UpdateWorkspace::new(state.path());
        prepare_lock(&writer(), flake.path(), &ws, &["nixpkgs".into()], &[]).unwrap();
        let original = fs::read_to_string(flake.path().join("flake.lock")).unwrap();

        commit_lock(flake.path(), &ws).unwrap();
        assert_eq!(
            fs::read_to_string(flake.path().join("flake.lock")).unwrap(),
            fixture("lock-after-update.json")
        );
        assert_eq!(
            fs::read_to_string(ws.backup_lock()).unwrap(),
            original,
            "o original fica guardado"
        );
        assert!(
            !flake.path().join(".flake.lock.zelador-tmp").exists(),
            "sem sobras ao lado do flake"
        );

        // Descartar só limpa a pasta de estado.
        symlink("/nix/store/aaa-x", ws.out_link()).unwrap();
        ws.discard().unwrap();
        assert!(!ws.new_lock().exists() && ws.out_link().symlink_metadata().is_err());
        assert_eq!(
            fs::read_to_string(flake.path().join("flake.lock")).unwrap(),
            fixture("lock-after-update.json")
        );
        // Descartar de novo, sem nada, também é ok.
        ws.discard().unwrap();
    }

    #[test]
    fn kernel_comparado_pelo_link_kernel() {
        let dir = tempfile::tempdir().unwrap();
        let system = |name: &str, kernel: &str| {
            let path = dir.path().join(name);
            fs::create_dir(&path).unwrap();
            symlink(
                format!("/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-linux-{kernel}/bzImage"),
                path.join("kernel"),
            )
            .unwrap();
            path
        };
        let (a, b, c) = (
            system("a", "6.18.57"),
            system("b", "6.18.57"),
            system("c", "6.19.1"),
        );
        assert_eq!(kernel_changed(&a, &b), Some(false));
        assert_eq!(kernel_changed(&a, &c), Some(true));
        assert_eq!(kernel_changed(&a, &dir.path().join("nada")), None);
    }

    #[test]
    fn resumo_junta_diff_tamanho_kernel_e_inputs() {
        let nix = writer();
        let change = InputChange {
            name: "nixpkgs".into(),
            old_rev: None,
            new_rev: None,
            old_last_modified: None,
            new_last_modified: None,
        };
        let summary = summarize_update(
            &nix,
            Path::new("/nix/store/atual"),
            Path::new("/nix/store/novo"),
            &[change],
        )
        .unwrap();
        assert_eq!(
            (
                summary.totals.updated,
                summary.totals.added,
                summary.totals.removed
            ),
            (1, 1, 1)
        );
        assert_eq!(summary.size_delta, Some(500));
        assert_eq!(summary.kernel_changed, None);
        assert_eq!(summary.changed_inputs, ["nixpkgs"]);
    }
}
