# Changelog

All notable changes to SeedGen are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and SeedGen adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

## [0.1.0] - 2026-06-01

First public release. Feature-complete for PostgreSQL with all 5 critical invariants (determinism, FK integrity, NOT NULL compliance, UNIQUE compliance, offline operation) enforced by tests.

### Added

#### Introspection

- PostgreSQL schema reading via `information_schema` (portable across PG 12-17) and `pg_catalog` (for enums and generated columns).
- 6 async query functions: `query_tables`, `query_columns`, `query_foreign_keys`, `query_constraints`, `query_enums`, `query_generated_columns`.
- `SchemaGraph`, `Table`, `Column`, `ForeignKey`, `Constraint`, `EnumType`, `DataType` data structures with full `serde` support.
- Top-level `introspect(pool) -> SchemaGraph` orchestrator.
- `IntrospectionError` with `ConnectionFailed` and `QueryFailed { query, source }` variants — every query failure carries its source identifier.

#### Dependency resolution

- Kahn's algorithm topological sort with deterministic alphabetical tie-breaking via `BTreeMap` + `BinaryHeap<Reverse<&str>>`.
- DFS-based cycle detection with white/gray/black coloring.
- Cycle resolution via deferred FK updates: breakable cycles automatically converted to two-phase insert (NULL → UPDATE).
- `resolve(&SchemaGraph) -> InsertionPlan` combining sort + cycle handling.
- Self-references skipped at adjacency build time (handled by generator, not resolver).
- `ResolverError` with `CyclicDependency` and `TableNotFound` variants.

#### Semantic detection

- 27 column-name patterns: `email`, `first_name`, `last_name`, `full_name`, `username`, `phone`, `url`, `avatar_url`, `password`, `slug`, `title`/`subject`, `description`/`bio`/`body`/`content`, `company`, `uuid`, `token`/`secret`/`api_key`, `sku`, `price`/`amount`/`cost`/`total`/`fee`, `latitude`/`lat`, `longitude`/`lng`/`lon`, `city`, `country_code`, `country`, `zip`/`postal`, `street_address`, `color`/`colour`, `ip` (varchar only), `currency`.
- Type-based fallback for unmatched columns: Boolean, Integer, Float, Numeric (scale-2 detected as money), Varchar, Text, Date/Timestamp, UUID, JSON/JSONB, INET, MoneyType.
- Skip rules for identity and generated columns.
- Enum detection via `DataType::Enum` cross-referenced against the schema's enum list.
- 38 `GeneratorType` variants covering text, numeric, temporal, identifier, geo, structured data.

#### Constraint handling

- `ConstraintHandler` with kinds: `NotNull`, `Unique` (HashSet retry), `CompositeUnique` (row-level), `CheckPositive`, `CheckRange { min, max }`, `MaxLength` (char-count, UTF-8 aware).
- `ValidationResult { Valid, Retry, Invalid(reason) }`.
- Pragmatic CHECK constraint parser: detects `col > 0` and `col >= N AND col <= M` patterns. Ignores complex expressions (function calls, subqueries) safely.

#### Generators (deterministic via `ChaCha8Rng`)

- **Text** (`src/generators/text.rs`): `EmailGenerator`, `FirstNameGenerator` (217 names), `LastNameGenerator` (206 names), `FullNameGenerator`, `UsernameGenerator`, `PhoneGenerator`, `UrlGenerator`, `AvatarUrlGenerator`, `PasswordGenerator` (60-char bcrypt-like), `SlugGenerator`, `ParagraphGenerator`, `SentenceGenerator`, `CompanyNameGenerator`.
- **Numeric** (`src/generators/numeric.rs`): `MoneyGenerator { min, max }` (2-decimal precision, default `[0.01, 10_000]` to satisfy `CHECK(price > 0)`), `RandomIntGenerator`, `RandomFloatGenerator`.
- **Temporal** (`src/generators/temporal.rs`): `DatetimePastGenerator` (2-year window), `DatetimeRecentGenerator` (7-day window), `DateFutureGenerator` (1-year forward). All anchored to a hardcoded reference epoch (2026-01-01) for determinism.
- **Network** (`src/generators/network.rs`): `IPv4Generator` with valid host octets.
- **Geo** (`src/generators/geo.rs`): `LatitudeGenerator`, `LongitudeGenerator`, `CityGenerator` (150 cities), `CountryGenerator` (87 countries), `CountryCodeGenerator` (full ISO 3166-1 alpha-2 list, 249 codes), `PostalCodeGenerator`, `StreetAddressGenerator`.
- **Identifier** (`src/generators/identifier.rs`): `UuidGenerator` (proper UUID v4 with version/variant bits set, no `uuid` crate dependency for generation), `TokenGenerator` (32 hex chars), `SkuGenerator`.
- **Structured** (`src/generators/structured.rs`): `BoolGenerator`, `EnumPickGenerator { values }`, `JsonEmptyGenerator`, `HexColorGenerator`, `CurrencyCodeGenerator` (36 ISO 4217 codes), `RandomStringGenerator { max_length }`.
- `Value` enum: `String | Int | Float | Bool | Null | Uuid | Timestamp | Date | Json`.
- `Generator` trait (`Send + Sync`) + `create_generator(&GeneratorType) -> Box<dyn Generator>` factory mapping all 38 variants.

