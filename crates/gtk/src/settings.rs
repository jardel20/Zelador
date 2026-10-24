//! Preferências do usuário, em `$XDG_CONFIG_HOME/zelador/config.ini`.

use std::path::PathBuf;

use gtk::glib;

const GROUP: &str = "appearance";
const KEY_SYSTEM_THEME: &str = "use-system-theme";
const GROUP_DIFF: &str = "diff";
const KEY_PREFER_NVD: &str = "prefer-nvd";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settings {
    /// `false` (padrão): identidade visual do Zelador. `true`: segue o tema do usuário.
    pub use_system_theme: bool,
    /// Compara sistemas com o `nvd` quando ele estiver instalado (senão, `nix store diff-closures`).
    pub prefer_nvd: bool,
}

fn path() -> PathBuf {
    glib::user_config_dir().join("zelador").join("config.ini")
}

impl Settings {
    pub fn load() -> Self {
        let key_file = glib::KeyFile::new();
        if key_file
            .load_from_file(path(), glib::KeyFileFlags::NONE)
            .is_err()
        {
            return Self::default();
        }
        Self {
            use_system_theme: key_file.boolean(GROUP, KEY_SYSTEM_THEME).unwrap_or(false),
            prefer_nvd: key_file
                .boolean(GROUP_DIFF, KEY_PREFER_NVD)
                .unwrap_or(false),
        }
    }

    /// Falhas ao gravar só viram aviso: a preferência continua valendo nesta sessão.
    pub fn save(&self) {
        let key_file = glib::KeyFile::new();
        key_file.set_boolean(GROUP, KEY_SYSTEM_THEME, self.use_system_theme);
        key_file.set_boolean(GROUP_DIFF, KEY_PREFER_NVD, self.prefer_nvd);
        let path = path();
        if let Some(dir) = path.parent()
            && let Err(error) = std::fs::create_dir_all(dir)
        {
            tracing::warn!(%error, "não foi possível criar a pasta de configuração");
            return;
        }
        if let Err(error) = key_file.save_to_file(&path) {
            tracing::warn!(%error, "não foi possível gravar as preferências");
        }
    }
}
