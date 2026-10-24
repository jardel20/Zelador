//! Conteúdo das telas que já têm dados (Fase 4: somente leitura).

mod espaco;
mod geracoes;
mod seu_sistema;

use std::rc::Rc;

use crate::ctx::Ctx;
use crate::pages::Page;

pub struct Screen {
    pub body: gtk::Widget,
    /// Controle à direita do título (ex.: alternador Sistema / Home Manager).
    pub header_end: Option<gtk::Widget>,
}

/// `None` para as telas que ainda não têm conteúdo.
pub fn build(
    page: Page,
    ctx: &Rc<Ctx>,
    intro: &gtk::Label,
    view: &adw::ToolbarView,
) -> Option<Screen> {
    match page {
        Page::SeuSistema => Some(seu_sistema::build(ctx, intro)),
        Page::Geracoes => Some(geracoes::build(ctx, intro, view)),
        Page::EspacoEmDisco => Some(espaco::build(ctx, intro)),
        Page::Atualizar | Page::LiberarEspaco | Page::Aplicando => None,
    }
}
