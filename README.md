# obolargus-core

Shared domain primitives for the Obolargus platform. A git submodule of the
parent Obolargus repository; see `specs/architecture/` for the system design and
`specs/001-boilerplate-submodules/` for the governing feature.

## Contents

- `decimal.rs` — `parse_decimal`: exact money parsing via `rust_decimal`
  (never floating point for money, constitution Principle V).
- `lib.rs` — crate documentation and the `VERSION` constant.

## Development

- Tests: `cargo test --all-targets`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Coverage: `cargo llvm-cov --all-targets` (threshold 90%+)
- Docs: `cargo doc --no-deps`

From the parent repo: `make test|lint|test-coverage PROJ=obolargus-core`.