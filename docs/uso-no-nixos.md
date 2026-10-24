# Usar o Zelador no seu NixOS

O Zelador **não edita** a sua configuração. Para que ele consiga atualizar, voltar e limpar o
sistema, você ativa o módulo por conta própria, colando o trecho abaixo no seu flake.

## O que o módulo faz

- instala o app (`zelador`), a CLI de diagnóstico (`zelador-core`) e o helper privilegiado;
- instala a *action* do polkit `br.dev.zelador.helper`, que **exige senha de administrador a
  cada operação**;
- instala as fontes Poppins e JetBrains Mono;
- liga o `zelador-helper.socket` no systemd. O app **não tem privilégio nenhum**: ele conversa
  com esse socket (`/run/zelador/helper.sock`), o systemd inicia o helper como root para cada
  pedido, e o helper pergunta ao polkit antes de executar. **Não há setuid**: o `pkexec` não é
  usado nem precisa ser habilitado. É preciso haver um agente de autenticação do polkit na sua
  sessão (o mesmo que abre a janela de senha em qualquer app gráfico).

## Trecho para o `flake.nix` do seu sistema

```nix
{
  inputs = {
    # ... seus inputs ...
    zelador.url = "github:jardel20/Zelador"; # ou "path:/caminho/para/Zelador"
    zelador.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, zelador, ... }: {
    nixosConfigurations.SEU_HOST = nixpkgs.lib.nixosSystem {
      modules = [
        # ... seus módulos ...
        zelador.nixosModules.default
        { programs.zelador.enable = true; }
      ];
    };
  };
}
```

Depois: `nixos-rebuild switch --flake .#SEU_HOST` (ou `nh os switch`). Se o seu `flake.nix`
usa `specialArgs`/`inputs`, basta acrescentar `zelador` à lista de módulos.

## Como desfazer

Remova as duas linhas do módulo e o input, e aplique de novo. O socket e o serviço somem.
Nada do Zelador fica no seu `/etc/nixos`.

## Verificar

```
zelador-core status          # lê o estado, sem alterar nada
zelador-core clean           # simula a limpeza, sem alterar nada
```

A limpeza real só roda com `zelador-core clean --execute --yes` (ou pelo botão "Limpar" do
app, que pede confirmação) e pede a senha de administrador **uma vez**.

## Verificação periódica (opcional)

```nix
programs.zelador.checkTimer = {
  enable = true;           # timer de usuário; só avisa, nunca altera o sistema
  interval = "weekly";     # OnCalendar do systemd
  staleDays = 14;          # avisa se um input tem mais dias que isto
  minFreeGiB = 10;         # avisa se o espaço livre cai abaixo disto
  reclaimableGiB = 20;     # avisa se podem sair mais que isto (0 desliga)
};
programs.zelador.flakePath = "/etc/nixos";   # flake que o timer observa
```

A notificação usa `notify-send` e depende de o seu daemon de notificações aceitá-lo. Para ver o
que o timer faria agora, sem notificar: `zelador-core check`.

## Problemas comuns

| Sintoma | Causa e solução |
|---|---|
| Aviso "O serviço do helper não está ativo" | O módulo não está ativado. Ative `programs.zelador.enable` e aplique. Confira com `systemctl status zelador-helper.socket` |
| Pede senha, mas a janela não aparece / a ação falha com "não autorizado" (código 77) | Falta um agente de autenticação do polkit na sessão. Inicie um (por exemplo, o do seu ambiente gráfico) |
| "outra operação está em andamento" (código 75) | O helper só roda uma operação por vez. Aguarde a anterior terminar |
| A atualização falha no build com "opção removida" | O nixpkgs novo removeu uma opção que a sua configuração usa. Nada foi alterado no sistema: corrija a configuração e tente de novo |
| "Espaço insuficiente para esta atualização" | Libere espaço (tela *Liberar espaço*) e tente de novo |
| Números de espaço parecem menores que o esperado | São **limites superiores**; o Nix compartilha arquivos idênticos entre caminhos |
