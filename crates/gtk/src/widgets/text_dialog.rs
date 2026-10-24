//! Diálogo com um texto longo em fonte monoespaçada (log completo de um build, por exemplo).

use adw::prelude::*;

pub fn present(parent: Option<&gtk::Window>, title: &str, text: &str) {
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(720)
        .content_height(560)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .yalign(0.0)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["mono", "log-view"])
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .margin_start(18)
        .margin_end(18)
        .margin_bottom(18)
        .child(&label)
        .build();
    toolbar.set_content(Some(&scroll));
    dialog.set_child(Some(&toolbar));
    dialog.present(parent);
}
