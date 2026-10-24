# Usar o Zelador no seu NixOS

O Zelador **não edita** a sua configuração. Para que ele consiga atualizar, voltar e limpar o
sistema, você ativa o módulo por conta própria, colando o trecho abaixo no seu flake.

## O que o módulo faz

- instala o app (`zelador`), a CLI de diagnóstico (`zelador-core`) e o helper privilegiado;
- instala a *action* do polkit `br.dev.zelador.helper`, que aponta para o helper no store e
  **exige senha de administrador a cada operação**;
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
    zelador.url = "path:/caminho/para/Zelador"; # ou o endereço do repositório
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
