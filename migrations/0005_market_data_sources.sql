-- Migration 0005: market data sources — three new tables.
--
-- Implements FR-002 / FR-007 / FR-022 of `specs/017-yahoo-data-source/spec.md`.
-- Adds `data_sources` (configuration persistence), `data_source_refresh_runs`
-- (append-only audit for refresh runs), and `data_source_reconciliation_audits`
-- (append-only audit for cross-source reconciliation). Idempotent.

-- =============================================================================
-- data_sources: one row per configured source.
-- =============================================================================
CREATE TABLE IF NOT EXISTS data_sources (
    id                     UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id              TEXT         NOT NULL UNIQUE,
    display_name           TEXT         NOT NULL,
    provider_type          TEXT         NOT NULL,
    enabled                BOOLEAN      NOT NULL DEFAULT TRUE,
    priority               INTEGER      NOT NULL,
    supported_schemes      TEXT[]       NOT NULL,
    rate_limit_per_minute  INTEGER      NULL,
    api_key_present        BOOLEAN      NOT NULL DEFAULT FALSE,
    user_agent             TEXT         NULL,
    timeout_seconds        INTEGER      NULL,
    created_at             TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at             TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    CHECK (priority > 0),
    CHECK (api_key_present = FALSE OR api_key_present = TRUE)
);

CREATE INDEX IF NOT EXISTS data_sources_enabled_priority_idx
    ON data_sources (enabled, priority);

-- =============================================================================
-- data_source_refresh_runs: append-only audit for refresh runs.
-- =============================================================================
CREATE TABLE IF NOT EXISTS data_source_refresh_runs (
    id                          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    started_at                  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    ended_at                    TIMESTAMPTZ  NULL,
    trigger                     TEXT         NOT NULL,
    assets_attempted            INTEGER      NOT NULL DEFAULT 0,
    assets_refreshed            INTEGER      NOT NULL DEFAULT 0,
    assets_no_op                INTEGER      NOT NULL DEFAULT 0,
    assets_missing_identifier   INTEGER      NOT NULL DEFAULT 0,
    assets_source_unavailable   INTEGER      NOT NULL DEFAULT 0,
    per_source_attempted        JSONB        NOT NULL DEFAULT '{}'::jsonb,
    per_source_failed           JSONB        NOT NULL DEFAULT '{}'::jsonb,
    duration_ms                 BIGINT       NULL,
    CHECK (started_at <= COALESCE(ended_at, NOW())),
    CHECK (assets_attempted >= 0),
    CHECK (assets_attempted =
        assets_refreshed + assets_no_op + assets_missing_identifier + assets_source_unavailable),
    CHECK (assets_refreshed >= 0),
    CHECK (assets_no_op >= 0),
    CHECK (assets_missing_identifier >= 0),
    CHECK (assets_source_unavailable >= 0)
);

CREATE INDEX IF NOT EXISTS data_source_refresh_runs_started_at_idx
    ON data_source_refresh_runs (started_at DESC);

-- Append-only enforcement: the application role is granted INSERT + SELECT,
-- never UPDATE or DELETE. (Granted at the role-grant level by the test
-- harness; SQL itself has no GRANT/REVOKE statements here.)

-- =============================================================================
-- data_source_reconciliation_audits: append-only audit for reconciliations.
-- =============================================================================
CREATE TABLE IF NOT EXISTS data_source_reconciliation_audits (
    id                       UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    asset_id                 UUID         NOT NULL REFERENCES assets(id),
    fact_type                TEXT         NOT NULL,
    as_of_date               DATE         NOT NULL,
    policy                   TEXT         NOT NULL,
    sources_tried            TEXT[]       NOT NULL,
    per_source_answers       JSONB        NOT NULL,
    canonical_value          TEXT         NOT NULL,
    decided_at               TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    CHECK (cardinality(sources_tried) >= 2),
    CHECK (policy IN ('primary_wins', 'first_non_empty', 'majority')),
    CHECK (fact_type <> '')
);

CREATE INDEX IF NOT EXISTS data_source_reconciliation_audits_asset_fact_date_idx
    ON data_source_reconciliation_audits (asset_id, fact_type, as_of_date DESC);
