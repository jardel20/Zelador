//! Ponto de entrada. Três modos, todos sem confiar em nada que a UI diga:
//!
//! - `--serve`: um processo por conexão, iniciado como root pelo systemd (socket activation).
//!   Lê o pedido do socket, confere o polkit e executa.
//! - `--call <subcomando...>`: cliente sem privilégio, que conversa com o socket.
//! - `<subcomando...>`: direto, só como root (diagnóstico; ex.: `sudo zelador-helper gc`).

use std::ffi::OsString;
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use zelador_helper::error::exit;
use zelador_helper::exec::{AnnouncingExecutor, Failure, ProcessExecutor, run_plan};
use zelador_helper::lock::{LOCK_PATH, Lock, LockError};
use zelador_helper::plan::validate;
use zelador_helper::request::parse_args;
use zelador_helper::roots::{Roots, Tools};
use zelador_helper::serve::{
    Authorization, EXIT_PREFIX, PKCHECK, Peer, SOCKET_PATH, authorize, call, peer_of, read_request,
};

/// Escreve no stderr sem entrar em pânico: se o cliente fechou o socket, a operação em
/// andamento ainda precisa terminar.
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

fn main() -> ExitCode {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|arg| arg.to_str()) {
        Some("--serve") => serve(),
        Some("--call") => {
            args.remove(0);
            ExitCode::from(call_mode(&args))
        }
        _ => ExitCode::from(handle(args, None)),
    }
}

fn call_mode(args: &[OsString]) -> u8 {
    match call(Path::new(SOCKET_PATH), args, &mut std::io::stderr()) {
        Ok(code) => code,
        Err(error) => {
            say!("zelador-helper: o serviço não respondeu em {SOCKET_PATH}: {error}");
            exit::INTERNAL
        }
    }
}

fn serve() -> ExitCode {
    let code = serve_inner();
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{EXIT_PREFIX}{code}");
    let _ = stdout.flush();
    ExitCode::from(code)
}

fn serve_inner() -> u8 {
    if !rustix::process::geteuid().is_root() {
        say!("zelador-helper: --serve precisa rodar como root, pelo systemd");
        return exit::INTERNAL;
    }
    let stdin = rustix::stdio::stdin();
    let peer = match peer_of(stdin) {
        Ok(peer) => peer,
        Err(error) => {
            say!("zelador-helper: a entrada não é um socket do systemd: {error}");
            return exit::INTERNAL;
        }
    };
    let stream = match stdin.try_clone_to_owned().map(UnixStream::from) {
        Ok(stream) => stream,
        Err(error) => {
            say!("zelador-helper: não foi possível usar o socket: {error}");
            return exit::INTERNAL;
        }
    };
    // Um cliente que conecta e não escreve não pode prender o serviço.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let args = match read_request(&mut BufReader::new(&stream)) {
        Ok(args) => args,
        Err(error) => {
            say!("zelador-helper: recusado: {error}");
            return exit::USAGE_OR_REJECTED;
        }
    };
    let _ = stream.set_read_timeout(None);
    handle(args, Some(peer))
}

/// Valida, autoriza (só pelo socket), trava e executa. Devolve o código de saída.
fn handle(args: Vec<OsString>, peer: Option<Peer>) -> u8 {
    let request = match parse_args(args) {
        Ok(request) => request,
        Err(rejection) => return reject(&rejection.to_string()),
    };
    let (roots, tools) = (Roots::production(), Tools::production());

    // Recusa cedo, sem privilégio, sem trava e sem pedir senha, tudo o que já se sabe inválido.
    if let Err(rejection) = validate(&request, &roots, &tools) {
        return reject(&rejection.to_string());
    }

    if !rustix::process::geteuid().is_root() {
        say!("zelador-helper: precisa rodar como root (use o serviço do systemd)");
        return exit::INTERNAL;
    }
    if let Some(peer) = peer
        && let Authorization::Denied(reason) = authorize(Path::new(PKCHECK), peer)
    {
        say!("zelador-helper: não autorizado: {reason}");
        return exit::DENIED;
    }
    let _lock = match Lock::acquire(Path::new(LOCK_PATH)) {
        Ok(lock) => lock,
        Err(LockError::Busy) => {
            say!("zelador-helper: outra operação está em andamento");
            return exit::BUSY;
        }
        Err(LockError::Io(error)) => {
            say!("zelador-helper: não foi possível travar {LOCK_PATH}: {error}");
            return exit::INTERNAL;
        }
    };

    // Valida de novo já com a trava: o estado pode ter mudado enquanto esperava.
    let plan = match validate(&request, &roots, &tools) {
        Ok(plan) => plan,
        Err(rejection) => return reject(&rejection.to_string()),
    };
    let mut executor =
        AnnouncingExecutor::new(ProcessExecutor, std::io::stderr(), plan.steps.len());
    match run_plan(&plan, &mut executor) {
        Ok(()) => 0,
        Err(Failure::Exit { step, code }) => {
            say!(
                "zelador-helper: passo {} terminou com código {code}",
                step + 1
            );
            u8::try_from(code).unwrap_or(1).max(1)
        }
        Err(Failure::Spawn { step, message }) => {
            say!("zelador-helper: passo {} não iniciou: {message}", step + 1);
            exit::INTERNAL
        }
    }
}

fn reject(message: &str) -> u8 {
    say!("zelador-helper: recusado: {message}");
    exit::USAGE_OR_REJECTED
}
