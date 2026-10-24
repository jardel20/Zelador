# Módulo NixOS do Zelador. Não escreve em nenhum arquivo do usuário: só instala o que o
# app precisa para pedir privilégio de forma controlada.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.zelador;
in
{
  options.programs.zelador = {
    enable = lib.mkEnableOption "o Zelador (painel operacional do NixOS)";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./app.nix {
        zelador-helper = cfg.helperPackage;
        nix = config.nix.package;
      };
      defaultText = lib.literalExpression "pkgs.callPackage ./app.nix { zelador-helper = config.programs.zelador.helperPackage; }";
      description = "O app (janela, ícones e entrada .desktop).";
    };

    flakePath = lib.mkOption {
      type = lib.types.str;
      default = "/etc/nixos";
      description = "Pasta do flake que o Zelador observa (usada pelo timer de verificação).";
    };

    checkTimer = {
      enable = lib.mkEnableOption ''
        o timer de usuário que verifica inputs velhos, pouco espaço e lixo grande e avisa por
        notificação. Só avisa: nunca altera o sistema
      '';
      interval = lib.mkOption {
        type = lib.types.str;
        default = "weekly";
        description = "Quando verificar (formato `OnCalendar` do systemd).";
      };
      staleDays = lib.mkOption {
        type = lib.types.ints.positive;
        default = 14;
        description = "Avisa quando um input do flake tem mais dias que isto.";
      };
      minFreeGiB = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 10;
        description = "Avisa quando o espaço livre onde fica o Nix store cai abaixo disto (GiB).";
      };
      reclaimableGiB = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 20;
        description = "Avisa quando podem sair mais que isto do store, em GiB. 0 desliga.";
      };
    };

    cliPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./core.nix {
        zelador-helper = cfg.helperPackage;
        nix = config.nix.package;
      };
      defaultText = lib.literalExpression "pkgs.callPackage ./core.nix { zelador-helper = config.programs.zelador.helperPackage; }";
      description = "CLI de diagnóstico (`zelador-core status|space|clean`).";
    };

    helperPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./helper.nix { nix = config.nix.package; };
      defaultText = lib.literalExpression "pkgs.callPackage ./helper.nix { nix = config.nix.package; }";
      description = ''
        Helper privilegiado chamado via pkexec. A action do polkit aponta para o caminho
        dele no store e exige senha de administrador a cada operação.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # O timer só observa e avisa; a ação (atualizar, limpar) é sempre do usuário, no app.
    systemd.user.services.zelador-check = lib.mkIf cfg.checkTimer.enable {
      description = "Zelador: verifica o Nix e avisa";
      serviceConfig = {
        Type = "oneshot";
        TimeoutStartSec = "5min";
        ExecStart = lib.escapeShellArgs [
          "${cfg.cliPackage}/bin/zelador-core"
          "check"
          "--notify"
          "--flake"
          cfg.flakePath
          "--notify-send"
          "${pkgs.libnotify}/bin/notify-send"
          "--open"
          "${cfg.package}/bin/zelador"
          "--stale-days"
          (toString cfg.checkTimer.staleDays)
          "--min-free-gib"
          (toString cfg.checkTimer.minFreeGiB)
          "--reclaimable-gib"
          (toString cfg.checkTimer.reclaimableGiB)
        ];
      };
    };
    systemd.user.timers.zelador-check = lib.mkIf cfg.checkTimer.enable {
      description = "Zelador: verificação periódica";
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = cfg.checkTimer.interval;
        Persistent = true;
        RandomizedDelaySec = "1h";
      };
    };

    security.polkit.enable = true;
    # No nixpkgs atual o pkexec setuid é opcional e vem desligado; sem ele, o app não
    # consegue chamar o helper. O app usa /run/wrappers/bin/pkexec, nunca o do PATH.
    security.polkit.enablePkexecWrapper = lib.mkDefault true;
    environment.systemPackages = [
      cfg.package
      cfg.cliPackage
      cfg.helperPackage
    ];
    # Fontes da identidade visual (Poppins para a interface, JetBrains Mono para caminhos).
    fonts.packages = [
      pkgs.poppins
      pkgs.jetbrains-mono
    ];
    environment.pathsToLink = [ "/share/polkit-1/actions" ];
  };
}
