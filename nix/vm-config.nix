# Máquina de teste da Fase 2: o módulo ativado, dois usuários e uma segunda closure de
# sistema (specialisation `alternativa`) para o helper ativar.
{ lib, pkgs, ... }:
{
  programs.zelador.enable = true;

  users.users.alice = {
    isNormalUser = true;
    password = "alice";
  };
  users.users.bob = {
    isNormalUser = true;
    password = "bob";
  };

  environment.etc."zelador-marcador".text = "original";
  specialisation.alternativa.configuration = {
    # `text` é do tipo `lines`: sem mkForce as definições se concatenam.
    environment.etc."zelador-marcador".text = lib.mkForce "alternativa";
  };

  # Sem agente gráfico dentro da VM: só a alice é autorizada, e só nesta action.
  # O bob continua sujeito ao padrão (auth_admin) e deve ser recusado.
  security.polkit.extraConfig = ''
    polkit.addRule(function(action, subject) {
      if (action.id == "br.dev.zelador.helper" && subject.user == "alice") {
        return polkit.Result.YES;
      }
    });
  '';

  # Só gera o menu (/boot/grub/grub.cfg); a VM não tem disco em que instalar o GRUB.
  boot.loader.grub = {
    enable = true;
    # O framework de testes define `device` com mkVMOverride (prioridade 10); 5 vence.
    device = lib.mkOverride 5 "nodev";
  };
  virtualisation.memorySize = 2048;
  environment.systemPackages = [ pkgs.polkit ];
}
