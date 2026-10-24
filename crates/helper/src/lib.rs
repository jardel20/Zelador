//! Helper privilegiado do Zelador, ativado pelo systemd por socket (sem setuid).
//!
//! A biblioteca separa três etapas, para que a validação seja testável sem root:
//! 1. [`request::parse_args`] transforma o `argv` em um [`request::Request`] (só sintaxe);
//! 2. [`plan::validate`] confere o pedido contra o sistema de arquivos e devolve um
//!    [`plan::Plan`] com os comandos exatos, sem executar nada;
//! 3. [`exec::run_plan`] executa o plano por um [`exec::Executor`].
//!
//! O conjunto de subcomandos é fechado e nenhum caminho ou ferramenta vem do ambiente.

pub mod error;
pub mod exec;
pub mod lock;
pub mod plan;
pub mod request;
pub mod roots;
pub mod serve;

pub use error::Rejection;
