# ZetDB — SDD Specification

Especificação formal de contratos, tipos, interfaces e protocolo do ZetDB.

---

## 1. Tipos de Domínio

### 1.1 Command

Comandos que o sistema aceita. A camada de protocolo converte input textual em variants deste enum.

```rust
pub enum Command {
    Ping,
    Get { key: String },
    Set {
        key: String,
        value: bytes::Bytes,
        ttl: Option<std::time::Duration>,
    },
    Del { key: String },
    Incr { key: String },
}
```

**Notas:**
- `key` é `String` no comando pois é curto e usado como lookup — o custo é aceitável.
- `value` é `bytes::Bytes` para suportar payload binário com mínimo de cópia.
- `ttl` é `Option<Duration>` — `None` significa sem expiração.

### 1.2 ValueEntry

Registro armazenado na engine.

```rust
pub struct ValueEntry {
    pub data: bytes::Bytes,
    pub expires_at: Option<std::time::Instant>,
}
```

**Notas:**
- `expires_at` usa `Instant` para evitar custos de relógio de parede no hot path.
- `None` em `expires_at` = chave sem expiração.

### 1.3 Response

Resposta do servidor ao cliente.

```rust
pub enum Response {
    Pong,
    Ok,
    Value(Option<bytes::Bytes>),
    Integer(i64),
    Error(ResponseError),
}
```

### 1.4 ResponseError

Erros categorizados que o servidor pode retornar.

```rust
pub enum ResponseError {
    UnknownCommand(String),
    SyntaxError(String),
    TypeError(String),
    NotFound(String),
    InternalError(String),
}
```

### 1.5 DomainError

Erros internos do domínio/engine — não são enviados diretamente ao cliente, mas mapeados para `ResponseError`.

```rust
pub enum DomainError {
    KeyNotFound(String),
    NotAnInteger(String),
    EngineError(String),
}

pub enum EngineError {
    StorageError(String),
}
```

---

## 2. Trait KvEngine

Contrato da engine de armazenamento. Toda implementação concreta (DashMap, futura engine customizada) deve satisfazer esta interface.

```rust
pub trait KvEngine: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<ValueEntry>, EngineError>;
    fn set(&self, key: String, value: ValueEntry) -> Result<(), EngineError>;
    fn del(&self, key: &str) -> Result<bool, EngineError>;
    fn incr(&self, key: &str) -> Result<i64, EngineError>;
}
```

**Regras:**
- `get` em chave expirada deve aplicar lazy eviction e retornar `Ok(None)`.
- `set` sobrescreve chave existente.
- `del` retorna `true` se a chave existia (e não estava expirada), `false` caso contrário.
- `incr` em chave inexistente cria com valor `1` e retorna `1`.
- `incr` em chave com valor não-inteiro retorna `Err(EngineError)` com tipo `NotAnInteger`.
- `incr` preserva TTL existente da chave.

---

## 3. Protocolo Textual (MVP)

### 3.1 Formato

Comandos são enviados como texto plano, terminados por `\r\n` ou `\n`:

```
COMMAND [arg1] [arg2] ...\r\n
```

### 3.2 Comandos

| Comando | Sintaxe | Resposta OK | Resposta Erro |
|---|---|---|---|
| PING | `PING` | `+PONG\r\n` | — |
| GET | `GET <key>` | `+<value>\r\n` ou `$-1\r\n` (nil) | `-ERR syntax\r\n` |
| SET | `SET <key> <value>` | `+OK\r\n` | `-ERR syntax\r\n` |
| DEL | `DEL <key>` | `:<0 ou 1>\r\n` | `-ERR syntax\r\n` |
| INCR | `INCR <key>` | `:<n>\r\n` | `-ERR not an integer\r\n` |

### 3.3 Regras de parsing

- Comando é case-insensitive (`PING` == `ping` == `Ping`).
- Chave é case-sensitive.
- Espaços em branco extras entre tokens são ignorados.
- Linha vazia retorna `-ERR empty command\r\n`.
- Comando desconhecido retorna `-ERR unknown command '<cmd>'\r\n`.

### 3.4 Formato de resposta

Inspiração RESP simplificada:

| Prefixo | Significado | Exemplo |
|---|---|---|
| `+` | Simple string | `+PONG\r\n`, `+OK\r\n` |
| `-` | Error | `-ERR not found\r\n` |
| `:` | Integer | `:1\r\n` |
| `$-1` | Nil/Null | `$-1\r\n` |

---

## 4. DashMapEngine

Implementação concreta de `KvEngine` usando `DashMap<String, ValueEntry>`.

### 4.1 Estrutura

```rust
pub struct DashMapEngine {
    map: DashMap<String, ValueEntry>,
}
```

### 4.2 Comportamento por operação

