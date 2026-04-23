# ZetDB — Project Instructions

## Metodologia: SDD (Specification-Driven Development)

Antes de escrever qualquer código:
1. Validar aderência com `architecture.md`
2. Especificar contratos, tipos e comportamento esperado
3. Implementar apenas o escopo da tarefa corrente em `task.md`
4. Adicionar testes
5. Marcar tarefa como concluída somente se critérios de aceite forem atendidos

## Regras de Código

### Estrutura
- Manter separação entre `protocol/`, `application/`, `domain/`, `storage/`, `server/`, `config/`, `observability/`
- Nunca concentrar lógica em `main.rs` — ele apenas orquestra
- Cada camada deve ser testável isoladamente

### Rust
- Rust estável apenas
- Tokio como runtime assíncrono
- `bytes::Bytes` para payloads — evitar `String` no hot path do storage
- Sem `unwrap()` em código de produção sem justificativa explícita
- Erros tipados com enums — sem strings para erros de domínio
- Código idiomático, nomes precisos

### Concorrência
- DashMap para storage concorrente (sharding por chave)
- Nunca `RwLock<HashMap>` global no hot path
- `INCR` deve ser atômico via `entry()` ou equivalente — nunca `get + set` separados
- TTL: lazy eviction no acesso + sweeper periódico em background

### Performance
- Minimizar cópias de dados
- Operações O(1) no hot path
- Considerar limites de FD, timeouts, conexões lentas

## Ordem de Execução

Seguir rigorosamente a ordem definida em `task.md` seção 8.

## Arquivos de Referência

| Arquivo | Função |
|---|---|
| `architecture.md` | Decisões arquiteturais e contratos |
| `task.md` | Tarefas com critérios de aceite |
| `prompt.md` | Metodologia SDD detalhada |
| `docs/SPECIFICATION.md` | Especificação formal de tipos e interfaces |
| `docs/PHASES.md` | Planejamento em fases |

## Formato de Entrega por Tarefa

```
1. Task em execução
2. Leitura arquitetural aplicada
3. Implementação
4. Testes
5. Critérios de aceite
6. Estado atualizado
```
