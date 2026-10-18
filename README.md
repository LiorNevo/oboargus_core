# obolargus-core

Shared domain primitives for the Obolargus platform. A git submodule of the
parent Obolargus repository; see `specs/architecture/` for the system design and
`specs/001-boilerplate-submodules/` for the governing feature.

## Contents

- `decimal.rs` — `parse_decimal`: exact money parsing via `rust_decimal`
  (never floating point for money, constitution Principle V).
- `lib.rs` — crate documentation and the `VERSION` constant.
- `src/domain/` — the high-level financial domain model (see below).

## Domain module

`obolargus_core::domain` implements the high-level domain model from
`specs/015-database-uml-domains/spec.md` (schema + typed Rust API per
`specs/016-implement-domain-model/spec.md`): `Portfolio` (a DAG — any level
may carry accounting), `Issuer` (its own parent/child DAG of legal
entities), `Asset`, `Position` (derived from events), `Event` (immutable,
append-only), the three dual-temporal reference-data entities, and the
external-identifier entities `AssetIdentifier` / `IssuerIdentifier` (ISIN,
ticker, CUSIP, Bloomberg ID, LEI, ... — indexed `(scheme, value)` keys with
an open scheme vocabulary). The module is the only sanctioned way to read
and write the model; `domain::model_description()` returns the
machine-readable description of every entity, field, aggregate,
relationship, and invariant.

## Development

- Tests: `cargo test --all-targets`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Coverage: `cargo llvm-cov --all-targets` (threshold 90%+)
- Docs: `cargo doc --no-deps`

From the parent repo: `make test|lint|test-coverage PROJ=obolargus-core`.
