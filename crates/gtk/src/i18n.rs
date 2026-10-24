//! Textos da interface, prontos para gettext.
//!
//! Os `msgid` são os próprios textos em pt-BR. Sem catálogo instalado para o idioma do
//! usuário, o gettext devolve o `msgid`, que já é o texto certo.
//!
//! - [`tr`]: texto fixo.
//! - [`trf`]: texto com valores, com marcadores numerados (`{0}`, `{1}`), que o tradutor pode
//!   reordenar.
//! - [`tn`]: plural, com `{0}` para o número.

pub const DOMAIN: &str = "zelador";

/// Pasta dos catálogos de reserva: a definida no build ou a do sistema.
const LOCALE_DIR: &str = match option_env!("ZELADOR_LOCALEDIR") {
    Some(dir) => dir,
    None => "/run/current-system/sw/share/locale",
};

/// Liga o domínio `zelador` à pasta dos catálogos. Chame uma vez, na abertura do app.
pub fn init() {
    // O wrapper do pacote aponta `ZELADOR_LOCALEDIR` para os catálogos instalados.
    let dir = std::env::var_os("ZELADOR_LOCALEDIR").unwrap_or_else(|| LOCALE_DIR.into());
    if let Err(error) = gettextrs::bindtextdomain(DOMAIN, dir) {
        tracing::warn!(%error, "não foi possível ligar o catálogo de traduções");
    }
    if let Err(error) = gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8") {
        tracing::warn!(%error, "não foi possível definir a codificação do catálogo");
    }
}

pub fn tr(msgid: &str) -> String {
    gtk::glib::dgettext(Some(DOMAIN), msgid).to_string()
}

/// `trf("Geração {0}, aplicada {1}.", &[n.to_string(), quando])`.
pub fn trf(msgid: &str, args: &[String]) -> String {
    fill(&tr(msgid), args)
}

/// `tn("{0} commit", "{0} commits", n)`: o plural certo para `n` no idioma ativo.
pub fn tn(singular: &str, plural: &str, n: u64) -> String {
    let text = gtk::glib::dngettext(
        Some(DOMAIN),
        singular,
        plural,
        std::ffi::c_ulong::try_from(n).unwrap_or(std::ffi::c_ulong::MAX),
    );
    fill(&text, &[n.to_string()])
}

/// Troca `{0}`, `{1}`... pelos argumentos. Marcador sem argumento fica como está.
fn fill(template: &str, args: &[String]) -> String {
    let mut out = template.to_owned();
    for (index, arg) in args.iter().enumerate() {
        out = out.replace(&format!("{{{index}}}"), arg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marcadores_numerados_podem_ser_reordenados() {
        let args = ["214".to_owned(), "3 dias".to_owned()];
        assert_eq!(
            fill("Geração {0}, aplicada {1}.", &args),
            "Geração 214, aplicada 3 dias."
        );
        // Um idioma que inverte a ordem usa os mesmos números.
        assert_eq!(
            fill("Applied {1}: generation {0}", &args),
            "Applied 3 dias: generation 214"
        );
        assert_eq!(fill("{0} e {9}", &args), "214 e {9}");
        assert_eq!(fill("sem marcador", &args), "sem marcador");
    }

    #[test]
    fn plural_sem_catalogo_usa_o_msgid_certo() {
        assert_eq!(tn("{0} commit", "{0} commits", 1), "1 commit");
        assert_eq!(tn("{0} commit", "{0} commits", 0), "0 commits");
        assert_eq!(tn("{0} commit", "{0} commits", 3), "3 commits");
    }
}
