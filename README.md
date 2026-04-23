# ZetDB

Banco de dados in-memory estilo Redis, implementado em Rust com foco em **alta concorrência**, **baixa latência** e **segurança de memória**.

## Visão Geral

ZetDB é um KV store TCP concorrente que evolui de um MVP funcional para um núcleo robusto preparado para RESP, TTL, persistência e extensões futuras.

### MVP

- Servidor TCP assíncrono (Tokio)
- Protocolo textual simples
- Comandos: `PING`, `SET`, `GET`, `DEL`, `INCR`
- Storage concorrente com DashMap (sharding por chave)
- TTL com lazy eviction + sweeper ativo
- Múltiplos clientes simultâneos

### Stack

| Componente | Tecnologia |
|---|---|
| Linguagem | Rust (estável) |
| Runtime assíncrono | Tokio |
| Buffers | `bytes::Bytes` / `BytesMut` |
| Storage concorrente | DashMap |
| Protocolo (MVP) | Textual simples |
| Protocolo (futuro) | RESP |

## Arquitetura

Arquitetura modular com inspiração hexagonal — o núcleo de domínio e storage é isolado de detalhes de transporte e protocolo.

```text
TCP Server -> Connection I/O -> Parser -> Dispatcher -> Storage Engine
                                                             |
                                                    TTL / Lazy Eviction
                                                    Background Sweeper
```

### Módulos

```text
src/
  main.rs              # Entry point
  config/              # Configuração do servidor
  server/              # TCP server, accept loop, sessões
  protocol/            # Parser, comandos, respostas
  application/         # Command dispatcher
  domain/              # Tipos centrais, erros
  storage/             # Engine concorrente, TTL
  observability/       # Logging, métricas
```

## Documentação

| Documento | Descrição |
|---|---|
| [architecture.md](architecture.md) | Arquitetura completa do sistema |
| [prompt.md](prompt.md) | Metodologia SDD e regras de execução |
| [task.md](task.md) | Tarefas decompostas por épico |
| [docs/SPECIFICATION.md](docs/SPECIFICATION.md) | Especificação formal de contratos e tipos |
| [docs/PHASES.md](docs/PHASES.md) | Planejamento por fases com milestones |

## Quick Start

```bash
# Build
cargo build --release

# Run
./target/release/zetdb

# Testar (em outro terminal)
echo "PING" | nc localhost 6379
echo "SET mykey hello" | nc localhost 6379
echo "GET mykey" | nc localhost 6379
echo "DEL mykey" | nc localhost 6379
echo "INCR counter" | nc localhost 6379
```

## Desenvolvimento

Este projeto segue **SDD (Specification-Driven Development)** — toda implementação é precedida por especificação, validação arquitetural e critérios de aceite claros.

Veja [docs/PHASES.md](docs/PHASES.md) para o planejamento completo em fases.

## Licença

Veja [LICENSE](LICENSE).
