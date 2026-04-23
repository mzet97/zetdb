# ZetDB — Arquitetura Técnica

## Visão Geral

ZetDB é um banco de dados chave-valor em memória, inspirado no Redis, implementado em Rust. Suporta protocolos inline e RESP, persistência via Snapshot + AOF, e concorrência lock-free com DashMap.

**Stack:** Rust (stable, edition 2021) | Tokio | DashMap | bytes

---

## Diagrama de Componentes

```mermaid
graph TB
    subgraph Clientes
        C1[Cliente 1]
        C2[Cliente 2]
        CN[Cliente N]
    end

    subgraph "server/"
        TCP[tcp.rs<br/>TCP Accept Loop]
        SES[session.rs<br/>Connection Handler]
    end

    subgraph "protocol/"
        PAR[parser.rs<br/>Inline + RESP Parser]
        RES[response.rs<br/>RESP Serializer]
    end

    subgraph "application/"
        DSP[dispatcher.rs<br/>Command Router]
    end

    subgraph "domain/"
        CMD[command.rs<br/>Command Enum]
        VAL[value.rs<br/>ValueEntry]
        ERR[errors.rs<br/>Error Types]
    end

    subgraph "storage/"
        ENG[engine.rs<br/>KvEngine Trait]
        DME[dashmap_engine.rs<br/>DashMap Implementation]
        SNAP[snapshot.rs<br/>Binary Snapshot]
        AOFA[aof.rs<br/>Append-Only File]
        TTL[ttl.rs<br/>TTL Sweeper]
    end

    subgraph "observability/"
        MET[metrics.rs<br/>Atomic Counters]
    end

    subgraph "config/"
        CFG[mod.rs<br/>Configuration]
    end

    C1 & C2 & CN --> TCP
    TCP --> SES
    SES --> PAR
    PAR --> DSP
    DSP --> ENG
    ENG --> DME
    DSP --> RES
    SES --> RES
    DSP --> MET
    DSP --> AOFA
    DME --> SNAP
    DME --> AOFA
    DME --> TTL
    DSP --> CMD
    DSP --> VAL
    DSP --> ERR
    CFG --> TCP
```

---

## Diagrama de Sequência — Processamento de Comando

```mermaid
sequenceDiagram
    participant C as Cliente
    participant T as TCP Listener
    participant S as Session
    participant P as Parser
    participant D as Dispatcher
    participant E as DashMapEngine
    participant A as AOF Writer
    participant M as Metrics

    C->>T: TCP Connect
    T->>S: spawn handle_session()
    S->>M: connection_opened()

    loop Read Loop
        C->>S: Dados TCP (pipeline batch)
        S->>P: try_parse_frame(read_buf)

        alt Inline Protocol
            P->>P: parse_inline_frame()
        else RESP Protocol
            P->>P: parse_resp_frame()
        end

        P-->>S: Complete { consumed, command }

        S->>S: command.is_write()?
        alt Write Command
            S->>A: to_aof_entry()
        end

        S->>D: dispatch(engine, command)
        D->>E: get() / set() / del() / incr()
        E-->>D: Result
        D-->>S: Response

        S->>M: record_command() [se métricas]

        alt Write + Success + AOF
            S->>A: append_raw(entry)
        end

        S->>S: response.write_to(write_buf)
    end

    S->>C: write_all(write_buf) [batch flush]
    C->>S: Disconnect / Timeout
    S->>M: connection_closed()
```

---

## Diagrama de Sequência — Snapshot (Dump)

```mermaid
sequenceDiagram
    participant BG as Background Task
    participant E as DashMapEngine
    participant FS as Filesystem

    BG->>E: dump_snapshot(engine, path)
    E->>E: dump_entries(|key, val, ttl| ...)
    E->>FS: create temp file

    loop Para cada entry não expirada
        E->>FS: write [key_len:u16][key][val_len:u32][val][ttl_ms:i64]
    end

    E->>E: CRC32(header + entries)
    E->>FS: write CRC32 footer
    E->>FS: fsync()
    E->>FS: rename(temp → path)
    Note over E,FS: Atomic write: temp + fsync + rename
```

### Formato Binário Snapshot (ZDB1)

```
┌─────────────────────────────────────────────────┐
│ Header                                          │
│  [ZDB1:4] [version:1] [flags:1] [count:u32 LE] │
│  [timestamp:u64 LE]                             │
├─────────────────────────────────────────────────┤
│ Entry × N                                       │
│  [key_len:u16 LE] [key] [val_len:u32 LE] [val]  │
│  [ttl_ms:i64 LE]  (-1 = sem TTL)                │
├─────────────────────────────────────────────────┤
│ Footer                                          │
│  [CRC32:u32 LE]                                 │
└─────────────────────────────────────────────────┘
```

---

## Diagrama de Sequência — Snapshot (Restore)

