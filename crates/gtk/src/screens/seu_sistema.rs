//! "Seu sistema": geração em uso, commits pendentes, arquivos fora do git, inputs do flake,
//! espaço e histórico.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use zelador_core::backends::GitBackend;
use zelador_core::flake::SECONDS_PER_DAY;
use zelador_core::history::{self, History};
use zelador_core::status::{ApproximationReason, PendingChanges, SystemStatus};
use zelador_core::units::format_bytes;

use super::Screen;
use crate::ctx::Ctx;
use crate::format::{describe_operation, relative_age};
use crate::i18n::{tn, tr, trf};
use crate::loader::run_in_background;
use crate::pages::Page;
use crate::widgets::rows;

pub fn build(ctx: &Rc<Ctx>, intro: &gtk::Label) -> Screen {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(32)
        .margin_top(10)
        .build();

    // Botões de abertura.
    let apply = gtk::Button::builder()
        .label(tr("Revisar e aplicar"))
        .css_classes(["suggested-action"])
        .build();
    apply.connect_clicked(glib::clone!(
        #[strong]
        ctx,
        move |_| ctx.go_to(Page::Atualizar)
    ));
    let open = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("folder-open-symbolic")
                .label(tr("Abrir pasta do flake"))
                .build(),
        )
        .build();
    open.connect_clicked(glib::clone!(
        #[strong]
        ctx,
        move |_| open_flake_folder(&ctx)
    ));
    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .halign(gtk::Align::Start)
        .build();
    buttons.append(&apply);
    buttons.append(&open);
    root.append(&buttons);

    let note = gtk::Label::builder()
        .css_classes(["caption-text"])
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();

    let banner = Banner::new();
    root.append(&banner.root);

    let inputs = rows::group(&tr("Inputs do flake"));
    let inputs_placeholder = rows::row(&tr("Lendo os inputs do flake…"), None, false);
    inputs_placeholder.add_suffix(&rows::spinner());
    inputs.add(&inputs_placeholder);
    root.append(&inputs);

    let space = rows::group(&tr("Espaço"));
    let space_row = rows::row(&tr("Calculando o tamanho do /nix/store…"), None, false);
    let space_spinner = rows::spinner();
    space_row.add_suffix(&space_spinner);
    space.add(&space_row);
    let maintenance = rows::row(&tr("Manutenção automática"), None, false);
    space.add(&maintenance);
    root.append(&space);

    let history = rows::group(&tr("Histórico"));
    let history_rows: Rc<RefCell<Vec<gtk::Widget>>> = Rc::default();
    root.append(&history);
    load_history(ctx, &history, &history_rows);
    {
        let (ctx2, group, held) = (Rc::clone(ctx), history.clone(), Rc::clone(&history_rows));
        ctx.history_changed
            .subscribe(move |()| load_history(&ctx2, &group, &held));
    }

    // Parte rápida.
    ctx.overview.get(glib::clone!(
        #[strong]
        ctx,
        #[weak]
        intro,
        #[weak]
        note,
        #[weak]
        inputs,
        #[weak]
        inputs_placeholder,
        #[weak]
        maintenance,
        #[strong]
        banner,
        move |status| {
            intro.set_label(&intro_text(&ctx, &status));
            if let Some(text) = approximation_note(&status) {
                note.set_label(&text);
                note.set_visible(true);
            }
            show_inputs(&ctx, &status, &inputs, &inputs_placeholder);
            maintenance.set_subtitle(&maintenance_text(&status));
            banner.show(&ctx, &status);
        }
    ));

    // Parte lenta: tamanho do store e quanto pode sair (os dois comandos rodam em paralelo).
    ctx.dead.get(|_| {});
    ctx.sizes.get(glib::clone!(
        #[weak]
        space_row,
        #[weak]
        space_spinner,
        #[strong]
        ctx,
        move |sizes| {
            let ctx2 = Rc::clone(&ctx);
            ctx.dead.get(move |dead| {
                show_space(&ctx2, &space_row, &space_spinner, &sizes, &dead);
            });
        }
    ));

    // O parágrafo de aviso fica logo abaixo do de abertura.
    if let Some(parent) = intro.parent().and_downcast::<gtk::Box>() {
        parent.insert_child_after(&note, Some(intro));
    }

    Screen {
        body: root.upcast(),
        header_end: None,
    }
}

