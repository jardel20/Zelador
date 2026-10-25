# Contribuindo com o Zelador

Obrigado por querer ajudar! Contribuições em **português ou inglês** são bem-vindas: issues, correções, traduções, documentação e testes.

*(English: contributions in Portuguese or English are welcome. The checklist below applies to both; the UI strings live in `po/` and need a `.po` file for each language.)*

## Antes de começar

1. Leia o [README](README.md), a [arquitetura](docs/arquitetura.md) e o [modelo de segurança](docs/seguranca.md).
2. Para algo grande (nova tela, nova operação privilegiada), **abra uma issue antes** para alinhar o escopo. O projeto é de propósito pequeno: fora de escopo estão editor de configuração, loja de pacotes, canais/`nix-env`, contêineres e agendador próprio de limpeza.

## Regras do projeto

- **Nunca** editar arquivos `.nix` do usuário. O único arquivo do flake que o app grava é o `flake.lock`, após uma ativação com sucesso.
- **A interface nunca roda como root.** O que exige privilégio passa pelo `zelador-helper`.
- **Nada destrutivo sem confirmação**, e testes destrutivos **só na VM**, nunca no seu sistema real.
- `thiserror` no `core`, `anyhow` só nos binários. **Sem `unwrap`/`expect` fora de testes** (o clippy barra). `unsafe` é proibido.
- Logs com `tracing`.
- Mudou a saída de uma CLI (`nix`, `nvd`, `flatpak`)? **Atualize a fixture e o teste**; não "conserte" o parser às cegas. Anonimize dados pessoais nas fixtures.
- Interface: siga `design/mockups/` e os tokens em `crates/gtk/resources/css/tokens-*.css`. Na dúvida entre um widget do libadwaita e um próprio, use o do libadwaita.

## Mudanças no helper privilegiado

Qualquer alteração em `crates/helper`, na action do polkit ou no módulo NixOS precisa:

1. manter o conjunto de subcomandos **fechado** (adicionar um é uma decisão de segurança: justifique na issue/PR);
2. validar **no helper**, sem confiar na UI;
3. ter **testes de rejeição** (caminhos fora do store, links simbólicos, argumentos extras...);
4. passar no teste de VM: `nix build .#checks.x86_64-linux.helper-vm`.

## Fluxo

```sh
nix develop                                   # ambiente com Rust, GTK4, libadwaita, gettext...
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

A CI (GitHub Actions) roda formatação, clippy, testes, build dos pacotes, avaliação do flake e a VM do helper. Um PR só é aceito com tudo verde. Detalhes (fixtures, screenshots headless, traduções) em [docs/desenvolvimento.md](docs/desenvolvimento.md).

### Commits e PRs

- Mensagens no imperativo e explicando o **porquê** (em português ou inglês).
- PRs pequenos e focados; preencha o modelo de PR (testes e checklist de segurança).
- Não inclua dados pessoais (e-mails, caminhos do seu `$HOME`, saídas completas do seu sistema).

## Traduções

Os textos da interface estão em `po/`. Para corrigir o inglês ou adicionar um idioma:

```sh
nix develop -c tools/extract-pot.sh            # atualiza po/zelador.pot e os .po existentes
msginit -i po/zelador.pot -l <idioma> -o po/<idioma>.po   # novo idioma
```

A tradução atual para o inglês foi feita por IA e **não foi revisada por uma pessoa**: revisões são muito bem-vindas.

## Relatando problemas

Use os modelos de issue. **Falhas de segurança**: não abra issue pública, veja [SECURITY.md](SECURITY.md).

## Licença

Ao contribuir, você concorda em licenciar sua contribuição sob a [MIT](LICENSE).
