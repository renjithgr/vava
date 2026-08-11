# Integration tests

This workspace is *virtual* (the root `Cargo.toml` has no package of its own),
so there is no root package to host a top-level `tests/` directory. Integration
tests therefore live next to the crate they exercise:

```text
crates/vava-core/tests/     fake-model agent loop tests, transcript checks
crates/vava-coding/tests/   session JSONL replay, tool boundary tests
crates/vava-deepseek/tests/ recorded SSE stream fixtures
```

Unit tests live beside the code they test, inside each module's
`#[cfg(test)] mod tests`.
