# ZetDB — Observabilidade Avançada Design

## Objetivo

Dar visibilidade operacional ao ZetDB em produção: métricas de performance, contadores de comandos, latência por operação, conexões ativas e health check.

---

## Decisões de Design

### 1. Contadores atômicos (lock-free)

Usar `std::sync::atomic::AtomicU64` para contadores no hot path. Zero contenção, zero lock.

```rust
pub struct Metrics {
    pub commands_total: AtomicU64,
    pub commands_by_type: [AtomicU64; 5],  // PING, GET, SET, DEL, INCR
    pub connections_total: AtomicU64,
    pub connections_active: AtomicU64,
    pub errors_total: AtomicU64,
    pub keys_current: AtomicU64,
    pub bytes_used: AtomicU64,
}
```

Singleton global via `OnceLock`:

```rust
static METRICS: OnceLock<Metrics> = OnceLock::new();

pub fn metrics() -> &'static Metrics {
    METRICS.get_or_init(Metrics::new)
}
```

**Por que global e não passed por parâmetro?**
- Evita poluir assinaturas de funções no hot path
- `AtomicU64` é lock-free, não há contenção
- Mesmo padrão usado por `log::info!` (global logger)
- Fácil de acessar de qualquer módulo

### 2. Latência — Histograma amostrado

Latência por tipo de comando com amostragem para minimizar overhead:

```rust
pub struct LatencyTracker {
    /// Amostra 1 a cada N comandos (configurável, default: 100)
    sample_rate: u32,
    /// Ring buffer com últimas 1024 amostras
    samples: Mutex<Vec<LatencySample>>,
    write_idx: AtomicU32,
}

struct LatencySample {
    command: CommandType,
    duration_us: u64,  // microssegundos
}
```

**Cálculo de percentis:**
- p50, p95, p99 calculados sob demanda (não no hot path)
- Usa o ring buffer de amostras
- Chamado via `INFO latency` ou HTTP metrics endpoint

### 3. Structured logging

Evoluir de `log::info!` para campos estruturados:

```rust
log::info!(
    "command processed";
    "command" => %cmd_name,
    "key" => %key,
    "duration_us" => elapsed.as_micros(),
    "peer" => %peer,
);
```

Compatível com `env_logger` (ignora campos extras) e `tracing` (parseia campos).

**Níveis por contexto:**
| Evento | Nível |
|---|---|
| Server start/bind | INFO |
| Client connect/disconnect | INFO |
| Cada comando | DEBUG |
| Slow command (>1ms) | WARN |
| Parse error | WARN |
| Internal error | ERROR |
| Snapshot save/restore | INFO |
| AOF write/rewrite | INFO |

### 4. Health check

Comando `INFO` que retorna estatísticas do servidor:

```
INFO
# Server
zetdb_version:0.1.0
uptime_seconds:3600

# Clients
connected_clients:4
total_connections:42

# Stats
total_commands:1234567
commands_per_sec:15432
keyspace_hits:890000
keyspace_misses:23000

# Keyspace
db0:keys=1000,expires=50,avg_ttl=300
```

Resposta em formato texto plano (mesmo estilo Redis), múltiplas linhas terminadas em `\r\n`.

### 5. Comando DBSIZE

Adicionar comando `DBSIZE` para retornar número de chaves:

```
DBSIZE → :1000\r\n
```

Útil para monitoramento e alertas.

### 6. Slow log

Buffer circular de comandos lentos:

```rust
pub struct SlowLog {
    entries: Mutex<Vec<SlowLogEntry>>,
    threshold_us: u64,  // default: 10000 (10ms)
    max_entries: usize, // default: 128
}

struct SlowLogEntry {
    timestamp: u64,
    duration_us: u64,
    command: String,
    peer: SocketAddr,
}
```

Comando `SLOWLOG GET` retorna as últimas N entradas.

---

## Integração arquitetural

### Módulos afetados

```
src/observability/
  mod.rs            // já existe
  metrics.rs        // NOVO: contadores atômicos
  latency.rs        // NOVO: histograma amostrado
  slowlog.rs        // NOVO: log de comandos lentos

src/protocol/
  parser.rs         // Adicionar INFO, DBSIZE, SLOWLOG

src/application/
  dispatcher.rs     // Adicionar handlers + metrics hook
```

### Diagrama

```
Session Loop
  ├─ try_parse_frame()
  ├─ let start = Instant::now();
  ├─ dispatch(engine, cmd)
  │    ├─ Executa na engine
  │    └─ metrics().commands_by_type[cmd].fetch_add(1)
  ├─ let elapsed = start.elapsed();
  ├─ latency.record(cmd, elapsed)
  ├─ if elapsed > slow_threshold: slowlog.record(...)
  └─ response.write_to(&mut write_buf)
```

### Formato de resposta INFO

```
Response::BulkString(Some(info_text))
```

Onde `info_text` é um `Bytes` contendo todas as linhas de estatísticas.

---

## Ordem de implementação

### F4.1: Contadores atômicos
- `observability/metrics.rs` com `Metrics` struct
- Hook no dispatcher: incrementar por tipo de comando
- Hook no session: conexões ativas/total
- Hook no engine: keys current

### F4.2: Comandos INFO e DBSIZE
- Adicionar ao parser e dispatcher
- INFO coleta de `Metrics` + uptime + versão
- DBSIZE usa `DashMap::len()`

### F4.3: Latência amostrada
- `observability/latency.rs`
- Ring buffer de amostras
- Cálculo de p50/p95/p99 sob demanda
- Integrado ao comando INFO (seção Latency)

### F4.4: Slow log
- `observability/slowlog.rs`
- Threshold configurável
- Comando `SLOWLOG GET`

### F4.5: Prometheus export (futuro)
- HTTP endpoint `/metrics` em formato Prometheus
- Reutiliza os mesmos `AtomicU64` counters
- Crate: `prometheus` ou manual com `tiny_http`
- **Fora do escopo atual**, apenas preparado via contadores atômicos

---

## Overhead no hot path

| Operação | Custo |
|---|---|
| `AtomicU64::fetch_add(1)` | ~1ns (lock-free) |
| Amostragem de latência (1/100) | ~50ns a cada 100 comandos |
| Slow log check | ~5ns (comparação apenas) |
| **Total estimado** | **<2ns por comando (média)** |

Overhead desprezível comparado ao custo de parse + dispatch (~500ns).

---

## Critérios de aceite

- [ ] Contadores por tipo de comando (PING, GET, SET, DEL, INCR)
- [ ] Conexões ativas/total rastreadas
- [ ] Comando `INFO` retorna estatísticas legíveis
- [ ] Comando `DBSIZE` retorna contagem de chaves
- [ ] Latência p50/p95/p99 calculável
- [ ] Slow log registra comandos acima do threshold
- [ ] Overhead < 2ns por comando no hot path
- [ ] Zero regressão nos testes existentes
