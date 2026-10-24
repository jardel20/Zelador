//! Histórico de operações e avisos já enviados, em SQLite local.
//!
//! O arquivo guarda só tipos, horários, números e mensagens curtas: nada de caminhos pessoais.
//! Falha no histórico nunca deve impedir uma operação (use [`record_best_effort`]).

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::status::StatusEnv;

/// Versão do esquema, guardada em `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 1;
/// Quantas operações ficam guardadas.
pub const RETAIN: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    /// Atualização do sistema (construir e aplicar).
    Update,
    Clean,
    DeleteGenerations,
    Rollback,
    SetBootDefault,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationKind::Update => "update",
            OperationKind::Clean => "clean",
            OperationKind::DeleteGenerations => "delete-generations",
            OperationKind::Rollback => "rollback",
            OperationKind::SetBootDefault => "set-boot-default",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [
            OperationKind::Update,
            OperationKind::Clean,
            OperationKind::DeleteGenerations,
            OperationKind::Rollback,
            OperationKind::SetBootDefault,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
    Cancelled,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Failure => "failure",
            Outcome::Cancelled => "cancelled",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Outcome::Success, Outcome::Failure, Outcome::Cancelled]
            .into_iter()
            .find(|outcome| outcome.as_str() == text)
    }
}

/// Uma operação a gravar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOperation {
    pub kind: OperationKind,
    /// Epoch, em segundos.
    pub started_at: i64,
    pub duration_ms: u64,
    pub outcome: Outcome,
    /// Geração gerada (atualização) ou alvo (voltar, tornar padrão).
    pub generation: Option<u64>,
    pub bytes_freed: Option<u64>,
    /// Texto curto: o motivo de uma falha, ou quantas gerações saíram.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub id: i64,
    pub kind: OperationKind,
    pub started_at: i64,
    pub duration_ms: u64,
    pub outcome: Outcome,
    pub generation: Option<u64>,
    pub bytes_freed: Option<u64>,
    pub detail: Option<String>,
}

pub struct History {
    conn: Connection,
}

/// `$XDG_DATA_HOME/zelador/history.db` (padrão `~/.local/share`).
pub fn default_path(env: &StatusEnv) -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| env.home.join(".local/share"));
    base.join("zelador/history.db")
}

impl History {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        // WAL deixa o app e o timer lerem e escreverem sem se bloquear por muito tempo.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let history = Self { conn };
        history.migrate()?;
        Ok(history)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 1 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS operations (
                     id INTEGER PRIMARY KEY,
                     kind TEXT NOT NULL,
                     started_at INTEGER NOT NULL,
                     duration_ms INTEGER NOT NULL,
                     outcome TEXT NOT NULL,
                     generation INTEGER,
                     bytes_freed INTEGER,
                     detail TEXT
                 );
                 CREATE INDEX IF NOT EXISTS operations_started ON operations(started_at DESC);
                 CREATE TABLE IF NOT EXISTS notices (
                     condition TEXT PRIMARY KEY,
                     notified_at INTEGER NOT NULL
                 );",
            )?;
        }
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    pub fn record(&self, op: &NewOperation) -> Result<i64> {
        let to_i64 = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        self.conn.execute(
            "INSERT INTO operations (kind, started_at, duration_ms, outcome, generation, bytes_freed, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                op.kind.as_str(),
                op.started_at,
                to_i64(op.duration_ms),
                op.outcome.as_str(),
                op.generation.map(to_i64),
                op.bytes_freed.map(to_i64),
                op.detail,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.prune(RETAIN)?;
        Ok(id)
    }

    /// As `limit` operações mais recentes, da mais nova para a mais antiga.
    pub fn recent(&self, limit: usize) -> Result<Vec<Operation>> {
        let mut statement = self.conn.prepare(
            "SELECT id, kind, started_at, duration_ms, outcome, generation, bytes_freed, detail
             FROM operations ORDER BY started_at DESC, id DESC LIMIT ?1",
        )?;
        let rows =
            statement.query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                let from_i64 = |n: Option<i64>| n.and_then(|n| u64::try_from(n).ok());
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    from_i64(row.get(5)?),
                    from_i64(row.get(6)?),
                    row.get::<_, Option<String>>(7)?,
                ))
            })?;
        let mut operations = Vec::new();
        for row in rows {
            let (id, kind, started_at, duration_ms, outcome, generation, bytes_freed, detail) =
                row?;
            // Linha de uma versão futura do app: ignora em vez de quebrar a lista.
            let (Some(kind), Some(outcome)) =
                (OperationKind::parse(&kind), Outcome::parse(&outcome))
            else {
                continue;
            };
            operations.push(Operation {
                id,
                kind,
                started_at,
                duration_ms: u64::try_from(duration_ms).unwrap_or(0),
                outcome,
                generation,
                bytes_freed,
                detail,
            });
        }
        Ok(operations)
    }

    fn prune(&self, keep: usize) -> Result<()> {
        self.conn.execute(
            "DELETE FROM operations WHERE id NOT IN
               (SELECT id FROM operations ORDER BY started_at DESC, id DESC LIMIT ?1)",
            params![i64::try_from(keep).unwrap_or(i64::MAX)],
        )?;
        Ok(())
    }

    /// Quando a condição foi avisada pela última vez (epoch).
    pub fn last_notice(&self, condition: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT notified_at FROM notices WHERE condition = ?1",
                params![condition],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_notice(&self, condition: &str, at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO notices (condition, notified_at) VALUES (?1, ?2)
             ON CONFLICT(condition) DO UPDATE SET notified_at = excluded.notified_at",
            params![condition, at],
        )?;
        Ok(())
    }
}

