//! "Atualizar": assistente de 3 etapas — escolher inputs, construir sem privilégio e revisar
//! e aplicar. O `flake.lock` do usuário só é gravado depois que a ativação dá certo.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};
use zelador_core::apply::{ActivationEvent, ApplyMode};
use zelador_core::backends::{BuildRequest, NixBuild};
use zelador_core::build::{BuildOutcome, BuildPhase, BuildProgress, run_build};
use zelador_core::cleanup::{HelperClient, HelperIssue};
use zelador_core::flake::{FlakeInput, InputChange, pin_reference};
use zelador_core::runner::{CancelToken, SystemRunner};
use zelador_core::status::SystemStatus;
use zelador_core::units::format_bytes;
use zelador_core::update::{
    LockPreparation, Pin, UpdateSummary, UpdateWorkspace, commit_lock, prepare_lock,
    summarize_update,
};

use super::Screen;
use crate::ctx::{ApplyEvent, Ctx, nix};
use crate::format::{count, relative_age};
use crate::i18n::tr;
use crate::loader::stream_in_background;
use crate::pages::Page;
use crate::widgets::{bottom_bar, diff, rows};

const MAX_LOG_LINES: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Choose,
    Building,
    /// O build falhou: mostra o motivo e só deixa voltar.
    Failed,
    Review,
}

impl Step {
    fn number(self) -> u32 {
        match self {
            Step::Choose => 1,
            Step::Building | Step::Failed => 2,
            Step::Review => 3,
        }
    }
}

/// O que o build deixou pronto para revisar.
struct Built {
    out_path: PathBuf,
    current_system: PathBuf,
    summary: UpdateSummary,
    duration: String,
    previous_generation: Option<u64>,
}

enum Event {
    Evaluating,
    Locking,
    Locked(LockPreparation),
    Progress(BuildProgress),
    Summary(Box<Result<UpdateSummary, String>>),
    Done(Result<BuildOutcome, String>),
}

struct InputRow {
    name: String,
    check: gtk::CheckButton,
    tag: gtk::Label,
}

struct Wizard {
    ctx: Rc<Ctx>,
    title: gtk::Label,
    intro: gtk::Label,
    step_label: gtk::Label,
    stack: gtk::Stack,
    helper_issue: Option<HelperIssue>,
    workspace: UpdateWorkspace,

    // 1. Escolher
    inputs_group: adw::PreferencesGroup,
    input_rows: RefCell<Vec<InputRow>>,
    /// Tudo o que o grupo de inputs contém agora (linhas e indicador de espera), para limpar.
    input_widgets: RefCell<Vec<gtk::Widget>>,
    inputs: RefCell<Vec<FlakeInput>>,
    pins: RefCell<HashMap<String, Pin>>,
    status: RefCell<Option<Rc<SystemStatus>>>,

    // 2. Construir
    lock_row: BuildStepRow,
    eval_row: BuildStepRow,
    build_row: BuildStepRow,
    progress: gtk::ProgressBar,
    activity: gtk::Label,
    log: gtk::Label,
    log_lines: RefCell<VecDeque<String>>,
    failure: gtk::Label,
    cancel: RefCell<Option<Arc<CancelToken>>>,

    // 3. Revisar
    summary_row: adw::ActionRow,
    review_changes: gtk::Button,
    boot_hint: gtk::Label,
    test_hint: gtk::Label,
    mode_boot: gtk::CheckButton,
    mode_switch: gtk::CheckButton,
    mode_test: gtk::CheckButton,
    built: RefCell<Option<Rc<Built>>>,
    /// O resumo chega antes do caminho construído; fica aqui até o build terminar.
    pending_summary: RefCell<Option<UpdateSummary>>,
    changes: RefCell<Vec<InputChange>>,

    // Barra inferior
    back: gtk::Button,
    primary: gtk::Button,
    dots: Vec<gtk::Box>,
    step: Cell<Step>,
    busy: Cell<bool>,
}

struct BuildStepRow {
    stack: gtk::Stack,
    label: gtk::Label,
}

