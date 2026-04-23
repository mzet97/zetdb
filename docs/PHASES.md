# ZetDB — Planejamento por Fases

Este documento agrupa as 24 tarefas de `task.md` em **5 fases de entrega**, cada uma com um milestone claro, dependências, riscos e gate de aceitação.

---

## Visão Geral

```text
Fase 1: Fundação        →  Compila, tipos e contratos definidos
Fase 2: Núcleo          →  Engine funcional, comandos executam
Fase 3: Integração      →  Servidor TCP ponta a ponta
Fase 4: Robustez        →  TTL, timeouts, logging
Fase 5: Validação       →  Testes, benchmarks, documentação final
```

---

## Fase 1 — Fundação (A1 → A4)

**Objetivo:** Estabelecer a base do projeto com tipos, contratos e parser funcionais.

**Tarefas:**
| Ordem | ID | Descrição | Dependências |
|---|---|---|---|
| 1 | A1 | Inicializar workspace e dependências | — |
| 2 | A2 | Definir contratos centrais do domínio | A1 |
| 3 | A3 | Implementar parser textual mínimo | A2 |
| 4 | A4 | Implementar serialização de respostas | A2 |

**Entregáveis:**
- `Cargo.toml` com tokio, bytes, dashmap
- Estrutura de módulos organizada
- `domain/`: `Command`, `ValueEntry`, `Response`, erros tipados
- `protocol/`: parser textual, serialização de resposta
- `storage/`: trait `KvEngine`

**Milestone:** Projeto compila. Parser converte texto em `Command`. Respostas são serializáveis. Domínio não depende de TCP.

**Riscos:**
- Over-engineering da trait `KvEngine` — manter mínima no MVP
- Parser muito rigido ou muito frouxo — seguir especificação em `docs/SPECIFICATION.md`

**Gate de aceitação:**
- [ ] `cargo build` sem erros
- [ ] `cargo test` com testes unitários do parser passando
- [ ] `cargo test` com testes unitários de serialização passando
- [ ] Módulo `domain` compila sem depender de `server` ou `tokio`

---

## Fase 2 — Núcleo de Armazenamento (B1 → B2)

**Objetivo:** Engine concorrente funcional com `get`, `set`, `del` e `incr` atômico.

**Tarefas:**
| Ordem | ID | Descrição | Dependências |
|---|---|---|---|
| 5 | B1 | Implementar DashMapEngine (get/set/del) | A2 |
| 6 | B2 | Implementar INCR atômico | B1 |

**Entregáveis:**
- `DashMapEngine` implementando `KvEngine`
- Operações `get`, `set`, `del` com `Bytes` como payload
- `incr` atômico via `entry()` API
- Testes unitários da engine

**Milestone:** Engine passa todos os testes unitários. `INCR` concorrente não perde atualizações.

**Riscos:**
- Race condition em `INCR` se não usar `entry()` — **crítico**
- Leak de memória se TTL não for implementado antes de carga pesada — mitigado na Fase 4

**Gate de aceitação:**
- [ ] Testes unitários de `get/set/del` passam
- [ ] Teste multithread de `INCR` — N tasks, valor final exato
- [ ] Sem `RwLock<HashMap>` no código
- [ ] Valores binários aceitos no modelo interno

---

## Fase 3 — Integração TCP (C1 → C3)

**Objetivo:** Servidor TCP aceita conexões e executa comandos ponta a ponta.

**Tarefas:**
| Ordem | ID | Descrição | Dependências |
|---|---|---|---|
| 7 | C1 | TCP server bootstrap (bind + accept loop) | B1 |
| 8 | C3 | Command dispatcher | A2, B1 |
| 9 | C2 | Sessão de conexão (read → parse → dispatch → write) | C1, C3, A3, A4 |

**Entregáveis:**
- TCP listener com accept loop
- Sessão por cliente (task Tokio)
- Dispatcher mapeando `Command` → `KvEngine`
- Integração completa: TCP → parser → dispatcher → engine → resposta

**Milestone:** `PING`, `SET`, `GET`, `DEL`, `INCR` funcionam via TCP com `nc` ou cliente simples.

**Riscos:**
- Buffer de leitura mal dimensionado — começar com `BufReader` simples
- Sessão bloqueando accept loop — cada sessão é task independente
- Erro de parse derrubando conexão — enviar erro e continuar

**Gate de aceitação:**
- [ ] `PING` responde `+PONG` via TCP
- [ ] `SET/GET` funcionam ponta a ponta
- [ ] `DEL` funciona ponta a ponta
- [ ] `INCR` funciona ponta a ponta
- [ ] Múltiplos clientes simultâneos sem crash
- [ ] Desconexão de cliente não derruba o servidor

---

## Fase 4 — Robustez e TTL (B3 → B5, D1 → D3)

**Objetivo:** Adicionar expiração, timeouts, logging e otimização de memória.

**Tarefas:**
| Ordem | ID | Descrição | Dependências |
|---|---|---|---|
| 10 | B3 | TTL no modelo de dado | B1 |
| 11 | B4 | Lazy eviction | B3 |
| 12 | B5 | Sweeper periódico | B3 |
| 13 | D1 | Timeouts e limites | C1 |
| 14 | D2 | Logging estruturado | C1 |
| 15 | D3 | Revisão de alocações e Bytes | B1, C2 |

**Entregáveis:**
- `ValueEntry` com `expires_at`
- Lazy eviction em `get`, `del`, `incr`
- Task de background sweeping periódica
- Timeout de leitura configurável
- Logging com `tracing` ou `log`
- Revisão de conversões `String` desnecessárias

**Milestone:** TTL funciona. Chaves expiram corretamente via lazy + active eviction. Servidor tem logging e timeouts.

