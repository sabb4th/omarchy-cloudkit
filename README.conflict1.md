# omarchy-cloudkit

Cliente de nuvem para Omarchy com serviço e CLI em **Rust**, integração com
**rclone**, montagem **FUSE** e o painel QML/Quickshell do Clouds Guacamole.
Não precisa de Python para executar, instalar ou usar o widget.

Na barra, todos os indicadores acompanham dinamicamente a cor do tema da barra:
nuvem simples quando ocioso,
check quando saudável, rotação durante sincronização, seta de envio para uploads
do streaming, pausa (Ⅱ), alterações pendentes (…), offline (×), erro (!) e desligado (⏻).
A sincronização bidirecional usa um indicador comum de atividade; o serviço
não informa a direção de cada transferência. Detalhes ficam no tooltip e painel.

Esta é uma migração em desenvolvimento. Os fluxos locais de montagem e
sincronização foram exercitados com rclone real. Login OAuth e operações em
contas reais de cada provedor ainda precisam de validação antes de uma versão
estável. A reescrita não constitui garantia de ausência de bugs.

## Funcionalidades

- Múltiplas contas e os provedores suportados pelo rclone: Google Drive, OneDrive,
  Dropbox, Box, pCloud, Nextcloud/ownCloud, WebDAV, S3/MinIO/R2, Proton Drive e
  remotos já configurados.
- Drives como pastas em `~/Cloud/Stream`, arquivos sob demanda, cache configurável,
  upload após salvar, acompanhamento de transferências e desmontagem protegida.
- Pastas reais em `~/Cloud/Sync` para trabalho offline, sincronizadas nos dois
  sentidos por `rclone bisync`. Alterações locais usam inotify e debounce;
  alterações remotas chegam periodicamente, ao retomar ou por solicitação.
- Paralelismo configurável (padrão: três pastas simultâneas), pausa por pasta,
  apelidos e caminhos personalizados.
- Cópias de conflito, limite de 50% de exclusões, backups locais, ressincronização
  sem propagar exclusões e reparo automático limitado com estado persistido.
- Filtros globais ou próprios por pasta, editor no painel e ressincronização
  segura após mudanças de regras.
- Guias de cliente OAuth próprio, reconexão, navegação remota, cotas, recentes,
  notificações, visualizador de logs, CLI e serviço systemd de usuário opcional.
- Sockets privados e autenticados por UID; credenciais fora dos argumentos dos
  processos; escritas atômicas e escape de controles no terminal e nos logs.

Filtros próprios **substituem** os globais. Pausar permite que uma sincronização
já iniciada termine. Os backups protegem arquivos locais substituídos ou
apagados; não são um histórico completo da nuvem.

## Configurar pelo painel

Clique na nuvem e depois na engrenagem. O seletor de seção organiza os controles:

- **Geral**: chave geral, notificações e serviço opcional no login.
- **Contas e pastas**: adicionar uma sincronização informando conta, caminho na
  nuvem e pasta local; navegar na nuvem; controlar streaming de cada conta;
  pausar, sincronizar, renomear, filtrar ou remover pastas existentes.
- **Sincronização e desempenho**: intervalo remoto, transferências simultâneas
  por pasta (1–32), verificações por pasta (1–64) e pastas simultâneas (1–8).
  Padrões preservados: 4 transferências, 8 verificações e 3 pastas. Alterações
  de paralelismo valem para os próximos trabalhos, sem interromper os atuais.
- **Cache do streaming**: tamanho máximo, reserva mínima de espaço e idade
  máxima (`24h`, `7d`, `720h`). Mudanças valem para a próxima montagem.
- **Caminhos e proteção local**: raiz das montagens, pasta local padrão de novas
  sincronizações e retenção das versões locais anteriores.
- **Ferramentas, filtros e logs**: motor, logs, exclusões globais e backups locais.

Os campos explicam o efeito de cada opção. Mudar a pasta local padrão não move
sincronizações existentes nem escolhe o destino na nuvem. A criação de pastas no
painel continua usando sincronização **bidirecional**, incluindo exclusões.

## Chave geral e economia de recursos

A chave ao lado de **Cloud Drives** liga ou desliga o funcionamento completo.
Ao desligar, o daemon encerra sincronizações com um prazo de conclusão de 35 s;
se necessário, cancela o restante preservando o estado para recuperação.
Depois desmonta os drives, encerra os processos rclone e remove os observadores.
Uploads pendentes no cache do streaming impedem o desligamento até terminarem.
Contas, preferências de montagem, pastas e pausas individuais são preservadas.

O estado desligado persiste após reiniciar a sessão. O widget permanece na barra
para religar; o processo compartilhado do Omarchy continua atendendo o desktop.
Enquanto desligado, CloudKit não mantém daemon, bridge, rclone ou timer de reconexão.
A chave de cada conta continua controlando somente o streaming daquela conta.

```bash
omarchy-cloudkit power off
omarchy-cloudkit power status  # consulta sem iniciar o daemon
omarchy-cloudkit power on
```

O comando `stop` continua sendo uma parada temporária para manutenção;
use `power off` para impedir a reinicialização automática pelo widget.

## Instalação no Omarchy

Clone o projeto e execute o instalador:

```bash
git clone https://github.com/sabb4th/omarchy-cloudkit.git
cd omarchy-cloudkit
./install.sh
```

O instalador verifica e instala os pacotes ausentes: `rclone`, `fuse3`,
`quickshell`, `wl-clipboard`, `libnotify`, `glib2` e `xdg-utils`; para compilar,
também `rust` e `gcc`.
Requer **rclone 1.75.2 ou superior**, incluindo as APIs RC/bisync usadas pelo plugin.
O instalador recusa versões antigas antes de substituir uma instalação.
A instalação de pacotes usa pacman com elevação; a compilação e a instalação do
plugin usam o usuário normal. Não execute o script inteiro como root.