pub fn build(
    ctx: &Rc<Ctx>,
    title: &gtk::Label,
    intro: &gtk::Label,
    step_label: &gtk::Label,
    view: &adw::ToolbarView,
) -> Screen {
    let helper_issue = HelperClient::production(SystemRunner).check().err();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .vhomogeneous(false)
        .build();

    // ── Página 1: escolher ────────────────────────────────────────────────
    let choose = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .margin_top(14)
        .build();
    let inputs_group = rows::group(&tr("Inputs do flake"));
    let loading = rows::row(&tr("Lendo os inputs do flake…"), None, false);
    loading.add_suffix(&rows::spinner());
    inputs_group.add(&loading);
    choose.append(&inputs_group);
    choose.append(&caption(&tr(
        "O seu flake.lock só muda se você aplicar a atualização. Inputs fixados numa revisão não são atualizados.",
    )));
    stack.add_named(&choose, Some("choose"));

    // ── Página 2: construir ───────────────────────────────────────────────
    let building = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(14)
        .build();
    // Quebra por caractere quando preciso: mensagens de erro trazem caminhos longos sem espaços.
    let activity = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["option-title"])
        .build();
    let progress = gtk::ProgressBar::builder().css_classes(["thin"]).build();
    let top = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    top.append(&activity);
    top.append(&progress);
    building.append(&top);
    let steps = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    let lock_row = build_step(&steps, &tr("Atualizar o lock dos inputs"));
    let eval_row = build_step(&steps, &tr("Avaliar a configuração"));
    let build_row = build_step(&steps, &tr("Construir e baixar"));
    building.append(&steps);
    let failure = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .visible(false)
        .css_classes(["mono", "log-view"])
        .build();
    building.append(&failure);
    let log = gtk::Label::builder()
        .xalign(0.0)
        .yalign(0.0)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::Char)
        .css_classes(["mono", "log-view"])
        .build();
    building.append(
        &gtk::Expander::builder()
            .label(tr("Detalhes"))
            .child(&log)
            .css_classes(["details-card"])
            .build(),
    );
    stack.add_named(&building, Some("building"));

    // ── Página 3: revisar ─────────────────────────────────────────────────
    let review = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(28)
        .margin_top(14)
        .build();
    if let Some(issue) = helper_issue {
        review.append(&helper_banner(issue));
    }
    let summary_group = rows::group("");
    let summary_row = rows::row(&tr("Calculando as mudanças…"), None, false);
    let review_changes = gtk::Button::builder()
        .label(tr("Ver mudanças"))
        .valign(gtk::Align::Center)
        .sensitive(false)
        .build();
    summary_row.add_suffix(&review_changes);
    summary_group.add(&summary_row);
    review.append(&summary_group);

    review.append(
        &gtk::Label::builder()
            .label(tr("Efetivar mudanças"))
            .css_classes(["section-title"])
            .xalign(0.0)
            .build(),
    );
    let mode_boot = gtk::CheckButton::new();
    let mode_switch = gtk::CheckButton::builder().group(&mode_boot).build();
    let mode_test = gtk::CheckButton::builder().group(&mode_boot).build();
    let boot_hint = caption("");
    let test_hint = caption("");
    let options = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    options.append(&mode_card(
        &mode_boot,
        &tr("No próximo reinício"),
        &boot_hint,
    ));
    options.append(&mode_card(
        &mode_switch,
        &tr("Agora"),
        &caption(&tr("Troca os serviços em execução. Alguns programas abertos podem precisar ser reiniciados.")),
    ));
    options.append(&mode_card(&mode_test, &tr("Só até reiniciar"), &test_hint));
    review.append(&options);
    let discard = gtk::Button::builder()
        .label(tr("Descartar esta atualização"))
        .halign(gtk::Align::Start)
        .css_classes(["flat", "link-button"])
        .build();
    review.append(&discard);
    stack.add_named(&review, Some("review"));

    // ── Barra inferior ────────────────────────────────────────────────────
    let back = gtk::Button::builder().label(tr("Voltar")).build();
    let primary = gtk::Button::builder()
        .label(tr("Continuar"))
        .css_classes(["suggested-action"])
        .build();
    let dots_box = gtk::Box::builder()
        .spacing(8)
        .valign(gtk::Align::Center)
        .build();
    let dots: Vec<gtk::Box> = (0..3)
        .map(|_| {
            let dot = gtk::Box::builder().css_classes(["dot"]).build();
            dots_box.append(&dot);
            dot
        })
        .collect();
    let bar = bottom_bar::shell();
    bar.append(&back);
    bar.append(&bottom_bar::spacer());
    bar.append(&dots_box);
    bar.append(&bottom_bar::spacer());
    bar.append(&primary);
    view.add_bottom_bar(&bar);

    let wizard = Rc::new(Wizard {
        ctx: Rc::clone(ctx),
        title: title.clone(),
        intro: intro.clone(),
        step_label: step_label.clone(),
        stack,
        helper_issue,
        workspace: UpdateWorkspace::default_for(&ctx.env),
        inputs_group,
        input_rows: RefCell::new(Vec::new()),
        input_widgets: RefCell::new(vec![loading.clone().upcast()]),
        inputs: RefCell::new(Vec::new()),
        pins: RefCell::new(HashMap::new()),
        status: RefCell::new(None),
        lock_row,
        eval_row,
        build_row,
        progress,
        activity,
        log,
        log_lines: RefCell::new(VecDeque::new()),
        failure,
        cancel: RefCell::new(None),
        summary_row,
        review_changes: review_changes.clone(),
        boot_hint,
        test_hint,
        mode_boot,
        mode_switch,
        mode_test,
        built: RefCell::new(None),
        pending_summary: RefCell::new(None),
        changes: RefCell::new(Vec::new()),
        back: back.clone(),
        primary: primary.clone(),
        dots,
        step: Cell::new(Step::Choose),
        busy: Cell::new(false),
    });

    {
        let w = Rc::clone(&wizard);
        wizard.primary.connect_clicked(move |_| w.on_primary());
        let w = Rc::clone(&wizard);
        wizard.back.connect_clicked(move |_| w.on_back());
        let w = Rc::clone(&wizard);
        discard.connect_clicked(move |_| w.discard_and_restart());
        let w = Rc::clone(&wizard);
        review_changes.connect_clicked(move |_| w.show_changes());
    }
    // Depois de uma ativação bem-sucedida, volta ao começo com os dados novos.
    {
        let w = Rc::clone(&wizard);
        ctx.apply_events.subscribe(move |event| {
            if let ApplyEvent::Finished(Ok(())) = event {
                w.reset_after_success();
            }
        });
    }
    wizard.set_step(Step::Choose);
    wizard.load_inputs();

    Screen {
        body: wizard.stack.clone().upcast(),
        header_end: None,
    }
}

fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption-text"])
        .build()
}

fn helper_banner(issue: HelperIssue) -> gtk::Box {
    let (title, hint) = match issue {
        HelperIssue::HelperMissing | HelperIssue::PolicyMissing => (
            tr("O helper privilegiado não está instalado neste sistema."),
            tr(
                "Você pode construir e revisar a atualização, mas só consegue aplicá-la depois de ativar programs.zelador.enable.",
            ),
        ),
        HelperIssue::PkexecMissing => (
            tr("O pkexec com setuid não está habilitado neste sistema."),
            tr(
                "Você pode construir e revisar a atualização, mas só consegue aplicá-la com security.polkit.enablePkexecWrapper ativo.",
            ),
        ),
    };
    let banner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["warning-banner"])
        .build();
    banner.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["banner-title"])
            .build(),
    );
    banner.append(
        &gtk::Label::builder()
            .label(hint)
            .xalign(0.0)
            .wrap(true)
            .build(),
    );
    banner
}

fn mode_card(radio: &gtk::CheckButton, title: &str, hint: &gtk::Label) -> gtk::Box {
    let card = gtk::Box::builder()
        .spacing(14)
        .css_classes(["option-card"])
        .build();
    radio.set_valign(gtk::Align::Start);
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .build();
    body.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["option-title"])
            .build(),
    );
    body.append(hint);
    card.append(radio);
    card.append(&body);
    let (card2, radio2) = (card.clone(), radio.clone());
    radio.connect_toggled(move |_| {
        if radio2.is_active() {
            card2.add_css_class("on");
        } else {
            card2.remove_css_class("on");
        }
    });
    // Clicar em qualquer ponto do cartão marca a opção.
    let click = gtk::GestureClick::new();
    let radio3 = radio.clone();
    click.connect_released(move |_, _, _, _| radio3.set_active(true));
    card.add_controller(click);
    card
}

fn build_step(container: &gtk::Box, text: &str) -> BuildStepRow {
    let row = gtk::Box::builder()
        .spacing(12)
        .css_classes(["step-row"])
        .build();
    let stack = gtk::Stack::builder().valign(gtk::Align::Center).build();
    stack.add_named(
        &gtk::Image::builder()
            .icon_name("circle-symbolic")
            .pixel_size(22)
            .css_classes(["step-pending"])
            .build(),
        Some("pending"),
    );
    stack.add_named(
        &gtk::Spinner::builder().spinning(true).build(),
        Some("running"),
    );
    stack.add_named(
        &gtk::Image::builder()
            .icon_name("check-circle-fill-symbolic")
            .pixel_size(22)
            .css_classes(["step-done"])
            .build(),
        Some("done"),
    );
    stack.add_named(
        &gtk::Image::builder()
            .icon_name("warning-symbolic")
            .pixel_size(22)
            .css_classes(["warn-icon"])
            .build(),
        Some("failed"),
    );
    stack.set_visible_child_name("pending");
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .hexpand(true)
        .build();
    row.append(&stack);
    row.append(&label);
    container.append(&row);
    BuildStepRow { stack, label }
}

fn set_state(row: &BuildStepRow, state: &str) {
    row.stack.set_visible_child_name(state);
    row.label.set_sensitive(state != "pending");
}

impl Wizard {
    // ── Navegação ─────────────────────────────────────────────────────────

