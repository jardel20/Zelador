//! Ações do aplicativo (menu principal).

use std::rc::Rc;

use adw::prelude::*;

use crate::i18n::tr;
use crate::settings::Settings;
use crate::style::Style;

pub fn install(app: &adw::Application, style: &Rc<Style>, settings: Settings) {
    let about = gtk::gio::ActionEntry::builder("about")
        .activate(|app: &adw::Application, _, _| show_about(app))
        .build();
    let quit = gtk::gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build();
    // Alterna entre a identidade do Zelador (padrão) e o tema do usuário.
    let style = Rc::clone(style);
    let system_theme = gtk::gio::ActionEntry::builder("system-theme")
        .state(settings.use_system_theme.to_variant())
        .change_state(move |_, action, value| {
            let Some(enabled) = value.and_then(|v| v.get::<bool>()) else {
                return;
            };
            action.set_state(&enabled.to_variant());
            let settings = Settings {
                use_system_theme: enabled,
            };
            settings.save();
            style.set_settings(settings);
        })
        .build();
    app.add_action_entries([about, quit, system_theme]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
}

fn show_about(app: &adw::Application) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Zelador")
        .application_icon(crate::APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .comments(tr(
            "Painel operacional do NixOS: atualizar, voltar e liberar espaço.",
        ))
        .license_type(gtk::License::MitX11)
        .build();
    dialog.present(app.active_window().as_ref());
}
