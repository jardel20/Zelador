//! Transporte do helper sem setuid: o systemd escuta em um socket Unix e inicia um
//! `zelador-helper --serve` como root por conexão; `zelador-helper --call` é o cliente sem
//! privilégio. O pedido é o mesmo `argv` de sempre, e cada conexão passa pelo polkit.
//!
//! Protocolo: o cliente envia cada argumento seguido de um byte NUL e termina com um NUL extra
//! (argumento vazio). O servidor responde com a saída do helper (stdout e stderr misturados) e,
//! por último, a linha `@zelador-exit <código>`.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::BorrowedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::exit;

pub const SOCKET_PATH: &str = "/run/zelador/helper.sock";
/// Action do polkit que autoriza cada operação.
pub const ACTION_ID: &str = "br.dev.zelador.helper";
pub const EXIT_PREFIX: &str = "@zelador-exit ";

const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_ARGS: usize = 256;

/// `pkcheck` do polkit, fixado no build como as demais ferramentas.
pub const PKCHECK: &str = match option_env!("ZELADOR_PKCHECK") {
    Some(path) => path,
    None => "/run/current-system/sw/bin/pkcheck",
};

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("pedido grande demais")]
    TooLarge,
    #[error("pedido incompleto")]
    Truncated,
    #[error("falha ao ler o pedido: {0}")]
    Io(#[from] io::Error),
}

pub fn write_request(out: &mut impl Write, args: &[OsString]) -> io::Result<()> {
    for arg in args {
        out.write_all(arg.as_bytes())?;
        out.write_all(&[0])?;
    }
    out.write_all(&[0])?;
    out.flush()
}

/// Lê um pedido de no máximo 16 KiB e 256 argumentos. Um argumento vazio encerra o pedido,
/// então um argumento realmente vazio não existe nesse protocolo (o helper também o recusaria).
pub fn read_request(input: &mut impl BufRead) -> Result<Vec<OsString>, RequestError> {
    let (mut args, mut total, mut buffer) = (Vec::new(), 0, Vec::new());
    loop {
        buffer.clear();
        let read = input.read_until(0, &mut buffer)?;
        if read == 0 || buffer.last() != Some(&0) {
            return Err(RequestError::Truncated);
        }
        total += read;
        if total > MAX_REQUEST_BYTES || args.len() >= MAX_ARGS {
            return Err(RequestError::TooLarge);
        }
        buffer.pop();
        if buffer.is_empty() {
            return Ok(args);
        }
        args.push(OsString::from_vec(std::mem::take(&mut buffer)));
    }
}

/// Cliente: envia o pedido, repassa a saída a `out` e devolve o código final do helper.
/// Sem a linha final (servidor morto no meio), devolve [`exit::INTERNAL`].
pub fn call(socket: &Path, args: &[OsString], out: &mut impl Write) -> io::Result<u8> {
    let mut stream = UnixStream::connect(socket)?;
    write_request(&mut stream, args)?;
    let mut reader = BufReader::new(stream);
    let (mut line, mut code) = (Vec::new(), exit::INTERNAL);
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(code);
        }
        if let Some(parsed) = std::str::from_utf8(&line)
            .ok()
            .and_then(|text| text.strip_prefix(EXIT_PREFIX))
            .and_then(|rest| rest.trim().parse::<u8>().ok())
        {
            code = parsed;
            continue;
        }
        // Falha ao repassar (cliente fechou a saída) não interrompe a leitura: o helper segue
        // até o fim de qualquer forma.
        let _ = out.write_all(&line).and_then(|()| out.flush());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub pid: i32,
    pub uid: u32,
}

/// Quem está do outro lado do socket, informado pelo kernel (não forjável pelo cliente).
pub fn peer_of(socket: BorrowedFd<'_>) -> io::Result<Peer> {
    let cred = rustix::net::sockopt::socket_peercred(socket)?;
    Ok(Peer {
        pid: cred.pid.as_raw_nonzero().get(),
        uid: cred.uid.as_raw(),
    })
}