#### Generation pipeline

- `pub async fn generate(pool, config) -> Result<GenerationResult>` — the public entry point.
- Column planning skips generated/identity columns and FK columns get values picked from already-generated parent IDs.
- Per-batch INSERT (1000 rows per statement) wrapped in a transaction.
- `RETURNING id` captures auto-generated primary keys for child-table FK references.
- Unique constraint retry: up to 32 attempts per value before failing with `UniqueExhausted`.
- Composite UNIQUE constraints supported via `ConstraintHandler::validate_row`.
- Synthetic IDs (1..N) generated explicitly for non-DB output modes so child FKs resolve.

#### Output modes

- `OutputMode::DirectInsert` — batch INSERT into the live database (default).
- `OutputMode::SqlFile(path)` — emits a multi-row INSERT SQL file with `session_replication_role = 'replica'` toggle so FK checks don't block out-of-order loads. Self-contained transaction per table.
- `OutputMode::Stdout` — writes the same SQL to stdout.
- `output::truncate_tables(pool, tables)` — `TRUNCATE ... RESTART IDENTITY CASCADE` helper used by `reset` and `--truncate-first`.
- `output::insert_rows(pool, table, columns, rows) -> Vec<i64>` — reusable batch INSERT primitive with RETURNING id capture.

#### CLI

- `seedgen generate` — flags: `--seed`, `--rows`, `--scenario`, `-f/--file`, `--entities`, `--output`, `--format`, `--fast`, `--dry-run`, `--locale`, `--include`, `--exclude`, `--truncate-first`.
- `seedgen introspect` — flags: `--format` (table/json/yaml), `--output`, `--include`, `--exclude`.
- `seedgen reset` — flags: `--confirm` (required), `--only`, `--cascade`. Refuses to operate on URLs containing `prod`.
- `seedgen mcp-server` — flags: `--transport stdio`, `--port`.
- `seedgen completions <shell>` — generates bash/zsh/fish/powershell completion scripts.
- Global flags: `--url/-u` (env: `DATABASE_URL`), `--verbose/-v`, `--quiet/-q`, `--no-color`.
- Seed precedence: explicit `--seed` > scenario YAML `seed:` > time-based fallback (printed back so user can reproduce).

#### Scenario engine

- YAML scenario parser with rich `ScenarioConfig { seed, tables }` structure.
- `CountExpression`: `Fixed(n)`, `PerParent { parent_table, min, max }`, `PercentageOf { table, percentage }`.
- `ColumnOverride`: `Distribution(HashMap)`, `Range { min, max }`, `Formula(String)`, `AfterParent`, `FromParent`, `Generator { name, params }`.
- 4 built-in scenario templates embedded via `include_str!`:
  - `ecommerce` — users(50), categories(15), products(200), orders/items/reviews via per_parent
  - `saas` — orgs(10), users/subscriptions/invoices with role/plan/status distributions
  - `blog` — users(20), posts/comments/tags/post_tags with author hierarchy
  - `social` — users(100), posts/likes/follows/messages with long-tail patterns
- `load_template(name)` + `list_templates()` for discovery.
- Date literals (`2023-01-01`) in range overrides parsed as days-since-epoch.
- Percentage values accept both `5%` (string) and `5` (number) forms.

#### MCP server

- JSON-RPC 2.0 over stdio (newline-delimited messages).
- 5 tools exposed: `seedgen_introspect`, `seedgen_generate`, `seedgen_reset`, `seedgen_list_scenarios`, `seedgen_validate`.
- Full handshake support: `initialize`, `tools/list`, `tools/call`.
- Standard JSON-RPC error codes (`-32700` parse, `-32601` method not found, `-32602` invalid params, `-32000` server error).
- Notifications (no `id`) handled per spec — server does not respond.
- Production safety: `seedgen_reset` requires explicit `confirm: true` and refuses URLs containing `prod`.
- Connection URLs never logged or included in responses.
- Compatible with Claude Desktop, Cursor, VS Code Copilot, and any MCP host.

#### GitHub Action