/// Grava sem nunca falhar para quem chama: o histórico é secundário.
pub fn record_best_effort(env: &StatusEnv, op: &NewOperation) {
    let result = History::open(&default_path(env)).and_then(|history| history.record(op));
    if let Err(error) = result {
        tracing::warn!(%error, "não foi possível gravar no histórico");
    }
}

/// Número da geração do sistema que o perfil `system` aponta agora (`system-N-link` → N).
pub fn current_system_generation(profiles_dir: &Path) -> Option<u64> {
    let target = fs::read_link(profiles_dir.join("system")).ok()?;
    crate::generations::parse_generation_number(target.file_name()?.to_str()?, "system")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: OperationKind, started_at: i64) -> NewOperation {
        NewOperation {
            kind,
            started_at,
            duration_ms: 1500,
            outcome: Outcome::Success,
            generation: None,
            bytes_freed: None,
            detail: None,
        }
    }

    #[test]
    fn grava_e_lista_da_mais_nova_para_a_mais_antiga() {
        let history = History::in_memory().unwrap();
        history
            .record(&NewOperation {
                generation: Some(214),
                ..op(OperationKind::Update, 100)
            })
            .unwrap();
        history
            .record(&NewOperation {
                bytes_freed: Some(6_100_000_000),
                detail: Some("3 gerações".into()),
                ..op(OperationKind::Clean, 200)
            })
            .unwrap();
        history
            .record(&NewOperation {
                outcome: Outcome::Failure,
                detail: Some("erro".into()),
                ..op(OperationKind::Update, 300)
            })
            .unwrap();

        let recent = history.recent(2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].outcome, Outcome::Failure);
        assert_eq!(recent[0].detail.as_deref(), Some("erro"));
        assert_eq!(recent[1].kind, OperationKind::Clean);
        assert_eq!(recent[1].bytes_freed, Some(6_100_000_000));
        assert_eq!(
            history.recent(10).unwrap().last().unwrap().generation,
            Some(214)
        );
    }

    #[test]
    fn guarda_so_as_ultimas_500() {
        let history = History::in_memory().unwrap();
        for i in 0..(RETAIN as i64 + 20) {
            history.record(&op(OperationKind::Clean, i)).unwrap();
        }
        let all = history.recent(10_000).unwrap();
        assert_eq!(all.len(), RETAIN);
        // As mais antigas saíram.
        assert_eq!(all.last().unwrap().started_at, 20);
        assert_eq!(all.first().unwrap().started_at, RETAIN as i64 + 19);
    }

    #[test]
    fn avisos_por_condicao() {
        let history = History::in_memory().unwrap();
        assert_eq!(history.last_notice("disco").unwrap(), None);
        history.set_notice("disco", 10).unwrap();
        history.set_notice("disco", 20).unwrap();
        history.set_notice("inputs", 5).unwrap();
        assert_eq!(history.last_notice("disco").unwrap(), Some(20));
        assert_eq!(history.last_notice("inputs").unwrap(), Some(5));
    }

    #[test]
    fn persiste_no_arquivo_e_reabre_sem_perder_nem_remigrar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zelador/history.db");
        {
            let history = History::open(&path).unwrap();
            history.record(&op(OperationKind::Rollback, 42)).unwrap();
        }
        let reopened = History::open(&path).unwrap();
        assert_eq!(reopened.recent(5).unwrap().len(), 1);
        let version: i64 = reopened
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn linha_de_versao_futura_e_ignorada() {
        let history = History::in_memory().unwrap();
        history.record(&op(OperationKind::Update, 1)).unwrap();
        history
            .conn
            .execute("INSERT INTO operations (kind, started_at, duration_ms, outcome) VALUES ('viagem-no-tempo', 2, 0, 'success')", [])
            .unwrap();
        let recent = history.recent(10).unwrap();
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn geracao_atual_vem_do_link_system() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("system-446-link", dir.path().join("system")).unwrap();
        assert_eq!(current_system_generation(dir.path()), Some(446));
        assert_eq!(current_system_generation(&dir.path().join("nada")), None);
    }

    #[test]
    fn caminho_padrao_respeita_xdg_data_home_ou_cai_no_home() {
        let env = StatusEnv::detect("/etc/nixos".into(), Some("h".into()));
        let path = default_path(&env);
        assert!(path.ends_with("zelador/history.db"));
    }
}
