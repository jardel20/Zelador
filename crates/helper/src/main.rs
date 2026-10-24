//! Ponto de entrada. Executado como root via `pkexec`; não confia em nada que a UI diga.

use std::path::Path;
use std::process::ExitCode;

use zelador_helper::error::exit;
use zelador_helper::exec::{Failure, ProcessExecutor, run_plan};
use zelador_helper::lock::{LOCK_PATH, Lock, LockError};
use zelador_helper::plan::validate;
use zelador_helper::request::parse_args;
use zelador_helper::roots::{Roots, Tools};

fn main() -> ExitCode {
    let request = match parse_args(std::env::args_os().skip(1)) {
        Ok(request) => request,
        Err(rejection) => return reject(&rejection.to_string()),
    };
    let (roots, tools) = (Roots::production(), Tools::production());

    // Recusa cedo, sem privilégio nem trava, tudo o que já se sabe inválido.
    if let Err(rejection) = validate(&request, &roots, &tools) {
        return reject(&rejection.to_string());
    }

    if !rustix::process::geteuid().is_root() {
        eprintln!("zelador-helper: precisa rodar como root (use pkexec)");
        return ExitCode::from(exit::INTERNAL);
    }
    let _lock = match Lock::acquire(Path::new(LOCK_PATH)) {
        Ok(lock) => lock,
        Err(LockError::Busy) => {
            eprintln!("zelador-helper: outra operação está em andamento");
            return ExitCode::from(exit::BUSY);
        }
        Err(LockError::Io(error)) => {
            eprintln!("zelador-helper: não foi possível travar {LOCK_PATH}: {error}");
            return ExitCode::from(exit::INTERNAL);
        }
    };

    // Valida de novo já com a trava: o estado pode ter mudado enquanto esperava.
    let plan = match validate(&request, &roots, &tools) {
        Ok(plan) => plan,
        Err(rejection) => return reject(&rejection.to_string()),
    };
    match run_plan(&plan, &mut ProcessExecutor) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Exit { step, code }) => {
            eprintln!(
                "zelador-helper: passo {} terminou com código {code}",
                step + 1
            );
            ExitCode::from(u8::try_from(code).unwrap_or(1).max(1))
        }
        Err(Failure::Spawn { step, message }) => {
            eprintln!("zelador-helper: passo {} não iniciou: {message}", step + 1);
            ExitCode::from(exit::INTERNAL)
        }
    }
}

fn reject(message: &str) -> ExitCode {
    eprintln!("zelador-helper: recusado: {message}");
    ExitCode::from(exit::USAGE_OR_REJECTED)
}
