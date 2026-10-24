//! Cada ferramenta externa fica atrás de um trait. As implementações `Cli*` montam
//! os argumentos, rodam o processo por um [`CommandRunner`] e passam a saída aos parsers,
//! então os testes trocam só o runner por fixtures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::diff::{DiffEntry, parse_diff_closures, strip_ansi};
use crate::error::{Error, Result};
use crate::flake::{FlakeMetadata, parse_flake_metadata};
use crate::flatpak::{self, FlatpakRef, FlatpakSummary};
use crate::git::{GitStatus, parse_status_v2};
use crate::runner::{CommandRunner, CommandSpec};
use crate::store::{
    PathInfo, parse_dead_paths, parse_gc_dry_run, parse_path_info, parse_path_list,
    parse_path_sizes,
};

/// `nix.gc.automatic` e `nix.optimise.automatic`, lidos da configuração (nunca escritos).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoMaintenance {
    pub gc: bool,
    pub optimise: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PathInfoOptions {
    pub closure_size: bool,
    pub recursive: bool,
}

pub trait NixBackend {
    fn flake_metadata(&self, flake: &Path) -> Result<FlakeMetadata>;
    fn path_info(&self, paths: &[String], options: PathInfoOptions) -> Result<Vec<PathInfo>>;
    /// Soma dos `narSize` de todo o store.
    fn store_total_bytes(&self) -> Result<u64>;
    /// `narSize` de todos os caminhos do store (caro: dezenas de segundos).
    fn path_sizes(&self) -> Result<HashMap<String, u64>>;
    /// Todos os caminhos de que `path` depende, incluindo ele mesmo.
    fn closure_paths(&self, path: &str) -> Result<Vec<String>>;
    /// Dependências diretas de `path`.
    fn references(&self, path: &str) -> Result<Vec<String>>;
    /// Quantos caminhos o GC apagaria agora.
    fn gc_dry_run(&self) -> Result<u64>;
    fn dead_paths(&self) -> Result<Vec<String>>;
    fn diff_closures(&self, old: &str, new: &str) -> Result<Vec<DiffEntry>>;
    /// Cadeia de dependência de `from` até `to`.
    fn why_depends(&self, from: &str, to: &str) -> Result<Vec<String>>;
    fn auto_maintenance(&self, flake: &Path, host: &str) -> Result<AutoMaintenance>;
    /// `services.flatpak.uninstallUnused` do nix-flatpak; `None` se a opção não existe.
    fn flatpak_uninstall_unused(&self, flake: &Path, host: &str) -> Result<Option<bool>>;
    /// Apaga gerações de um perfil do usuário (Home Manager), sem privilégio.
    fn delete_user_generations(&self, profile: &Path, numbers: &[u64]) -> Result<()>;
    /// Atualiza `inputs` (todos, se a lista for vazia) escrevendo o resultado em `output_lock`;
    /// o `flake.lock` do flake não é tocado.
    fn flake_update(&self, flake: &Path, inputs: &[String], output_lock: &Path) -> Result<()>;
    /// Fixa `input` em `flake_ref` partindo de `reference_lock`, escrevendo em `output_lock`.
    fn flake_pin(
        &self,
        flake: &Path,
        reference_lock: &Path,
        output_lock: &Path,
        input: &str,
        flake_ref: &str,
    ) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct CliNix<R> {
    runner: R,
}

impl<R: CommandRunner> CliNix<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    fn stdout(&self, spec: &CommandSpec) -> Result<String> {
        self.runner.run(spec)?.into_stdout(spec)
    }
}

fn nix() -> CommandSpec {
    CommandSpec::new("nix")
}

impl<R: CommandRunner> NixBackend for CliNix<R> {
    fn flake_metadata(&self, flake: &Path) -> Result<FlakeMetadata> {
        let spec = nix()
            .args(["flake", "metadata"])
            .arg(flake.to_string_lossy())
            .arg("--json");
        parse_flake_metadata(&self.stdout(&spec)?)
    }

    fn path_info(&self, paths: &[String], options: PathInfoOptions) -> Result<Vec<PathInfo>> {
        let mut spec = nix().args(["path-info", "--json"]);
        if options.recursive {
            spec = spec.arg("--recursive");
        }
        if options.closure_size {
            spec = spec.arg("--closure-size");
        }
        parse_path_info(&self.stdout(&spec.args(paths.iter().cloned()))?)
    }

    fn store_total_bytes(&self) -> Result<u64> {
        let spec = nix().args(["path-info", "--json", "--all"]);
        Ok(parse_path_info(&self.stdout(&spec)?)?
            .iter()
            .map(|info| info.nar_size)
            .sum())
    }

