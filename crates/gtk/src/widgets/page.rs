//! Moldura comum das telas: barra superior só com botões de ícone, título grande e
//! parágrafo de abertura, em coluna de largura máxima, mais a barra inferior das telas de fluxo.

use adw::prelude::*;

use crate::i18n::tr;
use crate::pages::Page;
use crate::widgets::{bottom_bar, column};

pub fn build(page: Page) -> adw::ToolbarView {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&top_bar(page));

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(8)
        .margin_bottom(40)
        .build();
    column.append(
        &gtk::Label::builder()
            .label(page.title())
            .css_classes(["page-title"])
            .xalign(0.0)
            .wrap(true)
            .build(),
    );
    column.append(
        &gtk::Label::builder()
            .label(page.intro())
            .css_classes(["page-intro"])
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(60)
            .build(),
    );

    // Margem lateral de 56 px (mockups) e coluna alinhada à esquerda.
    let content = column::left_column(page.max_width(), &column);
    content.set_margin_start(56);
    content.set_margin_end(56);
    content.set_halign(gtk::Align::Fill);
    let clamp = content;
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    view.set_content(Some(&scroll));

    if let Some(bar) = bottom_bar::for_page(page) {
        view.add_bottom_bar(&bar);
    }
    view
}

fn top_bar(page: Page) -> adw::HeaderBar {
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
    match page {
        Page::Atualizar | Page::Aplicando => {
            let label = if page == Page::Atualizar {
                tr("Atualização · etapa 3 de 3")
            } else {
                tr("Atualização")
            };
            header.pack_start(
                &gtk::Label::builder()
                    .label(label)
                    .css_classes(["step-label"])
                    .margin_start(22)
                    .build(),
            );
        }
        Page::SeuSistema => header.pack_end(&main_menu()),
        Page::Geracoes | Page::LiberarEspaco | Page::EspacoEmDisco => {}
    }
    header
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
