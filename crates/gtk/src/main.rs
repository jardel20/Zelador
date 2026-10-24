//! Zelador — painel operacional do NixOS (GTK4 + libadwaita).

mod actions;
mod ctx;
mod format;
mod i18n;
mod loader;
mod pages;
mod screens;
mod settings;
mod style;
mod widgets;
mod window;

use adw::prelude::*;

use crate::pages::Page;

pub const APP_ID: &str = "br.dev.zelador";

fn main() -> gtk::glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    i18n::init();
    if let Err(error) = gtk::gio::resources_register_include!("zelador.gresource") {
        tracing::error!(%error, "não foi possível registrar os recursos embutidos");
        return gtk::glib::ExitCode::FAILURE;
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|app| {
        let settings = settings::Settings::load();
        ctx::set_prefer_nvd(settings.prefer_nvd);
        if let Some(style) = style::Style::install(settings) {
            actions::install(app, &style, settings);
            // O estilo precisa viver tanto quanto o app.
            std::mem::forget(style);
        }
    });
    app.connect_activate(|app| {
        let context = window::Context::from_env();
        window::build(app, &context).present();
    });
    app.run()
}

/// Tela inicial pedida por `ZELADOR_START_PAGE` (usado para capturar telas na verificação visual).
pub fn start_page_from_env() -> Page {
    std::env::var("ZELADOR_START_PAGE")
        .ok()
        .and_then(|id| Page::from_id(&id))
        .unwrap_or(Page::SeuSistema)
}

/// Gancho de verificação visual: `ZELADOR_DEV_ACTION=select-two|compare|why` abre estados que
/// só existem depois de um clique, para as capturas de `tools/screenshots.sh`. Sem a variável,
/// nada muda.
pub fn dev_action() -> Option<String> {
    std::env::var("ZELADOR_DEV_ACTION")
        .ok()
        .filter(|v| !v.is_empty())
}