/// As últimas 3 operações do SQLite (ou o estado vazio).
fn load_history(
    ctx: &Rc<Ctx>,
    group: &adw::PreferencesGroup,
    held: &Rc<RefCell<Vec<gtk::Widget>>>,
) {
    let env = ctx.env.clone();
    let (group, held) = (group.clone(), Rc::clone(held));
    run_in_background(
        move || {
            History::open(&history::default_path(&env))
                .and_then(|history| history.recent(3))
                .map_err(|e| e.to_string())
        },
        move |result| {
            let mut rows_held = held.borrow_mut();
            rows::clear_group(&group, &mut rows_held);
            let year = gtk::glib::DateTime::now_local().map_or(0, |now| now.year());
            let operations = result.unwrap_or_default();
            if operations.is_empty() {
                let row = rows::muted_row(&tr("Nenhuma operação registrada ainda."));
                group.add(&row);
                rows_held.push(row.upcast());
                return;
            }
            for operation in &operations {
                let (title, when) = describe_operation(operation, year);
                let row = rows::row(&title, None, false);
                row.add_suffix(&rows::suffix_label(&when, &["caption-text"]));
                group.add(&row);
                rows_held.push(row.upcast());
            }
        },
    );
}

fn intro_text(ctx: &Ctx, status: &SystemStatus) -> String {
    let generation = status
        .current_generation()
        .map(|g| (g.number, (ctx.env.now - g.created).max(0) / SECONDS_PER_DAY));
    compose_intro(generation, status.pending.as_ref())
}

/// `Geração 214, aplicada há 3 dias. A configuração tem 3 commits que ainda não foram aplicados.`
fn compose_intro(generation: Option<(u64, i64)>, pending: Option<&PendingChanges>) -> String {
    let first = match generation {
        Some((number, days)) => trf(
            "Geração {0}, aplicada {1}.",
            &[number.to_string(), relative_age(days)],
        ),
        None => tr("Não foi possível identificar a geração em uso."),
    };
    let second = match pending {
        Some(PendingChanges::Exact(0)) => {
            Some(tr("Todos os commits da configuração já foram aplicados."))
        }
        Some(PendingChanges::Exact(1)) => Some(tr(
            "A configuração tem 1 commit que ainda não foi aplicado.",
        )),
        Some(PendingChanges::Exact(n)) => Some(trf(
            "A configuração tem {0} que ainda não foram aplicados.",
            &[tn("{0} commit", "{0} commits", *n)],
        )),
        Some(PendingChanges::Approximate { commits: 0, .. }) => {
            Some(tr("Nenhum commit novo desde que a geração foi aplicada."))
        }
        Some(PendingChanges::Approximate { commits, .. }) => Some(trf(
            "Cerca de {0} podem não ter sido aplicados.",
            &[tn("{0} commit", "{0} commits", *commits)],
        )),
        None => None,
    };
    match second {
        Some(second) => trf("{0} {1}", &[first, second]),
        None => first,
    }
}

/// Explica por que a contagem de commits é aproximada, quando for o caso.
fn approximation_note(status: &SystemStatus) -> Option<String> {
    let Some(PendingChanges::Approximate { reasons, .. }) = &status.pending else {
        return None;
    };
    let reason = reasons.first()?;
    let why = match reason {
        ApproximationReason::NoConfigurationRevision => {
            tr("A geração em uso não registra a revisão do flake")
        }
        ApproximationReason::DirtyConfigurationRevision => {
            tr("A geração em uso foi construída com alterações não commitadas")
        }
        ApproximationReason::RevisionNotInRepo => {
            tr("A revisão da geração em uso não existe mais neste repositório")
        }
    };
    Some(trf(
        "{0}, então a contagem é aproximada (commits feitos depois da data da geração).",
        &[why],
    ))
}

