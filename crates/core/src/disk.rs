//! Espaço livre no disco onde fica o Nix store.

use std::path::Path;

use crate::error::{Error, Result};

/// Bytes livres para usuários sem privilégio no sistema de arquivos de `path`.
pub fn free_bytes(path: &Path) -> Result<u64> {
    let stat = rustix::fs::statvfs(path).map_err(|e| Error::io(path, e.into()))?;
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disco_atual_tem_espaco_e_caminho_inexistente_falha() {
        assert!(free_bytes(Path::new("/")).unwrap() > 0);
        assert!(free_bytes(Path::new("/caminho/que/nao/existe")).is_err());
    }
}