    fn path_sizes(&self) -> Result<HashMap<String, u64>> {
        let spec = nix().args(["path-info", "--json", "--all"]);
        parse_path_sizes(&self.stdout(&spec)?)
    }

    fn closure_paths(&self, path: &str) -> Result<Vec<String>> {
        let spec = CommandSpec::new("nix-store").args(["--query", "--requisites", path]);
        Ok(parse_path_list(&self.stdout(&spec)?))
    }

    fn references(&self, path: &str) -> Result<Vec<String>> {
        let spec = CommandSpec::new("nix-store").args(["--query", "--references", path]);
        Ok(parse_path_list(&self.stdout(&spec)?))
    }

    fn gc_dry_run(&self) -> Result<u64> {
        let spec = nix().args(["store", "gc", "--dry-run"]);
        parse_gc_dry_run(&self.stdout(&spec)?)
    }

    fn dead_paths(&self) -> Result<Vec<String>> {
        let spec = CommandSpec::new("nix-store").args(["--gc", "--print-dead"]);
        Ok(parse_dead_paths(&self.stdout(&spec)?))
    }

    fn diff_closures(&self, old: &str, new: &str) -> Result<Vec<DiffEntry>> {
        let spec = nix().args(["store", "diff-closures", old, new]);
        Ok(parse_diff_closures(&self.stdout(&spec)?))
    }

    fn why_depends(&self, from: &str, to: &str) -> Result<Vec<String>> {
        let spec = nix().args(["why-depends", from, to]);
        Ok(parse_why_depends(&self.stdout(&spec)?))
    }

    fn auto_maintenance(&self, flake: &Path, host: &str) -> Result<AutoMaintenance> {
        let spec = nix()
            .args(["eval", "--json"])
            .arg(format!(
                "{}#nixosConfigurations.{host}.config.nix",
                flake.display()
            ))
            .args([
                "--apply",
                "c: { gc = c.gc.automatic; optimise = c.optimise.automatic; }",
            ]);
        #[derive(serde::Deserialize)]
        struct Raw {
            gc: bool,
            optimise: bool,
        }
        let raw: Raw = serde_json::from_str(&self.stdout(&spec)?)
            .map_err(|e| Error::json("nix eval (nix.gc/optimise)", e))?;
        Ok(AutoMaintenance {
            gc: raw.gc,
            optimise: raw.optimise,
        })
    }

    fn flake_update(&self, flake: &Path, inputs: &[String], output_lock: &Path) -> Result<()> {
        let spec = nix()
            .args(["flake", "update", "--flake"])
            .arg(flake.to_string_lossy())
            .arg("--output-lock-file")
            .arg(output_lock.to_string_lossy())
            .args(inputs.iter().cloned());
        self.runner.run(&spec)?.into_stdout(&spec).map(drop)
    }

    fn flake_pin(
        &self,
        flake: &Path,
        reference_lock: &Path,
        output_lock: &Path,
        input: &str,
        flake_ref: &str,
    ) -> Result<()> {
        // `nix flake lock` recebe o flake como argumento posicional (não tem `--flake`).
        let spec = nix()
            .args(["flake", "lock"])
            .arg(flake.to_string_lossy())
            .arg("--reference-lock-file")
            .arg(reference_lock.to_string_lossy())
            .args(["--override-input", input, flake_ref])
            .arg("--output-lock-file")
            .arg(output_lock.to_string_lossy());
        self.runner.run(&spec)?.into_stdout(&spec).map(drop)
    }

    fn delete_user_generations(&self, profile: &Path, numbers: &[u64]) -> Result<()> {
        let spec = CommandSpec::new("nix-env")
            .arg("-p")
            .arg(profile.to_string_lossy())
            .arg("--delete-generations")
            .args(numbers.iter().map(u64::to_string));
        self.runner.run(&spec)?.into_stdout(&spec).map(drop)
    }

    fn flatpak_uninstall_unused(&self, flake: &Path, host: &str) -> Result<Option<bool>> {
        let spec = nix().args(["eval", "--json"]).arg(format!(
            "{}#nixosConfigurations.{host}.config.services.flatpak.uninstallUnused",
            flake.display()
        ));
        let output = self.runner.run(&spec)?;
        if !output.success() {
            // A opção não existe quando o nix-flatpak não está nos módulos.
            return Ok(None);
        }
        Ok(serde_json::from_str(&output.stdout).ok())
    }
}