    fn set_step(&self, step: Step) {
        self.step.set(step);
        self.stack.set_visible_child_name(match step {
            Step::Choose => "choose",
            Step::Building | Step::Failed => "building",
            Step::Review => "review",
        });
        self.step_label.set_label(&format!(
            "{} {} {} 3",
            tr("Atualização · etapa"),
            step.number(),
            tr("de")
        ));
        for (index, dot) in self.dots.iter().enumerate() {
            if index + 1 == step.number() as usize {
                dot.add_css_class("dot-active");
            } else {
                dot.remove_css_class("dot-active");
            }
        }
        let (title, intro) = match step {
            Step::Choose => (
                tr("O que atualizar?"),
                tr("Escolha os inputs do flake. Nada muda no sistema até você aplicar."),
            ),
            Step::Building => (
                tr("Construindo a atualização"),
                tr(
                    "Você pode continuar usando o computador. Nada muda no sistema até você aplicar.",
                ),
            ),
            Step::Failed => (
                tr("A atualização não pôde ser construída"),
                tr("Nada mudou no sistema. O seu flake.lock continua como estava."),
            ),
            Step::Review => (
                tr("Aplicar atualizações?"),
                tr("O build terminou sem erros. Nada mudou no sistema ainda."),
            ),
        };
        self.title.set_label(&title);
        self.intro.set_label(&intro);
        self.update_buttons();
    }

    fn update_buttons(&self) {
        match self.step.get() {
            Step::Choose => {
                self.back.set_visible(false);
                self.primary.set_visible(true);
                self.primary.set_label(&tr("Continuar"));
                self.primary
                    .set_sensitive(self.has_work() && !self.busy.get());
                self.primary.set_tooltip_text(None);
            }
            Step::Building => {
                self.back.set_visible(true);
                self.back.set_label(&tr("Cancelar"));
                self.primary.set_visible(true);
                self.primary.set_label(&tr("Construindo…"));
                self.primary.set_sensitive(false);
            }
            Step::Failed => {
                self.back.set_visible(true);
                self.back.set_label(&tr("Voltar"));
                self.primary.set_visible(false);
            }
            Step::Review => {
                self.back.set_visible(true);
                self.back.set_label(&tr("Voltar"));
                self.primary.set_visible(true);
                self.primary.set_label(&tr("Aplicar"));
                let ready = self.helper_issue.is_none()
                    && self.built.borrow().is_some()
                    && !self.busy.get();
                self.primary.set_sensitive(ready);
                let reason = self
                    .helper_issue
                    .map(|_| tr("O helper privilegiado não está instalado neste sistema"));
                self.primary.set_tooltip_text(reason.as_deref());
            }
        }
    }

    fn on_primary(self: &Rc<Self>) {
        match self.step.get() {
            Step::Choose => self.start_update(),
            Step::Review => self.apply(),
            Step::Building | Step::Failed => {}
        }
    }

    fn on_back(self: &Rc<Self>) {
        match self.step.get() {
            Step::Choose => {}
            Step::Building => {
                // O resto acontece quando a thread termina e avisa `Done(Cancelled)`.
                if let Some(token) = self.cancel.borrow().as_ref() {
                    token.cancel();
                }
            }
            Step::Failed | Step::Review => self.discard_and_restart(),
        }
    }

    fn discard_and_restart(&self) {
        if let Err(error) = self.workspace.discard() {
            tracing::warn!(%error, "não foi possível limpar o estado da atualização");
        }
        *self.built.borrow_mut() = None;
        self.changes.borrow_mut().clear();
        self.set_step(Step::Choose);
    }

    fn reset_after_success(self: &Rc<Self>) {
        *self.built.borrow_mut() = None;
        self.changes.borrow_mut().clear();
        self.pins.borrow_mut().clear();
        self.busy.set(false);
        self.set_step(Step::Choose);
        // O flake.lock mudou: relê os inputs.
        let w = Rc::clone(self);
        self.ctx.overview.refresh(move |status| {
            *w.status.borrow_mut() = Some(Rc::clone(&status));
            *w.inputs.borrow_mut() = status.inputs.iter().map(|i| i.input.clone()).collect();
            w.render_inputs(&status);
        });
    }

    // ── 1. Escolher ───────────────────────────────────────────────────────

    fn load_inputs(self: &Rc<Self>) {
        let w = Rc::clone(self);
        self.ctx.overview.get(move |status| {
            *w.status.borrow_mut() = Some(Rc::clone(&status));
            *w.inputs.borrow_mut() = status.inputs.iter().map(|i| i.input.clone()).collect();
            w.render_inputs(&status);
            match crate::dev_action().as_deref() {
                Some("wizard-demo") => w.dev_demo(),
                // Atualiza tudo, como o botão "Continuar" com a seleção padrão.
                Some("wizard-update") => w.start_update(),
                _ => {}
            }
        });
    }