```mermaid
sequenceDiagram
    participant MAIN as main.rs
    participant FS as Filesystem
    participant E as DashMapEngine

    MAIN->>FS: load_snapshot(path)
    FS->>FS: read file
    FS->>FS: validar magic == "ZDB1"
    FS->>FS: validar version == 1
    FS->>FS: calcular CRC32 e validar

    loop Para cada entry
        FS->>E: engine.set(key, ValueEntry)
    end

    E-->>MAIN: Ok(count)
```

---

## Diagrama de Sequência — AOF Write

```mermaid
sequenceDiagram
    participant S as Session
    participant C as Command
    participant W as AofWriter
    participant FS as Filesystem

    S->>C: command.is_write()?
    alt Write Command
        S->>C: to_aof_entry()
        Note over C: SET: [0x01][key_len][key][val_len][val][ttl_ms]
        Note over C: DEL: [0x02][key_len][key]
        Note over C: INCR: [0x03][key_len][key]
        C-->>S: Some(Vec<u8>)

        alt dispatch success
            S->>W: append_raw(entry)
            W->>FS: write(file)

            alt FsyncPolicy::EveryWrite
                W->>FS: fsync()
            end
        end
    end
```

---

## Diagrama de Sequência — AOF Rewrite (Compaction)

```mermaid
sequenceDiagram
    participant BG as Background Task
    participant E as DashMapEngine
    participant W as AofWriter
    participant FS as Filesystem

    loop A cada 60s
        BG->>FS: file.size() > threshold?
        alt Size exceeded
            BG->>E: dump_entries()
            E->>FS: write new file (SET commands only)
            Note over FS: Apenas estado atual,<br/>sem DEL/INCR intermediários
            FS->>FS: fsync()
            FS->>FS: rename(new → path)
            BG->>W: reopen file handle
        end
    end
```

---

## Diagrama — Modelo de Dados

```mermaid
classDiagram
    class Command {
        <<enum>>
        Ping
        Get(key: String)
        Set(key: String, value: Bytes, ttl: Option~Duration~)
        Del(key: String)
        Incr(key: String)
        Info
        DbSize
        +is_write() bool
        +command_type() CommandType
        +to_aof_entry() Option~Vec~u8~~
    }

    class ValueEntry {
        +data: Bytes
        +expires_at: Option~Instant~
        +new(data: Bytes) ValueEntry
        +with_ttl(data: Bytes, dur: Duration) ValueEntry
        +is_expired() bool
    }

    class Response {
        <<enum>>
        Pong
        Ok
        Value(Option~Bytes~)
        Integer(i64)
        Error(ResponseError)
        +write_to(buf: BytesMut)
        +serialize() String
    }

    class ResponseError {
        <<enum>>
        UnknownCommand(String)
        SyntaxError(String)
        TypeError(String)
        NotFound(String)
        InternalError(String)
    }

    class KvEngine {
        <<trait>>
        +get(key: &str) Result~Option~ValueEntry~~
        +set(key: String, value: ValueEntry) Result~
        +del(key: &str) Result~bool~
        +incr(key: &str) Result~i64~
        +len() usize
    }

    class DashMapEngine {
        -map: DashMap~String, ValueEntry~
        +sweep_expired()
        +dump_entries(f: F) usize
    }

    KvEngine <|.. DashMapEngine : implements
    Command --> ValueEntry : creates
    Response --> ResponseError : contains
```

---

## Diagrama — Fluxo de Persistência (Startup)

```mermaid
flowchart TD
    A[main.rs: startup] --> B{snapshot.enabled?}
    B -->|Sim| C[load_snapshot dump.zdb]
    B -->|Não| D[Engine vazio]
    C --> E{aof.enabled?}
    D --> E
    E -->|Sim| F[replay_aof appendonly.zdb]
    E -->|Não| G[Pronto]
    F --> G

    G --> H[Spawn TTL Sweeper]
    H --> I{snapshot.enabled?}
    I -->|Sim| J[Spawn Snapshot Writer<br/>cada 60s]
    I -->|Não| K{aof.enabled?}
    J --> K
    K -->|Sim| L[Create AofWriter]
    K -->|Não| M[run_server TCP]
    L --> L1{fsync policy?}
    L1 -->|EverySecond| L2[Spawn AOF Fsync ticker]
    L1 -->|EveryWrite/Never| L3[Spawn AOF Rewriter]
    L2 --> L3
    L3 --> M
```

---

## Diagrama — Ciclo de Vida da Sessão