#### `get(key)`
1. Lookup na DashMap.
2. Se encontrado, verificar `expires_at`:
   - Expirado: remover entrada, retornar `None`.
   - Válido: retornar clone de `ValueEntry`.
3. Se não encontrado, retornar `None`.

#### `set(key, value)`
1. Inserir/sobrescrever entrada no mapa.
2. Retornar `Ok(())`.

#### `del(key)`
1. Verificar se chave existe e não está expirada.
2. Remover e retornar `true`, ou `false`.

#### `incr(key)`
1. Usar `entry()` API da DashMap para mutação atômica.
2. Se chave não existe: inserir `ValueEntry { data: "1".into(), expires_at: None }`, retornar `1`.
3. Se chave existe: parsear `data` como `i64`:
   - Sucesso: incrementar, atualizar valor, retornar novo valor. Preservar `expires_at`.
   - Falha: retornar erro `NotAnInteger`.

---

## 5. TTL e Expiração

### 5.1 Modelo

- `expires_at` é `Option<Instant>` dentro de `ValueEntry`.
- `SET` pode receber `ttl: Option<Duration>` que vira `expires_at: Some(Instant::now() + ttl)`.
- Sem `ttl` = `expires_at: None` = sem expiração.

### 5.2 Lazy Eviction

- Executado em `get()`: se `expires_at` é `Some` e `Instant::now() >= expires_at`, a chave é removida e retornada como inexistente.
- Também aplicado em `del()` (chave expirada conta como inexistente para retorno).
- Em `incr()`: chave expirada é tratada como inexistente (cria nova com valor `1`).

### 5.3 Sweeper Ativo

- Task Tokio periódica que itera sobre o mapa e remove entradas expiradas.
- Frequência configurável (default: a cada 1 segundo).
- Não deve bloquear o mapa por tempo excessivo — iterar com yields.
- Usa `retain()` ou equivalente para limpeza em batch.

```rust
pub async fn run_sweeper(engine: Arc<DashMapEngine>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        engine.sweep_expired();
    }
}
```

---

## 6. Configuração

```rust
pub struct Config {
    pub bind_addr: String,      // default: "127.0.0.1"
    pub port: u16,              // default: 6379
    pub read_timeout: Duration, // default: 30s
    pub sweep_interval: Duration, // default: 1s
}
```

- Configuração via variáveis de ambiente ou CLI flags.
- Fail fast em valores inválidos.

---

## 7. Servidor TCP

### 7.1 Accept Loop

```rust
pub async fn run_server(config: Config, engine: Arc<dyn KvEngine>) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind((config.bind_addr, config.port)).await?;
    loop {
        let (stream, addr) = listener.accept().await?;
        // spawn session task
    }
}
```

### 7.2 Sessão

Cada conexão vira uma task Tokio dedicada:

```rust
pub async fn handle_session(stream: TcpStream, addr: SocketAddr, engine: Arc<dyn KvEngine>) {
    // loop: read line -> parse -> dispatch -> write response
    // erro de I/O ou timeout = encerrar sessão
    // erro de parse = enviar resposta de erro e continuar
}
```

**Regras:**
- Falha em uma sessão não pode derrubar o accept loop.
- Erro de parse envia `ResponseError` mas mantém a conexão aberta.
- Timeout de leitura encerra a sessão graciosamente.

---

## 8. Dispatcher

Componente que recebe `Command` e delega para a engine, retornando `Response`.

```rust
pub fn dispatch(engine: &dyn KvEngine, cmd: Command) -> Response {
    match cmd {
        Command::Ping => Response::Pong,
        Command::Get { key } => { /* ... */ }
        Command::Set { key, value, ttl } => { /* ... */ }
        Command::Del { key } => { /* ... */ }
        Command::Incr { key } => { /* ... */ }
    }
}
```

**Regras:**
- Sessão TCP não acessa diretamente detalhes internos da engine.
- Dispatcher é testável sem servidor TCP.
- Erros da engine são mapeados para `ResponseError` aqui.

---

## 9. Observabilidade (MVP)

### 9.1 Logging

- `INFO` na inicialização (bind addr, porta).
- `INFO` em cada conexão aceita.
- `DEBUG` em cada comando recebido.
- `WARN` em erros de parse.
- `ERROR` em erros internos inesperados.

### 9.2 Contadores (futuro)

- Comandos processados por tipo.
- Conexões ativas.
- Erros de parse.
- Cache hits/misses.

---

## 10. Critérios de Consistência Arquitetural

Uma implementação é aderente se:

1. Engine não depende do protocolo textual.
2. `INCR` não é `get + set` ingênuo.
3. TTL existe no storage, não apenas no parser.
4. Sistema pode migrar para RESP sem reescrever o núcleo.
5. Não existe lock global central no hot path.
6. Separação clara entre rede, parser, dispatch e engine.
7. Cada módulo é testável isoladamente.
