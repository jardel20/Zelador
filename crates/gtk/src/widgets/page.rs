//! Moldura comum das telas: barra superior só com botões de ícone, título grande e
//! parágrafo de abertura, em coluna de largura máxima, mais a barra inferior das telas de fluxo.

use adw::prelude::*;

use std::rc::Rc;

use crate::ctx::Ctx;
use crate::i18n::tr;
use crate::pages::Page;
use crate::screens;
use crate::widgets::column;

pub fn build(page: Page, ctx: &Rc<Ctx>) -> adw::ToolbarView {
    let view = adw::ToolbarView::new();
    let (header, step_label) = top_bar(page);
    view.add_top_bar(&header);

    let title = gtk::Label::builder()
        .label(page.title())
        .css_classes(["page-title"])
        .xalign(0.0)
        .wrap(true)
        .build();
    let intro = gtk::Label::builder()
        .label(page.intro())
        .css_classes(["page-intro"])
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(60)
        .build();
    let texts = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .hexpand(true)
        .build();
    texts.append(&title);
    texts.append(&intro);

    let screen = screens::build(page, ctx, &title, &intro, step_label.as_ref(), &view);

    // Título e parágrafo à esquerda; um controle opcional (ex.: alternador) à direita.
    let head = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(16)
        .valign(gtk::Align::End)
        .build();
    head.append(&texts);
    if let Some(end) = screen.as_ref().and_then(|s| s.header_end.as_ref()) {
        head.append(end);
    }

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(8)
        .margin_bottom(40)
        .build();
    column.append(&head);
    if let Some(screen) = &screen {
        column.append(&screen.body);
    }

    // Margem lateral de 56 px (mockups) e coluna alinhada à esquerda.
    let content = column::left_column(page.max_width(), &column);
    content.set_margin_start(56);
    content.set_margin_end(56);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();
    view.set_content(Some(&scroll));
    // Só para verificação visual: rola até o fim depois de N segundos.
    if let Some(seconds) = std::env::var("ZELADOR_SCROLL_END_AFTER")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    {
        let scroll = scroll.clone();
        gtk::glib::timeout_add_seconds_local_once(seconds, move || {
            let adjustment = scroll.vadjustment();
            adjustment.set_value(adjustment.upper());
        });
    }

    view
}

fn top_bar(page: Page) -> (adw::HeaderBar, Option<gtk::Label>) {
    // Só botões de ícone à direita: o "fechar" é um botão comum (ícone Phosphor `x`) que
    // chama `window.close`, em vez dos controles de janela do libadwaita.
    let header = adw::HeaderBar::builder()
        .show_title(false)
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .css_classes(["flat"])
        .build();
    header.pack_end(
        &gtk::Button::builder()
            .icon_name("x-symbolic")
            .tooltip_text(tr("Fechar"))
            .action_name("window.close")
            .css_classes(["flat"])
            .build(),
    );
    let mut step_label = None;
    match page {
        Page::Atualizar | Page::Aplicando => {
            let label = if page == Page::Atualizar {
                tr("Atualização · etapa 1 de 3")
            } else {
                tr("Atualização")
            };
            let label = gtk::Label::builder()
                .label(label)
                .css_classes(["step-label"])
                .margin_start(22)
                .build();
            header.pack_start(&label);
            step_label = Some(label);
        }
        Page::SeuSistema => header.pack_end(&main_menu()),
        Page::Geracoes | Page::LiberarEspaco | Page::EspacoEmDisco => {}
    }
    (header, step_label)
}

fn main_menu() -> gtk::MenuButton {
    let menu = gtk::gio::Menu::new();
    menu.append(
        Some(&tr("Usar o tema do sistema")),
        Some("app.system-theme"),
    );
    menu.append(Some(&tr("Sobre o Zelador")), Some("app.about"));
    menu.append(Some(&tr("Sair")), Some("app.quit"));
    gtk::MenuButton::builder()
        .icon_name("list-symbolic")
        .tooltip_text(tr("Menu principal"))
        .menu_model(&menu)
        .css_classes(["flat"])
        .build()
}
