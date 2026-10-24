# Arquitetura do Zelador

Este documento explica **como cada parte funciona** e por que foi desenhada assim. Para o modelo de segurança, veja [seguranca.md](seguranca.md); para compilar e contribuir, [desenvolvimento.md](desenvolvimento.md).

## 1. Objetivo e princípios

O Zelador resolve um problema de **operação** de quem mantém um NixOS declarativo com flakes: depois de escrever a configuração, ainda é preciso atualizar inputs, conferir o que muda, ativar, voltar quando algo quebra e limpar o store. Hoje isso é um conjunto de comandos (`nix flake update`, `nixos-rebuild`, `nix-env --list-generations`, `nix-collect-garbage`...) com saídas longas e sem visão do conjunto.

Princípios que guiam todas as decisões:

1. **Camada fina sobre as ferramentas existentes.** O Zelador não reimplementa o Nix. Ele chama `nix`, `git`, `flatpak` (e opcionalmente `nvd`), interpreta a saída e a apresenta.
2. **Ler muito, escrever pouco.** Quase tudo é leitura. O que altera o sistema é um conjunto pequeno e explícito de operações.
3. **Privilégio mínimo, isolado e auditável.** A interface nunca é root. O código privilegiado é um binário pequeno, com subcomandos fechados e validação própria.
4. **Mostrar antes de fazer.** Atualização é construída e revisada antes de ser aplicada; limpeza pode ser simulada.
5. **Não editar a sua configuração.** Nenhum `.nix` é tocado. O `flake.lock` só é gravado depois de uma ativação bem-sucedida.
6. **Testável sem a máquina real.** Toda a lógica fica atrás de traits e é testada com saídas reais gravadas (fixtures); o helper é testado numa VM NixOS.

## 2. Visão geral do workspace

```
Zelador/
├── crates/
│   ├── core/      zelador-core   biblioteca + CLI (sem GTK, sem privilégio)
│   ├── helper/    zelador-helper binário privilegiado (root, via systemd)
│   └── gtk/       zelador-gtk    o app (binário `zelador`)
├── nix/           pacotes, módulo NixOS, teste de VM
├── flake.nix      pacotes, módulo, check de VM, devShell
├── po/            catálogos de tradução (gettext)
├── design/        mockups, ícones e capturas de tela
├── docs/          esta documentação
└── tools/         scripts de apoio (screenshots, extração de textos)
```

### Por que três crates

| Crate | Papel | Depende de GTK | Roda como |
|---|---|---|---|
| `zelador-core` | Parsing, estimativas, planos, backends das CLIs, histórico, verificação | não | usuário |
| `zelador-helper` | Executar operações privilegiadas validadas | não | root |
| `zelador-gtk` | Janelas, telas, navegação, tema, traduções | sim | usuário |

Manter o `core` sem GTK permite testar tudo sem display e expor a mesma lógica numa CLI (`zelador-core`). Manter o `helper` separado e com poucas dependências (`thiserror`, `rustix`) reduz a superfície do único código que roda como root.

Convenções do código: `thiserror` na biblioteca e `anyhow` só nos binários; sem `unwrap`/`expect` fora de testes (lint `deny`); `unsafe_code = "forbid"` no workspace inteiro; logs com `tracing`.

## 3. O `core`

### 3.1 Backends atrás de traits

Cada ferramenta externa está atrás de um trait, para trocar a implementação quando a CLI mudar e para testar com dados gravados:

| Trait | Implementação real | Faz |
|---|---|---|
| `NixBackend` | `CliNix` | `nix path-info`, `nix-store --gc --print-dead`, `nix store diff-closures` (ou `nvd`), listar gerações, `nix flake update/lock`, `nix-store --query` |
| `RebuildBackend` | `NixBuild` | monta os comandos `nix build ...#nixosConfigurations.<host>.config.system.build.toplevel` (real e `--dry-run`) |
| `GitBackend` | `CliGit` | `git status`, contagem de commits desde uma revisão, `git add` dos arquivos novos (com confirmação) |
| `FlatpakBackend` | `CliFlatpak` | listar/remover runtimes sem uso (usuário e sistema) |

