# Contributing to ZetDB

Obrigado pelo interesse em contribuir com o ZetDB! Este documento define o processo e as expectativas para contribuicoes.

## Setup de Desenvolvimento

### Pre-requisitos

- [Rust](https://rustup.rs/) (estavel, edition 2021)
- [Git](https://git-scm.com/)
- Opcional: [Docker](https://www.docker.com/) para testes com container

### Como Rodar Local

```bash
# Clone
git clone https://github.com/mzet97/zetdb.git
cd zetdb

# Build
cargo build --release

# Rodar com defaults
./target/release/zetdb

# Rodar com AOF e snapshot
cargo run --release -- --aof-path data/zetdb.aof --snapshot-secs 60
```

### Como Rodar os Testes

```bash
# Todos os testes
cargo test

# Com output visivel
cargo test -- --nocapture

# Testes especificos
cargo test storage::
cargo test server::tcp::

# Clippy (todos os warnings sao erros)
cargo clippy -- -D warnings

# Formatacao
cargo fmt -- --check

# Build de release
cargo build --release
```

## Modelo de Branching

- `main`: branch protegida. Apenas via Pull Request.
- `feat/<descricao>`: novas funcionalidades
- `fix/<descricao>`: correcoes de bugs
- `chore/<descricao>`: tarefas de manutencao (deps, CI, etc.)
- `docs/<descricao>`: documentacao

Exemplos:
- `feat/add-decr-command`
- `fix/aof-rewrite-race`
- `chore/update-dependencies`
- `docs/improve-readme`

## Conventional Commits

Todas as mensagens de commit devem seguir [Conventional Commits](https://www.conventionalcommits.org/):

```
<tipo>(<escopo>): <descricao>

[corpo opcional]

[rodape opcional]
```

### Tipos

| Tipo | Uso |
|---|---|
| `feat` | Nova funcionalidade |
| `fix` | Correcao de bug |
| `docs` | Mudanca apenas na documentacao |
| `style` | Mudanca que nao afeta o significado do codigo (formatacao) |
| `refactor` | Refatoracao de codigo |
| `perf` | Melhoria de performance |
| `test` | Adicao/correcao de testes |
| `chore` | Tarefas de build, CI, dependencias |
| `security` | Correcao de vulnerabilidade |

### Escopos comuns

- `protocol`: parser e serializacao
- `storage`: engine, snapshot, AOF, TTL
- `server`: TCP, session handler
- `config`: CLI e configuracao
- `observability`: metricas e logging
- `ci`: GitHub Actions e pipelines
- `docs`: documentacao

Exemplos:
- `feat(storage): add max_keys eviction policy`
- `fix(protocol): handle truncated RESP frames`
- `chore(ci): add cargo clippy to pipeline`

## Estilo de Codigo

- Siga o estilo idomatico Rust (formatacao automatica via `cargo fmt`).
- Todos os warnings do Clippy devem ser resolvidos (`cargo clippy -- -D warnings`).
- Funcoes publicas devem ter doc comments (`///`).
- Modulos devem ter documentacao no `mod.rs`.
- Evite `unwrap()` em codigo de producao; use `?` ou `match` com tratamento de erro.
- Erros de dominio devem ser tipados (`enum`), nao strings.

## Decisoes Arquiteturais (ADR)

Decisoes arquiteturais irreversiveis devem ser documentadas em `docs/adr/` seguindo o template:

```markdown
# ADR-NNN: Titulo da Decisao

## Status
Proposed / Accepted / Deprecated / Superseded

## Contexto
O problema que estamos tentando resolver.

## Decisao
O que decidimos fazer.

## Consequencias
Positivas e negativas.
```

## Checklist de Pull Request

Antes de abrir um PR, verifique:

- [ ] Codigo compila: `cargo build --release`
- [ ] Todos os testes passam: `cargo test`
- [ ] Sem warnings do Clippy: `cargo clippy -- -D warnings`
- [ ] Codigo formatado: `cargo fmt`
- [ ] CHANGELOG.md atualizado (secao `[Unreleased]`)
- [ ] ADR criado se aplicavel (decisoes arquiteturais irreversiveis)
- [ ] Documentacao atualizada (README, docs/)
- [ ] Sem breaking changes nao documentados
- [ ] Seguranca: nenhum segredo hardcoded, nenhum `unwrap()` novo sem justificativa

## Reportando Vulnerabilidades

**Nao abra uma issue publica para vulnerabilidades de seguranca.**

Leia [SECURITY.md](SECURITY.md) para instrucoes de reporte responsavel.

## Licenca

Ao contribuir, voce concorda que suas contribuicoes serao licenciadas sob a mesma licenca MIT do projeto.
