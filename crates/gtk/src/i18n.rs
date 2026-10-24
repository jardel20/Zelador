//! Textos da interface, prontos para gettext. O domínio `zelador` ainda não tem catálogos:
//! sem tradução instalada, `dgettext` devolve o próprio texto em pt-BR.

pub const DOMAIN: &str = "zelador";

pub fn tr(msgid: &str) -> String {
    gtk::glib::dgettext(Some(DOMAIN), msgid).to_string()
}
