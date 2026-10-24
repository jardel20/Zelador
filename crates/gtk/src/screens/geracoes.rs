//! "Gerações": lista do sistema e do Home Manager, com seleção e comparação.
//! Somente leitura: voltar, tornar padrão e excluir dependem do helper (fases seguintes).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use zelador_core::backends::NixBackend;
use zelador_core::diff::{DiffEntry, summarize};
use zelador_core::generations::{Generation, GenerationKind, generation_sizes};
use zelador_core::units::format_bytes;

use super::Screen;
use crate::ctx::{Ctx, nix};
use crate::format::short_date_from_epoch;
use crate::i18n::tr;
use crate::loader::run_in_background;
use crate::widgets::rows;

struct State {
    kind: GenerationKind,
    selected: BTreeSet<u64>,
    /// Tamanho incremental por tipo, quando já calculado.
    sizes: HashMap<GenerationKindKey, HashMap<u64, u64>>,
    system: Vec<Generation>,
    home_manager: Vec<Generation>,
}

/// `GenerationKind` não implementa `Hash`; esta chave resolve sem mexer no core.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GenerationKindKey(bool);

fn key(kind: GenerationKind) -> GenerationKindKey {
    GenerationKindKey(kind == GenerationKind::System)
}

struct Ui {
    ctx: Rc<Ctx>,
    state: RefCell<State>,
    list: gtk::ListBox,
    count: gtk::Label,
    compare: gtk::Button,
    view: adw::ToolbarView,
}

pub fn build(ctx: &Rc<Ctx>, intro: &gtk::Label, view: &adw::ToolbarView) -> Screen {
    let _ = intro;
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list.append(&loading_row());

    // Barra inferior: aparece com seleção.
    let count = gtk::Label::builder().css_classes(["strong"]).build();
    let delete = gtk::Button::builder()
        .label(tr("Excluir"))
        .css_classes(["destructive-outline"])
        .sensitive(false)
        .tooltip_text(tr(
            "A exclusão de gerações ainda não está disponível nesta versão",
        ))
        .build();
    let compare = gtk::Button::builder()
        .label(tr("Comparar"))
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["zelador-bottom-bar"])
        .build();
    bar.append(&count);
    bar.append(&gtk::Box::builder().hexpand(true).build());
    bar.append(&delete);
    bar.append(&compare);
    view.add_bottom_bar(&bar);
    view.set_reveal_bottom_bars(false);

    let ui = Rc::new(Ui {
        ctx: Rc::clone(ctx),
        state: RefCell::new(State {
            kind: GenerationKind::System,
            selected: BTreeSet::new(),
            sizes: HashMap::new(),
            system: Vec::new(),
            home_manager: Vec::new(),
        }),
        list,
        count,
        compare: compare.clone(),
        view: view.clone(),
    });

    compare.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| show_compare(&ui)
    ));

    // Alternador Sistema / Home Manager.
    let system_button = gtk::ToggleButton::builder()
        .label(tr("Sistema"))
        .active(true)
        .css_classes(["seg-button"])
        .build();
    let home_button = gtk::ToggleButton::builder()
        .label(tr("Home Manager"))
        .group(&system_button)
        .css_classes(["seg-button"])
        .build();
    let toggle = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["seg-group"])
        .valign(gtk::Align::End)
        .build();
    toggle.append(&system_button);
    toggle.append(&home_button);
    for (button, kind) in [
        (&system_button, GenerationKind::System),
        (&home_button, GenerationKind::HomeManager),
    ] {
        button.connect_toggled(glib::clone!(
            #[strong]
            ui,
            move |button| {
                if button.is_active() {
                    switch_kind(&ui, kind);
                }
            }
        ));
    }

    ctx.overview.get(glib::clone!(
        #[strong]
        ui,
        move |status| {
            {
                let mut state = ui.state.borrow_mut();
                state.system = status.generations.clone();
                state.home_manager = status.home_manager_generations.clone();
            }
            switch_kind(&ui, GenerationKind::System);
            dev_hook(&ui);
        }
    ));

    Screen {
        body: ui.list.clone().upcast(),
        header_end: Some(toggle.upcast()),
    }
}

/// Só para verificação visual (ver `crate::dev_action`): seleciona as duas mais novas e,
/// se pedido, abre a comparação.
fn dev_hook(ui: &Rc<Ui>) {
    let Some(action) = crate::dev_action() else {
        return;
    };
    if action != "select-two" && action != "compare" {
        return;
    }
    let newest: Vec<u64> = {
        let state = ui.state.borrow();
        let mut numbers: Vec<u64> = state.system.iter().map(|g| g.number).collect();
        numbers.sort_unstable_by(|a, b| b.cmp(a));
        numbers.into_iter().take(2).collect()
    };
    for number in newest {
        toggle_selection(ui, number, true);
    }
    render(ui);
    if action == "compare" {
        show_compare(ui);
    }
}

