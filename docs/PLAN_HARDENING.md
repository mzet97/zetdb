# Plano de Ação — Hardening ZetDB para MVP Final

> Metodologia: SDD (Specification-Driven Development)
> Baseado no review técnico completo do ZetDB v0.1.0

---

## 1. Visão Geral

**Objetivo:** Elevar o ZetDB de "funcional e bem arquitetado" para "robusto o suficiente para rodar em produção limitada sem surpresas operacionais".

**Escopo:** Não adiciona features de negócio (novos comandos, tipos de dados, Pub/Sub). Endereça exclusivamente falhas de robustez, race conditions, tratamento de erros e limites operacionais identificados no review.

**Princípios orientadores:**
1. Zero `unwrap()` / `expect()` em código de produção sem justificativa documentada.
2. Nenhuma operação de I/O ou parsing de dados externos deve causar panic.
3. Race conditions em persistência devem ser eliminadas, não apenas mitigadas.
4. O servidor deve se proteger de clientes maliciosos ou lentos.
5. Manter compatibilidade 100% com o comportamento existente — não quebrar contratos de API.

---

## 2. Fases de Execução

### Fase 1 — Hardening de Parsing Binário (Snapshot + AOF)
**Duração estimada:** 1 sessão de trabalho
**Dependências:** Nenhuma
**Risco:** Baixo

#### Contexto
`snapshot.rs` e `aof.rs` usam `try_into().unwrap()` ao converter slices de bytes para arrays fixos (`u16::from_le_bytes`, `u32::from_le_bytes`, etc.). Apesar de haver bounds checks anteriores, a presença do `unwrap` viola a regra do projeto e torna o servidor vulnerável a panic em arquivos corrompidos.

#### Tarefas

| ID | Tarefa | Arquivo(s) | Critério de Aceite |
|---|---|---|---|
| F1-T1 | Substituir todos `try_into().unwrap()` em `snapshot.rs` por `?` com erro `InvalidData` | `src/storage/snapshot.rs` | `cargo test` passa; `clippy` sem warnings; arquivo corrompido retorna `Err` sem panic |
| F1-T2 | Substituir todos `try_into().unwrap()` em `aof.rs` (replay) por `?` com erro `InvalidData` | `src/storage/aof.rs` | Mesmo critério acima |
| F1-T3 | Adicionar testes de integração injetando arquivos corrompidos (CRC válido mas payload truncado) | `src/storage/snapshot.rs`, `src/storage/aof.rs` | Testes confirmam `Err` em vez de panic |

#### Detalhe de Implementação

Atual (snapshot.rs):
```rust
let key_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
```

Alvo:
```rust
let key_len = u16::from_le_bytes(
    data[pos..pos + 2].try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "truncated key length"))?
) as usize;
```

**Nota:** Criar helper interno `read_u16_le(data: &[u8], pos: &mut usize) -> io::Result<u16>` para evitar repetição.

---

### Fase 2 — AOF: Mutex Assíncrono e Tratamento de Erros
**Duração estimada:** 1 sessão de trabalho
**Dependências:** Fase 1
**Risco:** Médio (mudança de semântica de sync para async no AofWriter)

#### Contexto
`AofWriter` usa `std::sync::Mutex`. Em Rust, `std::sync::Mutex` fica "envenenado" (poisoned) se uma thread panicar enquanto segura o lock. Todos os locks no código usam `.unwrap()`, o que significa que qualquer panic (mesmo em código não relacionado enquanto segura o lock) derruba o servidor permanentemente ao tentar escrever no AOF.

Além disso, `engine.set(...).ok()` em `replay_aof` descarta erros silenciosamente.

#### Tarefas

| ID | Tarefa | Arquivo(s) | Critério de Aceite |
|---|---|---|---|
| F2-T1 | Refatorar `AofWriter` para usar `tokio::sync::Mutex` em vez de `std::sync::Mutex` | `src/storage/aof.rs` | Todos os métodos `pub` tornam-se `async` onde necessário; `append_raw` e `flush_if_needed` funcionam corretamente |
| F2-T2 | Atualizar `session.rs` para `.await` nas chamadas de AOF | `src/server/session.rs` | Compila; comportamento de AOF inalterado em testes |
| F2-T3 | Atualizar `main.rs` (spawns de AOF) para lidar com interface async | `src/main.rs` | Compila; fsync ticker e rewriter funcionam |
| F2-T4 | Substituir `engine.set(...).ok()` em `replay_aof` por log de erro e contagem de falhas | `src/storage/aof.rs` | Replay retorna `Ok(replayed)` + `Ok(failed)` ou loga falhas individuais |
| F2-T5 | Substituir `engine.set(...).ok()` em `load_snapshot` por log de erro | `src/storage/snapshot.rs` | Falhas de `set` durante load são logadas como `warn` |