    fn render_inputs(self: &Rc<Self>, status: &SystemStatus) {
        let mut widgets = self.input_widgets.borrow_mut();
        rows::clear_group(&self.inputs_group, &mut widgets);
        self.input_rows.borrow_mut().clear();
        if status.inputs.is_empty() {
            let empty = rows::muted_row(&tr("Nenhum input encontrado no flake."));
            self.inputs_group.add(&empty);
            widgets.push(empty.upcast());
            return;
        }
        let mut held = self.input_rows.borrow_mut();
        for item in &status.inputs {
            let name = item.input.name.clone();
            let check = gtk::CheckButton::builder()
                .active(true)
                .valign(gtk::Align::Center)
                .build();
            let row = rows::row(&name, Some(&item.input.label()), true);
            row.add_prefix(&check);
            let age = item
                .input
                .age_days(self.ctx.env.now)
                .map(relative_age)
                .unwrap_or_default();
            row.add_suffix(&rows::suffix_label(
                &age,
                if item.stale {
                    &["age-stale"]
                } else {
                    &["caption-text"]
                },
            ));
            let tag = gtk::Label::builder()
                .css_classes(["input-tag"])
                .visible(false)
                .valign(gtk::Align::Center)
                .build();
            row.add_suffix(&tag);
            row.add_suffix(&self.input_menu(&item.input));
            let w = Rc::clone(self);
            check.connect_toggled(move |_| w.update_buttons());
            self.inputs_group.add(&row);
            widgets.push(row.clone().upcast());
            held.push(InputRow { name, check, tag });
        }
        drop(held);
        drop(widgets);
        self.update_buttons();
    }

    fn input_menu(self: &Rc<Self>, input: &FlakeInput) -> gtk::MenuButton {
        let menu = gio::Menu::new();
        menu.append(Some(&tr("Atualizar só este")), Some("input.only"));
        menu.append(Some(&tr("Fixar numa revisão…")), Some("input.pin"));
        menu.append(Some(&tr("Remover a fixação")), Some("input.unpin"));
        let actions = gio::SimpleActionGroup::new();
        let name = input.name.clone();
        let make = |id: &str, run: Box<dyn Fn()>| {
            let action = gio::SimpleAction::new(id, None);
            action.connect_activate(move |_, _| run());
            actions.add_action(&action);
        };
        let (w, n) = (Rc::clone(self), name.clone());
        make("only", Box::new(move || w.select_only(&n)));
        let (w, i) = (Rc::clone(self), input.clone());
        make("pin", Box::new(move || w.ask_pin(&i)));
        let (w, n) = (Rc::clone(self), name);
        make("unpin", Box::new(move || w.set_pin(&n, None)));
        let button = gtk::MenuButton::builder()
            .icon_name("dots-three-symbolic")
            .menu_model(&menu)
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .tooltip_text(tr("Opções do input"))
            .build();
        button.insert_action_group("input", Some(&actions));
        button
    }

    fn select_only(&self, name: &str) {
        for row in self.input_rows.borrow().iter() {
            row.check.set_active(row.name == name);
        }
    }

    /// Pede a revisão (40 caracteres) e fixa o input.
    fn ask_pin(self: &Rc<Self>, input: &FlakeInput) {
        let entry = gtk::Entry::builder()
            .text(input.rev.clone().unwrap_or_default())
            .css_classes(["mono"])
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading(format!("{} {}", tr("Fixar numa revisão:"), input.name))
            .body(tr(
                "Cole a revisão completa (40 caracteres). O input fica nela e não é atualizado.",
            ))
            .extra_child(&entry)
            .close_response("cancel")
            .default_response("pin")
            .build();
        dialog.add_responses(&[("cancel", &tr("Cancelar")), ("pin", &tr("Fixar"))]);
        dialog.set_response_appearance("pin", adw::ResponseAppearance::Suggested);
        let (w, input) = (Rc::clone(self), input.clone());
        dialog.connect_response(None, move |_, response| {
            if response != "pin" {
                return;
            }
            let rev = entry.text().trim().to_lowercase();
            match pin_reference(&input, &rev) {
                Some(flake_ref) => w.set_pin(
                    &input.name,
                    Some(Pin {
                        input: input.name.clone(),
                        flake_ref,
                    }),
                ),
                None => w
                    .ctx
                    .toast(&tr("Revisão inválida: use os 40 caracteres hexadecimais")),
            }
        });
        dialog.present(self.ctx.window().as_ref());
    }

    fn set_pin(&self, name: &str, pin: Option<Pin>) {
        let label = pin
            .as_ref()
            .map(|p| format!("{} {}", tr("fixado em"), short_rev_of(&p.flake_ref)));
        match pin {
            Some(pin) => {
                self.pins.borrow_mut().insert(name.to_owned(), pin);
            }
            None => {
                self.pins.borrow_mut().remove(name);
            }
        }
        if let Some(row) = self.input_rows.borrow().iter().find(|r| r.name == name) {
            row.check.set_sensitive(label.is_none());
            if label.is_some() {
                row.check.set_active(false);
            }
            row.tag.set_visible(label.is_some());
            row.tag.set_label(label.as_deref().unwrap_or_default());
        }
        self.update_buttons();
    }

