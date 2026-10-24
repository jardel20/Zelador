//! Identidade visual: um único `CssProvider` para o app inteiro, ícones e fontes.
//!
//! O provider é remontado quando o esquema de cores do sistema muda ou quando o usuário
//! alterna entre a identidade do Zelador e o tema do sistema.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::settings::Settings;

const RESOURCE_PREFIX: &str = "/br/dev/zelador";

const BASE: &str = include_str!("../resources/css/base.css");
const TOKENS_LIGHT: &str = include_str!("../resources/css/tokens-light.css");
const TOKENS_DARK: &str = include_str!("../resources/css/tokens-dark.css");
const TOKENS_SYSTEM: &str = include_str!("../resources/css/tokens-system.css");

/// Acima do `gtk.css` do usuário (prioridade `USER`), para a identidade prevalecer.
const IDENTITY_PRIORITY: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 1;

pub struct Style {
    display: gtk::gdk::Display,
    provider: gtk::CssProvider,
    settings: Cell<Settings>,
    priority: Cell<Option<u32>>,
    // Mantém o `Rc` do handler de `notify::dark` vivo e quebra o ciclo ao soltar o estilo.
    _keepalive: RefCell<Option<Rc<Style>>>,
}

impl Style {
    /// Instala ícones, fontes e o CSS. Devolve `None` se não houver display.
    pub fn install(settings: Settings) -> Option<Rc<Style>> {
        let Some(display) = gtk::gdk::Display::default() else {
            tracing::warn!("sem display; o estilo não foi aplicado");
            return None;
        };
        // Ícones Phosphor (`*-symbolic.svg`) e o ícone do app dentro do gresource.
        gtk::IconTheme::for_display(&display)
            .add_resource_path(&format!("{RESOURCE_PREFIX}/icons"));
        gtk::Window::set_default_icon_name(crate::APP_ID);

        let style = Rc::new(Style {
            display,
            provider: gtk::CssProvider::new(),
            settings: Cell::new(settings),
            priority: Cell::new(None),
            _keepalive: RefCell::new(None),
        });
        style.apply();

        // O sistema decide claro ou escuro; a paleta de cada um foi desenhada à parte.
        let manager = adw::StyleManager::default();
        manager.set_color_scheme(adw::ColorScheme::Default);
        let weak = Rc::downgrade(&style);
        manager.connect_dark_notify(move |_| {
            if let Some(style) = weak.upgrade() {
                style.apply();
            }
        });
        Some(style)
    }

    pub fn set_settings(&self, settings: Settings) {
        self.settings.set(settings);
        self.apply();
    }

    fn apply(&self) {
        let settings = self.settings.get();
        let tokens = if settings.use_system_theme {
            TOKENS_SYSTEM
        } else if adw::StyleManager::default().is_dark() {
            TOKENS_DARK
        } else {
            TOKENS_LIGHT
        };
        self.provider.load_from_string(&format!("{tokens}\n{BASE}"));

        let priority = if settings.use_system_theme {
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION
        } else {
            IDENTITY_PRIORITY
        };
        if self.priority.get() != Some(priority) {
            if self.priority.get().is_some() {
                gtk::style_context_remove_provider_for_display(&self.display, &self.provider);
            }
            gtk::style_context_add_provider_for_display(&self.display, &self.provider, priority);
            self.priority.set(Some(priority));
        }
    }
}