fn show_inputs(
    ctx: &Ctx,
    status: &SystemStatus,
    group: &adw::PreferencesGroup,
    placeholder: &adw::ActionRow,
) {
    group.remove(placeholder);
    if status.inputs.is_empty() {
        group.add(&rows::muted_row(&tr("Nenhum input encontrado no flake.")));
        return;
    }
    for item in &status.inputs {
        let row = rows::row(&item.input.name, Some(&item.input.label()), true);
        let age = item
            .input
            .age_days(ctx.env.now)
            .map(relative_age)
            .unwrap_or_default();
        let classes: &[&str] = if item.stale {
            &["age-stale"]
        } else {
            &["caption-text"]
        };
        row.add_suffix(&rows::suffix_label(&age, classes));
        group.add(&row);
    }
}

fn maintenance_text(status: &SystemStatus) -> String {
    let on_off = |value: bool| if value { tr("ativa") } else { tr("inativa") };
    let mut parts = Vec::new();
    if let Some(auto) = &status.auto_maintenance {
        parts.push(format!("{} {}", tr("Limpeza do Nix:"), on_off(auto.gc)));
        parts.push(format!(
            "{} {}",
            tr("otimização do store:"),
            on_off(auto.optimise)
        ));
    }
    match status.flatpak_uninstall_unused {
        Some(value) => parts.push(format!(
            "{} {}",
            tr("remoção de runtimes Flatpak:"),
            on_off(value)
        )),
        None => parts.push(tr("nix-flatpak não configurado")),
    }
    parts.join(" · ")
}

fn show_space(
    ctx: &Rc<Ctx>,
    row: &adw::ActionRow,
    spinner: &gtk::Spinner,
    sizes: &Result<std::collections::HashMap<String, u64>, String>,
    dead: &Result<std::collections::HashSet<String>, String>,
) {
    spinner.set_visible(false);
    match (sizes, dead) {
        (Ok(sizes), Ok(dead)) => {
            let total: u64 = sizes.values().sum();
            let reclaimable: u64 = dead.iter().filter_map(|path| sizes.get(path)).sum();
            row.set_title(&trf("{0} no /nix/store", &[format_bytes(total)]));
            row.set_subtitle(&trf(
                "Cerca de {0} podem ser liberados",
                &[format_bytes(reclaimable)],
            ));
            let button = gtk::Button::builder()
                .label(tr("Liberar espaço"))
                .valign(gtk::Align::Center)
                .build();
            let ctx = Rc::clone(ctx);
            button.connect_clicked(move |_| ctx.go_to(Page::LiberarEspaco));
            row.add_suffix(&button);
        }
        (Err(error), _) | (_, Err(error)) => {
            row.set_title(&tr("Não foi possível calcular o tamanho do store"));
            row.set_subtitle(error);
        }
    }
}

fn open_flake_folder(ctx: &Ctx) {
    let file = gtk::gio::File::for_path(&ctx.env.flake);
    let launcher = gtk::FileLauncher::new(Some(&file));
    launcher.launch(
        ctx.window().as_ref(),
        None::<&gtk::gio::Cancellable>,
        |result| {
            if let Err(error) = result {
                tracing::warn!(%error, "não foi possível abrir a pasta do flake");
            }
        },
    );
}

/// Aviso de arquivos fora do git, com o botão "Adicionar ao git" (pede confirmação).
#[derive(Clone)]
struct Banner {
    root: gtk::Box,
    title: gtk::Label,
    files: gtk::Label,
    button: gtk::Button,
}