    fn selected_inputs(&self) -> Vec<String> {
        self.input_rows
            .borrow()
            .iter()
            .filter(|r| r.check.is_active())
            .map(|r| r.name.clone())
            .collect()
    }

    fn has_work(&self) -> bool {
        !self.selected_inputs().is_empty() || !self.pins.borrow().is_empty()
    }

    // ── 2. Construir ──────────────────────────────────────────────────────

    fn start_update(self: &Rc<Self>) {
        if self.busy.get() || !self.has_work() {
            return;
        }
        self.busy.set(true);
        self.log_lines.borrow_mut().clear();
        self.log.set_label("");
        self.failure.set_visible(false);
        for row in [&self.lock_row, &self.eval_row, &self.build_row] {
            set_state(row, "pending");
        }
        self.progress.set_fraction(0.0);
        self.activity
            .set_label(&tr("Atualizando o lock dos inputs"));
        self.set_step(Step::Building);

        let inputs = self.selected_inputs();
        let pins: Vec<Pin> = self.pins.borrow().values().cloned().collect();
        let env = self.ctx.env.clone();
        let workspace = self.workspace.clone();
        let cancel = CancelToken::new();
        *self.cancel.borrow_mut() = Some(Arc::clone(&cancel));
        let started = std::time::Instant::now();
        let current_system = std::fs::canonicalize(&env.current_system)
            .unwrap_or_else(|_| env.current_system.clone());
        let previous_generation = self
            .status
            .borrow()
            .as_ref()
            .and_then(|s| s.current_generation().map(|g| g.number));

        let w = Rc::clone(self);
        let current_for_ui = current_system.clone();
        *self.pending_summary.borrow_mut() = None;
        stream_in_background(
            move |tx| {
                let send = |event: Event| {
                    let _ = tx.send_blocking(event);
                };
                send(Event::Locking);
                let prepared = match prepare_lock(&nix(), &env.flake, &workspace, &inputs, &pins) {
                    Ok(prepared) => prepared,
                    Err(error) => return send(Event::Done(Err(error.to_string()))),
                };
                send(Event::Locked(prepared.clone()));
                send(Event::Evaluating);
                let request = BuildRequest {
                    flake: env.flake.clone(),
                    host: env.host.clone(),
                    out_link: workspace.out_link(),
                    reference_lock: Some(workspace.new_lock()),
                };
                let outcome = run_build(
                    &SystemRunner,
                    &NixBuild,
                    &request,
                    &cancel,
                    &mut |progress| {
                        send(Event::Progress(progress.clone()));
                    },
                );
                match outcome {
                    Ok(BuildOutcome::Built { out_path }) => {
                        let summary =
                            summarize_update(&nix(), &current_system, &out_path, &prepared.changes)
                                .map_err(|e| e.to_string());
                        send(Event::Summary(Box::new(summary)));
                        send(Event::Done(Ok(BuildOutcome::Built { out_path })));
                    }
                    Ok(other) => send(Event::Done(Ok(other))),
                    Err(error) => send(Event::Done(Err(error.to_string()))),
                }
            },
            move |event| w.on_build_event(event, started, &current_for_ui, previous_generation),
        );
    }

    fn on_build_event(
        self: &Rc<Self>,
        event: Event,
        started: std::time::Instant,
        current: &std::path::Path,
        previous_generation: Option<u64>,
    ) {
        match event {
            Event::Locking => {
                set_state(&self.lock_row, "running");
                self.progress.pulse();
            }
            Event::Locked(prepared) => {
                set_state(&self.lock_row, "done");
                *self.changes.borrow_mut() = prepared.changes;
            }
            Event::Evaluating => {
                set_state(&self.eval_row, "running");
                self.activity.set_label(&tr("Avaliando a configuração"));
            }
            Event::Progress(progress) => {
                if progress.phase == BuildPhase::Building {
                    set_state(&self.eval_row, "done");
                    set_state(&self.build_row, "running");
                    self.activity.set_label(&format!(
                        "{} {}/{}",
                        tr("Construindo e baixando:"),
                        progress.builds.done,
                        progress.builds.expected
                    ));
                }
                match progress.fraction() {
                    Some(fraction) => self.progress.set_fraction(fraction),
                    None => self.progress.pulse(),
                }
                if let Some(line) = progress.last_log {
                    self.push_log(&line);
                }
            }
            Event::Summary(summary) => match *summary {
                Ok(summary) => *self.pending_summary.borrow_mut() = Some(summary),
                Err(error) => self.ctx.toast(&format!(
                    "{} {error}",
                    tr("Não foi possível resumir as mudanças:")
                )),
            },
            Event::Done(result) => {
                self.on_build_done(result, started, current, previous_generation)
            }
        }
    }