/// Campo 22 de `/proc/<pid>/stat` (início do processo), que protege contra reuso de PID.
/// O nome do processo vem entre parênteses e pode ter espaços: conta-se após o último `)`.
pub fn parse_start_time(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(19)?.parse().ok()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Authorization {
    Granted,
    Denied(String),
}

/// Pergunta ao polkit se `peer` pode usar a action, deixando-o pedir a senha ao usuário.
/// A saída do `pkcheck` vai para `stderr` (o socket), para o cliente ver o motivo.
pub fn authorize(pkcheck: &Path, peer: Peer) -> Authorization {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", peer.pid)) else {
        return Authorization::Denied("o cliente já terminou".into());
    };
    let Some(start) = parse_start_time(&stat) else {
        return Authorization::Denied("não foi possível identificar o cliente".into());
    };
    let status = Command::new(pkcheck)
        .args([
            "--action-id",
            ACTION_ID,
            "--allow-user-interaction",
            "--process",
        ])
        .arg(format!("{},{start},{}", peer.pid, peer.uid))
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => Authorization::Granted,
        Ok(status) => Authorization::Denied(format!(
            "o polkit não autorizou (pkcheck terminou com {})",
            status.code().map_or("sinal".into(), |c| c.to_string())
        )),
        Err(error) => Authorization::Denied(format!("não foi possível rodar o pkcheck: {error}")),
    }
}

/// Lê todo o resto de `input` sem guardar nada: só para esvaziar antes de fechar.
pub fn drain(input: &mut impl Read) {
    let _ = io::copy(input, &mut io::sink());
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::io::Cursor;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn pedido_vai_e_volta_igual() {
        let mut wire = Vec::new();
        write_request(
            &mut wire,
            &os(&["clean", "--generations", "1", "2", "--gc"]),
        )
        .unwrap();
        let back = read_request(&mut Cursor::new(wire)).unwrap();
        assert_eq!(back, os(&["clean", "--generations", "1", "2", "--gc"]));
    }

    #[test]
    fn pedido_vazio_e_valido_e_sem_terminador_e_recusado() {
        assert!(read_request(&mut Cursor::new(vec![0])).unwrap().is_empty());
        assert!(matches!(
            read_request(&mut Cursor::new(b"gc".to_vec())),
            Err(RequestError::Truncated)
        ));
        assert!(matches!(
            read_request(&mut Cursor::new(Vec::new())),
            Err(RequestError::Truncated)
        ));
    }

    #[test]
    fn pedido_grande_demais_e_recusado() {
        let big = vec![b'a'; MAX_REQUEST_BYTES + 1];
        let mut wire = big;
        wire.extend([0, 0]);
        assert!(matches!(
            read_request(&mut Cursor::new(wire)),
            Err(RequestError::TooLarge)
        ));
        let mut many = Vec::new();
        for _ in 0..=MAX_ARGS {
            many.extend(b"1\0");
        }
        many.push(0);
        assert!(matches!(
            read_request(&mut Cursor::new(many)),
            Err(RequestError::TooLarge)
        ));
    }

    #[test]
    fn tempo_de_inicio_ignora_espacos_e_parenteses_no_nome() {
        let stat = "123 (a b) c) S 1 123 123 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10 18446744073709551615";
        assert_eq!(parse_start_time(stat), Some(987654));
        assert_eq!(parse_start_time("sem parênteses"), None);
        assert_eq!(parse_start_time("1 (x) S 1"), None);
    }

    #[test]
    fn cliente_repassa_a_saida_e_obedece_a_linha_final() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let args = read_request(&mut reader).unwrap();
            let mut stream = stream;
            writeln!(stream, "@zelador {{\"step\":1}}").unwrap();
            writeln!(stream, "recebi {}", args.len()).unwrap();
            // Vale a última: o servidor escreve a linha final depois de tudo.
            writeln!(stream, "{EXIT_PREFIX}0").unwrap();
            writeln!(stream, "{EXIT_PREFIX}3").unwrap();
        });
        let mut out = Vec::new();
        let code = call(&path, &os(&["gc"]), &mut out).unwrap();
        server.join().unwrap();
        assert_eq!(code, 3, "vale a última linha de saída");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "@zelador {\"step\":1}\nrecebi 1\n"
        );
    }

    #[test]
    fn servidor_que_morre_sem_linha_final_e_erro_interno() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Lê o pedido antes de fechar, senão o kernel responde com RST.
            read_request(&mut BufReader::new(stream.try_clone().unwrap())).unwrap();
            writeln!(stream, "meio").unwrap();
        });
        let mut out = Vec::new();
        let code = call(&path, &os(&["gc"]), &mut out).unwrap();
        server.join().unwrap();
        assert_eq!(code, exit::INTERNAL);
    }

    #[test]
    fn sem_o_socket_o_cliente_falha() {
        let dir = tempfile::tempdir().unwrap();
        assert!(call(&dir.path().join("nada"), &os(&["gc"]), &mut Vec::new()).is_err());
    }
}