fn loading_row() -> gtk::ListBoxRow {
    let row = gtk::Box::builder()
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .build();
    row.append(&rows::spinner());
    row.append(
        &gtk::Label::builder()
            .label(tr("Lendo as gerações…"))
            .css_classes(["caption-text"])
            .build(),
    );
    gtk::ListBoxRow::builder()
        .child(&row)
        .activatable(false)
        .selectable(false)
        .build()
}

fn switch_kind(ui: &Rc<Ui>, kind: GenerationKind) {
    {
        let mut state = ui.state.borrow_mut();
        state.kind = kind;
        state.selected.clear();
    }
    update_selection_ui(ui);
    render(ui);
    // Tamanhos incrementais: uma consulta por geração, só na primeira vez de cada tipo.
    let generations = {
        let state = ui.state.borrow();
        if state.sizes.contains_key(&key(kind)) {
            return;
        }
        match kind {
            GenerationKind::System => state.system.clone(),
            GenerationKind::HomeManager => state.home_manager.clone(),
        }
    };
    if generations.is_empty() {
        return;
    }
    let ui = Rc::clone(ui);
    run_in_background(
        move || generation_sizes(&nix(), &generations),
        move |result| {
            match result {
                Ok(sizes) => {
                    ui.state
                        .borrow_mut()
                        .sizes
                        .insert(key(kind), sizes.into_iter().collect());
                }
                Err(error) => {
                    tracing::warn!(%error, "não foi possível calcular os tamanhos das gerações")
                }
            }
            if ui.state.borrow().kind == kind {
                render(&ui);
            }
        },
    );
}

