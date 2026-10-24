//! "Aplicando": acompanha a ativação, com etapas, progresso e o log do helper.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use zelador_core::apply::{ActivationStage, ApplyMode};

use super::Screen;
use crate::ctx::{ApplyEvent, Ctx};
use crate::i18n::tr;
use crate::pages::Page;
use crate::widgets::bottom_bar;

const MAX_LOG_LINES: usize = 400;

/// Estado de cada linha da lista de etapas.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Pending,
    Running,
    Done,
    Failed,
}

struct StepRow {
    stack: gtk::Stack,
    label: gtk::Label,
    detail: gtk::Label,
}

struct Rows {
    build: StepRow,
    register: StepRow,
    activate: StepRow,
    lock: StepRow,
}

struct State {
    mode: ApplyMode,
    writes_lock: bool,
    log: Vec<String>,
    finished: Option<Result<(), String>>,
}

struct Ui {
    ctx: Rc<Ctx>,
    intro: gtk::Label,
    stage: gtk::Label,
    counter: gtk::Label,
    progress: gtk::ProgressBar,
    rows: Rows,
    log: gtk::Label,
    started: gtk::Label,
    finish: gtk::Button,
    state: RefCell<State>,
}

pub fn build(
    ctx: &Rc<Ctx>,
    intro: &gtk::Label,
    step_label: &gtk::Label,
    view: &adw::ToolbarView,
) -> Screen {
    let _ = step_label;
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(28)
        .margin_top(10)
        .build();

    let stage = gtk::Label::builder()
        .label(tr("Nenhuma atualização em andamento."))
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["option-title"])
        .build();
    let counter = gtk::Label::builder().css_classes(["caption-text"]).build();
    let head = gtk::Box::builder().spacing(12).build();
    head.append(&stage);
    head.append(&counter);
    let progress = gtk::ProgressBar::builder().css_classes(["thin"]).build();
    let top = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    top.append(&head);
    top.append(&progress);
    root.append(&top);

    let list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    let rows = Rows {
        build: step_row(&list, &tr("Build concluído")),
        register: step_row(&list, &tr("Registrar a geração")),
        activate: step_row(&list, &tr("Ativar a configuração")),
        lock: step_row(&list, &tr("Gravar o novo flake.lock")),
    };
    root.append(&list);

    let log = gtk::Label::builder()
        .xalign(0.0)
        .yalign(0.0)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::Char)
        .css_classes(["mono", "log-view"])
        .build();
    let expander = gtk::Expander::builder()
        .label(tr("Detalhes"))
        .child(&log)
        .css_classes(["details-card"])
        .build();
    root.append(&expander);

    // Barra inferior: horário de início e "Concluir", habilitado só no fim.
    let started = gtk::Label::builder()
        .label(tr("Iniciado às --:--"))
        .css_classes(["caption-text"])
        .build();
    let finish = gtk::Button::builder()
        .label(tr("Concluir"))
        .sensitive(false)
        .build();
    let bar = bottom_bar::shell();
    bar.append(&started);
    bar.append(&bottom_bar::spacer());
    bar.append(&finish);
    view.add_bottom_bar(&bar);

    let ui = Rc::new(Ui {
        ctx: Rc::clone(ctx),
        intro: intro.clone(),
        stage,
        counter,
        progress,
        rows,
        log,
        started,
        finish: finish.clone(),
        state: RefCell::new(State {
            mode: ApplyMode::Boot,
            writes_lock: true,
            log: Vec::new(),
            finished: None,
        }),
    });

    {
        let ui = Rc::clone(&ui);
        ctx.apply_events.subscribe(move |event| ui.handle(event));
    }
    {
        let ui = Rc::clone(&ui);
        finish.connect_clicked(move |_| ui.conclude());
    }

    Screen {
        body: root.upcast(),
        header_end: None,
    }
}

fn step_row(container: &gtk::Box, text: &str) -> StepRow {
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
    let texts = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    let detail = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["caption-text"])
        .visible(false)
        .build();
    texts.append(&label);
    texts.append(&detail);
    row.append(&stack);
    row.append(&texts);
    container.append(&row);
    StepRow {
        stack,
        label,
        detail,
    }
}

fn mark(row: &StepRow, mark: Mark) {
    row.stack.set_visible_child_name(match mark {
        Mark::Pending => "pending",
        Mark::Running => "running",
        Mark::Done => "done",
        Mark::Failed => "failed",
    });
    row.label.set_sensitive(mark != Mark::Pending);
}

fn stage_text(stage: ActivationStage) -> String {
    match stage {
        ActivationStage::ActivatingConfiguration => tr("Ativando a configuração"),
        ActivationStage::StoppingUnits => tr("Parando serviços"),
        ActivationStage::StartingUnits => tr("Iniciando serviços"),
        ActivationStage::RestartingUnits => tr("Reiniciando serviços"),
        ActivationStage::UpdatingBootMenu => tr("Atualizando o menu de boot"),
        ActivationStage::InstallingBootLoader => tr("Instalando o carregador de boot"),
    }
}

impl Ui {
    fn handle(&self, event: &ApplyEvent) {
        match event {
            ApplyEvent::Started {
                mode,
                at,
                build_duration,
                previous_generation,
                writes_lock,
            } => self.started(
                *mode,
                at,
                build_duration.as_deref(),
                *previous_generation,
                *writes_lock,
            ),
            ApplyEvent::Step { step, of } => self.step(*step, *of),
            ApplyEvent::Stage(stage) => {
                let text = stage_text(*stage);
                self.stage.set_label(&text);
                self.rows.activate.detail.set_label(&text);
                self.rows.activate.detail.set_visible(true);
            }
            ApplyEvent::Log(line) => self.push_log(line),
            ApplyEvent::LockWritten => mark(&self.rows.lock, Mark::Done),
            ApplyEvent::Finished(result) => self.finished(result),
        }
    }

