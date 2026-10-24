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
    security.polkit.enable = true;
    # No nixpkgs atual o pkexec setuid é opcional e vem desligado; sem ele, o app não
    # consegue chamar o helper. O app usa /run/wrappers/bin/pkexec, nunca o do PATH.
    security.polkit.enablePkexecWrapper = lib.mkDefault true;
    environment.systemPackages = [ cfg.helperPackage ];
    environment.pathsToLink = [ "/share/polkit-1/actions" ];
  };
}
