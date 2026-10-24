//! "Liberar espaço": escolher o que remover, simular e limpar.
//!
//! As estimativas saem das closures (sem tocar em nada). "Simular" consulta o GC de verdade
//! (`nix store gc --dry-run`) e "Limpar" executa os passos, pedindo senha de administrador por
//! passo privilegiado. A execução só existe com o helper instalado.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use zelador_core::backends::CliFlatpak;
use zelador_core::cleanup::{
    Cleaner, CleanupPlan, ClosureIndex, DEFAULT_KEEP_DAYS, DEFAULT_KEEP_HOME_MANAGER,
    DEFAULT_KEEP_SYSTEM, Estimate, HelperClient, HelperIssue, Policy, Selection, Step, StepKind,
    build_plan, closure_index, disk_ratio, dry_run, estimate, link_sizes,
};
use zelador_core::disk::{free_bytes, used_bytes};
use zelador_core::history::{NewOperation, OperationKind, Outcome};
use zelador_core::runner::SystemRunner;
use zelador_core::status::SystemStatus;
use zelador_core::units::format_bytes;

use super::Screen;
use crate::ctx::{Ctx, elapsed_ms, epoch_now, nix};
use crate::format::relative_age;
use crate::i18n::{tr, trf};
use crate::loader::run_in_background;

type Sizes = Rc<Result<HashMap<String, u64>, String>>;
type Dead = Rc<Result<HashSet<String>, String>>;

/// Tudo de que a estimativa precisa, já carregado.
struct Data {
    status: Rc<SystemStatus>,
    index: ClosureIndex,
    sizes: Sizes,
    dead: Dead,
    link_sizes: HashMap<PathBuf, u64>,
    /// Disco ocupado sobre tamanho lógico do store (ver `disk_ratio`).
    ratio: f64,
}

struct Ui {
    ctx: Rc<Ctx>,
    helper_issue: Option<HelperIssue>,
    data: RefCell<Option<Rc<Data>>>,
    policy: Cell<Policy>,
    generations_on: Cell<bool>,
    links_on: RefCell<HashSet<PathBuf>>,
    flatpak_on: Cell<bool>,
    optimise_on: Cell<bool>,
    // Widgets atualizados a cada recálculo.
    generations_card: gtk::Box,
    generations_text: gtk::Label,
    generations_size: gtk::Label,
    links_card: gtk::Box,
    links_check: gtk::CheckButton,
    links_size: gtk::Label,
    links_list: gtk::Box,
    optimise_card: gtk::Box,
    flatpak_card: gtk::Box,
    flatpak_text: gtk::Label,
    flatpak_size: gtk::Label,
    unreferenced: gtk::Label,
    total: gtk::Label,
    simulate: gtk::Button,
    clean: gtk::Button,
}