    fn started(
        &self,
        mode: ApplyMode,
        at: &str,
        build_duration: Option<&str>,
        previous_generation: Option<u64>,
        writes_lock: bool,
    ) {
        *self.state.borrow_mut() = State {
            mode,
            writes_lock,
            log: Vec::new(),
            finished: None,
        };
        self.log.set_label("");
        self.started
            .set_label(&format!("{} {at}", tr("Iniciado às")));
        self.finish.set_sensitive(false);
        self.progress.set_fraction(0.0);
        self.counter.set_label("");
        self.stage.set_label(&tr("Preparando a ativação"));
        self.intro.set_label(&tr(
            "Você pode continuar usando o computador. Não desligue até terminar.",
        ));

        mark(&self.rows.build, Mark::Done);
        let build_text = match build_duration {
            Some(duration) => format!("{} · {duration}", tr("Build concluído")),
            None => tr("Build concluído"),
        };
        self.rows.build.label.set_label(&build_text);
        // "Só até reiniciar" não registra geração e não mexe no flake.lock.
        let registers = mode != ApplyMode::Test;
        self.rows.register.stack.set_visible(registers);
        self.rows.register.label.set_visible(registers);
        self.rows.register.detail.set_visible(false);
        self.rows.lock.stack.set_visible(writes_lock);
        self.rows.lock.label.set_visible(writes_lock);
        mark(&self.rows.register, Mark::Pending);
        mark(&self.rows.activate, Mark::Pending);
        mark(&self.rows.lock, Mark::Pending);
        self.rows.activate.detail.set_visible(false);
        self.rows.activate.label.set_label(&match mode {
            ApplyMode::Boot => tr("Preparar o próximo boot"),
            ApplyMode::Switch | ApplyMode::Test => tr("Ativar a configuração"),
        });
        let register_text = match previous_generation {
            Some(n) => format!("{} (a atual é a {n})", tr("Registrar a geração")),
            None => tr("Registrar a geração"),
        };
        self.rows.register.label.set_label(&register_text);
    }

    fn step(&self, step: u32, of: u32) {
        let mode = self.state.borrow().mode;
        let registers = mode != ApplyMode::Test;
        let activating = if registers { step >= 2 } else { step >= 1 };
        if registers {
            mark(
                &self.rows.register,
                if activating {
                    Mark::Done
                } else {
                    Mark::Running
                },
            );
        }
        if activating {
            mark(&self.rows.activate, Mark::Running);
        }
        self.stage.set_label(&if activating {
            match mode {
                ApplyMode::Boot => tr("Preparando o próximo boot"),
                ApplyMode::Switch | ApplyMode::Test => tr("Ativando a configuração"),
            }
        } else {
            tr("Registrando a geração")
        });
        self.counter
            .set_label(&format!("{} {step} {} {of}", tr("etapa"), tr("de")));
        self.progress.set_fraction(
            f64::from(step.saturating_sub(1)) / f64::from(of.max(1)) + 0.25 / f64::from(of.max(1)),
        );
    }

    fn push_log(&self, line: &str) {
        let mut state = self.state.borrow_mut();
        state.log.push(line.to_owned());
        if state.log.len() > MAX_LOG_LINES {
            state.log.remove(0);
        }
        self.log.set_label(&state.log.join("\n"));
    }

    fn finished(&self, result: &Result<(), String>) {
        let mode = self.state.borrow().mode;
        self.state.borrow_mut().finished = Some(result.clone());
        self.finish.set_sensitive(true);
        match result {
            Ok(()) => {
                for row in [&self.rows.register, &self.rows.activate] {
                    if row.stack.is_visible() {
                        mark(row, Mark::Done);
                    }
                }
                if self.state.borrow().writes_lock {
                    mark(&self.rows.lock, Mark::Done);
                }
                self.progress.set_fraction(1.0);
                self.counter.set_label("");
                self.stage.set_label(&tr("Concluído"));
                self.intro.set_label(&match mode {
                    ApplyMode::Boot => {
                        tr("Pronto. O sistema inicia na nova geração no próximo reinício.")
                    }
                    ApplyMode::Switch => tr("Pronto. A atualização está em uso."),
                    ApplyMode::Test => tr("Pronto. A atualização está em uso até você reiniciar."),
                });
            }
            Err(error) => {
                for row in [&self.rows.register, &self.rows.activate] {
                    if row.stack.visible_child_name().as_deref() == Some("running") {
                        mark(row, Mark::Failed);
                    }
                }
                self.stage.set_label(&tr("A ativação falhou"));
                self.intro
                    .set_label(&format!("{} {error}", tr("Nada foi concluído:")));
            }
        }
    }

    /// "Concluir": com sucesso volta ao início; com falha volta à revisão, que continua pronta.
    fn conclude(&self) {
        let ok = matches!(self.state.borrow().finished, Some(Ok(())));
        if ok {
            let ctx = Rc::clone(&self.ctx);
            self.ctx.overview.refresh(|_| {});
            ctx.go_to(Page::SeuSistema);
        } else {
            self.ctx.go_to(Page::Atualizar);
        }
    }
}
