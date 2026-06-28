# Plano de Ação — ZetDB Hardening & Security Updates

## Visão Geral

Este plano cobre a **Fase 6 (Hardening)** do ZetDB, correção de bugs críticos, atualização de dependências vulneráveis e hardening do CI/CD. O objetivo é deixar o projeto em estado **production-ready** com zero testes falhando, zero vulnerabilidades conhecidas e zero panics em código de produção.

---

## Fase 1: Correção de Bugs Críticos (Testes Falhando)

**Objetivo:** Corrigir os 2 testes que falham atualmente.

### 1.1 — `storage::aof::tests::rewrite_compacts_aof`

**Problema:** O teste chama `rewrite_aof(&engine, &path)` que gera um arquivo `.tmp` (ex: `compact.aof.tmp`), mas depois tenta fazer `replay_aof` no path original (`compact.aof`). Como o arquivo original não existe, o replay retorna 0 entries.

**Causa raiz:** A função `rewrite_aof` escreve para `{path}.tmp` e deixa o rename para o caller (`finalize_rewrite`). O teste não chama `finalize_rewrite`.

**Solução:** O teste deve fazer replay do arquivo `.tmp` (que é onde `rewrite_aof` escreve), ou deve chamar `finalize_rewrite` antes do replay. A abordagem correta é fazer replay do `.tmp` pois o teste unitário não tem acesso a um `AofWriter` para chamar `finalize_rewrite`.

**Arquivo:** `src/storage/aof.rs` (linha ~554)

**Critério de aceitação:** Teste passa, replay retorna 2 entries, valores corretos.

### 1.2 — `storage::snapshot::tests::truncated_snapshot_does_not_panic`

**Problema:** O teste calcula `truncate_at = HEADER_SIZE + 2 + 2 + 4 + 2 + 8 + 2` = 18 + 18 + 2 = 38 bytes. Mas o cálculo está incorreto — ele tenta truncar em um ponto que não faz sentido para a estrutura do snapshot, e o `load_snapshot` panica ao tentar ler dados truncados.

**Análise detalhada:**
- HEADER_SIZE = 18 bytes (magic 4 + version 1 + flags 1 + count 4 + timestamp 8)
- Entry 1 (k1=v1): key_len(2) + "k1"(2) + val_len(4) + "v1"(2) + ttl(8) = 18 bytes
- Entry 2 (k2=v2): key_len(2) + "k2"(2) + val_len(4) + "v2"(2) + ttl(8) = 18 bytes
- CRC: 4 bytes
- Total: 18 + 18 + 18 + 4 = 58 bytes

O teste trunca em 38 bytes (HEADER + Entry1 + 2 bytes do key_len de Entry2). Depois recalcula CRC e escreve. O `load_snapshot` lê o header, vê count=2, tenta ler a segunda entry mas os dados acabam no meio do key_len — isso causa panic no `read_u16_le` que faz `unwrap()` internamente.

**Solução:** O `load_snapshot` deve ser robusto a truncamento. O teste deve truncar em um ponto onde o parser pode detectar gracefully que os dados acabaram, ou o parser deve retornar erro em vez de panic.

**Arquivo:** `src/storage/snapshot.rs` (linhas ~159-223, função `load_snapshot`)

**Critério de aceitação:** Teste passa, snapshot truncado carrega 1 entry sem panic.

---

## Fase 2: Hardening Fase 6 (PHASES.md)

**Objetivo:** Implementar os 6 itens de hardening da Fase 6.

### 2.1 — H1: Remover `unwrap()` de parsing binário (snapshot + AOF)

**Problema:** As funções `read_u16_le`, `read_u32_le`, `read_i64_le` em `snapshot.rs` e `aof.rs` usam `unwrap()` ou `?` que pode panicar em dados truncados.

**Solução:**
- Tornar todas as funções de leitura binária retornar `Result` com mensagens claras
- Nunca panicar em dados de arquivo externo
- Logar warnings em vez de crashar

**Arquivos:** `src/storage/snapshot.rs`, `src/storage/aof.rs`

