# task.md

## Plano de execução em SDD — Banco de Dados estilo Redis em Rust

## 1. Regras de execução

Cada tarefa deve seguir este fluxo:

1. validar aderência à arquitetura
2. implementar apenas o escopo da tarefa
3. adicionar ou ajustar testes
4. documentar limitações
5. marcar a tarefa como concluída somente se os critérios de aceite forem atendidos

Status possíveis:
- `[TODO]`
- `[DOING]`
- `[DONE]`
- `[BLOCKED]`

---

## 2. Épico A — Fundação do projeto

### [DONE] A1. Inicializar workspace e dependências base
**Objetivo**
Criar a base do projeto Rust com organização de pastas e dependências mínimas.

**Entregáveis**
- `Cargo.toml`
- estrutura inicial de módulos
- dependências: `tokio`, `bytes`, `dashmap`
- configuração de lint e profile de release

**Critérios de aceite**
- projeto compila
- módulos vazios organizados
- runtime Tokio funcional

**Observações**
Não iniciar com tudo em `main.rs`.

---

### [DONE] A2. Definir contratos centrais do domínio
**Objetivo**
Criar os tipos fundamentais do sistema.

**Entregáveis**
- enum `Command`
- tipos de resposta
- `ValueEntry`
- erros de domínio e engine
- trait `KvEngine`

**Critérios de aceite**
- domínio compila sem dependência do servidor TCP
- tipos representam `PING`, `SET`, `GET`, `DEL`, `INCR`
- TTL já previsto em `ValueEntry`

---

### [DONE] A3. Implementar parser textual mínimo
**Objetivo**
Converter comandos de texto em `Command`.

**Entregáveis**
- tokenizer/parsing inicial
- normalização básica
- erros de sintaxe consistentes

**Critérios de aceite**
- `PING` parseia corretamente
- `GET key` parseia corretamente
- `SET key value` parseia corretamente
- `DEL key` parseia corretamente
- `INCR key` parseia corretamente
- testes unitários cobrindo entradas válidas e inválidas

**Observações**
Ainda não precisa suportar RESP.

---

### [DONE] A4. Implementar serialização de respostas
**Objetivo**
Padronizar como o servidor responde ao cliente.

**Entregáveis**
- formatação de `PONG`
- resposta de sucesso
- resposta de erro
- retorno de valor encontrado/não encontrado
- retorno inteiro para `INCR`

**Critérios de aceite**
- respostas são determinísticas
- testes unitários cobrem casos principais

---

## 3. Épico B — Núcleo de armazenamento

### [DONE] B1. Implementar engine concorrente inicial com DashMap
**Objetivo**
Criar a implementação concreta da trait `KvEngine` usando `DashMap`.

**Entregáveis**
- `DashMapEngine`
- operações `get`, `set`, `del`
- suporte a `Bytes` como payload

**Critérios de aceite**
- engine passa testes unitários
- não existe `RwLock<HashMap>` global no hot path
- valores binários são aceitos no modelo interno

**Base técnica**
A escolha por estrutura sharded via `DashMap` decorre diretamente do material-base que rejeita `RwLock` global por contenção. fileciteturn0file1

---

### [DONE] B2. Implementar `INCR` atômico por chave
**Objetivo**
Garantir incremento consistente em cenário concorrente.

**Entregáveis**
- operação `incr`
- erro quando valor não for inteiro
- preservação de coerência por chave

**Critérios de aceite**
- `INCR` em chave inexistente retorna `1`
- `INCR` sucessivo mantém monotonicidade correta
- concorrência não gera perda de incremento
- teste multithread/task valida comportamento

**Base técnica**
O material-base explicita que `get + insert` não é suficiente e recomenda `entry` ou operação equivalente por shard. fileciteturn0file2

---

### [DONE] B3. Implementar TTL no modelo de dado
**Objetivo**
Suportar validade opcional por chave.

**Entregáveis**
- `expires_at` em `ValueEntry`
- helper para verificar expiração
- `SET` com TTL opcional no domínio

**Critérios de aceite**
- chaves sem TTL permanecem válidas
- chaves com TTL expiram logicamente
- testes unitários cobrem bordas

---

### [DONE] B4. Implementar lazy eviction
**Objetivo**
Quando uma chave expirada for acessada, ela deve ser tratada como ausente e removida quando apropriado.

**Entregáveis**
- integração do check de expiração em `get`
- remoção sob demanda

**Critérios de aceite**
- `GET` em chave expirada retorna não encontrado
- chave expirada não continua sendo entregue após acesso

**Base técnica**
Essa estratégia já está indicada no material-base do bônus. fileciteturn0file2

---

### [DONE] B5. Implementar sweeper periódico de expiração
**Objetivo**
Limpar lixo expirado em background.

**Entregáveis**
- task periódica
- retenção segura de chaves não expiradas
- frequência configurável

**Critérios de aceite**
- limpeza roda sem derrubar o servidor
- chaves expiradas saem do mapa mesmo sem leitura posterior
- testes de integração validam limpeza

**Base técnica**
O material-base propõe varredura periódica como TTL ativo complementar ao lazy eviction. fileciteturn0file2

---

## 4. Épico C — Servidor TCP e fluxo ponta a ponta

### [DONE] C1. Implementar TCP server bootstrap
**Objetivo**
Subir o listener TCP e aceitar conexões.

**Entregáveis**
- bind configurável
- accept loop
- spawn de sessão por cliente