#### Decisão de Design

`tokio::sync::Mutex` é apropriado aqui porque:
- `AofWriter` é acessado de tasks async (session handler, fsync ticker, rewriter).
- O lock é held por operações de I/O (write, fsync), que são pontos de `.await` implícitos no kernel.
- `std::sync::Mutex` + `.block_on()` seria anti-padrão em Tokio.

**Cuidado:** `tokio::sync::Mutex` não deve ser held através de `.await` desnecessariamente. O padrão atual já minimiza o escopo do lock (lock → write → unlock), então a mudança é mecânica.

---

### Fase 3 — AOF Rewrite Race-Free
**Duração estimada:** 1 sessão de trabalho
**Dependências:** Fase 2
**Risco:** Médio (lógica de concorrência)

#### Contexto
O fluxo atual de rewrite:
1. Task periódica verifica tamanho do AOF.
2. Se > threshold, chama `rewrite_aof()` (escreve temp + `rename` para sobrescrever o original).
3. Chama `writer.reopen()` para reabrir o file handle.

O problema: entre o passo 2 e 3, uma sessão TCP pode chamar `append_raw` e escrever no handle antigo (que agora aponta para um arquivo renomeado/sobrescrito) ou pior, o `rename` pode fazer com que o write caia em um inode órfão.

#### Solução Proposta

Introduzir uma **epoch** ou **swap atomico com pausa de writes**:

```rust
// Em AofWriter
struct AofWriter {
    inner: tokio::sync::Mutex<AofInner>,
}

struct AofInner {
    file: fs::File,
    path: String,
    fsync_policy: FsyncPolicy,
    last_fsync: Instant,
    paused: bool,  // nova flag
}
```

Fluxo de rewrite:
1. `rewrite_aof(engine, path)` gera o arquivo compactado em `.aof.rewrite.tmp`.
2. Lock `inner`.
3. Set `paused = true`.
4. `rename(temp, path)`.
5. Reopen file handle.
6. Set `paused = false`.
7. Unlock.

`append_raw`:
- Se `paused == true`, bufferiza em memória ou retorna `Err(WouldBlock)` para que a sessão tente novamente (ou simplesmente aguarde o lock e retry).

Alternativa mais simples (aceitável para MVP):
- Manter `paused` e fazer `append_raw` aguardar com `while paused { inner.paused_notify.notified().await }` usando `tokio::sync::Notify`.

#### Tarefas

| ID | Tarefa | Arquivo(s) | Critério de Aceite |
|---|---|---|---|
| F3-T1 | Adicionar mecanismo de pausa atômica no `AofWriter` | `src/storage/aof.rs` | Rewrite não perde writes; writes durante rewrite são sequencializados |
| F3-T2 | Atualizar `run_aof_rewriter` para usar o novo mecanismo | `src/storage/aof.rs` | Teste de integração confirma que writes ocorrem após rewrite sem perda |
| F3-T3 | Teste de integração: escrever N comandos durante rewrite e verificar que todos estão presentes no replay | `src/server/tcp.rs` ou teste novo | Nenhum comando perdido |

---

### Fase 4 — Write Timeout e Proteção de Sessão
**Duração estimada:** 1 sessão de trabalho
**Dependências:** Nenhuma (pode rodar em paralelo com Fase 1)
**Risco:** Baixo

#### Contexto
`session.rs` aplica `tokio::time::timeout` na leitura, mas a escrita (`writer.write_all(&write_buf).await`) não tem timeout. Um cliente que abre conexão mas não consome respostas causará acúmulo indefinido de dados no kernel buffer (e no `write_buf` do ZetDB, que é zerado a cada flush, mas o `write_all` pode bloquear).

#### Tarefas

| ID | Tarefa | Arquivo(s) | Critério de Aceite |
|---|---|---|---|
| F4-T1 | Adicionar `write_timeout` na configuração (default 30s) | `src/config/mod.rs` | CLI arg `--write-timeout-secs` e env `ZETDB_WRITE_TIMEOUT` |
| F4-T2 | Aplicar `tokio::time::timeout` no `write_all` de `session.rs` | `src/server/session.rs` | Falha de write timeout desconecta o cliente graciosamente |
| F4-T3 | Adicionar limite de tamanho do `write_buf` (MAX_WRITE_BUF) para evitar OOM por cliente lento | `src/server/session.rs` | Se write_buf exceder ~1MB, fechar conexão |
| F4-T4 | Teste de integração: simular cliente que não lê respostas | `src/server/tcp.rs` | Servidor fecha conexão após write timeout ou buffer overflow |

