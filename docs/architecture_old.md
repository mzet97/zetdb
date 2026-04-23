# architecture.md

## Arquitetura — Banco de Dados In-Memory estilo Redis em Rust

## 1. Visão geral

Este documento define a arquitetura de um banco de dados em memória, concorrente, inspirado no Redis, implementado em Rust, com foco em:

- baixa latência
- alta concorrência
- segurança de memória
- separação clara de responsabilidades
- evolução progressiva do MVP para um núcleo mais robusto

O sistema inicia como um **KV store TCP in-memory** com protocolo textual simples e evolui para uma base preparada para RESP, TTL, persistência e extensões futuras.

---

## 2. Drivers arquiteturais

### Funcionais
- aceitar conexões TCP simultâneas
- processar comandos básicos de KV store
- manter dados em memória
- suportar expiração por chave
- suportar incremento atômico por chave

### Não funcionais
- alta concorrência
- baixo overhead de sincronização
- baixo custo de cópia de memória
- previsibilidade do hot path
- facilidade de teste
- evolução incremental sem reescrita maciça

### Restrições
- Rust estável
- Tokio como runtime de rede
- implementação limpa e modular
- sem lock global no caminho crítico do storage

---

## 3. Escopo arquitetural

### Incluído no MVP
- servidor TCP
- protocolo textual simples
- parser de comandos
- command dispatcher
- storage concorrente em memória
- comandos `PING`, `SET`, `GET`, `DEL`, `INCR`
- TTL com lazy eviction e limpeza ativa
- testes unitários e de integração básicos

### Fora do MVP, mas previsto
- RESP completo
- persistência por snapshot
- append-only log
- replicação
- clustering
- autenticação/ACL
- compressão
- pub/sub

---

## 4. Estilo arquitetural adotado

Será utilizada uma arquitetura **modular com inspiração hexagonal**, em que o núcleo de domínio e storage é isolado de detalhes de transporte e protocolo.

### Objetivo dessa decisão
Evitar que a lógica de rede, parsing e manipulação do banco se acoplem cedo demais. Isso permite trocar ou evoluir:

- protocolo textual → RESP
- armazenamento atual → engine customizada
- logs simples → observabilidade mais forte

---

## 5. Visão de componentes

```text
+---------------------------+
|        TCP Server         |
|  accept loop / sessions   |
+-------------+-------------+
              |
              v
+---------------------------+
|       Connection I/O      |
| read buffer / write resp  |
+-------------+-------------+
              |
              v
+---------------------------+
|     Frame / Command       |
|   tokenizer + parser      |
+-------------+-------------+
              |
              v
+---------------------------+
|      Command Dispatcher   |
| maps command -> handler   |
+-------------+-------------+
              |
              v
+---------------------------+
|      Storage Service      |
| get/set/del/incr/ttl      |
+-------------+-------------+
              |
              v
+---------------------------+
|    Concurrent KV Engine   |
| DashMap/sharded storage   |
+-------------+-------------+
              |
              +--------------------+
              |                    |
              v                    v
+----------------------+   +----------------------+
|  TTL/Lazy Eviction   |   | Background Sweeper   |
+----------------------+   +----------------------+
```

---

## 6. Módulos propostos

```text
src/
  main.rs
  config/
    mod.rs
  server/
    mod.rs
    tcp.rs
    session.rs
  protocol/
    mod.rs
    command.rs
    parser.rs
    response.rs
  application/
    mod.rs
    dispatcher.rs
  domain/
    mod.rs
    value.rs
    errors.rs
  storage/
    mod.rs
    engine.rs
    dashmap_engine.rs
    ttl.rs
  observability/
    mod.rs
    logging.rs
    metrics.rs
  tests/
    integration.rs
```

### Responsabilidades

#### `server/`
- bind TCP
- accept loop
- spawn de sessões
- gestão de timeout e encerramento de conexão

#### `protocol/`
- framing inicial
- parsing de comandos
- definição de enum de comandos
- serialização de respostas

#### `application/`
- orchestration dos comandos
- mapeamento de command → use case

