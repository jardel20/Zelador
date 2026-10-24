//! Conteúdo das telas que já têm dados (Fase 4: somente leitura).

mod aplicando;
mod atualizar;
mod espaco;
mod geracoes;
mod liberar;
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
    title: &gtk::Label,
    intro: &gtk::Label,
    step_label: Option<&gtk::Label>,
    view: &adw::ToolbarView,
) -> Option<Screen> {
    match page {
        Page::SeuSistema => Some(seu_sistema::build(ctx, intro)),
        Page::Geracoes => Some(geracoes::build(ctx, intro, view)),
        Page::EspacoEmDisco => Some(espaco::build(ctx, intro)),
        Page::LiberarEspaco => Some(liberar::build(ctx, intro, view)),
        Page::Atualizar => step_label.map(|label| atualizar::build(ctx, title, intro, label, view)),
        Page::Aplicando => step_label.map(|label| aplicando::build(ctx, intro, label, view)),
    }
}