#### Detalhe de Implementação

```rust
// session.rs
const MAX_WRITE_BUF: usize = 1024 * 1024;

// No loop principal, antes do flush:
if write_buf.len() > MAX_WRITE_BUF {
    log::warn!("{peer}: write buffer overflow, closing");
    break;
}

// Flush com timeout:
match tokio::time::timeout(write_timeout, writer.write_all(&write_buf)).await {
    Ok(Ok(())) => write_buf.clear(),
    Ok(Err(e)) => { log::warn!("{peer}: write error: {e}"); break; }
    Err(_) => { log::warn!("{peer}: write timeout"); break; }
}
```

---

### Fase 5 — MaxMemory e Eviction Básica
**Duração estimada:** 2 sessões de trabalho
**Dependências:** Fase 1 (para entender o padrão de erros)
**Risco:** Médio (requer estimativa de uso de memória)

#### Contexto
Em produção, sem um limite de memória, o ZetDB eventualmente causará OOM no sistema operacional. Redis resolve isso com `maxmemory` e políticas de eviction (`allkeys-lru`, `volatile-lru`, `allkeys-random`, etc.). Para o MVP, uma implementação simples é suficiente.

#### Decisão de Arquitetura

**Não vamos rastrear bytes exatos** (isso requer contabilização precisa de overhead do DashMap, alocações, etc.). Em vez disso:

1. Usar `engine.len()` (contagem de chaves) como proxy para memória, OU
2. Usar `sysinfo` (nova dep) para ler RSS do processo periodicamente.

Para manter o projeto enxuto, **opção 1** é preferida para o MVP: configurar `max_keys` em vez de `maxmemory_bytes`. Isso é um proxy grosseiro mas efetivo para evitar OOM.

Se o usuário quiser `maxmemory` real em bytes, isso pode ser uma feature pós-MVP.

#### Tarefas

| ID | Tarefa | Arquivo(s) | Critério de Aceite |
|---|---|---|---|
| F5-T1 | Adicionar config `max_keys` (0 = ilimitado) | `src/config/mod.rs` | CLI arg e env var funcionais |
| F5-T2 | Implementar política `allkeys-lru` aproximada no DashMapEngine | `src/storage/dashmap_engine.rs` | Quando `len() > max_keys`, remover ~1% das chaves mais antigas (usando timestamp de inserção interno) |
| F5-T3 | Rejeitar `SET` com erro quando max_keys atingido e eviction não resolve | `src/storage/dashmap_engine.rs`, `src/application/dispatcher.rs` | Retorna `ResponseError::OutOfMemory` ou similar |
| F5-T4 | Testes: verificar que `max_keys` eviction funciona e preserva TTL | `src/storage/dashmap_engine.rs` | Testes unitários confirmam comportamento |

#### Nota sobre LRU

DashMap não expõe ordem de inserção. Para um LRU real, precisaríamos de uma estrutura auxiliar (ex: `linked_hash_map` ou `lru` crate). Para o MVP, uma política de **random eviction** ou **sampled LRU** (amostrar N chaves aleatórias e remover a mais antiga por `expires_at` ou um campo `created_at` novo) é aceitável e muito mais simples.

**Proposta:** Adicionar `created_at: Instant` em `ValueEntry`. Eviction amostra 5 chaves aleatórias e remove a mais antiga por `created_at`. Isso é O(1) e efetivo o suficiente para um MVP.

---

### Fase 6 — Cleanup de Dependências e Documentação
**Duração estimada:** 0.5 sessão
**Dependências:** Fase 2 (porque serde_json é usado em bin, não em lib)
**Risco:** Baixo

#### Tarefas

| ID | Tarefa | Arquivo(s) | Critério de Aceite |
|---|---|---|---|
| F6-T1 | Mover `serde` e `serde_json` de `[dependencies]` para `[dev-dependencies]` ou para `[[bin]]` dependency | `Cargo.toml` | `cargo build --release` não compila serde na lib; bins ainda funcionam |
| F6-T2 | Verificar se `serde` é usado em algum módulo de produção (não deveria) | `src/` | Confirmação via grep que nenhum `use serde::` existe fora de `bin/` |
| F6-T3 | Adicionar seção "Limitações Conhecidas" no README | `README.md` | Lista: MSET não-atômico, KEYS bloqueante em milhões de chaves, maxmemory é max_keys (proxy), etc. |
| F6-T4 | Atualizar `docs/PHASES.md` com as novas fases de hardening | `docs/PHASES.md` | Documento reflete o estado atual e o plano futuro |

---

### Fase 7 — Validação Final
**Duração estimada:** 1 sessão
**Dependências:** Todas as anteriores
**Risco:** Baixo