```bash
./install.sh --link              # desenvolvimento, plugin aponta para o checkout
./install.sh --no-install-deps   # falha se faltar alguma dependência
./install.sh --no-enable         # instala sem habilitar o widget
./install.sh --no-restart        # mantém o serviço atual durante uma atualização
./install.sh --no-build          # usa bin/cloudkit-native já compilado
./install.sh --uninstall         # preserva contas, configurações e arquivos
```

O plugin usa o identificador `sabb4th.cloudkit`; seu nome público e comando são
`omarchy-cloudkit`. O atalho fica em `~/.local/bin/omarchy-cloudkit`. O comando
`bin/guac` permanece como alias de compatibilidade e também executa Rust.

Atualizações preparam uma cópia temporária antes de substituir a instalação.
Falhas ao verificar atividade não são interpretadas como serviço ocioso.
A desinstalação recusa agir enquanto há transferências ou atividade desconhecida.

### Instalação pelo gerenciador / marketplace (configuração manual)

O gerenciador só clona o código: não executa o instalador nem compila Rust.
Antes de habilitar o widget, faça a configuração no próprio checkout:

```bash
omarchy plugin add https://github.com/sabb4th/omarchy-cloudkit.git
cd "$HOME/.config/omarchy/plugins/sabb4th.cloudkit"
./install.sh --in-place
```

`--in-place` mantém o checkout Git e prepara o executável e o atalho da CLI.
Após atualizar pelo gerenciador, execute o mesmo instalador para recompilar.
Não há download automático de binários nem fallback para o daemon Python.

Para remover uma instalação feita pelo gerenciador:

```bash
cd "$HOME/.config/omarchy/plugins/sabb4th.cloudkit"
./install.sh --uninstall
omarchy plugin remove sabb4th.cloudkit
```

Para migrar a primeira versão com ID `local.cloudkit`, desligue o widget antigo
e pare o daemon antes de instalar/habilitar `sabb4th.cloudkit`. Os dados ficam nos
mesmos diretórios XDG; não habilite os dois widgets simultaneamente.

A preparação da listagem está em [docs/MARKETPLACE.md](docs/MARKETPLACE.md).

## Dados de instalações anteriores

Quando `~/.config/guacamole/config.json` existe e não existe uma configuração
nova em `~/.config/omarchy-cloudkit`, o programa reutiliza os diretórios
`guacamole` de configuração, estado e runtime. Isso mantém backups, filtros,
IDs, listagens do bisync e o mesmo lock do daemon anterior. Nada é movido ou
apagado automaticamente.

Instalações novas usam os diretórios XDG `omarchy-cloudkit`. As contas e os
segredos continuam no arquivo de configuração do rclone, incluindo suporte à
variável `RCLONE_CONFIG`.

Antes de trocar uma instalação antiga em execução, aguarde uploads e syncs,
desabilite o widget antigo e pare o serviço antigo. Se havia inicialização no
login, desabilite também a unidade `guacamole.service`. O instalador novo não
remove a instalação antiga nem seus dados. Não habilite as duas versões para
administrar as mesmas pastas simultaneamente.

## CLI

```bash
omarchy-cloudkit power status
omarchy-cloudkit status --json
omarchy-cloudkit stream Drive on
omarchy-cloudkit stream Drive off --force
omarchy-cloudkit keep Drive:Documents/Notes ~/Notes
omarchy-cloudkit sync
omarchy-cloudkit pause Drive:Documents/Notes
omarchy-cloudkit resume Drive:Documents/Notes
omarchy-cloudkit alias Drive:Documents/Notes "Notas"
omarchy-cloudkit exclude --add '*.log'
omarchy-cloudkit resolve Drive:Documents/Notes resync
omarchy-cloudkit browse Drive:Documents
omarchy-cloudkit add-oauth Work drive --own-client
omarchy-cloudkit add-credentials Home nextcloud < credentials.json
omarchy-cloudkit service install
omarchy-cloudkit --help
```

`FOLDER` pode ser ID, apelido, `REMOTE:PATH` ou caminho local. Remover uma pasta
ou uma conta preserva os arquivos locais; `unkeep --delete-local` solicita o uso
da lixeira pelo `gio`.

`keep` é sincronização bidirecional, incluindo exclusões. O modo de backup
somente de envio (`rclone copy`) ainda não é oferecido pelo plugin.

## Desenvolvimento e validação

```bash
cargo build --release --locked
install -m 755 target/release/omarchy-cloudkit bin/cloudkit-native
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
bash -n install.sh bin/omarchy-cloudkit bin/guac bin/guac-bridge
python3 tests/test_install.py
python3 tests/test_ipc.py
CLOUDKIT_BIN="$PWD/target/release/omarchy-cloudkit" python3 tests/test_daemon.py
```

Python é usado apenas em testes, ferramentas de desenvolvimento e na referência
anterior mantida em `lib/guac`. Essa referência não é copiada na instalação.
Os testes de ponta a ponta usam diretórios, configuração rclone e montagens
FUSE descartáveis. Não apontam para suas contas reais.

A arquitetura, as evidências e as limitações estão em
[docs/RUST-MIGRATION.md](docs/RUST-MIGRATION.md). A documentação original foi
preservada em [docs/UPSTREAM-README.md](docs/UPSTREAM-README.md) como referência,
com instruções referentes à implementação anterior.

## Licença

MIT. Fork de Clouds Guacamole, de Alessandro Pizzigolotto (chickymonkeys), com
partes do widget derivadas de ODrive, de TiniTinyTerminator. Os avisos originais
foram preservados em [LICENSE](LICENSE).