Por baixo estão `CommandRunner` e `StreamingRunner` (módulo `runner`): executam processos com ambiente controlado e, no caso do *streaming*, entregam as linhas à medida que chegam e aceitam cancelamento (`CancelToken`, que envia SIGINT ao `nix`).

### 3.2 Fixtures e CLIs que mudam

`crates/core/tests/fixtures/` guarda saídas **reais** (anonimizadas quando necessário) de `nix flake metadata --json`, `nix path-info --json`, `nix-store --gc --print-roots`, `git status --porcelain=v2`, `flatpak`, progresso `internal-json` de builds (com sucesso e com erro de avaliação), `nix build --dry-run`, `nvd diff` etc. Regra do projeto: **se uma CLI mudou de saída, atualiza-se a fixture e o teste**, em vez de ajustar o parser às cegas.

### 3.3 Módulos principais

| Módulo | Responsabilidade |
|---|---|
| `status` | Estado geral: geração em uso, commits pendentes (exato ou aproximado), inputs, Flatpak, GC/optimise automáticos |
| `flake` | Lê `flake.lock`/`flake metadata`: inputs, revisões, datas, comparação entre dois locks |
| `generations` | Gerações do sistema (`/nix/var/nix/profiles/system-N-link`) e do Home Manager |
| `gcroots` | GC roots do usuário: identifica links `result` e `.direnv` esquecidos |
| `store`, `space` | Tamanhos (`narSize`) e a divisão do store em categorias |
| `cleanup` | Política de limpeza, plano, estimativa, cliente do helper, execução por passos |
| `update`, `build`, `progress` | Preparar o lock novo, construir e interpretar o progresso `internal-json` |
| `precheck`, `disk` | Estimativa de download via `--dry-run` e checagem de espaço |
| `diff`, `nvd` | Resumo das mudanças de pacotes (via `diff-closures` ou `nvd`) |
| `failure` | Extrai a causa de uma falha de build e dicas (disco cheio, rede) |
| `apply` | Ativação pelo helper e eventos de progresso |
| `history` | Histórico em SQLite |
| `check` | Verificação periódica (timer) e notificação |

## 4. Fluxo: Seu sistema (leitura)

`status::collect` reúne, sem privilégio:

1. **Geração em uso**: resolve `/run/current-system` e acha o número da geração em `/nix/var/nix/profiles`.
2. **Commits pendentes**: a geração em uso pode registrar a revisão do flake com que foi construída (`system.configurationRevision`, que o NixOS grava em `/run/current-system/configuration-revision`). Se essa revisão existe, conta-se os commits do repositório desde ela. Quando não é possível (a revisão não foi registrada, o build era *dirty*, ou o commit sumiu do repositório), usa-se a **data** da geração e a contagem vira **aproximada**, e a UI avisa o motivo.
3. **Inputs**: parse do `flake.lock` e idade de cada input (cores indicam inputs velhos).
4. **Limpeza automática**: avalia, só para leitura, `nix.gc.automatic`, `nix.optimise.automatic` e `services.flatpak.uninstallUnused` (nix-flatpak) na configuração do seu flake, para dizer se a manutenção automática já está ligada.
5. **Tamanho do store**: a soma dos `narSize` de todos os caminhos, ajustada pelo uso real do disco (ver seção 8).

Tudo carrega em threads de fundo (`loader::Shared` e `run_in_background` no app), então a janela abre na hora e as seções aparecem quando ficam prontas.

## 5. Fluxo: Atualizar

O assistente tem três etapas e **nada muda no sistema até a última**.

### Etapa 1: escolher
Lista os inputs com idade. Pode-se atualizar todos, **só um** (`Atualizar só este`) ou **fixar** um input numa revisão de 40 caracteres.