**Critério de aceitação:** Testes de arquivos truncados/corrompidos passam sem panic.

### 2.2 — H2: Trocar `std::sync::Mutex` por `tokio::sync::Mutex` no AofWriter

**Problema:** Já está usando `tokio::sync::Mutex` — verificar se há algum `std::sync::Mutex` restante.

**Solução:** Verificar e garantir que todos os locks async usam `tokio::sync::Mutex`.

**Arquivo:** `src/storage/aof.rs`

**Critério de aceitação:** Nenhum `std::sync::Mutex` em código async.

### 2.3 — H3: AOF rewrite race-free (rename + reopen dentro do lock)

**Problema:** Já implementado em `finalize_rewrite` — verificar se está correto.

**Solução:** Revisar a implementação e garantir que o lock cobre rename + reopen atomicamente.

**Arquivo:** `src/storage/aof.rs` (função `finalize_rewrite`)

**Critério de aceitação:** Review de código confirma race-free.

### 2.4 — H4: Write timeout e proteção contra clientes lentos

**Problema:** Já implementado — `write_timeout` existe em `session.rs`. Verificar se está funcionando corretamente.

**Solução:** Adicionar teste de integração para write timeout.

**Arquivo:** `src/server/session.rs`

**Critério de aceitação:** Cliente lento é desconectado após write timeout.

### 2.5 — H5: `max_keys` com eviction amostral aproximado

**Problema:** Já implementado — `evict_one_if_needed` amostra 5 chaves. Verificar se está funcionando em todos os caminhos (SET, INCR, MSET).

**Solução:** Garantir que `max_keys` é respeitado em todos os comandos de escrita, incluindo MSET.

**Arquivo:** `src/storage/dashmap_engine.rs`

**Critério de aceitação:** Teste de eviction com MSET passa.

### 2.6 — H6: Logar erros de replay/load em vez de descartar silenciosamente

**Problema:** Alguns erros de AOF replay são logados como `warn!`, mas o snapshot truncado pode falhar silenciosamente.

**Solução:** Garantir que todos os erros de load/replay são logados com nível apropriado (warn para truncamento, error para corrupção).

**Arquivos:** `src/storage/snapshot.rs`, `src/storage/aof.rs`

**Critério de aceitação:** Todos os caminhos de erro em load/replay logam a causa.

---

## Fase 3: Atualização de Dependências (Cargo.toml)

**Objetivo:** Atualizar todas as dependências para versões mais recentes e seguras.

### Dependências a atualizar:

