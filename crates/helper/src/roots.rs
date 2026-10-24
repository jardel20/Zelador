//! Locais e ferramentas em que o helper confia.
//!
//! Em produção são constantes (e caminhos do store fixados no build). Os testes montam
//! `Roots` e `Tools` apontando para diretórios temporários; nada disso vem do ambiente.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Roots {
    /// Raiz do Nix store (`/nix/store`).
    pub store: PathBuf,
    /// Pasta dos perfis (`/nix/var/nix/profiles`).
    pub profiles: PathBuf,
    /// Link do sistema em execução (`/run/current-system`).
    pub current_system: PathBuf,
    /// Repositório da instalação Flatpak de sistema.
    pub flatpak_repo: PathBuf,
}

impl Roots {
    pub fn production() -> Self {
        Self {
            store: "/nix/store".into(),
            profiles: "/nix/var/nix/profiles".into(),
            current_system: "/run/current-system".into(),
            flatpak_repo: "/var/lib/flatpak/repo".into(),
        }
    }

    /// Link `system` do perfil: para onde o bootloader aponta por padrão.
    pub fn system_profile(&self) -> PathBuf {
        self.profiles.join("system")
    }

    pub fn generation_link(&self, number: u64) -> PathBuf {
        self.profiles.join(format!("system-{number}-link"))
    }
}

/// Caminhos absolutos das ferramentas executadas como root.
#[derive(Debug, Clone)]
pub struct Tools {
    pub nix_env: PathBuf,
    pub nix_store: PathBuf,
    pub systemd_run: PathBuf,
    pub flatpak: PathBuf,
}

/// Caminho fixado no build (`ZELADOR_<NOME>`), ou o do sistema como reserva para
/// desenvolvimento. O pacote Nix sempre define todos.
macro_rules! tool {
    ($var:literal, $fallback:literal) => {
        match option_env!($var) {
            Some(path) => path,
            None => $fallback,
        }
    };
}

impl Tools {
    pub fn production() -> Self {
        Self {
            nix_env: tool!("ZELADOR_NIX_ENV", "/run/current-system/sw/bin/nix-env").into(),
            nix_store: tool!("ZELADOR_NIX_STORE", "/run/current-system/sw/bin/nix-store").into(),
            systemd_run: tool!(
                "ZELADOR_SYSTEMD_RUN",
                "/run/current-system/sw/bin/systemd-run"
            )
            .into(),
            flatpak: tool!("ZELADOR_FLATPAK", "/run/current-system/sw/bin/flatpak").into(),
        }
    }
}
