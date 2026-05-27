# SeedGen

[![CI](https://github.com/ff4f/seedgen/actions/workflows/pr-check.yml/badge.svg)](https://github.com/ff4f/seedgen/actions/workflows/pr-check.yml)
[![Test Matrix](https://github.com/ff4f/seedgen/actions/workflows/test-matrix.yml/badge.svg)](https://github.com/ff4f/seedgen/actions/workflows/test-matrix.yml)
[![Crates.io](https://img.shields.io/crates/v/seedgen.svg)](https://crates.io/crates/seedgen)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Generate realistic seed data for your PostgreSQL database without writing a single factory.

SeedGen reads your schema, figures out the foreign keys, and fills your tables with data that actually looks right — emails in email columns, prices in price columns, dates that make sense. Point it at a database, get usable data in seconds.

## Install

```bash
cargo install seedgen
```

Or grab a prebuilt binary from the [releases page](https://github.com/ff4f/seedgen/releases/latest). Linux, macOS, and Windows are all there (x86_64 and arm64).

From source:

```bash
git clone https://github.com/ff4f/seedgen
cd seedgen
cargo build --release
```

## Usage

```bash
# Default: 10 rows per table
seedgen generate --url postgres://user:pass@localhost:5432/mydb

# Reproducible — same seed, same data
seedgen generate --url $DATABASE_URL --seed 42 --rows 100

# Use a built-in scenario
seedgen generate --url $DATABASE_URL --scenario ecommerce --seed 42

# Per-table row counts
seedgen generate --url $DATABASE_URL --entities users=1000,orders=5000

# Dump to a SQL file instead of inserting
seedgen generate --url $DATABASE_URL --output seed.sql --seed 42
```

Other commands:

```bash
seedgen introspect --url $DATABASE_URL          # inspect schema
seedgen reset --url $DATABASE_URL --confirm     # truncate all tables
seedgen completions zsh > ~/.zfunc/_seedgen     # shell completions
```

## What it actually does

You give it a database URL. SeedGen then:

1. Reads your tables, columns, and foreign keys via `information_schema`.
2. Sorts tables by dependency (Kahn's algorithm). Cyclic FKs get a two-phase insert.
3. Picks a generator per column based on its name and type — `email` columns get emails, `price` columns get money, `slug` columns get slugs.
4. Generates rows with a seeded PRNG (ChaCha8) and either inserts them or writes a SQL file.

The same seed always produces the same output. That's the main promise — if you ever see non-determinism for a given seed, it's a bug.

## Example output

```
$ seedgen generate --url $DATABASE_URL --seed 42 --rows 20

Generated 60 rows across 3 tables in 102.55ms (seed: 42)
  users                              20 rows  [3.84ms]
  posts                              20 rows  [3.63ms]
  comments                           20 rows  [3.43ms]
```

The SQL file output looks like this:

```sql
BEGIN;
SET session_replication_role = 'replica';

INSERT INTO "users" ("id", "email", "name", "bio", "is_active", "created_at") VALUES
  (1, 'ian.armstrong@fastmail.com', 'Greg Cook', '...', TRUE, '2025-03-17T12:22:11'),
  (2, 'john.myers@mail.com', 'Katelyn Hart', '...', FALSE, '2024-10-16T12:40:28'),
  ...;

SET session_replication_role = 'origin';
COMMIT;
```

## Scenarios

Four templates ship in the box: `ecommerce`, `saas`, `blog`, `social`. Each one comes with sensible distributions — most orders are `paid`, a small fraction are `pending`, that kind of thing.

You can also write your own:

```yaml
# my-scenario.yaml
seed: 42
tables:
  users:
    count: 100
    overrides:
      role:
        distribution: { admin: 5%, moderator: 10%, user: 85% }
      created_at:
        range: [2023-01-01, 2024-12-31]
  orders:
    count: per_parent(users, 0..10)
    overrides:
      status:
        distribution: { pending: 10%, paid: 60%, shipped: 20%, delivered: 10% }
  order_items:
    count: per_parent(orders, 1..5)
```

Run it with `seedgen generate -f my-scenario.yaml --url $DATABASE_URL`.

## GitHub Action

There's a composite action that grabs the binary and runs `generate` for you. Drop it into a workflow before your integration tests:

```yaml
- uses: ff4f/seedgen@v1
  with:
    database_url: ${{ secrets.TEST_DATABASE_URL }}
    scenario: ecommerce
    seed: 42
    truncate_first: true
```

Useful inputs: `database_url` (required), `scenario`, `scenario_file`, `seed`, `rows`, `entities`, `locale`, `fast`, `truncate_first`, `version`. The `database_url` is never echoed in logs.

For reproducible CI, pin both the action ref and the binary: `uses: ff4f/seedgen@v1` with `version: v0.1.0`.

## MCP server

If you're using Claude Desktop, Cursor, or another MCP-aware editor, you can let the model run SeedGen for you:

```json
{
  "mcpServers": {
    "seedgen": {
      "command": "seedgen",
      "args": ["mcp-server"],
      "env": { "DATABASE_URL": "postgres://user:pass@localhost/mydb" }
    }
  }
}
```

Then just ask: *"Seed my database with 100 users and 500 orders, seed 42."*

The server exposes five tools: introspect, generate, reset, list_scenarios, validate.

## Performance

Measured on an M1 MacBook with `cargo bench`. These are mean times over 100 samples.

| Operation | Mean |
|---|---:|
| Introspect (3 tables, 3 FKs, live PG 17) | 9.7 ms |
| Topological sort, 50 tables / 100 FKs | 15.7 µs |
| Generate 100 rows | 240 µs |
| Generate 1,000 rows | 2.4 ms |
| Generate 10,000 rows | 24.3 ms |

That's roughly 411k rows/sec for pure generation. End-to-end with inserts depends on your Postgres setup — locally, 60 rows across 3 tables with FK lookups and single-row inserts takes about 100 ms.

## Supported databases

PostgreSQL 12 through 17 is the only stable target right now. CI runs against all six versions. MySQL and SQLite are on the roadmap but not implemented yet.

## Status

Currently v0.1.0. PostgreSQL works end-to-end. Direct INSERT and SQL file output are stable. COPY protocol output, JSON output, and HTTP+SSE MCP transport are planned for v0.2. MySQL and SQLite adapters are further out.

## Contributing

PRs welcome — see [CONTRIBUTING.md](./CONTRIBUTING.md). Release notes live in [CHANGELOG.md](./CHANGELOG.md).

## License

MIT — see [LICENSE](./LICENSE).

## Acknowledgments

Built in the spirit of [Supabase Seed](https://github.com/supabase-community/seed), [schema-seed](https://github.com/AliNazar-111/schema-seed), [drizzle-seed](https://orm.drizzle.team/docs/seed-overview), and [Seedfast](https://seedfa.st/), with a focus on speed, zero config, and determinism.