| Dependência | Atual | Target | Motivo |
|-------------|-------|--------|--------|
| tokio | 1.52.1 | 1.52.3 | Security patch (dependabot #4) |
| bytes | 1.11.1 | 1.12.0 | Bug fixes (dependabot #15) |
| log | 0.4.29 | 0.4.33 | Security/bug fixes (dependabot #17) |
| env_logger | 0.11.10 | 0.11.11 | Bug fixes (dependabot #16) |
| memchr | 2.8.0 | 2.8.2 | Bug fixes (dependabot #14) |
| serde_json | 1.0.149 | 1.0.150 | Bug fixes (dependabot #9) |
| dashmap | 6.1.0 | 6.2.1 | Bug fixes (dependabot #8) |
| clap | 4.6.1 | 4.6.1 | Já atualizado (não há PR) |
| serde | 1.0.228 | 1.0.228 | Já atualizado (não há PR) |
| crc32fast | 1.5.0 | 1.5.0 | Já atualizado |
| itoa | 1.0.18 | 1.0.18 | Já atualizado |

### Ação:
1. Atualizar `Cargo.toml` com as novas versões
2. Rodar `cargo update` para atualizar `Cargo.lock`
3. Rodar `cargo test` para verificar compatibilidade
4. Rodar `cargo clippy` para verificar warnings

**Arquivo:** `Cargo.toml`

**Critério de aceitação:** Todas as dependências atualizadas, testes passam, clippy limpo.

---

## Fase 4: Hardening CI/CD (GitHub Actions + Docker)

**Objetivo:** Atualizar actions e imagens Docker para versões seguras.

### 4.1 — Atualizar GitHub Actions

| Action | Atual | Target | PR |
|--------|-------|--------|-----|
| actions/checkout | v4 | v6 | #1 |
| actions/download-artifact | v4 | v8 | #2 |
| softprops/action-gh-release | v2 | v3 | #7 |
| docker/setup-buildx-action | v3 | v4 | #6 |
| docker/metadata-action | v5 | v6 | #3 |

**Nota:** `actions/upload-artifact@v4` não tem PR aberto — verificar se precisa atualizar.

**Arquivo:** `.github/workflows/ci.yml`, `.github/workflows/release.yml`

### 4.2 — Atualizar Docker base image

| Imagem | Atual | Target | PR |
|--------|-------|--------|-----|
| rust | 1.85-bookworm | 1.96-bookworm | #12 |

**Nota:** Rust 1.96 é uma versão muito alta (não existe ainda — o PR provavelmente se refere a uma versão mais recente como 1.85.x ou 1.86). Verificar a versão correta disponível.

**Arquivo:** `Dockerfile`

### 4.3 — Adicionar cargo-audit ao CI

**Objetivo:** Detectar vulnerabilidades em dependências automaticamente.

**Ação:** Adicionar step ao CI:
```yaml
- name: Install cargo-audit
  run: cargo install cargo-audit
- name: Audit dependencies
  run: cargo audit
```

**Arquivo:** `.github/workflows/ci.yml`

**Critério de aceitação:** CI detecta vulnerabilidades em dependências.

---

## Fase 5: Validação Final

**Objetivo:** Garantir que tudo está funcionando perfeitamente.

### Checklist:
- [ ] `cargo test` — 166/166 passando (0 falhas)
- [ ] `cargo clippy -- -D warnings` — 0 warnings
- [ ] `cargo build --release` — sucesso
- [ ] `cargo fmt -- --check` — formatação correta
- [ ] `cargo audit` — 0 vulnerabilidades (se cargo-audit instalado)
- [ ] Docker build — sucesso
- [ ] Documentação atualizada (`CHANGELOG.md`, `README.md` se necessário)

---

## Ordem de Execução

```
Fase 1 (Bug Fixes)
  → Fase 2 (Hardening)
  → Fase 3 (Deps Cargo)
  → Fase 4 (CI/CD)
  → Fase 5 (Validação)
```

**Dependências:**
- Fase 1 deve ser feita antes de Fase 2 (os bugs são parte do hardening)
- Fase 3 pode ser feita em paralelo com Fase 1-2 (são independentes)
- Fase 4 depende de Fase 3 (CI deve testar com deps atualizadas)
- Fase 5 depende de todas as anteriores

---

## Estimativa de Tempo

| Fase | Estimativa | Complexidade |
|------|-----------|-------------|
| Fase 1: Bug Fixes | 2-3 horas | Média |
| Fase 2: Hardening | 4-6 horas | Alta |
| Fase 3: Deps Cargo | 1-2 horas | Baixa |
| Fase 4: CI/CD | 2-3 horas | Média |
| Fase 5: Validação | 1-2 horas | Baixa |
| **Total** | **10-16 horas** | |

---

## Riscos e Mitigações

| Risco | Probabilidade | Impacto | Mitigação |
|-------|-------------|---------|-----------|
| DashMap 6.2.1 quebra API | Baixa | Alto | Testar antes de commitar |
| Tokio 1.52.3 quebra async | Baixa | Alto | Testes de integração TCP |
| Atualização de Actions quebra CI | Média | Médio | Testar em branch separada |
| Hardening introduz regressão | Média | Alto | Testes extensivos após cada mudança |

---

## Critérios Globais de Sucesso

Ao final deste plano:
1. ✅ Zero testes falhando (166/166 passando)
2. ✅ Zero vulnerabilidades em dependências
3. ✅ Zero panics em parsing de dados externos
4. ✅ CI/CD atualizado e funcionando
5. ✅ Docker build funcionando
6. ✅ Clippy limpo (`-D warnings`)
7. ✅ Documentação reflete o estado atual
