# AGENTS.md — obolargus-core

Operational rules for contributors and AI agents working in this crate.

## Conventions (from `.specify/memory/constitution.md`)

- No `unsafe`, no `unwrap`, no `panic` in production code — use `Result`.
- Money MUST use `rust_decimal`; never `f32`/`f64`.
- Coverage must stay 90%+ (verified with `cargo-llvm-cov`).
- Every public item needs a rustdoc comment; documentation is code-driven.
- `rustfmt` and `clippy --all-targets -- -D warnings` must stay clean.

## Commands

- Tests: `cargo test --all-targets --no-fail-fast`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Coverage: `cargo llvm-cov --all-targets --no-fail-fast`
- Format check: `cargo fmt -- --check`

## Notes

- Keep this crate dependency-light: it is depended on by server and cli.
- Shared behaviors must respect `specs/001-boilerplate-submodules/contracts/`.
- The system data dictionary lives in `src/data_dictionary/`. Its source of
  truth is `domain::model_description()` plus `temporal::catalog::fact_catalog()`;
  never add a parallel catalog. After a deliberate model change, run
  `scripts/schema.sh --update` and commit
  `schema/system-data-dictionary.json`. Promote the baseline only after a
  deliberate `x-obolargus-schema-version` bump.
