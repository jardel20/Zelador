//! As telas do app e seus textos fixos (pt-BR).

use crate::i18n::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    SeuSistema,
    Atualizar,
    Geracoes,
    LiberarEspaco,
    EspacoEmDisco,
    /// Sem item na navegação: abre a partir de "Atualizar".
    Aplicando,
}

impl Page {
    /// Itens da barra lateral, na ordem dos mockups.
    pub const NAV: [Page; 5] = [
        Page::SeuSistema,
        Page::Atualizar,
        Page::Geracoes,
        Page::LiberarEspaco,
        Page::EspacoEmDisco,
    ];

    pub const ALL: [Page; 6] = [
        Page::SeuSistema,
        Page::Atualizar,
        Page::Geracoes,
        Page::LiberarEspaco,
        Page::EspacoEmDisco,
        Page::Aplicando,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Page::SeuSistema => "seu-sistema",
            Page::Atualizar => "atualizar",
            Page::Geracoes => "geracoes",
            Page::LiberarEspaco => "liberar-espaco",
            Page::EspacoEmDisco => "espaco-em-disco",
            Page::Aplicando => "aplicando",
        }
    }

    pub fn from_id(id: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|page| page.id() == id)
    }

    /// Rótulo na navegação.
    pub fn nav_label(self) -> String {
        match self {
            Page::SeuSistema => tr("Seu sistema"),
            Page::Atualizar => tr("Atualizar"),
            Page::Geracoes => tr("Gerações"),
            Page::LiberarEspaco => tr("Liberar espaço"),
            Page::EspacoEmDisco => tr("Espaço em disco"),
            Page::Aplicando => tr("Aplicando"),
        }
    }

    /// Ícone Phosphor (`<nome>-symbolic`) da navegação.
    pub fn nav_icon(self) -> &'static str {
        match self {
            Page::SeuSistema => "pulse-symbolic",
            Page::Atualizar | Page::Aplicando => "arrows-clockwise-symbolic",
            Page::Geracoes => "stack-symbolic",
            Page::LiberarEspaco => "broom-symbolic",
            Page::EspacoEmDisco => "hard-drives-symbolic",
        }
    }

    pub fn title(self) -> String {
        match self {
            Page::Atualizar => tr("Aplicar atualizações?"),
            Page::Aplicando => tr("Aplicando a atualização"),
            other => other.nav_label(),
        }
    }

    /// Parágrafo de abertura enquanto a tela ainda não tem dados.
    pub fn intro(self) -> String {
        match self {
            Page::SeuSistema => tr("Reunindo as informações da sua configuração."),
            Page::Atualizar => tr("O build terminou sem erros. Nada mudou no sistema ainda."),
            Page::Aplicando => {
                tr("Você pode continuar usando o computador. Não desligue até terminar.")
            }
            Page::Geracoes => tr(
                "Cada configuração aplicada se torna uma geração. É possível voltar a qualquer uma delas.",
            ),
            Page::LiberarEspaco => {
                tr("Escolha o que remover. Use Simular para ver o resultado antes de apagar.")
            }
            Page::EspacoEmDisco => tr("Veja o que ocupa espaço no Nix store e fora dele."),
        }
    }

    /// Largura máxima do conteúdo, em px (mockups: 820 nas telas de leitura, 720 nas de fluxo).
    pub fn max_width(self) -> i32 {
        match self {
            Page::SeuSistema | Page::Geracoes | Page::EspacoEmDisco => 820,
            Page::Atualizar | Page::Aplicando | Page::LiberarEspaco => 720,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_sao_unicos_e_fazem_ida_e_volta() {
        for page in Page::ALL {
            assert_eq!(Page::from_id(page.id()), Some(page));
        }
        assert_eq!(Page::from_id("nada"), None);
    }

    #[test]
    fn navegacao_segue_os_mockups() {
        let ids: Vec<_> = Page::NAV.iter().map(|p| p.id()).collect();
        assert_eq!(
            ids,
            [
                "seu-sistema",
                "atualizar",
                "geracoes",
                "liberar-espaco",
                "espaco-em-disco"
            ]
        );
    }
}