pub fn build(ctx: &Rc<Ctx>, intro: &gtk::Label, view: &adw::ToolbarView) -> Screen {
    let _ = intro;
    let helper_issue = HelperClient::production(SystemRunner).check().err();

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .margin_top(14)
        .build();

    if let Some(issue) = helper_issue {
        root.append(&helper_banner(issue));
    }

    // Cartão 1: gerações antigas.
    let policy = Policy::default();
    let generations_text = caption("");
    let generations_size = size_label();
    let generations_card = card(true);
    let generations_check = gtk::CheckButton::builder()
        .active(true)
        .valign(gtk::Align::Start)
        .build();
    let spin = |value: usize, max: f64| {
        gtk::SpinButton::builder()
            .adjustment(&gtk::Adjustment::new(value as f64, 0.0, max, 1.0, 5.0, 0.0))
            .width_chars(3)
            .css_classes(["num"])
            .valign(gtk::Align::Center)
            .build()
    };
    let keep_system = spin(DEFAULT_KEEP_SYSTEM, 999.0);
    let keep_days = spin(DEFAULT_KEEP_DAYS as usize, 3650.0);
    let keep_hm = spin(DEFAULT_KEEP_HOME_MANAGER, 999.0);
    let gen_body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .build();
    gen_body.append(&title_label(&tr("Gerações antigas")));
    gen_body.append(&generations_text);
    let fields = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .build();
    fields.append(&field_row(
        &tr("Manter as últimas"),
        &keep_system,
        &tr("gerações do sistema"),
    ));
    fields.append(&field_row(
        &tr("e todas as dos últimos"),
        &keep_days,
        &tr("dias"),
    ));
    fields.append(&field_row(
        &tr("Manter as últimas"),
        &keep_hm,
        &tr("do Home Manager"),
    ));
    gen_body.append(&fields);
    generations_card.append(&generations_check);
    generations_card.append(&gen_body);
    generations_card.append(&generations_size);
    root.append(&generations_card);

    // Cartão 2: links esquecidos.
    let links_card = card(true);
    let links_check = gtk::CheckButton::builder()
        .active(true)
        .valign(gtk::Align::Start)
        .build();
    let links_size = size_label();
    let links_list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_top(12)
        .build();
    let links_body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .build();
    links_body.append(&title_label(&tr("Links de build esquecidos")));
    links_body.append(&caption(&tr(
        "Cada um impede que o build correspondente seja apagado.",
    )));
    links_body.append(&links_list);
    links_card.append(&links_check);
    links_card.append(&links_body);
    links_card.append(&links_size);
    root.append(&links_card);

    // Cartão 3: deduplicar.
    let optimise_card = card(true);
    let optimise_check = gtk::CheckButton::builder()
        .active(true)
        .valign(gtk::Align::Start)
        .build();
    let optimise_body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .build();
    optimise_body.append(&title_label(&tr("Deduplicar arquivos do store")));
    optimise_body.append(&caption(&tr(
        "Troca arquivos idênticos por um só. Pode levar alguns minutos.",
    )));
    optimise_body.append(&caption(&tr(
        "Não há como estimar a economia antes de rodar.",
    )));
    optimise_card.append(&optimise_check);
    optimise_card.append(&optimise_body);
    root.append(&optimise_card);

    // Fora do Nix store.
    root.append(
        &gtk::Label::builder()
            .label(tr("Fora do Nix store"))
            .css_classes(["section-title"])
            .xalign(0.0)
            .margin_top(18)
            .build(),
    );
    let flatpak_card = card(false);
    let flatpak_check = gtk::CheckButton::builder()
        .valign(gtk::Align::Start)
        .build();
    let flatpak_text = caption(&tr("Lendo o Flatpak…"));
    let flatpak_size = size_label();
    let flatpak_body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .build();
    flatpak_body.append(&title_label(&tr("Runtimes Flatpak sem uso")));
    flatpak_body.append(&flatpak_text);
    flatpak_card.append(&flatpak_check);
    flatpak_card.append(&flatpak_body);
    flatpak_card.append(&flatpak_size);
    root.append(&flatpak_card);

    let unreferenced = caption("");
    unreferenced.set_margin_top(6);
    root.append(&unreferenced);

    // Barra inferior.
    let total = gtk::Label::builder().css_classes(["strong"]).build();
    let total_hint = gtk::Label::builder()
        .label(tr("podem ser liberados"))
        .css_classes(["caption-text"])
        .build();
    let simulate = gtk::Button::builder()
        .label(tr("Simular"))
        .sensitive(false)
        .build();
    let clean = gtk::Button::builder()
        .label(tr("Limpar"))
        .css_classes(["destructive-action"])
        .sensitive(false)
        .build();
    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["zelador-bottom-bar"])
        .build();
    bar.append(&total);
    bar.append(&total_hint);
    bar.append(&gtk::Box::builder().hexpand(true).build());
    bar.append(&simulate);
    bar.append(&clean);
    view.add_bottom_bar(&bar);

    let ui = Rc::new(Ui {
        ctx: Rc::clone(ctx),
        helper_issue,
        data: RefCell::new(None),
        policy: Cell::new(policy),
        generations_on: Cell::new(true),
        links_on: RefCell::new(HashSet::new()),
        flatpak_on: Cell::new(false),
        optimise_on: Cell::new(true),
        generations_card,
        generations_text,
        generations_size,
        links_card,
        links_check: links_check.clone(),
        links_size,
        links_list,
        optimise_card,
        flatpak_card,
        flatpak_text,
        flatpak_size,
        unreferenced,
        total,
        simulate,
        clean,
    });
    ui.refresh_total_placeholder();

    // Marcações e números.
    connect_check(&generations_check, &ui, |ui, on| ui.generations_on.set(on));
    connect_check(&links_check, &ui, |ui, on| {
        if on {
            if let Some(data) = ui.data.borrow().as_ref() {
                *ui.links_on.borrow_mut() = data
                    .status
                    .forgotten_roots
                    .iter()
                    .map(|r| r.path.clone())
                    .collect();
            }
        } else {
            ui.links_on.borrow_mut().clear();
        }
    });
    connect_check(&optimise_check, &ui, |ui, on| ui.optimise_on.set(on));
    connect_check(&flatpak_check, &ui, |ui, on| ui.flatpak_on.set(on));
    for (spin_button, set) in [(&keep_system, 0usize), (&keep_days, 1), (&keep_hm, 2)] {
        let ui = Rc::clone(&ui);
        spin_button.connect_value_changed(move |spin| {
            let value = spin.value_as_int().max(0);
            let mut policy = ui.policy.get();
            match set {
                0 => policy.keep_system = usize::try_from(value).unwrap_or(0),
                1 => policy.keep_days = u32::try_from(value).unwrap_or(0),
                _ => policy.keep_home_manager = usize::try_from(value).unwrap_or(0),
            }
            ui.policy.set(policy);
            ui.recompute();
        });
    }

    ui.simulate.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| simulate_clicked(&ui)
    ));
    ui.clean.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| confirm_clean(&ui)
    ));

    load(&ui);

    Screen {
        body: root.upcast(),
        header_end: None,
    }
}