#### `domain/`
- tipos centrais
- semântica do valor
- regras de erro de negócio

#### `storage/`
- contrato da engine
- implementação concorrente
- regras de TTL e limpeza

#### `observability/`
- logs
- contadores básicos
- rastreabilidade operacional mínima

---

## 7. Modelo de domínio

### Comando
A camada de protocolo deve converter entrada textual em uma enum semelhante a:

```rust
pub enum Command {
    Ping,
    Get { key: String },
    Set { key: String, value: bytes::Bytes, ttl: Option<std::time::Duration> },
    Del { key: String },
    Incr { key: String },
}
```

### Valor armazenado

```rust
pub struct ValueEntry {
    pub data: bytes::Bytes,
    pub expires_at: Option<std::time::Instant>,
}
```

### Motivação
- `Bytes` prepara o sistema para payload binário e menor cópia
- `expires_at` torna TTL parte do modelo do storage
- o parser não deve “embutir” lógica de expiração fora do domínio

---

## 8. Estratégia de concorrência

### Decisão
Usar uma engine concorrente baseada em **sharding**, inicialmente com `DashMap`.

### Justificativa
O material base já identifica o gargalo de um `RwLock<HashMap>` global: um único escritor degrada leituras e escritas concorrentes. A solução recomendada foi usar estrutura fragmentada/sharded, com `DashMap` como escolha prática no MVP. fileciteturn0file1

### Consequências
- boa produtividade inicial
- concorrência adequada por chave/shard
- menor contenção que lock global
- trade-off: menos controle fino que uma engine customizada

### Regra adicional
A engine deve ficar atrás de uma trait para permitir substituição futura.

Exemplo conceitual:

```rust
pub trait KvEngine: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<ValueEntry>, EngineError>;
    fn set(&self, key: String, value: ValueEntry) -> Result<(), EngineError>;
    fn del(&self, key: &str) -> Result<bool, EngineError>;
    fn incr(&self, key: &str) -> Result<i64, EngineError>;
}
```

---

## 9. Estratégia de protocolo

### Fase 1
Protocolo textual simples:
- `PING`
- `GET key`
- `SET key value`
- `DEL key`
- `INCR key`

### Fase 2
Evolução do parser para suportar framing mais rigoroso e futura compatibilidade RESP.

### Motivação
O material base começa com um protocolo textual simples para acelerar a validação do servidor TCP e da concorrência do banco. fileciteturn0file1

### Regra arquitetural
Separar:
- leitura de bytes
- delimitação de frame/comando
- parsing semântico
- execução do comando

Isso evita que a futura migração para RESP contamine toda a aplicação.

---

## 10. Estratégia de TTL

### Decisão
TTL será implementado com duas abordagens complementares:

1. **Lazy eviction** no acesso (`GET`, e opcionalmente em outras leituras)
2. **Active sweeping** periódico em task de background

### Justificativa
O material base já define essa combinação como evolução adequada do banco estilo Redis: expiração passiva na leitura e varredura ativa periódica para limpeza. fileciteturn0file2

### Trade-offs
- simples de implementar
- sem custo de timer individual por chave
- expiração não é “pontual exata ao milissegundo”
- bom compromisso para MVP

### Regras
- `expires_at` pertence ao storage/modelo de valor
- chave expirada deve se comportar como inexistente
- a limpeza ativa não pode bloquear excessivamente o banco

---

## 11. Estratégia de INCR atômico

### Risco
Implementar `INCR` como `get + parse + set` gera race lógica entre clientes concorrentes.

### Decisão
Usar API de mutação por entrada/shard, como `entry`, para manter coerência atômica por chave.

### Justificativa
O material base explicita que `INCR` precisa usar `entry` ou mecanismo equivalente para evitar race condition lógica em `DashMap`. fileciteturn0file2

### Regra
- se a chave não existir, iniciar em `0` e retornar `1`
- se o valor não for inteiro válido, retornar erro de protocolo/domínio
- preservar TTL conforme decisão explícita da arquitetura da engine

