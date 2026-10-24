//! Interpretação sintática do `argv`. Não toca no sistema de arquivos.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::Rejection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Switch,
    Boot,
    Test,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Switch => "switch",
            Mode::Boot => "boot",
            Mode::Test => "test",
        }
    }

    fn parse(text: &str) -> Result<Self, Rejection> {
        match text {
            "switch" => Ok(Mode::Switch),
            "boot" => Ok(Mode::Boot),
            "test" => Ok(Mode::Test),
            other => Err(Rejection::InvalidMode(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Activate { path: PathBuf, mode: Mode },
    Rollback(u64),
    SetBootDefault(u64),
    DeleteGenerations(Vec<u64>),
    Gc,
    Optimise,
    FlatpakRemoveUnused,
}

/// `args` não inclui o nome do programa.
pub fn parse_args<I>(args: I) -> Result<Request, Rejection>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let sub = args.next().ok_or(Rejection::Usage)?;
    let sub = sub.to_str().ok_or(Rejection::NotUtf8)?.to_owned();
    let rest: Vec<OsString> = args.collect();

    match sub.as_str() {
        "activate" => {
            let [path, mode] = exactly::<2>("activate", rest)?;
            let mode = Mode::parse(path_str(&mode)?)?;
            Ok(Request::Activate {
                path: PathBuf::from(path),
                mode,
            })
        }
        "rollback" => {
            let [n] = exactly::<1>("rollback", rest)?;
            Ok(Request::Rollback(generation(path_str(&n)?)?))
        }
        "set-boot-default" => {
            let [n] = exactly::<1>("set-boot-default", rest)?;
            Ok(Request::SetBootDefault(generation(path_str(&n)?)?))
        }
        "delete-generations" => {
            if rest.is_empty() {
                return Err(Rejection::WrongArgCount("delete-generations"));
            }
            let mut numbers = Vec::with_capacity(rest.len());
            for arg in &rest {
                let n = generation(path_str(arg)?)?;
                if numbers.contains(&n) {
                    return Err(Rejection::DuplicateGeneration(n));
                }
                numbers.push(n);
            }
            Ok(Request::DeleteGenerations(numbers))
        }
        "gc" => no_args("gc", rest).map(|()| Request::Gc),
        "optimise" => no_args("optimise", rest).map(|()| Request::Optimise),
        "flatpak-remove-unused" => {
            no_args("flatpak-remove-unused", rest).map(|()| Request::FlatpakRemoveUnused)
        }
        _ => Err(Rejection::UnknownSubcommand(sub)),
    }
}

fn exactly<const N: usize>(
    name: &'static str,
    args: Vec<OsString>,
) -> Result<[OsString; N], Rejection> {
    args.try_into().map_err(|_| Rejection::WrongArgCount(name))
}

fn no_args(name: &'static str, args: Vec<OsString>) -> Result<(), Rejection> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(Rejection::WrongArgCount(name))
    }
}

fn path_str(arg: &OsString) -> Result<&str, Rejection> {
    arg.to_str().ok_or(Rejection::NotUtf8)
}

/// Só dígitos, sem zeros à esquerda, sem sinal e maior que zero.
fn generation(text: &str) -> Result<u64, Rejection> {
    let invalid = || Rejection::InvalidGeneration(text.to_owned());
    if text.is_empty()
        || text.len() > 18
        || text.starts_with('0')
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    text.parse().map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Request, Rejection> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn pedidos_validos() {
        assert_eq!(
            parse(&["activate", "/nix/store/x-y", "boot"]),
            Ok(Request::Activate {
                path: "/nix/store/x-y".into(),
                mode: Mode::Boot
            })
        );
        assert_eq!(parse(&["rollback", "12"]), Ok(Request::Rollback(12)));
        assert_eq!(
            parse(&["delete-generations", "3", "1"]),
            Ok(Request::DeleteGenerations(vec![3, 1]))
        );
        assert_eq!(parse(&["gc"]), Ok(Request::Gc));
    }

    #[test]
    fn rejeicoes_sintaticas() {
        assert_eq!(parse(&[]), Err(Rejection::Usage));
        assert!(matches!(
            parse(&["rm"]),
            Err(Rejection::UnknownSubcommand(_))
        ));
        assert!(matches!(
            parse(&["gc", "--max-freed", "1"]),
            Err(Rejection::WrongArgCount(_))
        ));
        assert!(matches!(
            parse(&["activate", "/x"]),
            Err(Rejection::WrongArgCount(_))
        ));
        assert!(matches!(
            parse(&["activate", "/x", "boot", "extra"]),
            Err(Rejection::WrongArgCount(_))
        ));
        assert!(matches!(
            parse(&["activate", "/x", "reboot"]),
            Err(Rejection::InvalidMode(_))
        ));
        assert!(matches!(
            parse(&["delete-generations"]),
            Err(Rejection::WrongArgCount(_))
        ));
        assert_eq!(
            parse(&["delete-generations", "2", "2"]),
            Err(Rejection::DuplicateGeneration(2))
        );
    }

    #[test]
    fn numeros_de_geracao() {
        for bad in [
            "",
            "0",
            "01",
            "-1",
            "+1",
            "1 2",
            "1;ls",
            "--all",
            "99999999999999999999",
            "٣",
        ] {
            assert!(
                matches!(
                    parse(&["rollback", bad]),
                    Err(Rejection::InvalidGeneration(_))
                ),
                "{bad:?} deveria ser recusado"
            );
        }
    }

    #[test]
    fn argumentos_nao_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![0xff, 0xfe]);
        assert_eq!(
            parse_args([OsString::from("rollback"), bad]),
            Err(Rejection::NotUtf8)
        );
    }
}
