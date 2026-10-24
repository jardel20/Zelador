//! Núcleo do Zelador: executa ferramentas do Nix, interpreta a saída e calcula estimativas.
//! Não depende de GTK.

pub mod apply;
pub mod backends;
pub mod build;
pub mod check;
pub mod cleanup;
pub mod diff;
pub mod disk;
pub mod error;
pub mod failure;
pub mod flake;
pub mod flatpak;
pub mod gcroots;
pub mod generations;
pub mod git;
pub mod history;
pub mod nvd;
pub mod precheck;
pub mod progress;
pub mod runner;
pub mod space;
pub mod status;
pub mod store;
pub mod units;
pub mod update;

pub use error::{Error, Result};