**Riscos:**
- Sweeper causando contenção em mapa grande — usar `retain()` com break
- Timeouts muito agressivos desconectando clientes legítimos — default conservador (30s)
- Overhead de logging no hot path — usar nível `DEBUG` para comandos

**Gate de aceitação:**
- [ ] `GET` em chave expirada retorna nil
- [ ] Sweeper remove chaves expiradas sem leitura
- [ ] Conexão ociosa é encerrada após timeout
- [ ] Logs de inicialização, conexão e erros visíveis
- [ ] Sem `.to_string()` desnecessário no storage

---

## Fase 5 — Validação e Qualidade (E1 → E5)

**Objetivo:** Cobertura de testes completa, benchmarks e documentação final.

**Tarefas:**
| Ordem | ID | Descrição | Dependências |
|---|---|---|---|
| 16 | E1 | Testes unitários do parser | A3 |
| 17 | E2 | Testes unitários da engine | B1, B2, B3, B4 |
| 18 | E3 | Testes de integração TCP | C2 |
| 19 | E4 | Teste de concorrência INCR | B2 |
| 20 | E5 | Benchmark inicial | Tudo acima |

**Entregáveis:**
- Suite de testes unitários (parser, engine, TTL)
- Testes de integração TCP
- Teste de concorrência para `INCR` (N tasks, valor final exato)
- Benchmark de throughput e latência (GET/SET)
- Documentação técnica atualizada

**Milestone:** Cobertura de testes cobre parser, engine, TTL, integração TCP e concorrência. Baseline de performance estabelecida.

**Riscos:**
- Benchmarks não reproduzíveis — documentar ambiente, rodar múltiplas vezes
- Testes de concorrência flakes — usar asserts determinísticos com valor final conhecido

**Gate de aceitação:**
- [ ] `cargo test` passa 100%
- [ ] Teste de concorrência INCR: 100 tasks × 100 increments = 10000 final
- [ ] Integração TCP: fluxo completo PING/SET/GET/DEL/INCR
- [ ] Benchmark documentado com p50/p95/p99
- [ ] TTL: lazy eviction + sweeper validados em teste

---

## Fase 6 — Evolução Planejada (F1 → F4)

**Objetivo:** Preparar arquitetura para RESP, persistência e observabilidade avançada.

**Tarefas:**
| Ordem | ID | Descrição | Dependências |
|---|---|---|---|
| 21 | F1 | Preparar parser para RESP | Fase 5 |
| 22 | F2 | Planejar persistência snapshot | Fase 5 |
| 23 | F3 | Planejar append-only log | F2 |
| 24 | F4 | Planejar observabilidade avançada | D2 |

**Entregáveis:**
- Documento de design para RESP migration
- Documento de design para snapshot
- Documento de design para AOF
- Documento de design para métricas/tracing

**Milestone:** Roadmap documentado para próxima geração do produto.

**Gate de aceitação:**
- [ ] Designs revisados e aprovados
- [ ] Sem inconsistência com arquitetura atual

---

## Dependências Críticas

```text
A1 → A2 → A3 ──────────────────────────────┐
  │    │    │                                 │
  │    │    └── A4 (serialização)             │
  │    │                                      │
  │    └── B1 (DashMapEngine) → B2 (INCR)     │
  │         │    │                             │
  │         │    └── B3 (TTL) → B4 (lazy)     │
  │         │                └── B5 (sweeper)  │
  │         │                                 │
  │         └── C1 (TCP) → C3 (dispatcher)    │
  │                      └── C2 (session) ←──┘
  │
  └── D1 (timeouts), D2 (logging), D3 (Bytes)

E1-E4: validação de tudo acima
E5: benchmark final
F1-F4: designs futuros
```

---

## Matriz de Riscos

| Risco | Probabilidade | Impacto | Mitigação |
|---|---|---|---|
| Race em INCR | Média | Alto | Usar `entry()` API, teste concorrente |
| Memory leak sem TTL | Alta | Médio | Implementar TTL cedo (Fase 4) |
| Parser frágil | Média | Médio | Testes extensivos de edge cases |
| FD exhaustion | Baixa | Alto | Timeouts + limites (D1) |
| Overhead de logging | Baixa | Médio | Nível DEBUG no hot path |
| Benchmarks não reproduzíveis | Média | Baixo | Documentar ambiente, múltiplas runs |

---

## Cronograma Sugerido

| Fase | Tarefas | Foco | Entrega |
|---|---|---|---|
| **1. Fundação** | A1-A4 | Compila, tipos, parser | Projeto compilável com testes unitários |
| **2. Núcleo** | B1-B2 | Engine funcional | Storage concorrente com INCR atômico |
| **3. Integração** | C1-C3 | TCP ponta a ponta | Servidor funcional via `nc` |
| **4. Robustez** | B3-B5, D1-D3 | TTL, timeouts, logging | Servidor production-ready mínimo |
| **5. Validação** | E1-E5 | Testes e benchmarks | Cobertura completa + baseline perf |
| **6. Evolução** | F1-F4 | Designs futuros | Roadmap documentado |

---

## Critérios Globais de Sucesso

O projeto é considerado bem-sucedido se ao final da Fase 5:

1. MVP aceita múltiplos clientes simultaneamente via TCP
2. `SET`, `GET`, `DEL`, `PING`, `INCR` funcionam corretamente
3. TTL funciona com lazy eviction + sweeper ativo
4. Arquitetura separa responsabilidades de modo claro
5. Código é limpo o suficiente para evolução sem retrabalho
6. Sistema está preparado para RESP e persistência sem reescrita total
7. Testes cobrem parser, engine, TTL e fluxo TCP
8. Benchmark de baseline está documentado