### Etapa 2: construir (sem privilégio)
1. **Lock alternativo.** O Zelador **não** modifica o seu `flake.lock`. Ele gera um lock novo numa pasta de estado (`~/.local/state/zelador/update`) com `nix flake update --flake <pasta> --output-lock-file <novo> [inputs...]` (ou `nix flake lock <pasta> --reference-lock-file ... --override-input ...` para fixar). O build usa esse arquivo por `--reference-lock-file`.
2. **Avaliar e pré-checar disco.** `nix build ... --dry-run` informa o que será baixado/construído. O app estima o necessário (download + 2 GiB de margem + fração por derivação) e **bloqueia** se o espaço livre não bastar, explicando quanto falta.
3. **Construir.** `nix build <flake>#nixosConfigurations.<host>.config.system.build.toplevel --out-link ... --log-format internal-json -v`. O app interpreta as linhas `@nix {...}` do stderr (contadores de builds e downloads, log do build corrente) e mostra progresso real. Usa-se `nix build` direto, e não `nixos-rebuild build`, porque este reexecuta a si mesmo a partir do nixpkgs novo, disparando builds extras e escondendo o progresso.
4. **Falha.** Se o build falhar, o `failure` extrai a **causa** (por exemplo, "a opção `boot.initrd.systemd.enable` foi removida"), localiza arquivos seus citados no erro (com opção de abri-los), oferece o log completo (`nix log`), copiar o erro e **tentar de novo sem atualizar um input**. Dicas específicas aparecem para disco cheio e falha de rede.

### Etapa 3: revisar e aplicar
- **Resumo de mudanças** via `nix store diff-closures` (padrão) ou `nvd` (se instalado e habilitado no menu): pacotes atualizados, novos, removidos e variação de tamanho.
- Aviso de **kernel novo** (recomenda aplicar agora e reiniciar).
- Escolha do modo, que corresponde ao `switch-to-configuration`:
  - **Agora** (`switch`): ativa e registra a geração.
  - **No próximo reinício** (`boot`): registra a geração e prepara o boot, sem alterar o que roda agora.
  - **Só até reiniciar** (`test`): ativa sem registrar geração; ao reiniciar volta a anterior.
- Se há arquivos novos fora do git que o flake ignoraria, o app oferece `git add` (com confirmação; nada é commitado).

### Aplicando
O app chama o helper (`activate <caminho> <modo>`). O helper registra a geração (`nix-env -p .../system --set`, exceto em `test`) e roda `switch-to-configuration` **numa unidade transitória do systemd** (`systemd-run`, unidade `zelador-switch-to-configuration`), para que a ativação continue mesmo que a janela ou o serviço do helper sejam reiniciados. O helper anuncia cada passo com linhas `@zelador {"step":i,"of":n,...}` no stderr, e o app reconhece as fases do `switch-to-configuration` (parar/iniciar serviços, atualizar o menu de boot...) para mostrar o progresso.

Depois da ativação com sucesso, o app **grava o novo `flake.lock`**. "Descartar esta atualização" apaga o estado e não toca em mais nada. A ativação não é cancelável depois de iniciada.

## 6. Fluxo: Gerações

Lê as gerações do sistema e do Home Manager (nas instalações em que o HM é módulo do NixOS, a closure dele está dentro da do sistema, e o app trata isso). Ações:

| Ação | Como |
|---|---|
| **Comparar** duas | `diff-closures`/`nvd` entre as closures |
| **Voltar para esta** | helper `rollback <n>` (ativa e torna padrão) |
| **Tornar padrão no boot** | helper `set-boot-default <n>` |
| **Excluir** | helper `delete-generations <n...>`. O helper **recusa** a geração em uso e a padrão do boot |

As gerações do Home Manager **não passam pelo helper**: a exclusão é feita pelo app, com o `nix-env` do perfil do usuário (sem privilégio). Voltar para uma geração do Home Manager não é feito pelo Zelador: use o próprio `home-manager`.

