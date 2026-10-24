//! O binário de verdade, com os caminhos de produção. Só pedidos que são recusados (ou que
//! seriam recusados por falta de root antes de executar algo) entram aqui; um pedido válido
//! nunca é aceito neste teste, porque rodaria no sistema real.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::symlink;
use std::process::{Command, Output};

fn helper(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zelador-helper"))
        .args(args)
        .output()
        .unwrap()
}

fn assert_rejected(args: &[&str]) {
    let out = helper(args);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(65), "{args:?}: {stderr}");
    assert!(stderr.contains("recusado"), "{args:?}: {stderr}");
    assert!(out.stdout.is_empty());
}

#[test]
fn recusa_pedidos_malformados() {
    assert_rejected(&[]);
    assert_rejected(&["formatar-disco"]);
    assert_rejected(&["gc", "--max-freed", "1"]);
    assert_rejected(&["optimise", "extra"]);
    assert_rejected(&["flatpak-remove-unused", "x"]);
    assert_rejected(&["rollback"]);
    assert_rejected(&["rollback", "1", "2"]);
    assert_rejected(&["rollback", "-1"]);
    assert_rejected(&["rollback", "0"]);
    assert_rejected(&["set-boot-default", "1 2"]);
    assert_rejected(&["delete-generations"]);
    assert_rejected(&["delete-generations", "3", "3"]);
    assert_rejected(&["delete-generations", "--all"]);
    assert_rejected(&["activate", "/nix/store/x-y"]);
    assert_rejected(&["activate", "/nix/store/x-y", "reboot"]);
    assert_rejected(&["activate", "/nix/store/x-y", "boot", "extra"]);
}

#[test]
fn recusa_caminhos_fora_do_store() {
    for path in [
        "/etc",
        "/",
        "/nix/store",
        "/nix/store/",
        "/nix/store/..",
        "/nix/store/../etc",
        "/nix/store/x/../../etc",
        "/nix/store/nao-existe-zelador",
        "/nix/store/.oculto",
        "relativo",
        "",
    ] {
        assert_rejected(&["activate", path, "test"]);
    }
}

#[test]
fn recusa_symlink_para_o_store() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("sistema");
    // Qualquer destino serve: o caminho já está fora de /nix/store.
    symlink("/run/current-system", &link).unwrap();
    assert_rejected(&["activate", link.to_str().unwrap(), "switch"]);
    fs::write(dir.path().join("arquivo"), "").unwrap();
    assert_rejected(&[
        "activate",
        dir.path().join("arquivo").to_str().unwrap(),
        "boot",
    ]);
}

#[test]
fn nao_exclui_a_geracao_em_uso() {
    // O número da geração atual desta máquina, se houver; senão a geração "não existe".
    // Nos dois casos a recusa vem antes de qualquer execução.
    let current = fs::read_link("/nix/var/nix/profiles/system")
        .ok()
        .and_then(|target| target.to_str().map(str::to_owned))
        .and_then(|name| {
            name.strip_prefix("system-")?
                .strip_suffix("-link")
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "999999".to_owned());
    assert_rejected(&["delete-generations", &current]);
}

#[test]
fn um_pedido_valido_sem_root_nao_executa_nada() {
    // Um sistema válido de verdade (a geração em uso) passa pela validação, mas sem root o
    // helper para antes de travar ou executar qualquer coisa.
    let Ok(system) = fs::canonicalize("/run/current-system") else {
        return;
    };
    if rustix::process::geteuid().is_root() {
        return;
    }
    let out = helper(&["activate", system.to_str().unwrap(), "test"]);
    assert_eq!(out.status.code(), Some(70));
    assert!(String::from_utf8_lossy(&out.stderr).contains("precisa rodar como root"));
}
