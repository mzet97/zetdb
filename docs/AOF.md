# ZetDB — Append-Only Log (AOF) Design

## Objetivo

Garantir durabilidade incremental — cada comando de escrita é persistido em log antes ou após a execução, permitindo reconstrução completa do estado após crash.

---

## Decisões de Design

### 1. Formato do log

Cada entry no AOF é um comando RESP inline, terminado por `\n`:

```
SET mykey hello\n
DEL mykey\n
INCR counter\n
SET ttlkey world\n
```

**Por que RESP inline e não binário?**
- Human-readable, debuggável
- Reaproveita o parser existente (`parse_bytes`)
- Compatível com `redis-cli` para inspeção manual
- Trade-off: arquivo maior que binário, mas simplicidade compensa no MVP

### 2. Escrita síncrona vs assíncrona

Política de fsync configurável:

```rust
pub enum FsyncPolicy {
    EveryWrite,    // fsync após cada comando (mais seguro, mais lento)
    EverySecond,   // fsync a cada 1s (bom compromisso)
    Never,         // delega ao OS (mais rápido, risco de perda)
}
```

**Default recomendado:** `EverySecond` — bom equilíbrio entre performance e durabilidade.

### 3. Fluxo de escrita

```
CommandDispatcher
  ├─ Executa comando na engine (SET/DEL/INCR)
  └─ Se sucesso: append ao AOF buffer
       ├─ EveryWrite: fsync imediato
       └─ EverySecond: buffer acumula, background task faz fsync
```

**Comando de leitura (GET, PING) não gera log entry.**

### 4. Buffer e flush

```rust
struct AofWriter {
    file: std::fs::File,
    buf: Vec<u8>,         // accumulate writes
    fsync_policy: FsyncPolicy,
    last_fsync: Instant,
}
```

- `EveryWrite`: flush + fsync após cada append
- `EverySecond`: background ticker chama `flush_if_needed()`
- Buffer é `Vec<u8>` simples — sem `BytesMut` pois não está no hot path de rede

### 5. Compaction (rewrite)

AOF cresce indefinidamente. Compactação periódica reescreve o log mantendo apenas o estado final:

```
1. Criar <path>.tmp
2. Iterar DashMap, escrever SET para cada entry viva
3. fsync <path>.tmp
4. rename <path>.tmp → <path>
5. Truncar AOF para o novo arquivo
```

Mesmo padrão atômico do snapshot (temp + rename). Compactação roda em background task separada.

**Trigger:**
- Tamanho do AOF excede threshold (default: 64MB)
- Razão AOF/snapshot > 2x
- Manual via comando `COMPACT` (futuro)

### 6. Restore (replay)

Na inicialização, se AOF existe:

```
1. Abrir AOF
2. Para cada linha: parse_bytes() → dispatch()
3. Resultado: engine restaurada ao estado exato do último flush
```

Se snapshot E AOF existem:

```
1. load_snapshot()      // estado base
2. replay AOF           // comandos desde o snapshot
3. Entradas do AOF sobrescrevem snapshot
```

### 7. Configuração

```rust
pub struct AofConfig {
    pub enabled: bool,              // default: false
    pub path: String,               // default: "appendonly.zdb"
    pub fsync: FsyncPolicy,         // default: EverySecond
    pub rewrite_threshold_mb: u64,  // default: 64
}
```

Variáveis de ambiente:
- `ZETDB_AOF_ENABLED=true`
- `ZETDB_AOF_PATH=appendonly.zdb`
- `ZETDB_AOF_FSYNC=everysec` (always / everysec / no)
- `ZETDB_AOF_REWRITE_THRESHOLD=64`

---

## Integração arquitetural

### Módulo novo: `storage/aof.rs`

```
src/storage/
  mod.rs
  engine.rs
  dashmap_engine.rs
  ttl.rs
  snapshot.rs     // F2
  aof.rs          // NOVO
```

### Diagrama

```
main.rs
  ├─ load_snapshot()           // estado base (se existe)
  ├─ replay_aof()              // comandos incremental (se existe)
  ├─ run_server()
  │    └─ session → dispatch → engine
  │                         └→ aof.append()   // NOVO hook
  ├─ run_sweeper()
  ├─ run_snapshotter()
  └─ run_aof_rewriter()        // NOVO: compactação background
```

### Dispatcher hook

O dispatcher atual retorna `Response`. Para AOF, precisamos saber se o comando é de escrita e foi bem-sucedido:

```rust
pub fn dispatch(engine: &dyn KvEngine, cmd: Command) -> Response {
    let response = match cmd { ... };

    // AOF: log only successful writes
    // Caller checks response + command type to decide
    response
}
```

A lógica de "devo logar no AOF?" fica no caller (session), não no dispatcher. Isso mantém o dispatcher puro e testável.

### Session integration

```rust
// No session loop, after dispatch:
let response = dispatch(engine.as_ref(), command);
response.write_to(&mut write_buf);

// AOF: log successful writes
if let Some(aof) = aof_writer.as_ref() {
    if response.is_success() && command.is_write() {
        aof.append(&command_bytes);
    }
}
```

O session precisa saber se o comando é write. Solução: adicionar `Command::is_write(&self) -> bool`.

---

## Ordem de implementação

### F3.1: Tipos e writer
- `AofConfig`, `FsyncPolicy` em `config/mod.rs`
- `storage/aof.rs`: `AofWriter::new()`, `append()`, `flush()`
- `Command::is_write()`

### F3.2: Restore
- `replay_aof()` em `storage/aof.rs`
- Integração com `load_snapshot()` em `main.rs`

### F3.3: Session hook
- Passar `Option<Arc<AofWriter>>` para session
- Log de writes bem-sucedidos

### F3.4: Compaction
- `rewrite_aof()` — itera DashMap, reescreve log
- Background task `run_aof_rewriter()`

### F3.5: Configuração e testes
- Env vars para AOF config
- Unit: append + replay roundtrip
- Unit: TTL preservado via AOF
- Integration: SET → crash → restore → GET

---

## Comparação: Snapshot vs AOF

| Aspecto | Snapshot (F2) | AOF (F3) |
|---|---|---|
| Granularidade | Ponto no tempo | Cada comando |
| Durabilidade | Perde dados desde último snapshot | Perde dados desde último fsync |
| Tamanho em disco | Compacto (estado final) | Cresce linearmente |
| Restore speed | Fast (1 read) | Lento (replay N comandos) |
| Complexidade | Baixa | Média |
| Caso de uso | Backup, restart | Durabilidade contínua |

**Recomendação:** Implementar Snapshot (F2) primeiro. AOF é aditivo e pode ser habilitado independentemente.

---

## Critérios de aceite

- [ ] Comandos de escrita são appended ao AOF
- [ ] Replay reconstrói estado idêntico
- [ ] Fsync policy funciona (every-write, every-second, never)
- [ ] Compactação reduz tamanho do AOF
- [ ] Snapshot + AOF combinados funcionam
- [ ] AOF desabilitado por default (opt-in)
- [ ] Zero regressão nos testes existentes
