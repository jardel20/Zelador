//! Diálogo com as mudanças entre dois sistemas (`nix store diff-closures`).

use adw::prelude::*;
use zelador_core::backends::NixBackend;
use zelador_core::diff::{DiffEntry, summarize};
use zelador_core::units::format_bytes;

use crate::ctx::nix;
use crate::format::package_changes;
use crate::i18n::{tr, trf};
use crate::loader::run_in_background;
use crate::widgets::rows;

/// Abre o diálogo e preenche quando o diff chega. Os caminhos são de closures de sistema.
pub fn present(parent: Option<&gtk::Window>, title: String, old_path: String, new_path: String) {
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(560)
        .content_height(560)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .margin_top(6)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let summary = gtk::Label::builder()
        .label(tr("Comparando…"))
        .xalign(0.0)
        .wrap(true)
        .css_classes(["section-title"])
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();
    body.append(&summary);
    body.append(&scroll);
    toolbar.set_content(Some(&body));
    dialog.set_child(Some(&toolbar));
    dialog.present(parent);

    run_in_background(
        move || {
            nix()
                .diff_closures(&old_path, &new_path)
                .map_err(|e| e.to_string())
        },
        move |result| match result {
            Ok(entries) => fill(&summary, &list, &entries),
            Err(error) => {
                summary.set_label(&format!("{} {error}", tr("Não foi possível comparar:")))
            }
        },
    );
}

pub fn fill(summary: &gtk::Label, list: &gtk::ListBox, entries: &[DiffEntry]) {
    let totals = summarize(entries);
    let counts = package_changes(totals.updated, totals.added, totals.removed);
    // O `nvd` não informa tamanho por pacote: sem nenhum, não afirma nada sobre espaço.
    let has_sizes = entries.iter().any(|e| e.size_delta.is_some());
    summary.set_label(&if has_sizes {
        let amount = format_bytes(totals.size_delta.unsigned_abs());
        let delta = if totals.size_delta >= 0 {
            trf("o sistema vai ocupar {0} a mais", &[amount])
        } else {
            trf("o sistema vai ocupar {0} a menos", &[amount])
        };
        trf("{0} · {1}", &[counts, delta])
    } else {
        counts
    });
    let mut changed: Vec<&DiffEntry> = entries
        .iter()
        .filter(|e| e.is_updated() || e.is_added() || e.is_removed())
        .collect();
    changed.sort_by(|a, b| a.name.cmp(&b.name));
    if changed.is_empty() {
        list.append(
            &gtk::ListBoxRow::builder()
                .child(
                    &gtk::Label::builder()
                        .label(tr("Nenhuma mudança de versão entre as duas gerações."))
                        .css_classes(["caption-text"])
                        .margin_top(18)
                        .margin_bottom(18)
                        .build(),
                )
                .activatable(false)
                .build(),
        );
        return;
    }
    for entry in changed {
        list.append(&diff_row(entry));
    }
}

fn diff_row(entry: &DiffEntry) -> adw::ActionRow {
    let versions = |v: &Option<Vec<String>>| match v {
        Some(v) if v.iter().all(String::is_empty) => "—".to_owned(),
        Some(v) => v.join(", "),
        None => "∅".to_owned(),
    };
    let row = rows::row(
        &entry.name,
        Some(&format!(
            "{} → {}",
            versions(&entry.old),
            versions(&entry.new)
        )),
        true,
    );
    if let Some(delta) = entry.size_delta {
        let sign = if delta < 0 { "−" } else { "+" };
        row.add_suffix(&rows::suffix_label(
            &format!("{sign}{}", format_bytes(delta.unsigned_abs())),
            &["caption-text"],
        ));
    }
    row
}