Recomendação inicial: ao fazer `INCR`, preservar TTL existente daquela chave, se houver.

---

## 12. Estratégia de buffers e memória

### Decisão
Usar `bytes::Bytes` e evoluir gradualmente o parser para reduzir cópias.

### Justificativa
O material base aponta que `String` no caminho crítico cria alocações desnecessárias e que a evolução correta para produção passa por `bytes::Bytes`/`BytesMut`. fileciteturn0file1turn0file2

### Diretrizes
- evitar `.to_string()` em hot path quando desnecessário
- separar representação interna de resposta e serialização final
- preparar buffer incremental para múltiplos comandos por conexão no futuro

---

## 13. Resiliência e operação

### Requisitos mínimos
- cliente desconectado não pode derrubar task global
- erros de parse não podem panicar o servidor
- timeouts configuráveis para conexões lentas
- logs básicos de inicialização, conexão e erros
- limites de porta/endereço via configuração

### Riscos conhecidos
- exaustão de file descriptors
- clientes lentos mantendo conexões abertas
- payloads grandes causando pressão de memória
- contenção interna em cargas extremas

O material base já alerta para problemas de FD exhaustion e conexões lentas em serviços TCP concorrentes. fileciteturn0file0

---

## 14. Observabilidade

### MVP
- logging estruturado simples
- contadores básicos por comando
- contador de conexões abertas
- contador de erros de parsing

### Evolução
- métricas Prometheus
- tracing distribuído/OpenTelemetry
- histogramas de latência por comando

---

## 15. Estratégia de testes

### Unitários
- parser de comandos
- serialização de respostas
- comportamento da engine
- TTL/lazy eviction
- `INCR` e erros de tipo

### Integração
- cliente TCP envia comandos reais
- múltiplos clientes simultâneos
- TTL expira como esperado
- `INCR` concorrente mantém consistência

### Benchmarks iniciais
- throughput de `GET`
- throughput de `SET`
- latência p50/p95/p99 em carga local
- teste concorrente com 100, 1k e 10k operações

---

## 16. Roadmap arquitetural

### Fase A — Fundação
- estrutura modular do projeto
- TCP server
- parser textual mínimo
- engine concorrente
- `PING`, `GET`, `SET`, `DEL`

### Fase B — Coerência funcional
- `INCR` atômico
- TTL
- lazy eviction
- sweeper em background

### Fase C — Robustez
- melhor parser
- `Bytes`/`BytesMut`
- testes mais fortes
- timeouts e limites

### Fase D — Preparação de produto
- RESP
- persistência
- métricas
- benchmark consistente

---

## 17. Decisões explícitas

1. **DashMap no MVP**: sim.
2. **Trait para engine**: sim.
3. **TTL no modelo**: sim.
4. **Task periódica de limpeza**: sim.
5. **Parser textual primeiro**: sim.
6. **Preparação para RESP sem implementar tudo agora**: sim.
7. **`Bytes` como direção de evolução obrigatória**: sim.
8. **`INCR` via mutação atômica por entrada**: sim.
9. **Sem persistência no MVP**: sim.
10. **Sem cluster no MVP**: sim.

---

## 18. Critérios de consistência arquitetural

Uma implementação será considerada aderente a esta arquitetura se:

- houver separação entre rede, parser, dispatch e engine
- a engine não depender do protocolo textual
- `INCR` não for implementado como `get + set` ingênuo
- TTL existir no storage e não apenas no parser
- o sistema puder migrar para RESP sem reescrever o núcleo
- não existir lock global central no hot path do banco

---

## 19. Conclusão

A arquitetura proposta busca um equilíbrio deliberado entre:

- pragmatismo de MVP
- fundamentos corretos de concorrência
- baixo acoplamento
- preparação para crescimento real

Ela aproveita os princípios já estabelecidos no material-base sobre load balancer TCP, mini-Redis concorrente, `DashMap`, `INCR` atômico e TTL com lazy eviction/cleaner periódico. fileciteturn0file0turn0file1turn0file2
