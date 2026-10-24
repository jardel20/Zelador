# Teste automatizado do helper numa VM NixOS. Nada roda no sistema real.
{ pkgs, module }:
pkgs.testers.runNixOSTest {
  name = "zelador-helper";
  nodes.machine = {
    imports = [
      module
      ./vm-config.nix
    ];
  };

  testScript = { nodes, ... }: ''
    helper = "${nodes.machine.programs.zelador.helperPackage}/bin/zelador-helper"

    # O pkexec com setuid é o do wrapper; o do store não tem o bit.
    pkexec = "/run/wrappers/bin/pkexec"

    def as_user(user, cmd):
        return f"su - {user} -c '{cmd}'"

    machine.wait_for_unit("multi-user.target")

    with subtest("a action do polkit está instalada e aponta para o helper"):
        out = machine.succeed("pkaction --action-id br.dev.zelador.helper --verbose")
        assert "org.freedesktop.policykit.exec.path -> " + helper in out, out
        assert "implicit active:   auth_admin" in out, out
        assert "implicit inactive: auth_admin" in out, out

    with subtest("usuário comum recusado pelo polkit"):
        status, out = machine.execute(as_user("bob", f"{pkexec} {helper} gc 2>&1"))
        assert status != 0, out
        # Recusa por autorização (sem agente para pedir a senha), não por erro de ambiente.
        assert "setuid" not in out and ("agent" in out or "uthoriz" in out), out

    with subtest("pedidos inválidos são recusados com código 65, via pkexec"):
        for bad in [
            "activate /etc test",
            "activate /nix/store/../etc test",
            "gc --max-freed 1",
            "clean",
            "clean --gc --gc",
            "clean --generations --gc",
            "rollback -1",
            "delete-generations",
        ]:
            status, out = machine.execute(as_user("alice", f"{pkexec} {helper} {bad} 2>&1"))
            assert status == 65, (bad, status, out)

    alt = machine.succeed("readlink -f /run/current-system/specialisation/alternativa").strip()
    original = machine.succeed("readlink -f /run/current-system").strip()
    machine.succeed("test \"$(cat /etc/zelador-marcador)\" = original")

    with subtest("activate test troca o sistema sem mexer no perfil"):
        before = machine.succeed("ls /nix/var/nix/profiles | grep -c 'system-.*-link' || true").strip()
        machine.succeed(as_user("alice", f"{pkexec} {helper} activate {alt} test"))
        machine.succeed("test \"$(cat /etc/zelador-marcador)\" = alternativa")
        assert machine.succeed("readlink -f /run/current-system").strip() == alt
        after = machine.succeed("ls /nix/var/nix/profiles | grep -c 'system-.*-link' || true").strip()
        assert before == after, (before, after)

    with subtest("activate boot registra uma geração nova e o perfil aponta para ela"):
        machine.succeed(as_user("alice", f"{pkexec} {helper} activate {original} boot"))
        profile = machine.succeed("readlink -f /nix/var/nix/profiles/system").strip()
        assert profile == original, profile

    with subtest("delete-generations recusa a geração em uso e a padrão do boot"):
        gens = machine.succeed("ls /nix/var/nix/profiles | sed -n 's/^system-\\([0-9]*\\)-link$/\\1/p' | sort -n").split()
        boot_gen = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() == original][-1]
        status, out = machine.execute(as_user("alice", f"{pkexec} {helper} delete-generations {boot_gen} 2>&1"))
        assert status == 65, (status, out)

    with subtest("activate switch para a alternativa e rollback para a original"):
        machine.succeed(as_user("alice", f"{pkexec} {helper} activate {alt} switch"))
        machine.succeed("test \"$(cat /etc/zelador-marcador)\" = alternativa")
        gens = machine.succeed("ls /nix/var/nix/profiles | sed -n 's/^system-\\([0-9]*\\)-link$/\\1/p' | sort -n").split()
        orig_gen = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() == original][0]
        machine.succeed(as_user("alice", f"{pkexec} {helper} rollback {orig_gen}"))
        machine.succeed("test \"$(cat /etc/zelador-marcador)\" = original")

    with subtest("delete-generations remove uma geração antiga"):
        gens = machine.succeed("ls /nix/var/nix/profiles | sed -n 's/^system-\\([0-9]*\\)-link$/\\1/p' | sort -n").split()
        current = machine.succeed("readlink -f /run/current-system").strip()
        old = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() != current]
        assert old, gens
        entry = f"Configuration {old[0]} "
        assert entry in machine.succeed("cat /boot/grub/grub.cfg"), "geração antiga deveria estar no menu"
        machine.succeed(as_user("alice", f"{pkexec} {helper} delete-generations {old[0]}"))
        machine.fail(f"test -e /nix/var/nix/profiles/system-{old[0]}-link")
        # O passo `boot` regenera o menu sem a entrada da geração apagada.
        assert entry not in machine.succeed("cat /boot/grub/grub.cfg"), "menu do boot não foi regenerado"

    with subtest("gc e optimise rodam"):
        machine.succeed(as_user("alice", f"{pkexec} {helper} gc"))
        # O store da VM é virtiofs e não aceita os hard links do --optimise ("Stale file
        # handle"). Aqui basta provar que o comando rodou e que o helper propagou a falha.
        status, out = machine.execute(as_user("alice", f"{pkexec} {helper} optimise 2>&1"))
        assert status == 0 or ("Stale file handle" in out and "passo 1 terminou com código" in out), (status, out)

    with subtest("zelador-core clean: simula sem alterar e executa pelo helper com pkexec"):
        # Cria uma geração antiga (a alternativa) e devolve o boot para a original.
        machine.succeed(as_user("alice", f"{pkexec} {helper} activate {alt} boot"))
        gens = machine.succeed("ls /nix/var/nix/profiles | sed -n 's/^system-\\([0-9]*\\)-link$/\\1/p' | sort -n").split()
        orig_gen = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() == original][0]
        alt_gen = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() == alt][-1]
        machine.succeed(as_user("alice", f"{pkexec} {helper} set-boot-default {orig_gen}"))
        machine.succeed(f"test -L /nix/var/nix/profiles/system-{alt_gen}-link")

        # Sem --execute: não pode apagar nada.
        out = machine.succeed(as_user("alice", "zelador-core clean --keep-system 0 --keep-days 0 --no-links 2>&1"))
        assert f"[{alt_gen}" in out or f" {alt_gen}" in out, out
        assert "nada foi alterado" in out, out
        machine.succeed(f"test -L /nix/var/nix/profiles/system-{alt_gen}-link")

        # --execute exige --yes.
        status, out = machine.execute(as_user("alice", "zelador-core clean --keep-system 0 --keep-days 0 --no-links --execute 2>&1"))
        assert status != 0 and "--yes" in out, (status, out)
        machine.succeed(f"test -L /nix/var/nix/profiles/system-{alt_gen}-link")

        out = machine.succeed(as_user("alice", "zelador-core clean --keep-system 0 --keep-days 0 --no-links --execute --yes 2>&1"))
        assert "excluindo gerações do sistema" in out and "coletando lixo do store" in out and "Concluído" in out, out
        machine.fail(f"test -e /nix/var/nix/profiles/system-{alt_gen}-link")
        # A em uso e a padrão do boot continuam.
        machine.succeed(f"test -L /nix/var/nix/profiles/system-{orig_gen}-link")
        assert machine.succeed("readlink -f /nix/var/nix/profiles/system").strip() == original

    with subtest("sem instalação Flatpak de sistema, flatpak-remove-unused é recusado"):
        status, out = machine.execute(as_user("alice", f"{pkexec} {helper} flatpak-remove-unused 2>&1"))
        assert status == 65, (status, out)
  '';
}
