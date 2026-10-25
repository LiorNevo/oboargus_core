-- Migration 0007: reference data load runs — one new table.
--
-- Implements FR-008 of `specs/018-load-reference-data/spec.md`.
-- Adds `reference_data_load_runs`, the append-only audit record for a
-- batch reference-data load run (one row per run). Idempotent.
--
-- Note: the migration number follows `0006_portfolio_rules.sql`, which
-- already exists in this crate.

-- =============================================================================
-- reference_data_load_runs: append-only audit for load runs.
-- =============================================================================
CREATE TABLE IF NOT EXISTS reference_data_load_runs (
    id                        UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    started_at                TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    ended_at                  TIMESTAMPTZ  NULL,
    trigger                   TEXT         NOT NULL,
    -- The caller's named preferred source id (FR-013). NULL when the caller
    -- named none. Never rewritten on fallover: the answering source is
    -- recorded per identifier line, this column records the request.
    requested_source          TEXT         NULL,
    identifiers_requested_raw INTEGER      NOT NULL,
    identifiers_duplicated    INTEGER      NOT NULL DEFAULT 0,
    identifiers_attempted     INTEGER      NOT NULL,
    loaded                    INTEGER      NOT NULL DEFAULT 0,
    no_op                     INTEGER      NOT NULL DEFAULT 0,
    not_resolved              INTEGER      NOT NULL DEFAULT 0,
    no_data_for_date          INTEGER      NOT NULL DEFAULT 0,
    unsupported_scheme        INTEGER      NOT NULL DEFAULT 0,
    rejected                  INTEGER      NOT NULL DEFAULT 0,
    source_unavailable        INTEGER      NOT NULL DEFAULT 0,
    rate_limited              INTEGER      NOT NULL DEFAULT 0,
    storage_failure           INTEGER      NOT NULL DEFAULT 0,
    per_source_attempted      JSONB        NOT NULL DEFAULT '{}'::jsonb,
    per_source_failed         JSONB        NOT NULL DEFAULT '{}'::jsonb,
    per_identifier            JSONB        NOT NULL DEFAULT '[]'::jsonb,
    duration_ms               BIGINT       NULL,

    -- The fetch-attempted partition (research D13: "attempted" means the
    -- data-source registry was consulted for that identifier). The
    -- `unsupported_scheme` bucket belongs to the attempted side — the
    -- registry was consulted and answered "no candidate source".
    CHECK (identifiers_attempted = loaded + no_op + no_data_for_date
           + rejected + source_unavailable + rate_limited + storage_failure
           + unsupported_scheme),

    -- Every requested identifier is either attempted or not resolved, after
    -- de-duplication. `identifiers_requested_raw - identifiers_duplicated`
    -- is the unique identifier count.
    CHECK (not_resolved + identifiers_attempted
           = identifiers_requested_raw - identifiers_duplicated),

    CHECK (started_at <= COALESCE(ended_at, NOW())),
    CHECK (identifiers_requested_raw >= 0),
    CHECK (identifiers_duplicated >= 0),
    CHECK (identifiers_attempted >= 0),
    CHECK (loaded >= 0),
    CHECK (no_op >= 0),
    CHECK (not_resolved >= 0),
    CHECK (no_data_for_date >= 0),
    CHECK (unsupported_scheme >= 0),
    CHECK (rejected >= 0),
    CHECK (source_unavailable >= 0),
    CHECK (rate_limited >= 0),
    CHECK (storage_failure >= 0)
);

CREATE INDEX IF NOT EXISTS idx_reference_data_load_runs_started
    ON reference_data_load_runs (started_at DESC);

-- Append-only enforcement: the application role is granted INSERT + SELECT,
-- never UPDATE or DELETE. (Granted at the role-grant level by the test
-- harness; SQL itself has no GRANT/REVOKE statements here.)
