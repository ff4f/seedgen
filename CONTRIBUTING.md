# Contributing to SeedGen

Thank you for considering contributing to SeedGen! This guide covers everything you need to get started.

---

## Quick Start

```bash
# 1. Fork and clone
git clone https://github.com/ff4f/seedgen.git
cd seedgen

# 2. Install Rust (if not already)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 3. Start PostgreSQL (for integration tests)
docker compose up -d

# 4. Run tests
cargo test                          # Unit tests only
cargo test --features integration   # With database tests
DATABASE_URL=postgres://postgres:test@localhost:5432/seedgen_test

# 5. Run the CLI
cargo run -- introspect --url postgres://postgres:test@localhost:5432/seedgen_test
cargo run -- generate --url postgres://postgres:test@localhost:5432/seedgen_test --seed 42
```

---

## Development Environment

### Prerequisites

- Rust 1.75+ (install via [rustup](https://rustup.rs/))
- Docker (for running PostgreSQL locally)
- PostgreSQL client (`psql`) for manual testing

### Setup

```bash
# Start test databases (PG 12, 15, 17)
docker compose up -d

# Load test fixtures
psql postgres://postgres:test@localhost:5432/seedgen_test \
  -f tests/fixtures/schema_basic.sql

# Verify setup
cargo run -- introspect --url postgres://postgres:test@localhost:5432/seedgen_test
```

### Useful Commands

```bash
# Format code
cargo fmt

# Lint
cargo clippy -- -D warnings

# Run specific test
cargo test test_topological_sort

# Run integration tests only
cargo test --features integration -- integration

# Run benchmarks
cargo bench

# Check coverage
cargo tarpaulin --out html
open tarpaulin-report.html

# Review snapshot changes
cargo insta review
```

---

## Project Structure

```
src/
├── cli/            # CLI commands and argument parsing
├── introspection/  # Database schema reading
├── resolver/       # Dependency graph + topological sort
├── semantic/       # Column name → generator mapping
├── generators/     # Fake data generators
├── scenario/       # YAML scenario parsing + execution
├── output/         # SQL/JSON/COPY output formatting
├── mcp/            # MCP server implementation
└── config/         # Configuration management
```

When adding a feature, identify which module it belongs to. If it doesn't fit existing modules, discuss in the issue first.

---

## How to Contribute

### Reporting Bugs

Open an issue with:

1. **What you expected** to happen
2. **What actually happened** (include error message)
3. **Steps to reproduce** (minimal SQL schema + command)
4. **Environment:** OS, PostgreSQL version, SeedGen version

### Suggesting Features

Open an issue with:

1. **Problem:** What pain point does this solve?
2. **Proposal:** How should it work?
3. **Alternatives:** What other solutions did you consider?

### Submitting Code (Fork-based Workflow)

All code contributions go through Pull Requests. No direct pushes to `main`.

**Steps:**

1. **Fork** this repo to your GitHub account
2. **Clone** your fork:
   ```bash
   git clone https://github.com/ff4f/seedgen.git
   cd seedgen
   ```
3. **Create a new branch** from `main`:
   ```bash
   git checkout -b feat/my-feature
   # or: git checkout -b fix/my-bug
   ```
4. **Develop** — write code + tests
5. **Make sure all checks pass:**
   ```bash
   cargo fmt                        # Format code
   cargo clippy -- -D warnings      # Lint — zero warnings
   cargo test --all-features        # All tests pass
   ```
6. **Commit** using conventional commits (see section below)
7. **Push** to your fork:
   ```bash
   git push origin feat/my-feature
   ```
8. **Open a Pull Request** against `main` in the upstream repo
9. **Wait for automated review:**
   - GitHub Actions CI will run automatically (test, lint, security audit)
   - **CodeRabbit AI** will review your code within 2-5 minutes and leave feedback
   - Address feedback from CI and CodeRabbit if any
10. **Maintainer review** — a maintainer will review manually and approve or request changes
11. **Merge** — once approved, the PR will be squash-merged into `main`

### About CodeRabbit (AI Code Review)

This repo uses [CodeRabbit](https://coderabbit.ai) for automated AI code review. Every PR you open is reviewed automatically by CodeRabbit. You'll see:

- **PR Summary** — an automatic summary of your changes
- **Line comments** — per-line suggestions
- **Project-specific checks** — CodeRabbit verifies your code follows SeedGen rules (determinism, no unwrap, parameterized SQL, etc.)

You can **reply directly** to CodeRabbit comments to discuss or ask for clarification. CodeRabbit will respond.

**Important:** CodeRabbit is an assistant, not a replacement for maintainers. Final review is still done by a (human) maintainer.

---

## Contribution Areas

### Good First Issues

Look for issues tagged `good-first-issue`:

- Add a new semantic detection rule (e.g., detect `iban` → IBAN generator)
- Add locale data (names, cities for a new locale)
- Improve error messages
- Add documentation examples

### Medium Complexity

- Add a new built-in scenario template
- Implement a new generator type
- Add a new output format
- Improve CHECK constraint parsing

### Advanced

- MySQL adapter implementation
- COPY protocol output
- Cycle resolution improvements
- MCP transport improvements
- Performance optimizations

---

## Code Style

### Rust Style

- Follow standard Rust formatting (`cargo fmt`)
- Use `clippy` lints — no warnings allowed
- Prefer `thiserror` for error types
- Prefer `tracing` over `println!` for logging
- Write doc comments (`///`) for all public items

### Naming

- Modules: `snake_case`
- Types/Traits: `PascalCase`
- Functions/Variables: `snake_case`
- Constants: `SCREAMING_SNAKE_CASE`

### Error Handling

```rust
// Good: specific error types
#[derive(thiserror::Error, Debug)]
pub enum IntrospectionError {
    #[error("Failed to connect to database: {0}")]
    ConnectionFailed(#[from] sqlx::Error),

    #[error("Table '{0}' not found in schema")]
    TableNotFound(String),
}

// Bad: generic errors
fn introspect() -> Result<Schema, Box<dyn Error>> { ... }
```

### Testing

```rust
// Good: descriptive test names
#[test]
fn topological_sort_handles_diamond_dependency() { ... }

// Bad: vague names
#[test]
fn test_sort() { ... }
```

---

## Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add MySQL introspection adapter
fix: handle composite UNIQUE with nullable columns
docs: add scenario file examples
test: add property tests for FK integrity
perf: use COPY protocol for batch inserts
refactor: extract semantic rules into separate module
chore: update dependencies
ci: add PG 17 to test matrix
```

---

## Pull Request Process

1. PR title follows conventional commit format
2. PR description explains WHY, not just WHAT
3. All CI checks pass (lint, test, security audit)
4. CodeRabbit AI review feedback addressed (fix it or reply with a justification)
5. At least one approval from maintainer
6. No unresolved conversations
7. Squash merge into main

### PR Template

```markdown
## What

Brief description of the change.

## Why

What problem does this solve? Link to issue.

## How

Key implementation decisions and trade-offs.

## Testing

How was this tested? New tests added?

## Checklist

- [ ] Tests added/updated
- [ ] Documentation updated (if user-facing)
- [ ] `cargo fmt && cargo clippy -- -D warnings` passes
- [ ] Snapshot tests reviewed (if output changed)
```

---

## Release Process (Maintainers)

1. Ensure `main` is green
2. Update version in `Cargo.toml`
3. Update `CHANGELOG.md`
4. Commit: `release: v0.X.0`
5. Tag: `git tag v0.X.0`
6. Push: `git push origin main --tags`
7. GitHub Actions handles the rest (build, publish, Docker)

---

## Code of Conduct

We follow the [Contributor Covenant](https://www.contributor-covenant.org/version/2/1/code_of_conduct/). Be respectful, constructive, and inclusive.

---

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
