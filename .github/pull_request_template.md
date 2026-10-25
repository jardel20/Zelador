## O que muda e por quê

## Como foi testado

- [ ] `cargo fmt --all` e `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Se mexeu no helper (`crates/helper`), no módulo ou na action do polkit: `nix build .#checks.x86_64-linux.helper-vm`
- [ ] Se uma CLI externa mudou de saída: atualizei a **fixture** e o teste, sem ajustar o parser às cegas
- [ ] Se mudou textos da interface: rodei `tools/extract-pot.sh`

## Segurança

- [ ] Não edita `.nix` do usuário, não roda a UI como root e não amplia o conjunto de subcomandos do helper (ou explico acima por quê)