```mermaid
stateDiagram-v2
    [*] --> Connected: TCP Accept
    Connected --> Reading: await read_buf()
    Reading --> Parsing: dados recebidos

    Parsing --> FrameComplete: try_parse_frame() → Complete
    Parsing --> Incomplete: try_parse_frame() → Incomplete
    Parsing --> Error: try_parse_frame() → Err

    FrameComplete --> Dispatch: command pronto
    Dispatch --> Metrics: record_command()
    Dispatch --> AOF: is_write() → append
    Dispatch --> Serialize: response.write_to()
    Serialize --> Parsing: próximo frame

    Metrics --> Parsing
    AOF --> Serialize

    Incomplete --> Reading: ler mais dados

    Error --> SkipToNewline: skip_to_newline()
    SkipToNewline --> Serialize: error response

    Serialize --> Flush: sem mais frames
    Flush --> Reading: write_all() + clear

    Reading --> Disconnected: read = 0 / error / timeout
    Disconnected --> [*]
```

---

## Diagrama — Evicção de TTL

```mermaid
flowchart LR
    subgraph "Lazy (on access)"
        G1[get key] --> EX1{expired?}
        EX1 -->|Sim| RM1[remove + return None]
        EX1 -->|Não| RET[return value]
    end

    subgraph "Active (sweeper)"
        SW[TTL Sweeper<br/>cada 1s] --> ITER[iterate all entries]
        ITER --> RET2[retain !is_expired]
    end
```

---

## Diagrama — Concorrência

```mermaid
flowchart TD
    subgraph "Tokio Runtime (multi-thread)"
        A1[Accept Loop]
        W1[Worker 1]
        W2[Worker 2]
        WN[Worker N]
        BG1[TTL Sweeper]
        BG2[Snapshot Writer]
        BG3[AOF Fsync/Rewriter]
    end

    subgraph "Storage Layer (lock-free)"
        DM[DashMap<br/>sharded por hash(key)]
        DM --> S1[Shard 1: RwLock]
        DM --> S2[Shard 2: RwLock]
        DM --> SN[Shard N: RwLock]
    end

    A1 --> W1 & W2 & WN
    W1 & W2 & WN --> DM

    subgraph "Metrics (lock-free)"
        AT[AtomicU64 counters<br/>Ordering::Relaxed]
    end

    W1 & W2 & WN -.-> AT
```

---

## Protocolo

### Inline (texto)

```
SET mykey hello\r\n     →  +OK\r\n
GET mykey\r\n           →  +hello\r\n
DEL mykey\r\n           →  :1\r\n
INCR counter\r\n        →  :1\r\n
PING\r\n                →  +PONG\r\n
INFO\r\n                →  +<stats text>\r\n
DBSIZE\r\n              →  :42\r\n
```

### RESP (Redis Serialization Protocol)

```
*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$5\r\nhello\r\n  →  +OK\r\n
*2\r\n$3\r\nGET\r\n$5\r\nmykey\r\n                   →  +hello\r\n
*2\r\n$3\r\nDEL\r\n$5\r\nmykey\r\n                   →  :1\r\n
*2\r\n$4\r\nINCR\r\n$7\r\ncounter\r\n                →  :1\r\n
*1\r\n$4\r\nPING\r\n                                  →  +PONG\r\n
```

Auto-detecção: `buf[0] == '*'` → RESP, senão → inline.

---

## Otimizações de Performance

| Técnica | Local | Impacto |
|---|---|---|
| Zero-allocation parsing | parser.rs | Sem Vec intermediário, parse inline |
| Zero-allocation response | response.rs | `write_to(BytesMut)` direto |
| `itoa` para inteiros | response.rs, dashmap_engine.rs | Sem `format!` / `to_string()` |
| `from_utf8_unchecked` | parser.rs | Pula validação UTF-8 (garantido pelo protocolo) |
| Batched writes | session.rs | Flush único por read cycle |
| Metrics toggle | session.rs | `metrics_enabled: false` = zero overhead |
| DashMap sharding | dashmap_engine.rs | Lock por shard, não global |
| `entry()` API | dashmap_engine.rs | INCR atômico sem get+set |
| Atomic file write | snapshot.rs, aof.rs | temp + fsync + rename |
| `Bytes::copy_from_slice` | dashmap_engine.rs | INCR reutiliza buffer itoa |

---

## Configuração

| Parâmetro | Default | Descrição |
|---|---|---|
| `bind_addr` | 127.0.0.1 | Endereço de bind |
| `port` | 6379 | Porta TCP |
| `read_timeout` | 30s | Timeout de leitura por conexão |
| `sweep_interval` | 1s | Intervalo do TTL sweeper |
| `snapshot.enabled` | true | Persistência por snapshot |
| `snapshot.path` | dump.zdb | Arquivo de snapshot |
| `snapshot.interval` | 60s | Intervalo entre snapshots |
| `aof.enabled` | false | Append-Only File |
| `aof.path` | appendonly.zdb | Arquivo AOF |
| `aof.fsync` | EverySecond | Política de fsync |
| `aof.rewrite_threshold_mb` | 64 | Limite para compaction |
| `metrics_enabled` | false | Contadores por comando |
