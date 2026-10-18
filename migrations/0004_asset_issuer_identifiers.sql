-- 0004_asset_issuer_identifiers.sql
-- External identifiers for assets and issuers.
--
-- Implements FR-022 of `specs/016-implement-domain-model/spec.md` and the
-- `AssetIdentifier` / `IssuerIdentifier` entities defined by the predecessor
-- (`specs/015-database-uml-domains/spec.md` FR-028..FR-034, added 2026-10-18).
--
-- Design notes (see the specs for the full rationale):
--   * `(scheme, value)` is UNIQUE within each table — a lookup resolves at
--     most one subject per domain (predecessor Invariant 9 / FR-032).
--   * The scheme vocabulary is OPEN (FR-026): no enum constraint on `scheme`.
--     Schemes are stored trimmed and uppercased by the application (FR-025);
--     the CHECK constraints below only reject empty keys.
--   * Identifiers are current-state keys, NOT dual-temporal facts: no
--     `as_of_date` / `knowledge_date` columns (predecessor FR-033).
--   * ON DELETE CASCADE: identifiers die with their subject (predecessor
--     Assumption 25 / FR-034). They are replaceable keys, not protected
--     immutable history like events or reference data.
--
-- The migration is idempotent (IF NOT EXISTS / CREATE INDEX IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS asset_identifiers (
    id          UUID PRIMARY KEY,
    asset_id    UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    scheme      TEXT NOT NULL,
    value       TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT asset_identifiers_scheme_non_empty CHECK (btrim(scheme) <> ''),
    CONSTRAINT asset_identifiers_value_non_empty CHECK (btrim(value) <> ''),
    CONSTRAINT asset_identifiers_unique_scheme_value UNIQUE (scheme, value)
);

CREATE INDEX IF NOT EXISTS idx_asset_identifiers_asset_id
    ON asset_identifiers(asset_id);

CREATE TABLE IF NOT EXISTS issuer_identifiers (
    id          UUID PRIMARY KEY,
    issuer_id   UUID NOT NULL REFERENCES issuers(id) ON DELETE CASCADE,
    scheme      TEXT NOT NULL,
    value       TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT issuer_identifiers_scheme_non_empty CHECK (btrim(scheme) <> ''),
    CONSTRAINT issuer_identifiers_value_non_empty CHECK (btrim(value) <> ''),
    CONSTRAINT issuer_identifiers_unique_scheme_value UNIQUE (scheme, value)
);

CREATE INDEX IF NOT EXISTS idx_issuer_identifiers_issuer_id
    ON issuer_identifiers(issuer_id);
