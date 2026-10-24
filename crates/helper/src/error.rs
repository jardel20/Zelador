use std::path::PathBuf;

/// Códigos de saída do helper. Falha de um comando filho devolve o código dele.
pub mod exit {
    pub const USAGE_OR_REJECTED: u8 = 65;
    pub const INTERNAL: u8 = 70;
    pub const BUSY: u8 = 75;
}

/// Motivo pelo qual um pedido foi recusado antes de executar qualquer coisa.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Rejection {
    #[error("uso: zelador-helper <subcomando> [argumentos]; veja a documentação")]
    Usage,
    #[error("subcomando desconhecido: {0}")]
    UnknownSubcommand(String),
    #[error("argumentos a mais ou a menos para `{0}`")]
    WrongArgCount(&'static str),
    #[error("argumento não é UTF-8 válido")]
    NotUtf8,
    #[error("modo inválido `{0}`: use switch, boot ou test")]
    InvalidMode(String),
    #[error("número de geração inválido `{0}`")]
    InvalidGeneration(String),
    #[error("geração {0} repetida")]
    DuplicateGeneration(u64),
    #[error("o caminho precisa ser absoluto: {}", .0.display())]
    NotAbsolute(PathBuf),
    #[error("o caminho precisa estar diretamente em {}: {}", .store.display(), .path.display())]
    OutsideStore { path: PathBuf, store: PathBuf },
    #[error("o caminho não existe ou não pode ser lido: {}", .0.display())]
    NotFound(PathBuf),
    #[error("o caminho é um link simbólico ou resolve para outro lugar: {}", .0.display())]
    Symlink(PathBuf),
    #[error("o caminho não é um diretório: {}", .0.display())]
    NotDirectory(PathBuf),
    #[error("não é um sistema NixOS (sem bin/switch-to-configuration executável): {}", .0.display())]
    NotASystem(PathBuf),
    #[error("a geração {0} não existe")]
    GenerationMissing(u64),
    #[error("a geração {0} está em uso ou é o padrão do boot e não pode ser excluída")]
    GenerationProtected(u64),
    #[error("não foi possível determinar a geração em uso: {}", .0.display())]
    CurrentSystemUnknown(PathBuf),
    #[error("não há instalação Flatpak de sistema")]
    NoSystemFlatpak,
}
