//! Barras inferiores fixas das telas de fluxo: brancas, com borda superior.
//!
//! Todas são montadas pela própria tela, porque dependem do estado dela (a de "Atualizar"
//! muda por etapa, a de "Liberar espaço" mostra o total estimado, a de "Gerações" aparece com
//! seleção). Aqui ficam só as peças comuns.

use gtk::prelude::*;

/// A moldura da barra: horizontal, com a classe que dá fundo e borda.
pub fn shell() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["zelador-bottom-bar"])
        .build()
}

/// Espaço elástico entre os grupos de botões.
pub fn spacer() -> gtk::Widget {
    gtk::Box::builder().hexpand(true).build().upcast()
}
