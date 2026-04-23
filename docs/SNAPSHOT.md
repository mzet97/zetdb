# ZetDB — Snapshot Persistence Design

## Objetivo

Permitir que o estado do KV store seja persistido em disco e restaurado após restart, garantindo consistência e atomicidade.

---

## Decisões de Design

### 1. Formato binário versionado

Snapshot usa formato binário compacto com header, entries e footer com checksum.

```
┌─────────────────────────────────────┐
│ Header (18 bytes)                   │
│   Magic: b"ZDB1" (4 bytes)         │
│   Version: u8 = 1 (1 byte)         │
│   Flags: u8 = 0 (1 byte, reserved) │
│   Entry count: u32 LE (4 bytes)    │
│   Created at: u64 LE (8 bytes,     │
│     unix epoch millis)             │
├─────────────────────────────────────┤
│ Entries (repeated)                  │
│   Key length: u16 LE               │
│   Key bytes: [u8; key_len]         │
│   Value length: u32 LE             │
│   Value bytes: [u8; value_len]     │
│   TTL remaining ms: i64 LE         │
│     (-1 = sem TTL, >= 0 = ms)      │
├─────────────────────────────────────┤
│ Footer (4 bytes)                    │
│   CRC32: u32 LE                    │
└─────────────────────────────────────┘
```

**Overhead por entry:** 14 bytes (2 + 4 + 8 fixos) + key/value lengths.

### 2. Serialização de TTL

`Instant` não é serializável (monotônico, não relativo a epoch). Solução:

- **Dump:** `remaining_ms = expires_at.map(|t| t.duration_since(Instant::now()).as_millis() as i64).unwrap_or(-1)`
- **Restore:** se `remaining_ms >= 0`, `expires_at = Some(Instant::now() + Duration::from_millis(remaining_ms))`
- **Expiração durante dump:** se `remaining_ms <= 0`, a entry é omitida do snapshot

### 3. Escrita atômica

Snapshots são escritos de forma atômica usando o padrão temp-file + rename:

```
1. Abrir <path>.tmp para escrita
2. Serializar header + entries + CRC32
3. fsync(fd)
4. rename(<path>.tmp → <path>)
```

No Windows, `rename` sobrescreve o arquivo destino. No Linux, é atomic-replace. Em ambos, o arquivo final nunca fica em estado parcial.

### 4. Background task

Snapshot roda como task Tokio periódica em background:

```rust
pub async fn run_snapshotter(
    engine: Arc<DashMapEngine>,
    config: SnapshotConfig,
) {
    let mut ticker = tokio::time::interval(config.interval);
    loop {
        ticker.tick().await;
        match dump_snapshot(engine.as_ref(), &config.path) {
            Ok(count) => log::info!("snapshot saved: {count} entries"),
            Err(e) => log::error!("snapshot failed: {e}"),
        }
    }
}
```

**Características:**
- Iteração concorrente sobre DashMap (não bloqueia reads/writes)
- Entries expiradas são filtradas durante dump
- Roda em task separada, não afeta accept loop ou sessions
- Frequência configurável (default: 60s)

### 5. Restore na inicialização

Carga do snapshot antes de aceitar conexões:

```rust
// Em main.rs, antes de run_server:
let engine = Arc::new(DashMapEngine::new());
if let Ok(count) = load_snapshot(engine.as_ref(), &config.snapshot.path) {
    log::info!("restored {count} entries from snapshot");
}
run_server(config, engine).await?;
```

**Comportamento:**
- Se arquivo não existe: engine vazia, sem erro
- Se arquivo existe mas CRC inválido: log error, engine vazia
- Se versão não suportada: log error, engine vazia
- TTLs expirados durante restore são descartados (lazy eviction natural)

### 6. Configuração

```rust
pub struct SnapshotConfig {
    pub enabled: bool,           // default: true
    pub path: String,            // default: "dump.zdb"
    pub interval: Duration,      // default: 60s
}
```

Adicionado a `Config` existente:

```rust
pub struct Config {
    pub bind_addr: String,
    pub port: u16,
    pub read_timeout: Duration,
    pub sweep_interval: Duration,
    pub snapshot: SnapshotConfig,
}
```

---

## Integração arquitetural

### Módulo novo: `storage/snapshot.rs`

```
src/storage/
  mod.rs
  engine.rs          // trait KvEngine
  dashmap_engine.rs  // implementação concorrente
  ttl.rs             // sweeper
  snapshot.rs        // NOVO: dump/load
```

`snapshot.rs` depende de `DashMapEngine` diretamente (precisa acessar o mapa para iterar), não da trait `KvEngine`.

### Diagrama de fluxo

```
main.rs
  ├─ load_snapshot()        // antes de accept
  ├─ run_server()           // accept loop + sessions
  └─ run_snapshotter()      // background, periódico
       └─ dump_snapshot()
            ├─ DashMap::iter()
            ├─ Filtra expirados
            ├─ Serializa entries
            ├─ Calcula CRC32
            └─ Atomic write (tmp + rename)
```

### main.rs

```rust
#[tokio::main(flavor = "multi_thread", worker_threads = 12)]
async fn main() {
    env_logger::init();
    let config = Config::from_env();

    let engine = Arc::new(DashMapEngine::new());

    // Restore
    if config.snapshot.enabled {
        match load_snapshot(engine.as_ref(), &config.snapshot.path) {
            Ok(n) => log::info!("restored {n} entries"),
            Err(e) => log::warn!("snapshot restore failed: {e}"),
        }
    }

    // Sweeper
    let sweeper_engine = engine.clone();
    tokio::spawn(run_sweeper(sweeper_engine, config.sweep_interval));

    // Snapshotter
    if config.snapshot.enabled {
        let snap_engine = engine.clone();
        let snap_config = config.snapshot.clone();
        tokio::spawn(run_snapshotter(snap_engine, snap_config));
    }

    // Server
    run_server(config, engine).await.unwrap();
}
```

---

## Dependências

| Crate | Uso |
|---|---|
| `crc32fast` | Checksum CRC32 do snapshot (fast, SIMD) |

Adicionar ao Cargo.toml:
```toml
crc32fast = "1"
```

---

## Plano de implementação

### F2.1: Tipos e serialização
- `SnapshotConfig` em `config/mod.rs`
- `storage/snapshot.rs`: `dump_snapshot()`, `load_snapshot()`
- Formato binário com CRC32

### F2.2: Background task
- `run_snapshotter()` task periódica
- Integração em `main.rs`

### F2.3: Restore na inicialização
- `load_snapshot()` antes de `run_server`
- Tratamento de erros (arquivo ausente, CRC inválido, versão)

### F2.4: Configuração
- `SnapshotConfig` em `Config`
- Variáveis de ambiente: `ZETDB_SNAPSHOT_ENABLED`, `ZETDB_SNAPSHOT_PATH`, `ZETDB_SNAPSHOT_INTERVAL`

### F2.5: Testes
- Unit: serialização/deserialização de entries com TTL
- Unit: CRC32 detecta corrupção
- Unit: entries expiradas são omitidas no dump
- Integração: dump → load → GET retorna valores corretos
- Integração: TTLs são restaurados com remaining time

---

## Critérios de aceite

- [ ] Snapshot contém todas as entries não-expiradas
- [ ] Escrita é atômica (rename de temp file)
- [ ] CRC32 detecta arquivo corrompido
- [ ] TTLs são preservados como remaining duration
- [ ] Restore funciona após restart do processo
- [ ] Background task não bloqueia operações normais
- [ ] Configuração via variáveis de ambiente
- [ ] Zero regressão nos 83 testes existentes
