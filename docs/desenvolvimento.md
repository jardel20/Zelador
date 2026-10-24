# Desenvolvimento

## Ambiente

```sh
nix develop            # cargo, rustc, clippy, rustfmt, rust-analyzer, pkg-config, gtk4,
                       # libadwaita, git, nvd, gettext e as fontes da identidade visual
```

Sem o shell, use `nix develop path:. -c <comando>` (o `path:` evita que o Nix ignore arquivos ainda não adicionados ao git). Os crates GTK precisam do `pkg-config` do shell.

## Comandos de todo dia

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p zelador-core -- status                      # CLI (lê o seu sistema, não altera nada)
cargo run -p zelador-gtk                                  # o app
nix build .#zelador .#zelador-helper .#zelador-core      # pacotes
nix build .#checks.x86_64-linux.helper-vm                 # teste do helper numa VM NixOS
```

Antes de cada commit: `cargo fmt` e `cargo clippy -- -D warnings` limpos, e testes verdes.

## Estrutura do código

```
crates/core/src/       biblioteca + CLI (veja docs/arquitetura.md, seção 3)
crates/core/tests/     testes de integração e fixtures/ (saídas reais gravadas)
crates/helper/src/     request.rs (sintaxe) · plan.rs (validação → Plan) · exec.rs (execução)
                       roots.rs (locais e ferramentas confiáveis) · lock.rs · serve.rs (socket)
crates/helper/tests/   rejeições e o binário de verdade
crates/gtk/src/        window.rs, pages.rs, screens/, widgets/, style.rs, ctx.rs, i18n.rs...
crates/gtk/data/       ícones (Phosphor), .desktop, CSS, gresource
nix/                   helper.nix, core.nix, app.nix, module.nix, vm-config.nix, vm-test.nix
po/                    zelador.pot, pt_BR.po, en.po
```

## Convenções

- `thiserror` na biblioteca, `anyhow` só nos binários. **Sem `unwrap`/`expect` fora de testes** (lint `deny`; `clippy.toml` permite nos testes). `unsafe` é proibido no workspace.
- Logs com `tracing`.
- **Nunca** execute comandos destrutivos ou privilegiados no seu sistema real durante o desenvolvimento. Testes destrutivos são só na VM.
- **Nunca** edite arquivos `.nix` fora do repositório.
- Interface: siga `design/mockups/` e os tokens do `SPEC.md`. Na dúvida entre um widget libadwaita e um próprio, use o do libadwaita.
- Textos da interface em pt-BR, sempre por `tr`/`trf`/`tn` (veja Traduções).

## Fixtures: quando uma CLI muda

Os parsers são testados contra saídas **reais** em `crates/core/tests/fixtures/`. Se uma versão nova do `nix`, `nvd` ou `flatpak` mudar o formato:

1. Capture a saída nova (somente comandos de **leitura**) e substitua a fixture.
2. Rode os testes: a falha mostra exatamente o que mudou.
3. Ajuste o parser e o teste com base na fixture, não o contrário.

Dados pessoais (usuário, caminhos do `$HOME`) devem ser anonimizados nas fixtures.

## Teste do helper numa VM NixOS

`nix build .#checks.x86_64-linux.helper-vm` sobe uma VM com o módulo ativado, dois usuários (`alice`, autorizada pelo polkit sem senha, e `bob`, sem autorização) e uma segunda closure de sistema (specialisation `alternativa`). Ele verifica o socket, as recusas, `activate` em `test`/`boot`/`switch`, `rollback`, `set-boot-default`, `delete-generations`, `gc`, `optimise`, `clean` (simulação e execução), o histórico, o timer e a notificação (com um `notify-send` falso).

Requisitos: KVM (`/dev/kvm`). Em máquinas Intel, `sudo modprobe kvm_intel`. Para repetir um teste já construído use `--rebuild`.

## Capturas de tela headless

```sh
nix develop path:. -c tools/screenshots.sh design/screenshots/minha-pasta
```

Roda o app dentro do compositor `cage` (Wayland sem tela) e tira capturas com `grim`, sem abrir janelas na sua sessão. Variáveis úteis:

| Variável | Efeito |
|---|---|
| `PAGINAS="seu-sistema geracoes"` | Quais telas capturar |
| `ESQUEMA=prefer-dark` | Tema escuro |
| `TEMA_SISTEMA=1` | Usa o tema do sistema em vez da identidade do Zelador |
| `ESPERA=6` | Segundos até capturar |
| `ZELADOR_FLAKE=<pasta>` | Flake a observar (use uma **cópia**, nunca o seu `/etc/nixos`) |
| `ZELADOR_DEV_ACTION=...` | Abre um estado específico: `select-two`, `compare`, `why`, `simulate`, `confirm-clean`, `wizard-demo`, `wizard-update`, `wizard-blocked` |

Variáveis de ambiente de desenvolvimento do app: `ZELADOR_FLAKE`, `ZELADOR_HOST`, `ZELADOR_START_PAGE`, `ZELADOR_DEV_ACTION`, `ZELADOR_LOCALEDIR` (pasta dos `.mo`).

## Traduções (gettext)

```sh
nix develop path:. -c tools/extract-pot.sh   # extrai os textos para po/zelador.pot e atualiza os .po
```

- Os `msgid` são os textos em pt-BR. `tr("texto")`, `trf("Geração {0}, aplicada {1}.", &[...])` (marcadores numerados, que o tradutor pode reordenar) e `tn("{0} commit", "{0} commits", n)` (plural).
- Edite `po/<idioma>.po`; no build, o `app.nix` compila cada um em `share/locale/<idioma>/LC_MESSAGES/zelador.mo`.
- Para testar um idioma: `LANGUAGE=en ZELADOR_LOCALEDIR=<pasta com os .mo> zelador`.
- Para **adicionar um idioma**: `msginit` a partir de `po/zelador.pot`, e envie o `.po`.

## Segurança ao contribuir no helper

Qualquer mudança em `crates/helper` precisa: manter o conjunto de subcomandos fechado, validar no helper (nunca confiar na UI), ter **testes de rejeição** e passar na VM. Veja [seguranca.md](seguranca.md).

## Estado da CI

Não há CI configurada ainda. O equivalente local é: fmt, clippy `-D warnings`, `cargo test --workspace`, `nix build` dos três pacotes e o check da VM.
