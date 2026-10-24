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

    def as_user(user, cmd):
        return f"su - {user} -c '{cmd}'"

    machine.wait_for_unit("multi-user.target")

    with subtest("o socket do systemd está ativo e não há pkexec setuid"):
        machine.wait_for_unit("zelador-helper.socket")
        machine.succeed("test -S /run/zelador/helper.sock")
        machine.fail("test -e /run/wrappers/bin/pkexec")

    with subtest("a action do polkit está instalada"):
        out = machine.succeed("pkaction --action-id br.dev.zelador.helper --verbose")
        assert "implicit active:   auth_admin" in out, out
        assert "implicit inactive: auth_admin" in out, out

    with subtest("usuário comum recusado pelo polkit"):
        status, out = machine.execute(as_user("bob", f"{helper} --call gc 2>&1"))
        # Código 77: o polkit não autorizou (sem agente para pedir a senha).
        assert status == 77, (status, out)
        assert "não autorizado" in out, out

    with subtest("pedidos inválidos são recusados com código 65, pelo socket"):
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
            status, out = machine.execute(as_user("alice", f"{helper} --call {bad} 2>&1"))
            assert status == 65, (bad, status, out)

    alt = machine.succeed("readlink -f /run/current-system/specialisation/alternativa").strip()
    original = machine.succeed("readlink -f /run/current-system").strip()
    machine.succeed("test \"$(cat /etc/zelador-marcador)\" = original")

    with subtest("activate test troca o sistema sem mexer no perfil"):
        before = machine.succeed("ls /nix/var/nix/profiles | grep -c 'system-.*-link' || true").strip()
        machine.succeed(as_user("alice", f"{helper} --call activate {alt} test"))
        machine.succeed("test \"$(cat /etc/zelador-marcador)\" = alternativa")
        assert machine.succeed("readlink -f /run/current-system").strip() == alt
        after = machine.succeed("ls /nix/var/nix/profiles | grep -c 'system-.*-link' || true").strip()
        assert before == after, (before, after)

    with subtest("activate boot registra uma geração nova e o perfil aponta para ela"):
        machine.succeed(as_user("alice", f"{helper} --call activate {original} boot"))
        profile = machine.succeed("readlink -f /nix/var/nix/profiles/system").strip()
        assert profile == original, profile

    with subtest("zelador-core activate: o helper anuncia as etapas e as fases são reconhecidas"):
        out = machine.succeed(as_user("alice", f"zelador-core activate {original} boot 2>&1"))
        # Dois passos: registrar a geração (nix-env) e ativar (systemd-run).
        assert "etapa 1 de 2: nix-env" in out, out
        assert "etapa 2 de 2: systemd-run" in out, out
        # As linhas do switch-to-configuration viram fases e vão para o log.
        assert "fase: UpdatingBootMenu" in out, out
        assert "Concluído." in out, out
        # Em `test` não há registro de geração: um passo só.
        out = machine.succeed(as_user("alice", f"zelador-core activate {alt} test 2>&1"))
        assert "etapa 1 de 1: systemd-run" in out and "etapa 2" not in out, out
        machine.succeed(as_user("alice", f"{helper} --call activate {original} test"))

    with subtest("delete-generations recusa a geração em uso e a padrão do boot"):
        gens = machine.succeed("ls /nix/var/nix/profiles | sed -n 's/^system-\\([0-9]*\\)-link$/\\1/p' | sort -n").split()
        boot_gen = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() == original][-1]
        status, out = machine.execute(as_user("alice", f"{helper} --call delete-generations {boot_gen} 2>&1"))
        assert status == 65, (status, out)

    with subtest("activate switch para a alternativa e rollback para a original"):
        machine.succeed(as_user("alice", f"{helper} --call activate {alt} switch"))
        machine.succeed("test \"$(cat /etc/zelador-marcador)\" = alternativa")
        gens = machine.succeed("ls /nix/var/nix/profiles | sed -n 's/^system-\\([0-9]*\\)-link$/\\1/p' | sort -n").split()
        orig_gen = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() == original][0]
        machine.succeed(as_user("alice", f"{helper} --call rollback {orig_gen}"))
        machine.succeed("test \"$(cat /etc/zelador-marcador)\" = original")

    with subtest("delete-generations remove uma geração antiga"):
        gens = machine.succeed("ls /nix/var/nix/profiles | sed -n 's/^system-\\([0-9]*\\)-link$/\\1/p' | sort -n").split()
        current = machine.succeed("readlink -f /run/current-system").strip()
        old = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() != current]
        assert old, gens
        entry = f"Configuration {old[0]} "
        assert entry in machine.succeed("cat /boot/grub/grub.cfg"), "geração antiga deveria estar no menu"
        machine.succeed(as_user("alice", f"{helper} --call delete-generations {old[0]}"))
        machine.fail(f"test -e /nix/var/nix/profiles/system-{old[0]}-link")
        # O passo `boot` regenera o menu sem a entrada da geração apagada.
        assert entry not in machine.succeed("cat /boot/grub/grub.cfg"), "menu do boot não foi regenerado"

    with subtest("gc e optimise rodam"):
        machine.succeed(as_user("alice", f"{helper} --call gc"))
        # O store da VM é virtiofs e não aceita os hard links do --optimise ("Stale file
        # handle"). Aqui basta provar que o comando rodou e que o helper propagou a falha.
        status, out = machine.execute(as_user("alice", f"{helper} --call optimise 2>&1"))
        assert status == 0 or ("Stale file handle" in out and "passo 1 terminou com código" in out), (status, out)

    with subtest("zelador-core clean: simula sem alterar e executa pelo helper (socket + polkit)"):
        # Cria uma geração antiga (a alternativa) e devolve o boot para a original.
        machine.succeed(as_user("alice", f"{helper} --call activate {alt} boot"))
        gens = machine.succeed("ls /nix/var/nix/profiles | sed -n 's/^system-\\([0-9]*\\)-link$/\\1/p' | sort -n").split()
        orig_gen = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() == original][0]
        alt_gen = [g for g in gens if machine.succeed(f"readlink -f /nix/var/nix/profiles/system-{g}-link").strip() == alt][-1]
        machine.succeed(as_user("alice", f"{helper} --call set-boot-default {orig_gen}"))
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

    with subtest("zelador-core history: as operações anteriores foram registradas"):
        out = machine.succeed(as_user("alice", "zelador-core history 2>&1"))
        # A limpeza e as ativações feitas pela CLI entram no histórico, com resultado.
        assert " clean success " in out, out
        assert " update success " in out, out

    with subtest("o timer de usuário é gerado pelo módulo e só avisa"):
        timer = machine.succeed("cat /etc/systemd/user/zelador-check.timer")
        assert "OnCalendar=daily" in timer and "Persistent=true" in timer, timer
        service = machine.succeed("cat /etc/systemd/user/zelador-check.service")
        assert "zelador-core check --notify" in service, service
        assert "--stale-days 1" in service and "--flake /etc/zelador-check" in service, service
        # Nada de ações: o serviço não chama activate, clean nem update.
        assert "clean" not in service.replace("zelador-check", "") and "activate" not in service, service

    with subtest("check --notify avisa uma vez e não repete antes do intervalo"):
        fake = "/etc/zelador-fake-notify-send"
        cmd = f"zelador-core check --notify --flake /etc/zelador-check --notify-send {fake} --open /run/current-system/sw/bin/true --stale-days 1 --min-free-gib 100000 --reclaimable-gib 0"
        out = machine.succeed(as_user("alice", f"{cmd} 2>&1"))
        assert "Inputs do flake desatualizados" in out and "Pouco espaço em disco" in out, out
        # (`log` é uma global do driver de testes: não reaproveitar o nome.)
        notified = machine.succeed("cat /tmp/zelador-notify.log")
        assert notified.count("--app-name=Zelador") >= 2, notified
        # Título e corpo vão depois do `--`, e a ação abre o app.
        assert "--action=open=Abrir o Zelador" in notified, notified
        assert "-- Inputs do flake desatualizados" in notified, notified
        before = len(notified.splitlines())
        out = machine.succeed(as_user("alice", f"{cmd} 2>&1"))
        assert "já avisado" in out, out
        assert len(machine.succeed("cat /tmp/zelador-notify.log").splitlines()) == before

    with subtest("sem instalação Flatpak de sistema, flatpak-remove-unused é recusado"):
        status, out = machine.execute(as_user("alice", f"{helper} --call flatpak-remove-unused 2>&1"))
        assert status == 65, (status, out)
  '';
}