fn render(ui: &Rc<Ui>) {
    while let Some(child) = ui.list.first_child() {
        ui.list.remove(&child);
    }
    let (generations, sizes, selected, kind) = {
        let state = ui.state.borrow();
        let generations = match state.kind {
            GenerationKind::System => state.system.clone(),
            GenerationKind::HomeManager => state.home_manager.clone(),
        };
        (
            generations,
            state.sizes.get(&key(state.kind)).cloned(),
            state.selected.clone(),
            state.kind,
        )
    };
    if generations.is_empty() {
        let text = match kind {
            GenerationKind::System => tr("Nenhuma geração do sistema encontrada."),
            GenerationKind::HomeManager => tr("Nenhuma geração do Home Manager encontrada."),
        };
        ui.list.append(
            &gtk::ListBoxRow::builder()
                .child(
                    &gtk::Label::builder()
                        .label(text)
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
    let newest = generations.iter().map(|g| g.number).max();
    for generation in &generations {
        let size = sizes
            .as_ref()
            .and_then(|s| s.get(&generation.number))
            .copied();
        let row = generation_row(
            ui,
            generation,
            size,
            Some(generation.number) == newest,
            selected.contains(&generation.number),
        );
        ui.list.append(&row);
    }
}

fn generation_row(
    ui: &Rc<Ui>,
    generation: &Generation,
    size: Option<u64>,
    is_newest: bool,
    selected: bool,
) -> gtk::ListBoxRow {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(18)
        .margin_end(10)
        .build();

    let check = gtk::CheckButton::builder()
        .active(selected)
        .valign(gtk::Align::Center)
        .build();
    let number = generation.number;
    check.connect_toggled(glib::clone!(
        #[strong]
        ui,
        move |check| toggle_selection(&ui, number, check.is_active())
    ));
    content.append(&check);

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .hexpand(true)
        .build();
    let title = gtk::Box::builder().spacing(8).build();
    title.append(
        &gtk::Label::builder()
            .label(format!("{} {}", tr("Geração"), generation.number))
            .css_classes(["gen-title"])
            .build(),
    );
    if generation.is_current {
        title.append(&tag(&tr("em uso")));
    }
    if generation.is_boot_default {
        title.append(&tag(&tr("padrão no boot")));
    }
    text.append(&title);
    let when = short_date_from_epoch(generation.created);
    let subtitle = match &generation.kernel {
        Some(kernel) => format!("{when} · kernel {kernel}"),
        None => when,
    };
    text.append(
        &gtk::Label::builder()
            .label(subtitle)
            .xalign(0.0)
            .css_classes(["caption-text"])
            .build(),
    );
    content.append(&text);

    let size_text = match size {
        Some(bytes) if is_newest => format_bytes(bytes),
        Some(bytes) => format!("+{}", format_bytes(bytes)),
        None => String::new(),
    };
    content.append(
        &gtk::Label::builder()
            .label(size_text)
            .css_classes(["caption-text"])
            .valign(gtk::Align::Center)
            .build(),
    );
    content.append(&options_menu(generation));

    let row = gtk::ListBoxRow::builder()
        .child(&content)
        .activatable(false)
        .build();
    if selected {
        row.add_css_class("gen-selected");
    }
    row
}

fn tag(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes(["tag"])
        .build()
}

/// Menu ⋯ da geração. As ações existem, mas ficam desabilitadas: dependem do helper.
fn options_menu(generation: &Generation) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    menu.append(Some(&tr("Voltar para esta")), Some("gen.rollback"));
    if generation.kind == GenerationKind::System {
        menu.append(Some(&tr("Tornar padrão no boot")), Some("gen.set-boot"));
    }
    let actions = gio::SimpleActionGroup::new();
    for name in ["rollback", "set-boot"] {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(false);
        actions.add_action(&action);
    }
    let button = gtk::MenuButton::builder()
        .icon_name("dots-three-symbolic")
        .menu_model(&menu)
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .tooltip_text(format!("{} {}", tr("Opções da geração"), generation.number))
        .build();
    button.insert_action_group("gen", Some(&actions));
    button
}

fn toggle_selection(ui: &Rc<Ui>, number: u64, selected: bool) {
    {
        let mut state = ui.state.borrow_mut();
        if selected {
            state.selected.insert(number);
        } else {
            state.selected.remove(&number);
        }
    }
    update_selection_ui(ui);
    // Atualiza só o destaque das linhas, sem reconstruir a lista (o foco do teclado fica).
    let selected = ui.state.borrow().selected.clone();
    let mut index = 0;
    let generations = {
        let state = ui.state.borrow();
        match state.kind {
            GenerationKind::System => state.system.clone(),
            GenerationKind::HomeManager => state.home_manager.clone(),
        }
    };
    while let Some(row) = ui.list.row_at_index(index) {
        if let Some(generation) = generations.get(usize::try_from(index).unwrap_or(usize::MAX)) {
            if selected.contains(&generation.number) {
                row.add_css_class("gen-selected");
            } else {
                row.remove_css_class("gen-selected");
            }
        }
        index += 1;
    }
}

fn update_selection_ui(ui: &Rc<Ui>) {
    let n = ui.state.borrow().selected.len();
    ui.view.set_reveal_bottom_bars(n > 0);
    let label = match n {
        1 => tr("1 selecionada"),
        n => format!("{n} {}", tr("selecionadas")),
    };
    ui.count.set_label(&label);
    ui.compare.set_sensitive(n == 2);
    let hint = tr("Selecione duas gerações para comparar");
    ui.compare
        .set_tooltip_text(if n == 2 { None } else { Some(hint.as_str()) });
}

/// Compara as duas gerações selecionadas: da mais antiga para a mais nova.
fn show_compare(ui: &Rc<Ui>) {
    let (older, newer) = {
        let state = ui.state.borrow();
        let generations = match state.kind {
            GenerationKind::System => &state.system,
            GenerationKind::HomeManager => &state.home_manager,
        };
        let mut picked: Vec<&Generation> = generations
            .iter()
            .filter(|g| state.selected.contains(&g.number))
            .collect();
        picked.sort_by_key(|g| g.number);
        match picked.as_slice() {
            [a, b] => ((*a).clone(), (*b).clone()),
            _ => return,
        }
    };
    let dialog = adw::Dialog::builder()
        .title(format!(
            "{} {} → {}",
            tr("Geração"),
            older.number,
            newer.number
        ))
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
    dialog.present(ui.ctx.window().as_ref());

    let (old_path, new_path) = (
        older.store_path.to_string_lossy().into_owned(),
        newer.store_path.to_string_lossy().into_owned(),
    );
    run_in_background(
        move || {
            nix()
                .diff_closures(&old_path, &new_path)
                .map_err(|e| e.to_string())
        },
        move |result| match result {
            Ok(entries) => fill_diff(&summary, &list, &entries),
            Err(error) => {
                summary.set_label(&format!("{} {error}", tr("Não foi possível comparar:")))
            }
        },
    );
}

fn fill_diff(summary: &gtk::Label, list: &gtk::ListBox, entries: &[DiffEntry]) {
    let totals = summarize(entries);
    let delta = if totals.size_delta >= 0 {
        format!(
            "o sistema vai ocupar {} a mais",
            format_bytes(totals.size_delta.unsigned_abs())
        )
    } else {
        format!(
            "o sistema vai ocupar {} a menos",
            format_bytes(totals.size_delta.unsigned_abs())
        )
    };
    summary.set_label(&format!(
        "{} atualizados, {} novos e {} removidos · {delta}",
        totals.updated, totals.added, totals.removed
    ));
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
