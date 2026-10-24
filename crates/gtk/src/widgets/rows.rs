//! Peças comuns das telas, sobre widgets do libadwaita.

use adw::prelude::*;

/// Seção com título e lista em cartão (`AdwPreferencesGroup`).
pub fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

/// Linha com título e subtítulo opcional. O subtítulo pode ser monoespaçado (revisões, caminhos).
pub fn row(title: &str, subtitle: Option<&str>, mono_subtitle: bool) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .use_markup(false)
        .title_lines(1)
        .build();
    if let Some(subtitle) = subtitle {
        row.set_subtitle(subtitle);
        row.set_subtitle_lines(0);
    }
    if mono_subtitle {
        row.add_css_class("mono-subtitle");
    }
    row
}

/// Texto secundário alinhado à direita de uma linha.
pub fn suffix_label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes(classes.to_vec())
        .valign(gtk::Align::Center)
        .build()
}

pub fn spinner() -> gtk::Spinner {
    gtk::Spinner::builder()
        .spinning(true)
        .valign(gtk::Align::Center)
        .build()
}

/// Linha só com texto esmaecido (estado vazio ou falha).
pub fn muted_row(text: &str) -> adw::ActionRow {
    let row = row(text, None, false);
    row.add_css_class("muted-row");
    row
}

/// Remove todas as linhas de um grupo, por um único caminho (o grupo não tem `clear`).
pub fn clear_group(group: &adw::PreferencesGroup, rows: &mut Vec<gtk::Widget>) {
    for row in rows.drain(..) {
        group.remove(&row);
    }
}