**Critérios de aceite**
- servidor sobe com sucesso
- aceita múltiplas conexões
- falha de uma conexão não derruba o loop principal

**Base técnica**
O padrão de accept loop + task por conexão já está estabelecido no material-base TCP. fileciteturn0file0turn0file1

---

### [DONE] C2. Implementar sessão de conexão
**Objetivo**
Ler bytes do socket, parsear comando, despachar e responder.

**Entregáveis**
- loop de leitura por conexão
- buffer inicial
- integração parser + dispatcher + engine
- escrita de resposta

**Critérios de aceite**
- `PING` responde corretamente via TCP
- `SET/GET/DEL/INCR` funcionam ponta a ponta
- testes de integração com cliente TCP real

---

### [DONE] C3. Implementar dispatcher de comandos
**Objetivo**
Separar o tratamento de cada comando da camada de rede.

**Entregáveis**
- componente de dispatch
- handlers por comando
- conversão de erro em resposta

**Critérios de aceite**
- sessão TCP não acessa diretamente detalhes internos do mapa
- handlers são testáveis isoladamente

---

## 5. Épico D — Robustez operacional

### [DONE] D1. Adicionar timeouts e limites básicos
**Objetivo**
Evitar conexões presas indefinidamente e reduzir risco operacional.

**Entregáveis**
- timeout de leitura
- timeout opcional de escrita
- configuração de limites

**Critérios de aceite**
- conexão ociosa excessiva é encerrada
- servidor continua aceitando novas conexões normalmente

**Base técnica**
O material-base já alerta para exaustão de FD e conexões lentas em serviços TCP. fileciteturn0file0

---

### [DONE] D2. Logging estruturado mínimo
**Objetivo**
Dar visibilidade operacional básica ao servidor.

**Entregáveis**
- logs de start
- logs de conexão/desconexão
- logs de erro de parse e execução

**Critérios de aceite**
- fluxo principal fica observável sem poluir excessivamente o hot path

---

### [DONE] D3. Tratamento de payload e alocações
**Objetivo**
Reduzir desperdício de memória e preparar evolução do parser.

**Entregáveis**
- adoção consistente de `Bytes`
- revisão de clonagens desnecessárias
- plano ou início de parser incremental

**Critérios de aceite**
- não há conversões para `String` por conveniência no storage
- decisões sobre cópia estão explicitadas

**Base técnica**
O material-base aponta `bytes::Bytes` como direção correta para reduzir cópias e suportar blobs maiores. fileciteturn0file1turn0file2

---

## 6. Épico E — Qualidade e validação

### [DONE] E1. Testes unitários do parser
**Critérios de aceite**
- comandos válidos
- sintaxe inválida
- bordas de whitespace

---

### [DONE] E2. Testes unitários da engine
**Critérios de aceite**
- `set/get/del`
- `incr`
- TTL
- lazy eviction

---

### [DONE] E3. Testes de integração TCP
**Critérios de aceite**
- cliente conecta e executa fluxo real
- múltiplos clientes concorrentes
- comandos básicos e erros

---

### [DONE] E4. Teste de concorrência para `INCR`
**Critérios de aceite**
- N tasks incrementando mesma chave
- valor final exato
- sem perda de atualização

---

### [DONE] E5. Benchmark inicial
**Objetivo**
Criar linha de base de performance.

**Entregáveis**
- script simples ou benchmark documentado
- throughput `GET/SET`
- latência p50/p95/p99

**Critérios de aceite**
- benchmark reproduzível localmente
- números documentados

---

## 7. Épico F — Evolução planejada

### [DONE] F1. Preparar parser para RESP
**Objetivo**
Evoluir framing/parsing sem quebrar o núcleo.

**Entregáveis**
- `try_parse_frame()` com auto-detecção de protocolo (`*` = RESP, resto = inline)
- `FrameResult` enum (Complete / Incomplete / Skip)
- RESP parser: `*<count>\r\n$<len>\r\n<data>\r\n...`
- Session loop refatorado para usar frame parser unificado
- 25 testes novos (18 parser + 7 integração TCP RESP)
- Inline existente sem nenhuma quebra

**Critérios de aceite**
- 83 testes passando
- RESP clients (redis-cli compat) funcionam via TCP
- Inline clients (nc) continuam funcionando
- Pipeline RESP funcional

### [TODO] F2. Planejar persistência snapshot
**Objetivo**
Documentar desenho inicial de snapshot consistente.

### [TODO] F3. Planejar append-only log
**Objetivo**
Definir estratégia incremental de durabilidade futura.

### [TODO] F4. Planejar observabilidade avançada
**Objetivo**
Definir métricas e tracing de produção.

---

## 8. Ordem recomendada de execução

1. A1
2. A2
3. A3
4. A4
5. B1
6. C1
7. C3
8. C2
9. B2
10. B3
11. B4
12. B5
13. D1
14. D2
15. D3
16. E1
17. E2
18. E3
19. E4
20. E5
21. F1
22. F2
23. F3
24. F4

---

## 9. Definition of Done global

Um incremento só pode ser considerado concluído quando:

- compila
- possui testes compatíveis com o escopo
- respeita `architecture.md`
- não cria acoplamento indevido
- não enfraquece evolução futura para RESP e persistência
- foi documentado de forma suficiente para continuidade

---

## 10. Observação final

Este `task.md` foi derivado diretamente dos princípios técnicos presentes no material-base do mini-Redis concorrente e do bônus avançado com `INCR`, `Bytes` e TTL, reorganizados em formato de execução SDD. fileciteturn0file1turn0file2