## 7. Fluxo: Liberar espaço

### Política e plano
A política (`Policy`) define: manter as últimas **N gerações do sistema** (padrão 5) **e** todas dos últimos **D dias** (padrão 7), as últimas **M do Home Manager** (padrão 3), remover links `result`/`.direnv` esquecidos, runtimes Flatpak sem uso e deduplicar o store. `build_plan` transforma isso em um `CleanupPlan` com **passos**:

| Passo | Privilégio |
|---|---|
| Remover links esquecidos (só links para o store dentro do `$HOME`) | não |
| Excluir gerações do Home Manager | não |
| Runtimes Flatpak de **usuário** | não |
| Gerações do sistema, Flatpak de **sistema**, coleta de lixo, `optimise` | **sim**, numa **única** chamada agregada `clean` ao helper |

A chamada agregada existe para que a limpeza peça **uma senha só**, não uma por passo. O helper executa na ordem fixa gerações → Flatpak → GC → `optimise`.

### Simulação
`clean` sem `--execute`/o botão **Simular** não altera nada: mostra quais gerações sairiam, quais links, e quantos caminhos o GC apagaria hoje (`nix-store --gc --print-dead`).

### Estimativa de espaço
O Nix não diz o que cada raiz mantém vivo, então o app monta um `ClosureIndex` (closure de cada raiz) e calcula o que **só** as raízes descartadas mantinham. Soma os `narSize`. Como o `narSize` é o tamanho **lógico** e ignora arquivos compartilhados por *hardlink* (do `optimise`), a soma pode passar do tamanho do disco; por isso o app aplica uma razão (`disk::real_ratio`) quando isso acontece, e os textos dizem **"Até X"** (limite superior). A estimativa de gerações é aproximada por construção e a de `optimise` é impossível de prever, e a UI diz isso.

## 8. Fluxo: Espaço em disco

`space::breakdown` classifica cada caminho do store, em ordem de prioridade: **sem referência** (o GC apagaria) → **Home Manager atual** → **sistema atual/padrão do boot** → **gerações antigas** → **links esquecidos** → **ambientes de desenvolvimento** (tudo que sobra: `.direnv`, `nix develop`, pacotes avulsos). Dividido em "Em uso" e "Pode sair". A lista dos maiores pacotes usa `closureSize`, e "Por quê?" roda `nix why-depends` do sistema até o pacote, para mostrar a cadeia de dependência.

## 9. O helper privilegiado

Resumo; o detalhe de segurança está em [seguranca.md](seguranca.md).

```
zelador-helper --call <pedido>      (cliente, sem privilégio, usado pelo app)
        │  envia o pedido por /run/zelador/helper.sock
        ▼
systemd (zelador-helper.socket, Accept=yes)
        │  inicia, como root, uma instância por conexão
        ▼
zelador-helper --serve
   1. lê o pedido (limite de 16 KiB, 256 argumentos, 10 s)
   2. parse_args  → Request (conjunto fechado de subcomandos)
   3. validate    → recusa cedo o que é inválido (sem pedir senha)
   4. SO_PEERCRED → pid/uid de quem conectou (o kernel informa)
   5. pkcheck     → polkit autoriza a action br.dev.zelador.helper
   6. flock       → uma operação privilegiada por vez
   7. validate de novo (o estado pode ter mudado)  → Plan
   8. executa o Plan, passo a passo, com ambiente limpo
   9. escreve "@zelador-exit <código>" e termina
```

Código de saída: `0` sucesso, `65` pedido recusado, `70` erro interno, `75` outra operação em andamento, `77` polkit não autorizou; falha de um comando filho devolve o código dele.

## 10. O app GTK