/// Extrai os caminhos de `nix why-depends` na ordem em que aparecem.
pub fn parse_why_depends(text: &str) -> Vec<String> {
    strip_ansi(text)
        .lines()
        .filter_map(|line| {
            let start = line.find("/nix/store/")?;
            let path = line[start..]
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or_default();
            Some(path.to_owned())
        })
        .collect()
}

pub trait GitBackend {
    fn status(&self, repo: &Path) -> Result<GitStatus>;
    /// Commits em `HEAD` que não estão em `rev`; `None` se `rev` não existe no repositório.
    fn commits_since_rev(&self, repo: &Path, rev: &str) -> Result<Option<u64>>;
    /// Commits de `HEAD` mais novos que `epoch` (aproximação quando não há revisão).
    fn commits_since_time(&self, repo: &Path, epoch: i64) -> Result<u64>;
    /// `git add -- <paths>`. A UI só chama depois de confirmação.
    fn add(&self, repo: &Path, paths: &[String]) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct CliGit<R> {
    runner: R,
}

impl<R: CommandRunner> CliGit<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    fn git(&self, repo: &Path) -> CommandSpec {
        CommandSpec::new("git")
            .arg("-C")
            .arg(repo.to_string_lossy())
    }
}

impl<R: CommandRunner> GitBackend for CliGit<R> {
    fn status(&self, repo: &Path) -> Result<GitStatus> {
        let spec = self
            .git(repo)
            .args(["status", "--porcelain=v2", "--branch"]);
        Ok(parse_status_v2(
            &self.runner.run(&spec)?.into_stdout(&spec)?,
        ))
    }

    fn commits_since_rev(&self, repo: &Path, rev: &str) -> Result<Option<u64>> {
        let spec = self
            .git(repo)
            .args(["rev-list", "--count"])
            .arg(format!("{rev}..HEAD"));
        let output = self.runner.run(&spec)?;
        if !output.success() {
            return Ok(None);
        }
        Ok(output.stdout.trim().parse().ok())
    }

    fn commits_since_time(&self, repo: &Path, epoch: i64) -> Result<u64> {
        let spec = self
            .git(repo)
            .args(["rev-list", "--count"])
            .arg(format!("--since=@{epoch}"))
            .arg("HEAD");
        let out = self.runner.run(&spec)?.into_stdout(&spec)?;
        out.trim()
            .parse()
            .map_err(|_| Error::parse("git rev-list --count", out.trim().to_owned()))
    }

    fn add(&self, repo: &Path, paths: &[String]) -> Result<()> {
        let spec = self
            .git(repo)
            .args(["add", "--"])
            .args(paths.iter().cloned());
        self.runner.run(&spec)?.into_stdout(&spec).map(drop)
    }
}

pub trait FlatpakBackend {
    fn summary(&self) -> Result<FlatpakSummary>;
    /// Remove os runtimes sem uso da instalação do usuário (a de sistema passa pelo helper).
    fn remove_unused_user(&self) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct CliFlatpak<R> {
    runner: R,
}

impl<R: CommandRunner> CliFlatpak<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    fn list(&self, kind: &str) -> Result<Vec<FlatpakRef>> {
        let spec = CommandSpec::new("flatpak")
            .args([
                "list",
                kind,
                "--columns=application,branch,size,installation",
            ])
            .env("LC_ALL", "C");
        Ok(flatpak::parse_list(
            &self.runner.run(&spec)?.into_stdout(&spec)?,
        ))
    }
}

impl<R: CommandRunner> FlatpakBackend for CliFlatpak<R> {
    fn remove_unused_user(&self) -> Result<()> {
        let spec = CommandSpec::new("flatpak")
            .args(["uninstall", "--user", "--unused", "--noninteractive"])
            .env("LC_ALL", "C");
        self.runner.run(&spec)?.into_stdout(&spec).map(drop)
    }

    fn summary(&self) -> Result<FlatpakSummary> {
        let apps = self.list("--app")?;
        let runtimes = self.list("--runtime")?;
        // Responder "n" ao prompt lista o que sairia sem remover nada. O código de saída
        // é 1 por causa da recusa, então ele não indica falha aqui.
        let spec = CommandSpec::new("flatpak")
            .args(["uninstall", "--unused"])
            .env("LC_ALL", "C")
            .stdin("n\n");
        let output = self.runner.run(&spec)?;
        let unused = flatpak::parse_unused(&output.stdout);
        Ok(flatpak::summarize(apps, runtimes, unused))
    }
}

