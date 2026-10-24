//! Trava de instância única: duas operações privilegiadas nunca rodam juntas.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub const LOCK_PATH: &str = "/run/zelador-helper.lock";

#[derive(Debug)]
pub enum LockError {
    Busy,
    Io(io::Error),
}

/// A trava vale enquanto o valor existir.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

impl Lock {
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(LockError::Io)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(LockError::Busy),
            Err(std::fs::TryLockError::Error(error)) => Err(LockError::Io(error)),
        }
    }
}