impl Banner {
    fn new() -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(14)
            .css_classes(["warning-banner"])
            .visible(false)
            .build();
        root.append(
            &gtk::Image::builder()
                .icon_name("warning-symbolic")
                .pixel_size(22)
                .css_classes(["warn-icon"])
                .valign(gtk::Align::Start)
                .build(),
        );
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .hexpand(true)
            .build();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["banner-title"])
            .build();
        let files = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .css_classes(["mono", "banner-files"])
            .build();
        text.append(&title);
        text.append(&files);
        root.append(&text);
        let button = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("git-branch-symbolic")
                    .label(tr("Adicionar ao git"))
                    .build(),
            )
            .valign(gtk::Align::Center)
            .build();
        root.append(&button);
        Self {
            root,
            title,
            files,
            button,
        }
    }

    fn show(&self, ctx: &Rc<Ctx>, status: &SystemStatus) {
        let untracked = status
            .git
            .as_ref()
            .map(|g| g.untracked.clone())
            .unwrap_or_default();
        if untracked.is_empty() {
            self.root.set_visible(false);
            return;
        }
        let title = if untracked.len() == 1 {
            tr("1 arquivo novo está fora do git. O flake vai ignorá-lo no próximo build.")
        } else {
            trf(
                "{0} arquivos novos estão fora do git. O flake vai ignorá-los no próximo build.",
                &[untracked.len().to_string()],
            )
        };
        self.title.set_label(&title);
        self.files.set_label(&untracked.join("\n"));
        self.root.set_visible(true);

        let (ctx, banner) = (Rc::clone(ctx), self.clone());
        self.button
            .connect_clicked(move |_| confirm_add(&ctx, &banner, &untracked));
    }
}

fn confirm_add(ctx: &Rc<Ctx>, banner: &Banner, files: &[String]) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Adicionar ao git?"))
        .body(format!(
            "{}\n\n{}",
            tr("Os arquivos abaixo passam a fazer parte do próximo build do flake. Nada é commitado."),
            files.join("\n")
        ))
        .close_response("cancel")
        .default_response("add")
        .build();
    dialog.add_responses(&[("cancel", &tr("Cancelar")), ("add", &tr("Adicionar"))]);
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    let parent = ctx.window();
    let (ctx, banner, files) = (Rc::clone(ctx), banner.clone(), files.to_vec());
    dialog.connect_response(None, move |_, response| {
        if response != "add" {
            return;
        }
        let (flake, files) = (ctx.env.flake.clone(), files.clone());
        let (ctx, banner) = (Rc::clone(&ctx), banner.clone());
        run_in_background(
            move || crate::ctx::git().add(&flake, &files),
            move |result| match result {
                Ok(()) => {
                    ctx.toast(&tr("Arquivos adicionados ao git"));
                    banner.root.set_visible(false);
                    let ctx2 = Rc::clone(&ctx);
                    ctx.overview
                        .refresh(move |status| banner.show(&ctx2, &status));
                }
                Err(error) => ctx.toast(&format!("{} {error}", tr("Não foi possível adicionar:"))),
            },
        );
    });
    dialog.present(parent.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abertura_com_contagem_exata() {
        assert_eq!(
            compose_intro(Some((214, 3)), Some(&PendingChanges::Exact(3))),
            "Geração 214, aplicada há 3 dias. A configuração tem 3 commits que ainda não foram aplicados."
        );
        assert_eq!(
            compose_intro(Some((5, 0)), Some(&PendingChanges::Exact(1))),
            "Geração 5, aplicada hoje. A configuração tem 1 commit que ainda não foi aplicado."
        );
        assert!(
            compose_intro(Some((5, 1)), Some(&PendingChanges::Exact(0)))
                .contains("Todos os commits")
        );
    }

    #[test]
    fn abertura_aproximada_e_sem_dados() {
        let approx = PendingChanges::Approximate {
            commits: 4,
            reasons: vec![ApproximationReason::NoConfigurationRevision],
        };
        assert!(
            compose_intro(Some((9, 2)), Some(&approx))
                .ends_with("Cerca de 4 commits podem não ter sido aplicados.")
        );
        assert_eq!(
            compose_intro(None, None),
            "Não foi possível identificar a geração em uso."
        );
    }
}