fn connect_check(check: &gtk::CheckButton, ui: &Rc<Ui>, apply: impl Fn(&Ui, bool) + 'static) {
    let ui = Rc::clone(ui);
    check.connect_toggled(move |check| {
        apply(&ui, check.is_active());
        ui.recompute();
    });
}

fn card(on: bool) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .css_classes(["option-card"])
        .build();
    if on {
        card.add_css_class("on");
    }
    card
}

fn title_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["option-title"])
        .build()
}

fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption-text"])
        .build()
}

fn size_label() -> gtk::Label {
    gtk::Label::builder()
        .css_classes(["size"])
        .valign(gtk::Align::Start)
        .build()
}

/// `Manter as últimas [ 5 ] gerações do sistema`.
fn field_row(before: &str, spin: &gtk::SpinButton, after: &str) -> gtk::Box {
    let row = gtk::Box::builder().spacing(12).build();
    row.append(&gtk::Label::new(Some(before)));
    row.append(spin);
    row.append(&gtk::Label::new(Some(after)));
    row
}

fn helper_banner(issue: HelperIssue) -> gtk::Box {
    let (title, hint) = match issue {
        HelperIssue::HelperMissing | HelperIssue::PolicyMissing => (
            tr("O helper privilegiado não está instalado neste sistema."),
            tr(
                "Ative programs.zelador.enable no módulo NixOS para poder limpar. Simular continua disponível.",
            ),
        ),
        HelperIssue::SocketMissing => (
            tr("O serviço do helper não está ativo neste sistema."),
            tr(
                "Ative programs.zelador.enable (ele liga o zelador-helper.socket no systemd). Simular continua disponível.",
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

/// Carrega o estado rápido, as closures de cada raiz, os tamanhos e o lixo.
fn load(ui: &Rc<Ui>) {
    ui.ctx.dead.get(|_| {});
    ui.ctx.sizes.get(|_| {});
    let ui = Rc::clone(ui);
    let ctx = Rc::clone(&ui.ctx);
    ctx.overview.get(move |status| {
        let (env, for_index) = (ui.ctx.env.clone(), (*status).clone());
        run_in_background(
            move || closure_index(&nix(), &env, &for_index).map_err(|e| e.to_string()),
            move |index| {
                let ui2 = Rc::clone(&ui);
                ui.ctx.sizes.get(move |sizes| {
                    let ui3 = Rc::clone(&ui2);
                    ui2.ctx.dead.get(move |dead| {
                        ui3.loaded(&status, index, &sizes, &dead);
                    });
                });
            },
        );
    });
}

impl Ui {
    fn refresh_total_placeholder(&self) {
        self.total.set_label(&tr("Calculando…"));
    }

    fn loaded(
        self: &Rc<Self>,
        status: &Rc<SystemStatus>,
        index: Result<ClosureIndex, String>,
        sizes: &Sizes,
        dead: &Dead,
    ) {
        let (Ok(index), Ok(size_map), Ok(_)) = (index, &**sizes, &**dead) else {
            self.total
                .set_label(&tr("Não foi possível calcular as estimativas"));
            return;
        };
        let per_link = link_sizes(&index, size_map);
        let ratio = disk_ratio(
            size_map,
            used_bytes(std::path::Path::new("/nix")).unwrap_or(u64::MAX),
        );
        *self.data.borrow_mut() = Some(Rc::new(Data {
            status: Rc::clone(status),
            index,
            sizes: Rc::clone(sizes),
            dead: Rc::clone(dead),
            link_sizes: per_link,
            ratio,
        }));
        self.populate_links(status);
        self.populate_flatpak(status);
        self.simulate.set_sensitive(true);
        self.recompute();
        // Só para verificação visual (ver `crate::dev_action`).
        match crate::dev_action().as_deref() {
            Some("simulate") => simulate_clicked(self),
            Some("confirm-clean") => confirm_clean(self),
            _ => {}
        }
    }

    fn populate_links(self: &Rc<Self>, status: &SystemStatus) {
        let Some(data) = self.data.borrow().clone() else {
            return;
        };
        while let Some(child) = self.links_list.first_child() {
            self.links_list.remove(&child);
        }
        if status.forgotten_roots.is_empty() {
            self.links_list
                .append(&caption(&tr("Nenhum link esquecido foi encontrado.")));
            self.links_card.remove_css_class("on");
            self.links_check.set_active(false);
            self.links_check.set_sensitive(false);
            return;
        }
        let mut selected = self.links_on.borrow_mut();
        for root in &status.forgotten_roots {
            if root.selected_by_default {
                selected.insert(root.path.clone());
            }
            let check = gtk::CheckButton::builder()
                .active(root.selected_by_default)
                .build();
            let row = gtk::Box::builder().spacing(12).build();
            row.append(&check);
            row.append(
                &gtk::Label::builder()
                    .label(root.path.to_string_lossy())
                    .xalign(0.0)
                    .hexpand(true)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::Char)
                    .css_classes(["mono"])
                    .build(),
            );
            let bytes = data.link_sizes.get(&root.path).copied().unwrap_or(0);
            row.append(
                &gtk::Label::builder()
                    .label(format!(
                        "{} · {}",
                        format_bytes(bytes),
                        relative_age(root.age_days)
                    ))
                    .css_classes(["caption-text"])
                    .build(),
            );
            let (ui, path) = (Rc::clone(self), root.path.clone());
            check.connect_toggled(move |check| {
                if check.is_active() {
                    ui.links_on.borrow_mut().insert(path.clone());
                } else {
                    ui.links_on.borrow_mut().remove(&path);
                }
                ui.recompute();
            });
            self.links_list.append(&row);
        }
    }

    fn populate_flatpak(&self, status: &SystemStatus) {
        match &status.flatpak {
            Some(flatpak) if !flatpak.unused.is_empty() => {
                let names: Vec<String> = flatpak
                    .unused
                    .iter()
                    .map(|(id, branch)| format!("{id} {branch}"))
                    .collect();
                self.flatpak_text.set_label(&names.join(", "));
                let plus = if flatpak.unused_bytes_partial {
                    "+"
                } else {
                    ""
                };
                self.flatpak_size
                    .set_label(&format!("{}{plus}", format_bytes(flatpak.unused_bytes)));
            }
            Some(_) => self.flatpak_text.set_label(&tr("Nenhum runtime sem uso.")),
            None => self.flatpak_text.set_label(&tr("Flatpak não encontrado.")),
        }
    }

    fn selection(&self, generations: bool) -> Selection {
        Selection {
            generations,
            links: self.links_on.borrow().iter().cloned().collect(),
            flatpak: self.flatpak_on.get(),
            optimise: self.optimise_on.get(),
        }
    }

    fn plan(&self, data: &Data) -> CleanupPlan {
        build_plan(
            &data.status,
            self.policy.get(),
            &self.selection(self.generations_on.get()),
            self.ctx.env.now,
        )
    }

    /// Reatualiza as descrições, os tamanhos, o total e o estado dos botões.
    fn recompute(&self) {
        let Some(data) = self.data.borrow().clone() else {
            return;
        };
        let (Ok(sizes), Ok(dead)) = (&*data.sizes, &*data.dead) else {
            return;
        };
        let flatpak_bytes = data.status.flatpak.as_ref().map_or(0, |f| f.unused_bytes);

        // Cartão de gerações: mostra o efeito da política mesmo desmarcado.
        let only_generations = build_plan(
            &data.status,
            self.policy.get(),
            &Selection {
                generations: true,
                ..Selection::default()
            },
            self.ctx.env.now,
        );
        let gens = estimate(&data.index, sizes, dead, &only_generations, 0).scaled(data.ratio);
        let (n_system, n_hm) = (
            only_generations.system_generations.len(),
            only_generations.home_manager_generations.len(),
        );
        self.generations_text.set_label(&if n_system + n_hm == 0 {
            tr("Nenhuma geração sairia com essa política.")
        } else {
            trf(
                "Saem {0} do sistema e {1} do Home Manager.",
                &[n_system.to_string(), n_hm.to_string()],
            )
        });
        self.generations_size
            .set_label(&format_bytes(gens.generations));

        let selected_links: u64 = self
            .links_on
            .borrow()
            .iter()
            .filter_map(|path| data.link_sizes.get(path))
            .sum();
        self.links_size
            .set_label(&format_bytes((selected_links as f64 * data.ratio) as u64));

        set_on(&self.generations_card, self.generations_on.get());
        set_on(&self.links_card, !self.links_on.borrow().is_empty());
        set_on(&self.optimise_card, self.optimise_on.get());
        set_on(&self.flatpak_card, self.flatpak_on.get());

        let plan = self.plan(&data);
        let total: Estimate =
            estimate(&data.index, sizes, dead, &plan, flatpak_bytes).scaled(data.ratio);
        if plan.runs_gc() {
            self.unreferenced.set_label(&trf(
                "Também podem sair até {0} de caminhos sem referência, na coleta de lixo. O valor real costuma ser menor: o Nix compartilha arquivos idênticos entre caminhos.",
                &[format_bytes(total.unreferenced)],
            ));
        } else {
            self.unreferenced.set_label("");
        }
        self.total
            .set_label(&format!("{} {}", tr("Até"), format_bytes(total.total())));
        self.clean
            .set_sensitive(!plan.is_empty() && self.helper_issue.is_none());
        self.simulate.set_sensitive(!plan.is_empty());
        self.clean.set_tooltip_text(match self.helper_issue {
            Some(_) => Some("O helper privilegiado não está instalado neste sistema"),
            None => None,
        });
    }
}

fn set_on(card: &gtk::Box, on: bool) {
    if on {
        card.add_css_class("on");
    } else {
        card.remove_css_class("on");
    }
}

fn current_plan(ui: &Rc<Ui>) -> Option<(Rc<Data>, CleanupPlan)> {
    let data = ui.data.borrow().clone()?;
    let plan = ui.plan(&data);
    Some((data, plan))
}

// ── Simular ─────────────────────────────────────────────────────────────────

fn simulate_clicked(ui: &Rc<Ui>) {
    let Some((data, plan)) = current_plan(ui) else {
        return;
    };
    ui.simulate.set_sensitive(false);
    ui.simulate.set_label(&tr("Simulando…"));
    let (ui, plan_for_thread) = (Rc::clone(ui), plan.clone());
    run_in_background(
        move || dry_run(&nix(), &plan_for_thread).map_err(|e| e.to_string()),
        move |result| {
            ui.simulate.set_label(&tr("Simular"));
            ui.simulate.set_sensitive(true);
            let body = match result {
                Ok(dry) => describe_plan(&data, &plan, dry.gc_paths, ui.policy.get()),
                Err(error) => format!("{} {error}", tr("Não foi possível simular:")),
            };
            let dialog = adw::AlertDialog::builder()
                .heading(tr("Resultado da simulação"))
                .body(body)
                .build();
            dialog.add_response("close", &tr("Fechar"));
            dialog.present(ui.ctx.window().as_ref());
        },
    );
}

fn describe_plan(
    data: &Data,
    plan: &CleanupPlan,
    gc_paths: Option<u64>,
    _policy: Policy,
) -> String {
    let mut lines = vec![tr("Nada foi alterado. Isto é o que a limpeza faria:")];
    let list = |numbers: &[u64]| {
        numbers
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !plan.system_generations.is_empty() {
        lines.push(trf(
            "• Gerações do sistema: {0}",
            &[list(&plan.system_generations)],
        ));
    }
    if !plan.home_manager_generations.is_empty() {
        lines.push(trf(
            "• Gerações do Home Manager: {0}",
            &[list(&plan.home_manager_generations)],
        ));
    }
    for root in &plan.links {
        lines.push(trf("• Link: {0}", &[root.path.display().to_string()]));
    }
    if plan.flatpak {
        lines.push(tr("• Runtimes Flatpak sem uso"));
    }
    if plan.optimise {
        lines.push(tr("• Deduplicar o store"));
    }
    if let Some(paths) = gc_paths {
        lines.push(trf(
            "• A coleta de lixo apagaria hoje {0} caminhos (simulação do Nix).",
            &[paths.to_string()],
        ));
    }
    if let (Ok(sizes), Ok(dead)) = (&*data.sizes, &*data.dead) {
        let total = estimate(
            &data.index,
            sizes,
            dead,
            plan,
            data.status.flatpak.as_ref().map_or(0, |f| f.unused_bytes),
        )
        .scaled(data.ratio);
        lines.push(trf(
            "Estimativa total (limite superior): {0}.",
            &[format_bytes(total.total())],
        ));
    }
    lines.join("\n")
}

// ── Limpar ──────────────────────────────────────────────────────────────────

fn confirm_clean(ui: &Rc<Ui>) {
    let Some((_, plan)) = current_plan(ui) else {
        return;
    };
    if plan.is_empty() {
        return;
    }
    let prompts = plan.steps().iter().filter(|s| s.privileged).count();
    let body = format!(
        "{}\n\n{}",
        tr("Esta limpeza apaga gerações e arquivos do store e não pode ser desfeita."),
        if prompts == 0 {
            tr("Nenhuma senha de administrador será pedida.")
        } else {
            tr("Será pedida a senha de administrador uma vez, para a etapa privilegiada.")
        }
    );
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Limpar agora?"))
        .body(body)
        .close_response("cancel")
        .default_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", &tr("Cancelar")), ("clean", &tr("Limpar"))]);
    dialog.set_response_appearance("clean", adw::ResponseAppearance::Destructive);
    let ui = Rc::clone(ui);
    let parent = ui.ctx.window();
    dialog.connect_response(None, move |_, response| {
        if response == "clean" {
            start_cleaning(&ui);
        }
    });
    dialog.present(parent.as_ref());
}

fn step_label(kind: StepKind, plan: &CleanupPlan) -> String {
    match kind {
        StepKind::RemoveLinks => tr("Removendo links esquecidos"),
        StepKind::HomeManagerGenerations => tr("Excluindo gerações do Home Manager"),
        StepKind::FlatpakUser => tr("Removendo runtimes Flatpak sem uso"),
        StepKind::Privileged => {
            // Um único passo do helper; o rótulo diz tudo o que ele faz.
            let mut parts = Vec::new();
            if !plan.system_generations.is_empty() {
                parts.push(tr("exclui gerações do sistema"));
            }
            if plan.flatpak && plan.flatpak_system {
                parts.push(tr("remove runtimes Flatpak do sistema"));
            }
            if plan.runs_gc() {
                parts.push(tr("coleta o lixo do store"));
            }
            if plan.optimise {
                parts.push(tr("deduplica o store"));
            }
            let mut text = parts.join(", ");
            if let Some(first) = text.get(..1).map(str::to_uppercase) {
                text.replace_range(..1, &first);
            }
            text
        }
    }
}

/// Linha de passo no diálogo de progresso: ícone de estado e rótulo.
struct StepRow {
    stack: gtk::Stack,
    label: gtk::Label,
}

fn start_cleaning(ui: &Rc<Ui>) {
    let Some((_, plan)) = current_plan(ui) else {
        return;
    };
    let steps = plan.steps();

    let dialog = adw::Dialog::builder()
        .title(tr("Limpando"))
        .content_width(460)
        .can_close(false)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(
        &adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .build(),
    );
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .margin_start(18)
        .margin_end(18)
        .margin_bottom(18)
        .margin_top(6)
        .build();
    let status_label = gtk::Label::builder()
        .label(tr("Cada etapa privilegiada pede a senha de administrador."))
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption-text"])
        .build();
    body.append(&status_label);
    let rows_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    let step_rows: Vec<StepRow> = steps
        .iter()
        .map(|step| make_step_row(&rows_box, step, &plan))
        .collect();
    body.append(&rows_box);
    let stop = Rc::new(Cell::new(false));
    let button = gtk::Button::builder()
        .label(tr("Parar depois desta etapa"))
        .halign(gtk::Align::End)
        .build();
    {
        let stop = Rc::clone(&stop);
        button.connect_clicked(move |button| {
            stop.set(true);
            button.set_sensitive(false);
        });
    }
    body.append(&button);
    toolbar.set_content(Some(&body));
    dialog.set_child(Some(&toolbar));
    dialog.present(ui.ctx.window().as_ref());

    let before = free_bytes(std::path::Path::new("/nix")).unwrap_or(0);
    let run = Rc::new(Run {
        ui: Rc::clone(ui),
        dialog,
        status_label,
        button,
        steps,
        rows: step_rows,
        plan,
        stop,
        before,
        started: std::time::Instant::now(),
        started_at: epoch_now(),
    });
    run_step(&run, 0);
}

struct Run {
    ui: Rc<Ui>,
    dialog: adw::Dialog,
    status_label: gtk::Label,
    button: gtk::Button,
    steps: Vec<Step>,
    rows: Vec<StepRow>,
    plan: CleanupPlan,
    stop: Rc<Cell<bool>>,
    before: u64,
    started: std::time::Instant,
    started_at: i64,
}

fn make_step_row(container: &gtk::Box, step: &Step, plan: &CleanupPlan) -> StepRow {
    let row = gtk::Box::builder().spacing(12).build();
    let stack = gtk::Stack::builder().valign(gtk::Align::Center).build();
    stack.add_named(
        &gtk::Image::builder()
            .icon_name("circle-symbolic")
            .pixel_size(20)
            .css_classes(["step-pending"])
            .build(),
        Some("pending"),
    );
    let spinner = gtk::Spinner::builder().spinning(true).build();
    stack.add_named(&spinner, Some("running"));
    stack.add_named(
        &gtk::Image::builder()
            .icon_name("check-circle-fill-symbolic")
            .pixel_size(20)
            .css_classes(["step-done"])
            .build(),
        Some("done"),
    );
    stack.add_named(
        &gtk::Image::builder()
            .icon_name("warning-symbolic")
            .pixel_size(20)
            .css_classes(["warn-icon"])
            .build(),
        Some("failed"),
    );
    stack.set_visible_child_name("pending");
    let suffix = if step.privileged {
        format!(" ({})", tr("pede senha"))
    } else {
        String::new()
    };
    let label = gtk::Label::builder()
        .label(format!("{}{suffix}", step_label(step.kind, plan)))
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    row.append(&stack);
    row.append(&label);
    container.append(&row);
    StepRow { stack, label }
}

/// Roda o passo `index` numa thread e, ao terminar, segue para o próximo.
fn run_step(run: &Rc<Run>, index: usize) {
    if index >= run.steps.len() {
        return finish(run, None);
    }
    if run.stop.get() {
        return finish(run, Some(tr("Interrompida a seu pedido.")));
    }
    run.rows[index].stack.set_visible_child_name("running");
    let step = run.steps[index];
    let plan = run.plan.clone();
    let env = run.ui.ctx.env.clone();
    let run2 = Rc::clone(run);
    run_in_background(
        move || {
            let nix = nix();
            let flatpak = CliFlatpak::new(SystemRunner);
            let helper = HelperClient::production(SystemRunner);
            let profile = env.home_manager_profiles_dir.join("home-manager");
            Cleaner {
                nix: &nix,
                flatpak: &flatpak,
                helper: &helper,
                home: &env.home,
                home_manager_profile: &profile,
            }
            .run(step, &plan)
            .map_err(|e| e.to_string())
        },
        move |result| match result {
            Ok(()) => {
                run2.rows[index].stack.set_visible_child_name("done");
                run_step(&run2, index + 1);
            }
            Err(error) => {
                run2.rows[index].stack.set_visible_child_name("failed");
                run2.rows[index].label.set_label(&format!(
                    "{} — {error}",
                    step_label(run2.steps[index].kind, &run2.plan)
                ));
                finish(&run2, Some(error));
            }
        },
    );
}

fn finish(run: &Rc<Run>, problem: Option<String>) {
    run.dialog.set_can_close(true);
    run.button.set_label(&tr("Concluir"));
    run.button.set_sensitive(true);
    let dialog = run.dialog.clone();
    run.button.connect_clicked(move |_| {
        dialog.close();
    });
    let after = free_bytes(std::path::Path::new("/nix")).unwrap_or(0);
    let freed = after.saturating_sub(run.before);
    run.ui.ctx.record(NewOperation {
        kind: OperationKind::Clean,
        started_at: run.started_at,
        duration_ms: elapsed_ms(run.started),
        outcome: if problem.is_none() {
            Outcome::Success
        } else {
            Outcome::Failure
        },
        generation: None,
        bytes_freed: problem.is_none().then_some(freed),
        detail: problem.as_ref().map(|p| p.chars().take(200).collect()),
    });
    match problem {
        None => {
            run.status_label.set_label(&format!(
                "{} {}",
                tr("Concluído. Espaço livre recuperado:"),
                format_bytes(freed)
            ));
            run.ui.ctx.toast(&format!(
                "{} {}",
                tr("Limpeza concluída:"),
                format_bytes(freed)
            ));
        }
        Some(problem) => run
            .status_label
            .set_label(&format!("{} {problem}", tr("A limpeza parou:"))),
    }
    // Os números das outras telas mudaram: recarrega tudo.
    let ui = Rc::clone(&run.ui);
    ui.ctx.sizes.refresh(|_| {});
    ui.ctx.dead.refresh(|_| {});
    let ui2 = Rc::clone(&ui);
    ui.ctx.overview.refresh(move |status| {
        let (env, for_index) = (ui2.ctx.env.clone(), (*status).clone());
        run_in_background(
            move || closure_index(&nix(), &env, &for_index).map_err(|e| e.to_string()),
            move |index| {
                let ui3 = Rc::clone(&ui2);
                ui2.ctx.sizes.get(move |sizes| {
                    let ui4 = Rc::clone(&ui3);
                    ui3.ctx
                        .dead
                        .get(move |dead| ui4.loaded(&status, index, &sizes, &dead));
                });
            },
        );
    });
}