- **Estrutura:** `AdwNavigationSplitView` com barra lateral (Seu sistema, Atualizar, Gerações, Liberar espaço, Espaço em disco) e páginas. A tela Aplicando é aberta pelo fluxo de atualização.
- **Contexto compartilhado (`Ctx`):** carrega uma vez (e compartilha entre telas) o estado geral, os tamanhos do store e os caminhos mortos. Há barramentos de eventos (`Bus`) para o progresso da atualização e para o histórico.
- **Assincronia:** nada bloqueia a thread da interface. `run_in_background` executa trabalho numa thread e devolve o resultado à main loop; `stream_in_background` entrega eventos contínuos por canal.
- **Identidade visual:** os tokens (cores, raios, fontes) do "v3" ficam em `style.rs` e viram um único `CssProvider`, em versões clara, escura e "sistema". Por padrão a identidade do Zelador vence o `~/.config/gtk-4.0/gtk.css` do usuário; o menu tem a opção **"Usar o tema do sistema"**. Ícones Phosphor e o CSS são embutidos no binário (gresource); widgets próprios existem só onde o libadwaita não serve (por exemplo, as colunas alinhadas à esquerda).
- **Preferências** (`~/.config/zelador/config.ini`): usar o tema do sistema; preferir o `nvd`.
- **Internacionalização:** os `msgid` são os próprios textos em pt-BR (`tr`, `trf` com `{0}`, `{1}`, `tn` para plural). Os catálogos `.po` ficam em `po/` e viram `.mo` instalados em `share/locale`.
- **Histórico:** SQLite em `$XDG_DATA_HOME/zelador/history.db` (padrão `~/.local/share/zelador/`). Guarda tipo, início, duração, resultado, geração e espaço liberado das últimas 500 operações, mais a data do último aviso por condição (anti-spam do timer). A tela mostra as 3 mais recentes.

## 11. Verificação periódica (timer)

Com `programs.zelador.checkTimer.enable`, o módulo cria um **serviço e um timer de usuário** (`zelador-check`) que roda `zelador-core check --notify`. Ele avalia: inputs mais velhos que `staleDays`, espaço livre abaixo de `minFreeGiB`, e lixo recuperável acima de `reclaimableGiB`. Se algo dispara e já não foi avisado nos últimos 7 dias, envia **uma** notificação (via `notify-send`) com a ação "Abrir o Zelador". **Nunca age**: não atualiza nem limpa nada sozinho.

## 12. Empacotamento Nix

- `nix/helper.nix`: compila o helper e **fixa por caminho do store** as ferramentas que ele executa como root (`nix-env`, `nix-store`, `systemd-run`, `flatpak`, `pkcheck`). Nada vem do `PATH` em tempo de execução.
- `nix/core.nix` e `nix/app.nix`: compilam a CLI e o app; apontam o helper do build; acrescentam `nix`, `git` e `flatpak` **no fim** do `PATH` (o do sistema vence); o app instala ícones, `.desktop` e os `.mo` das traduções.
- `nix/module.nix`: o módulo `programs.zelador` (opções, socket, serviço do helper, polkit, fontes, timer opcional).
- `nix/vm-config.nix` + `nix/vm-test.nix`: a VM de teste (ver desenvolvimento).

## 13. Decisões e trade-offs

| Decisão | Motivo |
|---|---|
| `nix build` direto, não `nixos-rebuild` | Progresso fiel e sem builds extras |
| Lock alternativo (`--reference-lock-file`) | Nunca modifica o `flake.lock` antes da hora; descartar é apagar uma pasta |
| Helper com subcomandos fechados + `clean` agregado | Superfície mínima e uma senha por limpeza |
| Socket do systemd em vez de `pkexec` | Sem setuid no sistema inteiro; o polkit continua sendo a autoridade |
| `systemd-run` para ativar | A ativação sobrevive à janela e ao serviço do helper |
| Estimativas como "limite superior" | O Nix não dá o número exato antes de limpar; melhor ser honesto |
| Timer só avisa | O app não tem agendador próprio de limpeza (fora de escopo) |