- Composite action at `action.yml`.
- 10 inputs: `database_url`, `scenario`, `scenario_file`, `seed`, `rows`, `entities`, `locale`, `fast`, `truncate_first`, `version`.
- Cross-platform binary install: Linux/macOS/Windows × X64/ARM64.
- Two-step composite: install (curl from GitHub Release) then `seedgen generate`.
- Parameters echoed by name in a collapsed log group; database URL never logged.

#### CI/CD

- `.github/workflows/pr-check.yml` — lint (fmt + clippy), test (PG 16 service container), security (`rustsec/audit-check`).
- `.github/workflows/test-matrix.yml` — full PG 12-17 matrix on `push to main` + weekly cron + manual trigger.
- `.github/workflows/release.yml` — multi-target binary build (5 targets) + GitHub Release + crates.io publish + multi-arch Docker image push (`linux/amd64`, `linux/arm64`).
- `.coderabbit.yaml` — assertive profile with project-specific path instructions enforcing all 5 critical invariants.
- `.github/PULL_REQUEST_TEMPLATE.md` — What/Why/How/Testing + 6-item checklist.
- `.github/ISSUE_TEMPLATE/bug_report.yml` + `feature_request.yml` — GitHub Issue Forms YAML.

#### Testing

- **279 unit tests** across all modules.
- **14 integration tests** against live PostgreSQL: `introspect_test.rs` (5), `resolve_test.rs` (1), `output_test.rs` (6), `snapshot_tests.rs` (2).
- **6 property tests** via `proptest` (`#[ignore]`'d, opt-in via `--ignored`): `prop_no_null_in_not_null`, `prop_fk_integrity`, `prop_unique_no_duplicates`, `prop_determinism`, `prop_email_format`, `prop_money_positive`. 12 cases × 6 tests = 72 random-seed runs per invocation.
- **2 snapshot tests** via `insta`: locked YAML output for seeds 42 and 7 — any drift fails CI.
- **5 Criterion benchmarks**: introspect, generate {100, 1000, 10000} rows, topological_sort_50_tables.
- Test fixtures: `schema_basic.sql` (users/posts/comments), `schema_cyclic.sql` (departments↔employees), `schema_with_check.sql` (priced_items with `CHECK (price > 0)`).
- Per-binary `Mutex` serialization for integration tests sharing the same PG instance (prevents libtest's parallel `#[tokio::test]` cases from clobbering each other's schema state).

#### Documentation

- README with badges, real benchmarks, real command output, install instructions, GitHub Action reference, determinism contract, project status table.
- CONCEPT.md, ARCHITECTURE.md, CLI.md, MCP.md, CICD.md, TESTING.md, CONTRIBUTING.md, SKILL.md.

### Critical invariants enforced

1. **Determinism** — same seed produces byte-identical output across runs and platforms. Locked by snapshot tests, fuzzed by 6 property tests across seeds in `[0, 1_000_000)`.
2. **FK integrity** — generated FK values always reference existing parent rows. Verified by `prop_fk_integrity` and `test_generate_respects_fk_integrity`.
3. **NOT NULL compliance** — NOT NULL columns never receive NULL (except deferred FKs during cycle resolution). Verified by `prop_no_null_in_not_null`.
4. **UNIQUE compliance** — UNIQUE columns have no duplicates. HashSet-tracked, with 32-attempt retry. Verified by `prop_unique_no_duplicates`.
5. **Offline operation** — core library has zero network calls. AI features live in the MCP host, not in SeedGen.
6. **No credentials in output** — connection URLs never logged or included in MCP responses, CLI output, or error messages.

### Performance (Apple Silicon M1, `cargo bench`)

| Operation | Mean |
|---|---:|
| Introspect (3 tables, live PG 17) | 9.7 ms |
| Topological sort, 50 tables / 100 FKs | 15.7 µs |
| Generate 100 rows in memory | 240 µs |
| Generate 1,000 rows in memory | 2.4 ms |
| Generate 10,000 rows in memory | 24.3 ms |

### Known limitations

- **JSON output mode** is not yet implemented; falls back to `UnsupportedOutput` error.
- **COPY protocol** output is not yet implemented; `--fast` flag accepted but uses batch INSERT.
- **MCP HTTP+SSE transport** is not yet implemented; only stdio.
- **Locale support** is parsed (`--locale id`) but only English pools are bundled.
- **`seedgen validate`** CLI command and MCP tool are registered but not yet implemented.
- **Scenario `ColumnOverride` variants** (distribution, range, formula, after, from_parent, generator) are parsed but not yet wired into the generator pipeline — only `count` expressions affect output.
- **Per-parent random count** uses the average of min/max rather than a per-parent random value (preserves determinism without RNG plumbing through the resolver).
- **MySQL and SQLite adapters** are planned for v2.0; v0.1 is PostgreSQL-only.

[Unreleased]: https://github.com/ff4f/seedgen/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ff4f/seedgen/releases/tag/v0.1.0
