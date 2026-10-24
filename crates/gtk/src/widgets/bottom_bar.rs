//! Barra inferior fixa das telas de fluxo: branca, com borda superior.

use gtk::prelude::*;

use crate::i18n::tr;
use crate::pages::Page;

/// Barra de cada tela de fluxo, ou `None` nas telas de leitura.
pub fn for_page(page: Page) -> Option<gtk::Widget> {
    match page {
        Page::Atualizar => Some(wizard_bar()),
        Page::Aplicando => Some(applying_bar()),
        Page::LiberarEspaco => Some(clean_bar()),
        Page::SeuSistema | Page::Geracoes | Page::EspacoEmDisco => None,
    }
}

fn shell() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["zelador-bottom-bar"])
        .build()
}

fn spacer() -> gtk::Widget {
    gtk::Box::builder().hexpand(true).build().upcast()
}

fn button(label: &str, classes: &[&str]) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .css_classes(classes.to_vec())
        .sensitive(false)
        .build();
    button.set_valign(gtk::Align::Center);
    button
}

/// "Voltar", pontos de progresso e "Aplicar" (etapa 3 de 3).
fn wizard_bar() -> gtk::Widget {
    let bar = shell();
    bar.append(&button(&tr("Voltar"), &[]));
    bar.append(&spacer());
    let dots = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .valign(gtk::Align::Center)
        .build();
    for step in 0..3 {
        let dot = gtk::Box::builder()
            .css_classes(if step == 2 {
                vec!["dot", "dot-active"]
            } else {
                vec!["dot"]
            })
            .build();
        dots.append(&dot);
    }
    bar.append(&dots);
    bar.append(&spacer());
    bar.append(&button(&tr("Aplicar"), &["suggested-action"]));
    bar.upcast()
}

fn applying_bar() -> gtk::Widget {
    let bar = shell();
    bar.append(
        &gtk::Label::builder()
            .label(tr("Iniciado às --:--"))
            .css_classes(["caption-text"])
            .build(),
    );
    bar.append(&spacer());
    bar.append(&button(&tr("Concluir"), &[]));
    bar.upcast()
}

fn clean_bar() -> gtk::Widget {
    let bar = shell();
    bar.append(
        &gtk::Label::builder()
            .label(tr("Nada selecionado"))
            .css_classes(["strong"])
            .build(),
    );
    bar.append(
        &gtk::Label::builder()
            .label(tr("para liberar"))
            .css_classes(["caption-text"])
            .build(),
    );
    bar.append(&spacer());
    bar.append(&button(&tr("Simular"), &[]));
    bar.append(&button(&tr("Limpar"), &["destructive-action"]));
    bar.upcast()
}
