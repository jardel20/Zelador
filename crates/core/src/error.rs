use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("não foi possível executar `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{command}` terminou com código {code:?}: {stderr}")]
    Command {
        command: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("saída inesperada de {what}: {detail}")]
    Parse { what: &'static str, detail: String },
    #[error("JSON inválido em {what}: {source}")]
    Json {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("erro de E/S em {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    pub(crate) fn parse(what: &'static str, detail: impl Into<String>) -> Self {
        Error::Parse {
            what,
            detail: detail.into(),
        }
    }

    pub(crate) fn json(what: &'static str, source: serde_json::Error) -> Self {
        Error::Json { what, source }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}
