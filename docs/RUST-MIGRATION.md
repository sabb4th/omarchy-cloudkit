# Migração para omarchy-cloudkit

## Estado

O caminho de execução do daemon, CLI, ponte IPC e inicialização do serviço é
Rust. O widget permanece QML/Quickshell e o motor permanece rclone/FUSE.
Não há fallback Python na execução. `lib/guac` é a referência anterior para
comparação e testes, não uma dependência instalada.

Isso é uma implementação inicial completa do caminho de execução, ainda em
validação. Não significa paridade comprovada em todos os provedores, nem uma
versão estável auditada. OAuth, particularidades de servidores e contas reais
precisam de validação adicional.

## Controle global (0.2.0)

O estado de energia usa um marcador privado `disabled` no diretório de
configuração, sem reescrever as preferências de contas ou pastas. CLI, daemon e
widget respeitam esse estado, inclusive após novo login. O widget não mantém
ponte IPC nem reconexão enquanto desligado. Cancelamento no desligamento não é
tratado como falha de resync; a primeira sincronização pode continuar ao religar.

## Arquitetura

| Módulo | Responsabilidade |
| --- | --- |
| `src/cli.rs` | Comandos, prompts, início do daemon e unidades systemd |
| `src/daemon.rs` | Ator de estado, comandos IPC, agendamento, recuperação e eventos |
| `src/model.rs` | Modelo publicado ao painel e estado persistente das pastas |
| `src/rc.rs` | HTTP sobre socket Unix e ciclo de vida dos processos rclone |
| `src/jobs.rs` | Montagem, bisync, inotify, recentes e backups |
| `src/auth.rs` | Credenciais, OAuth, OneDrive e região do pCloud |
| `src/storage.rs` | XDG, compatibilidade, configuração, escrita atômica e texto seguro |
| `src/filters.rs` | Regras, validação, matching e mudanças seguras de filtros |
| `src/ipc.rs` | Autenticação SO_PEERCRED, ponte e consulta conservadora de atividade |
| `src/providers.rs` | Provedores, guias e validação de cliente próprio |

O ator recebe eventos por uma fila limitada. Transferências, montagem e OAuth
rodam em tarefas separadas. Clientes recebem snapshots por watch channels, sem
acumular uma fila ilimitada de estados. Há limites de quadros, de respostas HTTP
e de solicitações simultâneas por conexão. Tarefas rclone independentes preservam
o isolamento de logs do bisync.

Rust usa Tokio, reqwest com rustls, serde_json, regex, md5 e libc. MD5 identifica
mudanças nas regras e mantém compatibilidade com o estado anterior; não autentica
conteúdo nem credenciais. As bibliotecas são incorporadas ao executável. Rust,
Cargo e linker só são necessários para compilar; Python não é necessário no uso.

## Compatibilidade e segurança

- Mantidos os comandos IPC usados pelo widget e seus campos de estado.
- Mantidos rclone.conf, opções de VFS, bisync, limite de exclusões, cópias de
  conflito, backups locais e resync sem propagar exclusões.
- Configuração e estado antigos continuam nos diretórios `guacamole` quando um
  perfil existente é detectado. Isso evita mudar os nomes de sessões do bisync.
- Runtime compartilhado com o perfil antigo inclui o mesmo lock, evitando dois
  daemons concorrentes. Uma instalação antiga já rodando deve ser parada durante
  a troca; o cliente não a converte automaticamente em Rust.
- Credenciais seguem por sockets privados e pelo ambiente do subprocesso OAuth,
  nunca em argv. Erros de credenciais são redigidos antes da publicação.
- Diretórios de socket inseguros são recusados; nenhum chmod tenta torná-los
  confiáveis. Configuração inválida é preservada e impede uma inicialização que
  a sobrescreveria.
- A ponte valida o UID do servidor antes de transmitir dados. O daemon valida o
  UID dos clientes. Arquivos atômicos usam criação exclusiva e modo 0600.
- O instalador prepara os arquivos antes de substituir uma instalação e mantém a
  anterior se a cópia falhar. Atividade desconhecida não autoriza reinício.

## Correções verificadas durante a migração

Os testes detectaram respostas antes da publicação do estado, diferenças nos
códigos de desmontagem, problemas com montagens deixadas por um rclone encerrado,
diferenças na navegação e apresentação de controles de terminal e publicação
tardia após remover pastas. Esses casos foram corrigidos e os cenários originais
foram executados novamente contra o daemon Rust.

O código também evita gravar state.json a cada tick ocioso, limita as varreduras
de arquivos recentes, ajusta a frequência das consultas de métricas conforme a
atividade, mantém o catálogo de provedores em cache e limita o runtime Tokio a
quatro workers. O monitor usa poll e agrupa eventos de uma mesma rajada.

## Evidências e limites

A suíte original de 19 cenários passou contra o executável Rust: streaming,
writeback, desmontagem ocupada/forçada, sincronização nos dois sentidos,
exclusões, aliases, filtros, pausa/retomada, conflitos, safety stop, recentes,
recuperação do engine, takeover, paralelismo, preservação de arquivos, validação,
texto hostil, mudança de raízes, comandos e shutdown. Dois cenários adicionais
exercitam remoção de conta montada preservando arquivos locais e monitoramento de
arquivos após renomear uma subpasta.

Os testes IPC incluem a ponte Rust, rejeição de diretório inseguro, mensagens,
EOF e controles reais do widget offscreen. Testes nativos adicionais verificam
recuperação limitada e persistida, backoff, conflitos após falha, credenciais
Nextcloud via RC simulado, rollback/redação de segredos e escritas atômicas.

A suíte Python de referência não deve ser confundida com cobertura nativa:
alguns testes unitários antigos continuam exercitando exclusivamente `lib/guac`.
Os testes E2E usam o binário nativo explicitamente por `CLOUDKIT_BIN`.

Não foram usados tokens nem contas reais para validar login OAuth ou os backends
remotos. Não há auditoria externa ou benchmark comparativo que sustente uma
promessa de ganho de desempenho. A medição deve comparar daemon e processos
rclone sob a mesma carga, versão de rclone, cache e perfil.

## Validação desta implementação

- `cargo test --locked`: 15 testes unitários Rust, 3 testes IPC e 1 teste de ciclo de energia.
- `tests/test_daemon.py` com o release Rust: 21 cenários anteriores aprovados; teste adicional de desligamento global com FUSE.
- `tests/test_ipc.py`: 15 testes aprovados, incluindo ponte nativa e widget offscreen.
- `tests/test_install.py`: 9 testes aprovados, incluindo falha de cópia e execução sem Python.
- `tests/test_power_widget.py`: botão QML liga/desliga, respeita CLI e não reconecta desligado.
- Build release, Clippy com warnings como erros, rustfmt e sintaxe Bash aprovados.

Após colocar o catálogo fixo de provedores em cache, os testes Rust e os cenários
E2E de streaming, comandos e remoção de conta foram repetidos no release.