    fn on_build_done(
        self: &Rc<Self>,
        result: Result<BuildOutcome, String>,
        started: std::time::Instant,
        current: &std::path::Path,
        previous_generation: Option<u64>,
    ) {
        self.busy.set(false);
        *self.cancel.borrow_mut() = None;
        match result {
            Ok(BuildOutcome::Built { out_path }) => {
                let Some(summary) = self.pending_summary.borrow_mut().take() else {
                    return self.show_failure(
                        &tr("O build terminou, mas o resumo das mudanças não ficou pronto."),
                        "",
                    );
                };
                for row in [&self.lock_row, &self.eval_row, &self.build_row] {
                    set_state(row, "done");
                }
                self.progress.set_fraction(1.0);
                let built = Rc::new(Built {
                    out_path,
                    current_system: current.to_owned(),
                    summary,
                    duration: format_duration(started.elapsed().as_secs()),
                    previous_generation,
                });
                self.fill_review(&built);
                *self.built.borrow_mut() = Some(built);
                self.set_step(Step::Review);
            }
            Ok(BuildOutcome::Cancelled) => {
                let _ = self.workspace.discard();
                self.set_step(Step::Choose);
                self.ctx.toast(&tr("Atualização cancelada. Nada mudou."));
            }
            Ok(BuildOutcome::Failed(failure)) => {
                self.show_failure(&failure.headline, &failure.details)
            }
            Err(error) => self.show_failure(&tr("Não foi possível preparar a atualização"), &error),
        }
    }

    fn show_failure(&self, headline: &str, details: &str) {
        for row in [&self.lock_row, &self.eval_row, &self.build_row] {
            if row.stack.visible_child_name().as_deref() == Some("running") {
                set_state(row, "failed");
            }
        }
        self.progress.set_fraction(0.0);
        self.activity.set_label(headline);
        self.failure.set_label(details);
        self.failure.set_visible(!details.is_empty());
        // O estado temporário já pode sair; o flake.lock nunca foi tocado.
        let _ = self.workspace.discard();
        self.set_step(Step::Failed);
    }

    fn push_log(&self, line: &str) {
        let mut lines = self.log_lines.borrow_mut();
        lines.push_back(line.to_owned());
        while lines.len() > MAX_LOG_LINES {
            lines.pop_front();
        }
        self.log
            .set_label(&lines.iter().cloned().collect::<Vec<_>>().join("\n"));
    }

    // ── 3. Revisar ────────────────────────────────────────────────────────

    fn fill_review(&self, built: &Built) {
        let totals = &built.summary.totals;
        let title = if totals.updated + totals.added + totals.removed == 0 {
            tr("Nenhuma mudança nos pacotes")
        } else {
            format!(
                "{} atualizados, {} e {}",
                totals.updated,
                count(totals.added as u64, "novo", "novos"),
                count(totals.removed as u64, "removido", "removidos")
            )
        };
        self.summary_row.set_title(&title);
        let mut parts = Vec::new();
        if !built.summary.changed_inputs.is_empty() {
            parts.push(built.summary.changed_inputs.join(", "));
        }
        if let Some(delta) = built.summary.size_delta {
            let amount = format_bytes(delta.unsigned_abs());
            parts.push(match delta {
                0 => tr("o tamanho do sistema não muda"),
                d if d > 0 => format!("o sistema vai ocupar {amount} a mais"),
                _ => format!("o sistema vai ocupar {amount} a menos"),
            });
        }
        self.summary_row.set_subtitle(&parts.join(" · "));
        self.review_changes.set_sensitive(true);

        let kernel_changed = built.summary.kernel_changed == Some(true);
        self.boot_hint.set_label(&if kernel_changed {
            tr("Recomendado agora: o kernel mudou e só passa a valer depois de reiniciar.")
        } else {
            tr("O sistema passa a usar a atualização ao reiniciar. Nada muda agora.")
        });
        self.test_hint.set_label(&match built.previous_generation {
            Some(n) => format!(
                "{} {n}.",
                tr("Para testar. Ao reiniciar, o sistema volta para a geração")
            ),
            None => tr("Para testar. Ao reiniciar, o sistema volta para a geração atual."),
        });
        if kernel_changed {
            self.mode_boot.set_active(true);
        } else {
            self.mode_switch.set_active(true);
        }
    }

    fn show_changes(&self) {
        let Some(built) = self.built.borrow().clone() else {
            return;
        };
        diff::present(
            self.ctx.window().as_ref(),
            tr("Mudanças da atualização"),
            built.current_system.to_string_lossy().into_owned(),
            built.out_path.to_string_lossy().into_owned(),
        );
    }

    fn chosen_mode(&self) -> ApplyMode {
        if self.mode_test.is_active() {
            ApplyMode::Test
        } else if self.mode_switch.is_active() {
            ApplyMode::Switch
        } else {
            ApplyMode::Boot
        }
    }

    // ── Aplicar ───────────────────────────────────────────────────────────

