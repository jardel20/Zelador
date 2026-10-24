//! Estado compartilhado entre as telas: ambiente, navegação, avisos e cargas lentas.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use zelador_core::backends::{CliFlatpak, CliGit, CliNix, NixBackend};
use zelador_core::history::{NewOperation, record_best_effort};
use zelador_core::runner::SystemRunner;
use zelador_core::status::{StatusEnv, SystemStatus, collect_overview};

use crate::loader::{Bus, Shared};
use crate::pages::Page;

/// Troca a tela mostrada (e o item destacado na barra lateral).
type Navigate = Rc<dyn Fn(Page)>;

/// O que a tela "Aplicando" mostra; publicado pelo assistente "Atualizar".
#[derive(Debug, Clone)]
pub enum ApplyEvent {
    Started {
        mode: zelador_core::apply::ApplyMode,
        /// Horário de início (`14:32`).
        at: String,
        /// Quanto durou o build, se houve.
        build_duration: Option<String>,
        /// Número da geração em uso antes da ativação.
        previous_generation: Option<u64>,
        /// Se o `flake.lock` será gravado ao fim.
        writes_lock: bool,
    },
    Step {
        step: u32,
        of: u32,
    },
    Stage(zelador_core::apply::ActivationStage),
    Log(String),
    LockWritten,
    Finished(Result<(), String>),
}

pub struct Ctx {
    pub env: StatusEnv,
    pub toasts: adw::ToastOverlay,
    window: RefCell<Option<gtk::glib::WeakRef<gtk::Window>>>,
    navigate: RefCell<Option<Navigate>>,
    /// Parte rápida do estado (gerações, inputs, git...).
    pub overview: Rc<Shared<SystemStatus>>,
    /// `narSize` de todos os caminhos do store (cerca de 15 s).
    pub sizes: Rc<Shared<Result<HashMap<String, u64>, String>>>,
    /// Caminhos que o GC apagaria (cerca de 10 s).
    pub dead: Rc<Shared<Result<HashSet<String>, String>>>,
    pub apply_events: Bus<ApplyEvent>,
    /// Avisa quando uma operação nova entrou no histórico.
    pub history_changed: Bus<()>,
}

/// Preferência "comparar com o nvd", definida na abertura e pelo menu.
static PREFER_NVD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_prefer_nvd(prefer: bool) {
    PREFER_NVD.store(prefer, std::sync::atomic::Ordering::Relaxed);
}

/// O backend do Nix, já com o `nvd` quando o usuário o prefere e ele existe no PATH.
pub fn nix() -> CliNix<SystemRunner> {
    let nvd = PREFER_NVD
        .load(std::sync::atomic::Ordering::Relaxed)
        .then(|| zelador_core::nvd::find_in_path("nvd"))
        .flatten()
        .map(|path| path.to_string_lossy().into_owned());
    CliNix::new(SystemRunner).with_nvd(nvd)
}

pub fn git() -> CliGit<SystemRunner> {
    CliGit::new(SystemRunner)
}

impl Ctx {
    pub fn new(env: StatusEnv) -> Rc<Self> {
        let overview = {
            let env = env.clone();
            Shared::new(move || {
                collect_overview(&env, &nix(), &git(), &CliFlatpak::new(SystemRunner))
            })
        };
        let sizes = Shared::new(|| nix().path_sizes().map_err(|e| e.to_string()));
        let dead = Shared::new(|| {
            nix()
                .dead_paths()
                .map(|paths| paths.into_iter().collect())
                .map_err(|e| e.to_string())
        });
        Rc::new(Self {
            env,
            toasts: adw::ToastOverlay::new(),
            window: RefCell::new(None),
            navigate: RefCell::new(None),
            overview,
            sizes,
            dead,
            apply_events: Bus::default(),
            history_changed: Bus::default(),
        })
    }

    pub fn set_window(&self, window: &impl IsA<gtk::Window>) {
        *self.window.borrow_mut() = Some(window.upcast_ref::<gtk::Window>().downgrade());
    }

    pub fn window(&self) -> Option<gtk::Window> {
        self.window
            .borrow()
            .as_ref()
            .and_then(|weak| weak.upgrade())
    }

    pub fn set_navigation(&self, navigate: impl Fn(Page) + 'static) {
        *self.navigate.borrow_mut() = Some(Rc::new(navigate));
    }

    pub fn go_to(&self, page: Page) {
        let navigate = self.navigate.borrow().clone();
        if let Some(navigate) = navigate {
            navigate(page);
        }
    }

    /// Grava a operação no histórico (fora da thread principal) e avisa quem exibe a lista.
    pub fn record(self: &Rc<Self>, op: NewOperation) {
        let env = self.env.clone();
        let ctx = Rc::clone(self);
        crate::loader::run_in_background(
            move || record_best_effort(&env, &op),
            move |()| ctx.history_changed.publish(&()),
        );
    }

    pub fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }
}

/// Epoch atual, em segundos. `StatusEnv::now` é fixado na abertura do app: serve para idades,
/// não para carimbar operações.
pub fn epoch_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Milissegundos desde `started`, para o histórico.
pub fn elapsed_ms(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
