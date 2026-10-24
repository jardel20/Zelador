//! Núcleo do Zelador: executa ferramentas do Nix, interpreta a saída e calcula estimativas.
//! Não depende de GTK.

pub mod backends;
pub mod diff;
pub mod error;
pub mod flake;
pub mod flatpak;
pub mod gcroots;
pub mod generations;
pub mod git;
pub mod progress;
pub mod runner;
pub mod status;
pub mod store;
pub mod units;

pub use error::{Error, Result};