/// Parâmetros de um build do sistema sem privilégio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub flake: PathBuf,
    pub host: String,
    /// Link de saída do build (uma raiz do GC até ser descartado ou aplicado). Fica na pasta de
    /// estado do app, nunca na do flake.
    pub out_link: PathBuf,
    /// Lock alternativo: constrói com ele sem alterar o `flake.lock`.
    pub reference_lock: Option<PathBuf>,
}

pub trait RebuildBackend {
    /// Comando que constrói o sistema sem ativá-lo, com progresso `internal-json`.
    fn build_command(&self, request: &BuildRequest) -> CommandSpec;
}

/// `nix build` direto no `toplevel`. O `nixos-rebuild build` reexecuta a si mesmo a partir da
/// nova nixpkgs, o que dispara builds extras e esconde o progresso.
#[derive(Debug, Clone, Copy, Default)]
pub struct NixBuild;

impl RebuildBackend for NixBuild {
    fn build_command(&self, request: &BuildRequest) -> CommandSpec {
        let mut spec = CommandSpec::new("nix")
            .arg("build")
            .arg(format!(
                "{}#nixosConfigurations.{}.config.system.build.toplevel",
                request.flake.display(),
                request.host
            ))
            .arg("--out-link")
            .arg(request.out_link.to_string_lossy())
            .args(["--print-out-paths", "--log-format", "internal-json", "-v"]);
        if let Some(lock) = &request.reference_lock {
            spec = spec
                .arg("--reference-lock-file")
                .arg(lock.to_string_lossy());
        }
        spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn why_depends_em_ordem() {
        let text = "/nix/store/a-system\n└───/nix/store/b-fw\x1b[0m\n    └───/nix/store/c-linux-firmware-1\n";
        assert_eq!(
            parse_why_depends(text),
            [
                "/nix/store/a-system",
                "/nix/store/b-fw",
                "/nix/store/c-linux-firmware-1"
            ]
        );
    }

    #[test]
    fn build_nao_ativa_nem_usa_root() {
        let spec = NixBuild.build_command(&BuildRequest {
            flake: "/etc/nixos".into(),
            host: "nixos".into(),
            out_link: "/s/result".into(),
            reference_lock: Some("/s/novo.lock".into()),
        });
        assert_eq!(spec.program, "nix");
        assert_eq!(
            spec.args,
            [
                "build",
                "/etc/nixos#nixosConfigurations.nixos.config.system.build.toplevel",
                "--out-link",
                "/s/result",
                "--print-out-paths",
                "--log-format",
                "internal-json",
                "-v",
                "--reference-lock-file",
                "/s/novo.lock",
            ]
        );
        assert!(!spec.args.iter().any(|a| a == "switch" || a == "--elevate"));
        // Sem lock alternativo, usa o do flake.
        let plain = NixBuild.build_command(&BuildRequest {
            flake: "/etc/nixos".into(),
            host: "nixos".into(),
            out_link: "/s/result".into(),
            reference_lock: None,
        });
        assert!(!plain.args.iter().any(|a| a == "--reference-lock-file"));
    }

    #[test]
    fn comandos_de_update_e_pin() {
        use crate::runner::{CommandRunner, Output};
        use std::cell::RefCell;
        struct Rec(RefCell<Vec<CommandSpec>>);
        impl CommandRunner for Rec {
            fn run(&self, spec: &CommandSpec) -> Result<Output> {
                self.0.borrow_mut().push(spec.clone());
                Ok(Output {
                    stdout: String::new(),
                    stderr: String::new(),
                    code: Some(0),
                })
            }
        }
        let rec = Rec(RefCell::default());
        let nix = CliNix::new(&rec);
        nix.flake_update(
            Path::new("/etc/nixos"),
            &["nixpkgs".into(), "home-manager".into()],
            Path::new("/s/novo.lock"),
        )
        .unwrap();
        nix.flake_pin(
            Path::new("/etc/nixos"),
            Path::new("/s/novo.lock"),
            Path::new("/s/pin.lock"),
            "nixpkgs",
            "github:NixOS/nixpkgs/abc",
        )
        .unwrap();
        let calls = rec.0.borrow();
        assert_eq!(
            calls[0].args,
            [
                "flake",
                "update",
                "--flake",
                "/etc/nixos",
                "--output-lock-file",
                "/s/novo.lock",
                "nixpkgs",
                "home-manager"
            ]
        );
        assert_eq!(
            calls[1].args,
            [
                "flake",
                "lock",
                "/etc/nixos",
                "--reference-lock-file",
                "/s/novo.lock",
                "--override-input",
                "nixpkgs",
                "github:NixOS/nixpkgs/abc",
                "--output-lock-file",
                "/s/pin.lock"
            ]
        );
    }
}
