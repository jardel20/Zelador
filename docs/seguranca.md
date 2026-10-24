# Modelo de segurança

O Zelador altera o sistema (ativa configurações, apaga gerações, roda a coleta de lixo). Esta página descreve **o que protege o seu sistema**, o que o código privilegiado aceita e recusa, e o que **não** é garantido.

## Regras invioláveis

1. O app **nunca edita arquivos `.nix`** do usuário. O único arquivo do flake que ele grava é o `flake.lock`, depois de uma ativação bem-sucedida.
2. A interface **nunca roda como root** e não tem nenhum bit especial (sem setuid, sem capabilities).
3. Nada destrutivo ou privilegiado acontece **sem confirmação explícita** do usuário, e cada operação privilegiada exige **senha de administrador** (autorização do polkit a cada vez).

## Peças e privilégios

| Peça | Roda como | Pode fazer |
|---|---|---|
| `zelador` (app GTK) | usuário | Ler o sistema, construir atualizações (`nix build`), gravar o próprio estado em `~/.local`, excluir gerações do Home Manager, links `result` esquecidos e runtimes Flatpak **do usuário** |
| `zelador-core` (CLI) | usuário | O mesmo que o app, em texto |
| `zelador-helper --call` | usuário | Só enviar um pedido ao socket e repassar a resposta |
| `zelador-helper --serve` | **root**, iniciado pelo systemd | Executar o conjunto fechado de operações abaixo, depois de validar e autorizar |

## Como o pedido chega ao helper

Não há `pkexec` nem binário setuid.

1. O módulo NixOS cria `zelador-helper.socket`, que escuta em `/run/zelador/helper.sock` (modo `0666`: qualquer usuário local pode **conectar**; poder conectar não autoriza nada).
2. O systemd (`Accept=yes`) inicia **uma instância de `zelador-helper --serve` como root por conexão**, com o socket como stdin/stdout/stderr. Limites: no máximo 16 conexões simultâneas e 4 por usuário.
3. O helper lê o pedido: argumentos terminados por NUL, **no máximo 16 KiB e 256 argumentos**, e com **limite de 10 s** de leitura (um cliente que conecta e não escreve não prende o serviço).
4. O helper valida o pedido (abaixo) **antes** de pedir a senha: pedidos inválidos nunca chegam a mostrar um diálogo.
5. O helper descobre **quem** conectou pelo kernel (`SO_PEERCRED`: PID e UID, não forjáveis pelo cliente) e lê o instante de início do processo em `/proc/<pid>/stat` para proteger contra reuso de PID.
6. Ele chama `pkcheck --action-id br.dev.zelador.helper --process <pid>,<início>,<uid> --allow-user-interaction`. O polkit exibe o diálogo de senha por meio do **agente de autenticação da sessão do usuário**. A action usa `auth_admin` (e `allow_any=no`): **sem `auth_admin_keep`, nada fica autorizado em segundo plano**.
7. Só então toma uma trava exclusiva (`flock` em `/run/zelador-helper.lock`: uma operação privilegiada por vez), **valida de novo** (o estado pode ter mudado enquanto esperava) e executa.

Resultado e progresso voltam pelo mesmo socket; o código de saída vai na última linha (`@zelador-exit N`).

## O conjunto fechado de operações

O helper aceita **somente** estes subcomandos, sem argumentos livres. Qualquer outra coisa é recusada (código 65).

| Subcomando | Validação feita **pelo próprio helper** |
|---|---|
| `activate <caminho> <switch\|boot\|test>` | O caminho precisa ser **absoluto**, estar **diretamente em `/nix/store`**, existir, **não ser link simbólico** nem resolver para outro lugar, ser um diretório e ser um **sistema NixOS** (conter `bin/switch-to-configuration` executável). Rejeita `..`, caminhos relativos e arquivos soltos |
| `rollback <n>`, `set-boot-default <n>` | `n` é um inteiro positivo; a geração `system-<n>-link` precisa existir |
| `delete-generations <n...>` | Inteiros positivos, sem repetição; **recusa a geração em uso e a padrão do boot** |
| `gc`, `optimise` | Sem argumentos |
| `flatpak-remove-unused` | Só se existir instalação Flatpak **de sistema** |
| `clean [--generations n...] [--flatpak] [--gc] [--optimise]` | Gramática fixa (cada opção no máximo uma vez, ao menos uma presente). Executa, nesta ordem: gerações → Flatpak → GC → `optimise`. As mesmas validações das operações individuais |

Outras defesas na execução:

- **Ferramentas por caminho do store, fixadas no build** (`nix-env`, `nix-store`, `systemd-run`, `flatpak`, `pkcheck`). Nada é resolvido pelo `PATH` nem pelo ambiente do cliente.
- **Ambiente limpo** nos processos filhos (`PATH` mínimo, `HOME=/root`, sem stdin).
- **Nenhum caminho, flag ou variável vem do cliente** além do que a gramática acima permite.
- A ativação (`switch-to-configuration`) roda numa **unidade transitória do systemd** (`systemd-run`), isolada do ciclo de vida do cliente e do serviço do helper.
- Falhas de escrita no socket (cliente que fechou a janela) **não interrompem** uma operação em andamento; ela termina de forma consistente.

## Códigos de saída

`0` sucesso · `65` pedido recusado na validação · `70` erro interno/ambiente · `75` outra operação em andamento · `77` o polkit não autorizou · qualquer outro valor: código do comando filho que falhou.

## Testes que sustentam isto

- **Testes de rejeição** (`crates/helper/tests`): caminhos fora do store, `..`, links simbólicos, argumentos extras, gerações protegidas, gramática do `clean`. **Nenhum teste aceita um pedido válido no sistema real.**
- **Teste em VM NixOS** (`nix build .#checks.x86_64-linux.helper-vm`): sobe uma máquina virtual, ativa o módulo e verifica, **sem pkexec setuid**, que (a) o socket existe, (b) um usuário sem autorização do polkit recebe 77, (c) pedidos inválidos recebem 65, (d) `activate` em `test`, `boot` e `switch` funciona, (e) `rollback`, `set-boot-default`, `delete-generations`, `gc` e `clean` funcionam, e a geração protegida é recusada. Todo teste destrutivo roda só na VM.

## O que **não** é garantido

- **Quem pode autenticar como administrador pode fazer tudo.** O Zelador não é uma barreira contra um administrador; ele reduz o risco de erro e limita o que um processo comum consegue pedir.
- **Depende do polkit e do agente de autenticação da sessão.** Se uma regra local do polkit autorizar a action sem senha, o helper obedecerá (a VM de teste faz isso de propósito). Revise regras próprias do polkit.
- **O socket é acessível a todos os usuários locais**, para poder conectar; a defesa é a validação do helper e a autorização do polkit, não a permissão do arquivo.
- **O app lê e executa `nix`, `git` e `flatpak` como o usuário.** Rodar `nix build` de um flake é executar código de avaliação do Nix, como sempre foi.
- **Notificações do timer** passam pelo `notify-send` do sistema; o texto mostra nomes de inputs e tamanhos, nada além.

## Reportar um problema de segurança

Abra uma *issue* privada (ou um *security advisory*) no repositório com a descrição e, se possível, um pedido que o helper aceitou e não deveria. Não inclua dados pessoais.