    fn apply(self: &Rc<Self>) {
        let Some(built) = self.built.borrow().clone() else {
            return;
        };
        if self.helper_issue.is_some() || self.busy.get() {
            return;
        }
        self.busy.set(true);
        self.update_buttons();
        let mode = self.chosen_mode();
        // Em "só até reiniciar" o sistema volta ao reiniciar: o lock não deve mudar.
        let writes_lock = mode != ApplyMode::Test;
        let bus_ctx = Rc::clone(&self.ctx);
        bus_ctx.apply_events.publish(&ApplyEvent::Started {
            mode,
            at: glib::DateTime::now_local()
                .ok()
                .and_then(|d| d.format("%H:%M").ok())
                .map(|s| s.to_string())
                .unwrap_or_default(),
            build_duration: Some(built.duration.clone()),
            previous_generation: built.previous_generation,
            writes_lock,
        });
        self.ctx.go_to(Page::Aplicando);

        let (flake, workspace, out_path) = (
            self.ctx.env.flake.clone(),
            self.workspace.clone(),
            built.out_path.clone(),
        );
        let ctx = Rc::clone(&self.ctx);
        let w = Rc::clone(self);
        stream_in_background(
            move |tx| {
                let send = |event: ApplyEvent| {
                    let _ = tx.send_blocking(event);
                };
                let helper = HelperClient::production(SystemRunner);
                let result = helper.activate(&out_path, mode, &mut |event| match event {
                    ActivationEvent::Step(marker) => send(ApplyEvent::Step {
                        step: marker.step,
                        of: marker.of,
                    }),
                    ActivationEvent::Stage(stage) => send(ApplyEvent::Stage(stage)),
                    ActivationEvent::Log(line) => send(ApplyEvent::Log(line)),
                });
                match result {
                    Ok(()) => {
                        if writes_lock {
                            match commit_lock(&flake, &workspace) {
                                Ok(()) => send(ApplyEvent::LockWritten),
                                Err(error) => send(ApplyEvent::Log(format!(
                                    "aviso: não foi possível gravar o flake.lock: {error}"
                                ))),
                            }
                            let _ = workspace.discard();
                        }
                        send(ApplyEvent::Finished(Ok(())));
                    }
                    Err(error) => send(ApplyEvent::Finished(Err(error.to_string()))),
                }
            },
            move |event| {
                let finished_err = matches!(event, ApplyEvent::Finished(Err(_)));
                ctx.apply_events.publish(&event);
                if matches!(event, ApplyEvent::Finished(_)) {
                    w.busy.set(false);
                    w.update_buttons();
                }
                let _ = finished_err;
            },
        );
    }

    // ── Verificação visual (ver `crate::dev_action`) ─────────────────────

    /// Fixa nixpkgs e home-manager nas revisões atuais e segue: o build sai do cache e a revisão
    /// aparece com dados reais, sem alterar nada.
    fn dev_demo(self: &Rc<Self>) {
        let inputs = self.inputs.borrow().clone();
        for name in ["nixpkgs", "home-manager"] {
            let flake_ref = inputs
                .iter()
                .find(|i| i.name == name)
                .and_then(|input| pin_reference(input, input.rev.as_deref()?));
            if let Some(flake_ref) = flake_ref {
                let pin = Pin {
                    input: name.to_owned(),
                    flake_ref,
                };
                self.set_pin(name, Some(pin));
            }
        }
        for row in self.input_rows.borrow().iter() {
            row.check.set_active(false);
        }
        self.start_update();
    }
}

/// Os 7 primeiros caracteres da revisão de uma referência de fixação
/// (`github:o/r/<rev>` ou `git+url?rev=<rev>`).
fn short_rev_of(flake_ref: &str) -> String {
    let rev = flake_ref.rsplit_once("rev=").map_or_else(
        || flake_ref.rsplit('/').next().unwrap_or(""),
        |(_, rev)| rev,
    );
    rev.chars().take(7).collect()
}

fn format_duration(seconds: u64) -> String {
    if seconds >= 60 {
        format!("{} min {} s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisao_curta_da_referencia() {
        let rev = "02c087f3fe673c9e317f206a627cb6b910336dbe";
        assert_eq!(
            short_rev_of(&format!("github:NixOS/nixpkgs/{rev}")),
            "02c087f"
        );
        assert_eq!(
            short_rev_of(&format!("git+https://x.org/r.git?rev={rev}")),
            "02c087f"
        );
        assert_eq!(short_rev_of(""), "");
    }

    #[test]
    fn duracao_legivel() {
        assert_eq!(format_duration(0), "0 s");
        assert_eq!(format_duration(59), "59 s");
        assert_eq!(format_duration(372), "6 min 12 s");
    }

    #[test]
    fn etapas_numeradas_como_no_mockup() {
        assert_eq!(
            [Step::Choose, Step::Building, Step::Review].map(Step::number),
            [1, 2, 3]
        );
    }
}