#### Checklist de Validação

| ID | Tarefa | Critério de Aceite |
|---|---|---|
| F7-T1 | `cargo test` — todos os testes existentes passam | 0 falhas |
| F7-T2 | `cargo test` — novos testes de robustez passam | 0 falhas |
| F7-T3 | `cargo clippy -- -D warnings` | 0 warnings (exceto os previamente permitidos) |
| F7-T4 | `cargo build --release` | Build limpo, sem erros |
| F7-T5 | Teste manual: injetar snapshot corrompido | Servidor loga erro e inicia vazio |
| F7-T6 | Teste manual: injetar AOF truncado | Servidor loga erro, replay parcial, não panic |
| F7-T7 | Teste manual: stress test com `cargo run --release --bin pipeline` | Benchmark não regrediu > 5% vs baseline |
| F7-T8 | Teste manual: SIGINT durante AOF rewrite | Graceful shutdown funciona, snapshot final é válido |

---

## 3. Ordem de Execução e Dependências

```
Fase 1 (Parsing Hardening)
    │
    ▼
Fase 2 (AOF Mutex Async) ──► Fase 3 (AOF Rewrite Race-Free)
    │
    ▼
Fase 4 (Write Timeout)          Fase 5 (MaxMemory)
    │                              │
    └──────────────┬───────────────┘
                   ▼
            Fase 6 (Cleanup + Docs)
                   │
                   ▼
            Fase 7 (Validação Final)
```

**Fase 1 e Fase 4** podem ser executadas em paralelo (não há dependências entre elas).
**Fase 3** depende de **Fase 2** (interface async do AofWriter).
**Fase 5** é independente, mas pode ser feita em paralelo com Fases 2-4 se houver capacidade.

---

## 4. Definição de "Pronto" (Definition of Done)

Para cada tarefa, os seguintes critérios devem ser atendidos:

1. **Compila** com `cargo build` e `cargo build --release`.
2. **Testes** apropriados ao escopo estão implementados e passando.
3. **Respeita `architecture.md`** — não quebra separação de camadas.
4. **Sem acoplamento impróprio** criado.
5. **Não enfraquece** evolução futura (RESP, persistência, cluster).
6. **Documentado** suficientemente para continuidade (comentários de "por que", não "o que").
7. **Sem `unwrap()`** em código de produção sem justificativa explícita em comentário.
8. **Clippy limpo** (`cargo clippy -- -D warnings`).

---

## 5. Riscos e Mitigações

| Risco | Probabilidade | Impacto | Mitigação |
|---|---|---|---|
| Mudança para `tokio::sync::Mutex` introduzir deadlock | Baixa | Alto | Manter escopo mínimo do lock; revisar com `clippy::await_holding_lock` |
| MaxMemory (`max_keys`) quebrar testes existentes de TTL | Baixa | Médio | Default `max_keys = 0` (ilimitado); testes antigos não são afetados |
| Performance regredir após hardening | Média | Médio | Benchmark baseline antes e depois; aceitar regressão < 5% |
| AOF rewrite com pausa introduzir latência em writes | Baixa | Médio | Pausa deve durar apenas o tempo do `rename` + `reopen` (< 1ms) |
| Arquivos corrompidos em produção causarem falha silenciosa | Média | Alto | Logar WARN/ERROR claro indicando arquivo ignorado e motivo |

---

## 6. Métricas de Sucesso

Ao final deste plano, o ZetDB deve atender:

- [ ] 0 `unwrap()` / `expect()` em parsing de dados externos (snapshot, AOF, protocolo).
- [ ] 0 race conditions conhecidas em persistência.
- [ ] Proteção contra clientes lentos (read + write timeout + buffer limits).
- [ ] Limite operacional de memória configurável (`max_keys`).
- [ ] 100% dos testes existentes passando.
- [ ] Benchmark de throughput não regrediu > 5%.
- [ ] Documentação reflete limitações reais do sistema.

---

## 7. Notas para o Executor

- **Ferramenta recomendada:** Use `ast_grep_replace` ou buscas estruturadas para encontrar todos `unwrap()` e `expect()` nos arquivos alvo antes de começar.
- **Testes primeiro:** Para Fase 1, escreva o teste com arquivo corrompido *antes* de remover o `unwrap`. O teste deve falhar com panic, depois passar com `Err`.
- **Commits atômicos:** Um commit por tarefa (F1-T1, F1-T2, etc.). Isso facilita rollback e review.
- **Não otimize prematuramente:** A meta é robustez, não micro-otimização. Se uma mudança adiciona 1-2µs de latência mas elimina um panic, é aceitável.
