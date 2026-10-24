//! "Gerações": lista do sistema e do Home Manager, com seleção e comparação.
//! Somente leitura: voltar, tornar padrão e excluir dependem do helper (fases seguintes).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use zelador_core::backends::NixBackend;
use zelador_core::cleanup::{HelperClient, HelperIssue, PrivilegedHelper};
use zelador_core::generations::{Generation, GenerationKind, generation_sizes};
use zelador_core::runner::SystemRunner;
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
    delete: gtk::Button,
    view: adw::ToolbarView,
    /// Por que as ações privilegiadas estão indisponíveis (helper não instalado), se for o caso.
    helper_issue: Option<HelperIssue>,
    /// Uma ação em andamento: bloqueia outra ao mesmo tempo.
    busy: Cell<bool>,
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
        delete: delete.clone(),
        view: view.clone(),
        helper_issue: HelperClient::production(SystemRunner).check().err(),
        busy: Cell::new(false),
    });

    delete.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| confirm_delete(&ui)
    ));

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
    content.append(&options_menu(ui, generation));

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

/// Menu ⋯ da geração. As ações do sistema dependem do helper; as do Home Manager não existem
/// aqui (reativar uma geração do Home Manager por fora do `home-manager switch` é arriscado).
fn options_menu(ui: &Rc<Ui>, generation: &Generation) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    menu.append(Some(&tr("Voltar para esta")), Some("gen.rollback"));
    if generation.kind == GenerationKind::System {
        menu.append(Some(&tr("Tornar padrão no boot")), Some("gen.set-boot"));
    }
    let is_system = generation.kind == GenerationKind::System;
    let helper_ok = ui.helper_issue.is_none();
    let actions = gio::SimpleActionGroup::new();
    let make = |name: &str, enabled: bool, on_activate: Box<dyn Fn()>| {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(enabled);
        action.connect_activate(move |_, _| on_activate());
        actions.add_action(&action);
    };
    let number = generation.number;
    let (ui1, ui2) = (Rc::clone(ui), Rc::clone(ui));
    make(
        "rollback",
        is_system && helper_ok && !generation.is_current,
        Box::new(move || confirm_rollback(&ui1, number)),
    );
    make(
        "set-boot",
        is_system && helper_ok && !generation.is_boot_default,
        Box::new(move || confirm_set_boot(&ui2, number)),
    );
    let tooltip = if let Some(issue) = ui.helper_issue.filter(|_| is_system) {
        helper_issue_text(issue)
    } else if !is_system {
        tr("Para o Home Manager, use o próprio home-manager")
    } else {
        format!("{} {}", tr("Opções da geração"), generation.number)
    };
    let button = gtk::MenuButton::builder()
        .icon_name("dots-three-symbolic")
        .menu_model(&menu)
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .tooltip_text(tooltip)
        .build();
    button.insert_action_group("gen", Some(&actions));
    button
}

fn helper_issue_text(issue: HelperIssue) -> String {
    match issue {
        HelperIssue::PkexecMissing => tr("O pkexec com setuid não está habilitado neste sistema"),
        HelperIssue::HelperMissing | HelperIssue::PolicyMissing => {
            tr("O helper privilegiado não está instalado neste sistema")
        }
    }
}

// ── Ações que alteram o sistema (sempre com confirmação) ───────────────────

fn confirm(
    ui: &Rc<Ui>,
    heading: String,
    body: String,
    action_label: String,
    appearance: adw::ResponseAppearance,
    on_confirm: impl Fn() + 'static,
) {
    if ui.busy.get() {
        return;
    }
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .close_response("cancel")
        .default_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", &tr("Cancelar")), ("go", &action_label)]);
    dialog.set_response_appearance("go", appearance);
    dialog.connect_response(None, move |_, response| {
        if response == "go" {
            on_confirm();
        }
    });
    dialog.present(ui.ctx.window().as_ref());
}

fn confirm_rollback(ui: &Rc<Ui>, number: u64) {
    let ui2 = Rc::clone(ui);
    confirm(
        ui,
        format!("{} {number}?", tr("Voltar para a geração")),
        tr(
            "O sistema passa a usar esta geração agora e ela vira o padrão no boot. Serviços podem reiniciar e programas abertos podem precisar ser reiniciados. Será pedida a senha de administrador.",
        ),
        tr("Voltar"),
        adw::ResponseAppearance::Suggested,
        move || {
            run_helper(
                &ui2,
                vec!["rollback".to_owned(), number.to_string()],
                format!("{} {number}", tr("Voltou para a geração")),
            );
        },
    );
}

