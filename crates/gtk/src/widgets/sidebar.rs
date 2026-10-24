//! Barra lateral: ícone do app, nome e caminho do flake, navegação e versões.

use adw::prelude::*;
use gtk::glib;
use zelador_core::runner::SystemRunner;
use zelador_core::status::system_versions;

use crate::pages::Page;

pub struct Sidebar {
    pub widget: gtk::Widget,
    pub list: gtk::ListBox,
}

pub fn build(flake_label: &str, on_select: impl Fn(Page) + 'static) -> Sidebar {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("zelador-sidebar");

    root.append(&brand(flake_label));

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    for page in Page::NAV {
        list.append(&nav_row(page));
    }
    list.connect_row_selected(move |_, row| {
        let page = row
            .and_then(|row| usize::try_from(row.index()).ok())
            .and_then(|index| Page::NAV.get(index).copied());
        if let Some(page) = page {
            on_select(page);
        }
    });
    root.append(&list);

    let versions = gtk::Label::builder()
        .css_classes(["versions"])
        .xalign(0.0)
        .vexpand(true)
        .valign(gtk::Align::End)
        .build();
    root.append(&versions);
    show_versions_later(&versions);

    Sidebar {
        widget: root.upcast(),
        list,
    }
}

fn brand(flake_label: &str) -> gtk::Widget {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_start(8)
        .margin_end(8)
        .margin_top(4)
        .margin_bottom(17)
        .build();
    row.append(
        &gtk::Image::builder()
            .icon_name(crate::APP_ID)
            .pixel_size(36)
            .valign(gtk::Align::Center)
            .build(),
    );
    let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
    names.append(
        &gtk::Label::builder()
            .label("Zelador")
            .css_classes(["app-name"])
            .xalign(0.0)
            .build(),
    );
    names.append(
        &gtk::Label::builder()
            .label(flake_label)
            .css_classes(["flake-path", "mono"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Start)
            .build(),
    );
    row.append(&names);
    // Permite arrastrar a janela pela área do cabeçalho da barra lateral.
    gtk::WindowHandle::builder().child(&row).build().upcast()
}

fn nav_row(page: Page) -> gtk::ListBoxRow {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(
        &gtk::Image::builder()
            .icon_name(page.nav_icon())
            .pixel_size(20)
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label(page.nav_label())
            .xalign(0.0)
            .build(),
    );
    gtk::ListBoxRow::builder().child(&content).build()
}

/// Lê as versões fora da thread principal: `nix --version` é um processo externo.
fn show_versions_later(label: &gtk::Label) {
    glib::spawn_future_local(glib::clone!(
        #[weak]
        label,
        async move {
            let versions = gtk::gio::spawn_blocking(|| system_versions(&SystemRunner)).await;
            let text = match versions {
                Ok((nixos, nix)) => [
                    nixos.map(|v| format!("NixOS {v}")),
                    nix.map(|v| format!("Nix {v}")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · "),
                Err(_) => String::new(),
            };
            label.set_label(&text);
        }
    ));
}
