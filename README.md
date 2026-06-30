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

# Lifecycle simulation — data that grows, churns, and trends over time
seedgen generate --url $DATABASE_URL -f scenarios/lifecycle-ecommerce.yaml --seed 42

# Preview the per-bucket plan without touching the database
seedgen generate -f scenarios/lifecycle-ecommerce.yaml --seed 42 --dry-run
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

## Lifecycle simulation

Normal generation creates every row as if it appeared at once. **Lifecycle mode**
instead makes data evolve over a time window — adoption curves, churn, seasonal
spikes, and child rows that follow their parents in time. It's opt-in: add a
`lifecycle:` block and SeedGen runs bucket by bucket (day/week/month/quarter),
generating new rows, churning old ones, and timestamping everything coherently.
Without the block, behavior is exactly as before.

```yaml
# lifecycle-ecommerce.yaml
seed: 42

lifecycle:
  start: 2023-01-01
  end: 2026-06-01
  bucket: month

tables:
  users:
    growth:                       # linear | exponential | s_curve | logistic | custom
      model: s_curve
      initial: 10
      capacity: 5000
      rate: 0.15
    churn:
      rate: 0.03                  # 3% of active users leave each month
      grace_period: 2             # ...but not before they're 2 months old
      column: is_active
      value: false
    overrides:
      plan:
        timeline:                 # distribution interpolated between keyframes
          2023-01-01: { pro: 80%, free: 20% }
          2025-06-01: { pro: 25%, free: 55%, enterprise: 20% }

  orders:
    growth:
      follows: users              # ~3.2 orders per active user per month
      ratio: 3.2
      variance: 0.35
    seasonality:
      monthly: [1.0, 0.7, 0.85, 1.0, 1.1, 0.8, 0.7, 0.85, 1.2, 1.4, 1.8, 2.5]
    temporal:
      created_at:
        after: users.created_at   # always after the user signed up
        offset: 1d..60d
```

```bash
seedgen generate --url $DATABASE_URL -f lifecycle-ecommerce.yaml --seed 42
seedgen generate -f lifecycle-ecommerce.yaml --seed 42 --dry-run   # plan only, no DB
```

The result has a real story: `SELECT date_trunc('month', created_at), count(*) FROM users GROUP BY 1`
shows an adoption curve, December outsells July, churned users stop ordering, and
no child row is ever timestamped before its parent. Same seed still gives the
same data.

## Production statistics profiling

Faker data has the wrong *shape*: every status equally likely, prices uniformly
random, no nulls where production has 40%. **Profiling** fixes that without ever
copying a row. `seedgen profile` reads only aggregate statistics from a
production (or staging) database — counts, distributions, numeric ranges, null
rates, FK ratios — and writes them to a YAML profile. Then `generate --profile`
produces seed data shaped like production but fully synthetic and deterministic.

```bash
# Read statistics from a read-only replica (zero row-level data leaves the DB)
seedgen profile --url postgres://readonly@prod-replica/myapp --output prod-profile.yaml

# Generate 1% of production size, deterministically, shaped like production
seedgen generate --url $DEV_URL --profile prod-profile.yaml --scale 0.01 --seed 42
```

Nothing but aggregate numbers ever leaves the source database, enforced by a
six-layer security model:

- **Query whitelist** — only `COUNT`/`MIN`/`MAX`/`AVG`/`STDDEV`/`PERCENTILE`/low-cardinality `GROUP BY`; never `SELECT *`, never row-level reads, never writes.
- **Cardinality guard** — value distributions are captured only for low-cardinality (enum-like) columns; a 48k-distinct `email` column yields a count, never addresses.
- **Sensitive detection** — `password`, `ssn`, `credit_card`, `token`, … are auto-skipped (overridable with `--include`).
- **Dry-run & audit** — `--dry-run-queries` prints every query first; each run writes `.seedgen-profile-audit.log`.
- **Offline mode** — for fully air-gapped production: export one read-only query, have a DBA run it, import the result. SeedGen never connects to prod.
- **Connection safety** — read-only transaction + statement timeout; warns on superuser.

```bash
# Review the exact queries before running anything
seedgen profile --url $PROD_URL --dry-run-queries

# Offline / air-gapped: export → DBA runs it → import (SeedGen never touches prod)
seedgen profile --url $PROD_URL --export-queries > collect.sql
psql "$PROD_URL" -At -f collect.sql -o results.json
seedgen profile --import-results results.json --output prod-profile.yaml
```

After profile-based generation, SeedGen prints a compliance report comparing the
generated data back to the profile (distributions, ratios, null rates) with a
`✓`/`✗` per check. The `--scale` factor preserves proportions: shrink the row
count and every table ratio stays intact.

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

For reproducible CI, pin both the action ref and the binary: `uses: ff4f/seedgen@v1` with `version: v0.2.0`.

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

Currently v0.3.0. PostgreSQL works end-to-end; production statistics profiling landed in this release.

| Capability | Status |
|---|:--:|
| PostgreSQL introspection + generation | ✅ |
| Determinism / FK / NOT NULL / UNIQUE invariants | ✅ |
| Direct INSERT + SQL file output | ✅ |
| Scenario files (counts, distributions, templates) | ✅ |
| Lifecycle simulation (growth, churn, seasonality, temporal) | ✅ |
| **Production statistics profiling (`profile`, scale, offline, compliance)** | ✅ |
| MCP server (stdio) | ✅ |
| COPY protocol output (`--fast`) | ⬜ |
| JSON output | ⬜ |
| HTTP+SSE MCP transport | ⬜ |
| MySQL / SQLite adapters | ⬜ |

COPY protocol output, JSON output, and HTTP+SSE MCP transport are planned next. MySQL and SQLite adapters are further out.

## Contributing

PRs welcome — see [CONTRIBUTING.md](./CONTRIBUTING.md). Release notes live in [CHANGELOG.md](./CHANGELOG.md).

## License

MIT — see [LICENSE](./LICENSE).

## Acknowledgments

Built in the spirit of [Supabase Seed](https://github.com/supabase-community/seed), [schema-seed](https://github.com/AliNazar-111/schema-seed), [drizzle-seed](https://orm.drizzle.team/docs/seed-overview), and [Seedfast](https://seedfa.st/), with a focus on speed, zero config, and determinism.
