//! Espaço livre no disco onde fica o Nix store.

use std::path::Path;

use crate::error::{Error, Result};

/// Bytes livres para usuários sem privilégio no sistema de arquivos de `path`.
pub fn free_bytes(path: &Path) -> Result<u64> {
    let stat = rustix::fs::statvfs(path).map_err(|e| Error::io(path, e.into()))?;
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

/// Razão entre o que o disco ocupa (`used`) e o tamanho lógico (soma dos `narSize`).
///
/// O `narSize` ignora arquivos compartilhados por hardlink, então a soma lógica pode passar do
/// tamanho do próprio disco. `used` inclui o que não é store, logo a razão ainda é um limite
/// superior; só impede números impossíveis. Vale 1 quando a soma lógica cabe no disco.
pub fn real_ratio(logical_total: u64, used: u64) -> f64 {
    if logical_total == 0 || logical_total <= used {
        1.0
    } else {
        used as f64 / logical_total as f64
    }
}

/// Aplica `ratio` a um tamanho lógico.
pub fn scale_bytes(bytes: u64, ratio: f64) -> u64 {
    (bytes as f64 * ratio.clamp(0.0, 1.0)) as u64
}

/// Bytes ocupados no sistema de arquivos de `path` (tudo o que não está livre).
pub fn used_bytes(path: &Path) -> Result<u64> {
    let stat = rustix::fs::statvfs(path).map_err(|e| Error::io(path, e.into()))?;
    Ok(stat
        .f_blocks
        .saturating_sub(stat.f_bfree)
        .saturating_mul(stat.f_frsize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disco_atual_tem_bytes_ocupados() {
        assert!(used_bytes(Path::new("/")).unwrap() > 0);
    }

    #[test]
    fn disco_atual_tem_espaco_e_caminho_inexistente_falha() {
        assert!(free_bytes(Path::new("/")).unwrap() > 0);
        assert!(free_bytes(Path::new("/caminho/que/nao/existe")).is_err());
    }
}
