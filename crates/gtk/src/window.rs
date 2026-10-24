//! Janela principal: `AdwNavigationSplitView` com a barra lateral e a pilha de telas.

use crate::pages::Page;
use crate::widgets::{page, sidebar};

const SIDEBAR_WIDTH: f64 = 248.0;
const DEFAULT_FLAKE: &str = "/etc/nixos";

pub struct Context {
    /// Caminho do flake como aparece na barra lateral (com `~`).
    pub flake_label: String,
    pub start: Page,
}

impl Context {
    pub fn from_env() -> Self {
        let flake = std::env::var("ZELADOR_FLAKE").unwrap_or_else(|_| DEFAULT_FLAKE.to_owned());
        Self {
            flake_label: abbreviate_home(&flake),
            start: crate::start_page_from_env(),
        }
    }
}

/// `/home/ana/nixos-config` → `~/nixos-config`.
fn abbreviate_home(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => match path.strip_prefix(&home) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
            _ => path.to_owned(),
        },
        _ => path.to_owned(),
    }
}

pub fn build(app: &adw::Application, context: &Context) -> adw::ApplicationWindow {
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    for page in Page::ALL {
        stack.add_named(&page::build(page), Some(page.id()));
    }

    let sidebar = sidebar::build(&context.flake_label, {
        let stack = stack.clone();
        move |page| stack.set_visible_child_name(page.id())
    });

    let sidebar_page = adw::NavigationPage::builder()
        .title("Zelador")
        .child(&sidebar.widget)
        .build();
    let content_page = adw::NavigationPage::builder()
        .title("Zelador")
        .child(&stack)
        .build();
    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content_page)
        .min_sidebar_width(SIDEBAR_WIDTH)
        .max_sidebar_width(SIDEBAR_WIDTH)
        .sidebar_width_fraction(0.25)
        .build();

    // "Aplicando" não tem item próprio: destaca "Atualizar" e mostra a tela depois que a
    // janela mapeia, porque o ListBox seleciona uma linha sozinho ao receber o foco.
    let (highlight, show_after) = match context.start {
        Page::Aplicando => (Page::Atualizar, Some(Page::Aplicando)),
        page => (page, None),
    };
    if let Some(row) = Page::NAV
        .iter()
        .position(|page| *page == highlight)
        .and_then(|index| i32::try_from(index).ok())
        .and_then(|index| sidebar.list.row_at_index(index))
    {
        sidebar.list.select_row(Some(&row));
    }
    if let Some(page) = show_after {
        let stack = stack.clone();
        gtk::glib::idle_add_local_once(move || stack.set_visible_child_name(page.id()));
    }

    adw::ApplicationWindow::builder()
        .application(app)
        .title("Zelador")
        .default_width(1100)
        .default_height(760)
        .content(&split)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_vira_til() {
        // SAFETY-free: o teste só lê HOME; se não houver, o caminho fica como veio.
        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(
                abbreviate_home(&format!("{home}/nixos-config")),
                "~/nixos-config"
            );
            assert_eq!(abbreviate_home(&home), "~");
            assert_eq!(abbreviate_home(&format!("{home}x/y")), format!("{home}x/y"));
        }
        assert_eq!(abbreviate_home("/etc/nixos"), "/etc/nixos");
    }
}