fn confirm_set_boot(ui: &Rc<Ui>, number: u64) {
    let ui2 = Rc::clone(ui);
    confirm(
        ui,
        format!(
            "{} {number} {}",
            tr("Tornar a geração"),
            tr("o padrão no boot?")
        ),
        tr(
            "No próximo reinício o sistema inicia nesta geração. Nada muda agora. Será pedida a senha de administrador.",
        ),
        tr("Tornar padrão"),
        adw::ResponseAppearance::Suggested,
        move || {
            run_helper(
                &ui2,
                vec!["set-boot-default".to_owned(), number.to_string()],
                format!(
                    "{} {number} {}",
                    tr("Geração"),
                    tr("é o novo padrão no boot")
                ),
            );
        },
    );
}

fn confirm_delete(ui: &Rc<Ui>) {
    let (kind, numbers) = {
        let state = ui.state.borrow();
        (
            state.kind,
            state.selected.iter().copied().collect::<Vec<u64>>(),
        )
    };
    if numbers.is_empty() {
        return;
    }
    let list = numbers
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let password = if kind == GenerationKind::System {
        tr(" Será pedida a senha de administrador.")
    } else {
        String::new()
    };
    let ui2 = Rc::clone(ui);
    confirm(
        ui,
        if numbers.len() == 1 {
            tr("Excluir 1 geração?")
        } else {
            format!("{} {} {}", tr("Excluir"), numbers.len(), tr("gerações?"))
        },
        format!(
            "{} {list}. {}{password}",
            tr("Gerações:"),
            tr(
                "Os arquivos só saem do disco na próxima limpeza (coleta de lixo). Não dá para desfazer."
            )
        ),
        tr("Excluir"),
        adw::ResponseAppearance::Destructive,
        move || delete_generations(&ui2, kind, numbers.clone()),
    );
}

fn delete_generations(ui: &Rc<Ui>, kind: GenerationKind, numbers: Vec<u64>) {
    let message = if numbers.len() == 1 {
        tr("Geração excluída")
    } else {
        tr("Gerações excluídas")
    };
    match kind {
        GenerationKind::System => {
            let mut args = vec!["delete-generations".to_owned()];
            args.extend(numbers.iter().map(u64::to_string));
            run_helper(ui, args, message);
        }
        GenerationKind::HomeManager => {
            let profile = ui.ctx.env.home_manager_profiles_dir.join("home-manager");
            run_blocking(ui, message, move || {
                nix()
                    .delete_user_generations(&profile, &numbers)
                    .map_err(|e| e.to_string())
            });
        }
    }
}

fn run_helper(ui: &Rc<Ui>, args: Vec<String>, success: String) {
    run_blocking(ui, success, move || {
        HelperClient::production(SystemRunner)
            .run(&args)
            .map_err(|e| e.to_string())
    });
}

/// Roda a ação fora da thread principal, avisa o resultado e recarrega a lista.
fn run_blocking(
    ui: &Rc<Ui>,
    success: String,
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    ui.busy.set(true);
    ui.delete.set_sensitive(false);
    let ui = Rc::clone(ui);
    run_in_background(work, move |result| {
        ui.busy.set(false);
        match result {
            Ok(()) => ui.ctx.toast(&success),
            Err(error) => ui
                .ctx
                .toast(&format!("{} {error}", tr("Não foi possível concluir:"))),
        }
        reload(&ui);
    });
}

/// Relê as gerações e refaz a lista (e os tamanhos) depois de uma mudança.
fn reload(ui: &Rc<Ui>) {
    let ui = Rc::clone(ui);
    let ctx = Rc::clone(&ui.ctx);
    ctx.overview.refresh(move |status| {
        let kind = {
            let mut state = ui.state.borrow_mut();
            state.system = status.generations.clone();
            state.home_manager = status.home_manager_generations.clone();
            state.sizes.clear();
            state.kind
        };
        switch_kind(&ui, kind);
    });
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
    let (n, protected, kind) = {
        let state = ui.state.borrow();
        let generations = match state.kind {
            GenerationKind::System => &state.system,
            GenerationKind::HomeManager => &state.home_manager,
        };
        let protected = generations
            .iter()
            .filter(|g| state.selected.contains(&g.number))
            .any(|g| g.is_current || (g.kind == GenerationKind::System && g.is_boot_default));
        (state.selected.len(), protected, state.kind)
    };
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

    // Excluir: nunca a geração em uso nem a padrão do boot; as do sistema precisam do helper.
    let helper_needed = kind == GenerationKind::System;
    let reason = if protected {
        Some(tr(
            "A geração em uso e a padrão do boot não podem ser excluídas",
        ))
    } else if let (true, Some(issue)) = (helper_needed, ui.helper_issue) {
        Some(helper_issue_text(issue))
    } else {
        None
    };
    ui.delete
        .set_sensitive(n > 0 && reason.is_none() && !ui.busy.get());
    ui.delete.set_tooltip_text(reason.as_deref());
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
    crate::widgets::diff::present(
        ui.ctx.window().as_ref(),
        format!("{} {} → {}", tr("Geração"), older.number, newer.number),
        older.store_path.to_string_lossy().into_owned(),
        newer.store_path.to_string_lossy().into_owned(),
    );
}
