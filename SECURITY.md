# Política de segurança

O Zelador tem um componente privilegiado (`zelador-helper`, executado como root pelo systemd, com autorização do polkit). Falhas nele são levadas a sério. O modelo de segurança está em [docs/seguranca.md](docs/seguranca.md).

## Como relatar

**Não abra uma issue pública.** Use o relato privado do GitHub:
<https://github.com/jardel20/Zelador/security/advisories/new>

Inclua, se possível, o pedido exato que o helper aceitou e não deveria (ou recusou e deveria aceitar), a versão (`rev` do flake) e a saída do `zelador-helper --call ...`. Não inclua dados pessoais.

## Escopo

- Dentro: o helper (`crates/helper`), o módulo NixOS e a action do polkit (`nix/`), o socket do systemd e qualquer caminho em que a interface consiga fazer algo privilegiado sem a autorização do polkit.
- Fora: administradores que já autenticaram (o Zelador não é uma barreira contra quem pode ser root), regras locais de polkit que liberam a action sem senha, e problemas do próprio Nix, systemd ou polkit.

## Versões suportadas

O projeto está na versão 0.1: apenas o `master` recebe correções.
