## What

<!-- Brief description of the change. One or two sentences. -->

## Why

<!--
What problem does this solve? Link to the issue it closes (e.g. `Closes #123`)
or paste the bug report / feature request that prompted the change.
-->

## How

<!--
Key implementation decisions. Highlight anything non-obvious:
- New types or modules and why they exist
- Trade-offs you considered (and rejected) and why
- Anything reviewers should look at first
-->

## Testing

<!--
How did you verify this works?
- Unit tests added (link to them)
- Integration tests added (link to them)
- Manual test commands run (paste the output)
- Property/snapshot tests updated
-->

## Checklist

- [ ] Tests added or updated for the change
- [ ] Documentation updated (if user-facing — README, CLI.md, MCP.md, scenarios/)
- [ ] `cargo fmt && cargo clippy -- -D warnings` passes locally
- [ ] Snapshot tests reviewed (if generator output changed) — `cargo insta review`
- [ ] No `.unwrap()` or `.expect()` in library code (`src/` outside `cli/` and `main.rs`)
- [ ] No `rand::thread_rng()` or `SystemTime::now()` for randomness — only seeded `ChaCha8Rng`
