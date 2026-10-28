-- Migration 0010: Data Load page — domain, range grouping, source-state audit,
-- and the unified history read model.
--
-- Implements `specs/022-market-data-load-ui/spec.md` FR-026, FR-028..FR-035.
-- All changes are additive and idempotent. The range is reconstructed from
-- its per-date `reference_data_load_runs` rows; no range table is created
-- (018 FR-014).

-- =============================================================================
-- reference_data_load_runs: domain, as-of date, range grouping, answering set
-- =============================================================================

ALTER TABLE reference_data_load_runs
    ADD COLUMN IF NOT EXISTS data_domain TEXT NOT NULL DEFAULT 'market_data',
    ADD COLUMN IF NOT EXISTS as_of_date DATE NULL,
    ADD COLUMN IF NOT EXISTS range_group_id UUID NULL,
    ADD COLUMN IF NOT EXISTS range_from DATE NULL,
    ADD COLUMN IF NOT EXISTS range_to DATE NULL,
    ADD COLUMN IF NOT EXISTS range_concurrency INTEGER NULL,
    ADD COLUMN IF NOT EXISTS answering_sources TEXT[] NOT NULL DEFAULT '{}'::text[];

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'reference_data_load_runs_data_domain_non_empty'
    ) THEN
        ALTER TABLE reference_data_load_runs
            ADD CONSTRAINT reference_data_load_runs_data_domain_non_empty
            CHECK (data_domain <> '');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'reference_data_load_runs_range_concurrency_bounds'
    ) THEN
        ALTER TABLE reference_data_load_runs
            ADD CONSTRAINT reference_data_load_runs_range_concurrency_bounds
            CHECK (range_concurrency IS NULL OR range_concurrency BETWEEN 1 AND 32);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'reference_data_load_runs_range_order'
    ) THEN
        ALTER TABLE reference_data_load_runs
            ADD CONSTRAINT reference_data_load_runs_range_order
            CHECK (range_from IS NULL OR range_to IS NULL OR range_from <= range_to);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'reference_data_load_runs_range_group_metadata'
    ) THEN
        ALTER TABLE reference_data_load_runs
            ADD CONSTRAINT reference_data_load_runs_range_group_metadata
            CHECK (
                range_group_id IS NULL
                OR (range_from IS NOT NULL AND range_to IS NOT NULL)
            );
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_reference_data_load_runs_range_group
    ON reference_data_load_runs (range_group_id)
    WHERE range_group_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_reference_data_load_runs_domain_started
    ON reference_data_load_runs (data_domain, started_at DESC);

-- =============================================================================
-- data_sources: supported data domains
-- =============================================================================

ALTER TABLE data_sources
    ADD COLUMN IF NOT EXISTS supported_data_domains TEXT[] NOT NULL
        DEFAULT '{market_data}'::text[];

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'data_sources_supported_data_domains_non_empty'
    ) THEN
        ALTER TABLE data_sources
            ADD CONSTRAINT data_sources_supported_data_domains_non_empty
            CHECK (cardinality(supported_data_domains) > 0);
    END IF;
END $$;

-- =============================================================================
-- data_source_state_changes: append-only enable/disable audit
-- =============================================================================

CREATE TABLE IF NOT EXISTS data_source_state_changes (
    id              UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id       TEXT         NOT NULL,
    changed_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    before_enabled  BOOLEAN      NOT NULL,
    after_enabled   BOOLEAN      NOT NULL,
    actor           TEXT         NULL,
    reason          TEXT         NULL,

    CHECK (source_id <> ''),
    CHECK (before_enabled <> after_enabled)
);

CREATE INDEX IF NOT EXISTS idx_data_source_state_changes_changed
    ON data_source_state_changes (changed_at DESC);

CREATE INDEX IF NOT EXISTS idx_data_source_state_changes_source
    ON data_source_state_changes (source_id, changed_at DESC);

-- =============================================================================
-- Backfill the answering-source union for rows written before 0010
-- =============================================================================

UPDATE reference_data_load_runs r
SET answering_sources = COALESCE((
    SELECT array_agg(DISTINCT line->>'source_id')
    FROM jsonb_array_elements(r.per_identifier) AS line
    WHERE line->>'source_id' IS NOT NULL
), '{}'::text[])
WHERE r.answering_sources = '{}'::text[];

-- =============================================================================
-- data_load_history: one row per load run, range group, or source change
-- =============================================================================

CREATE OR REPLACE VIEW data_load_history AS
WITH run_rows AS (
    SELECT
        COALESCE(range_group_id, id) AS entry_id,
        CASE WHEN range_group_id IS NULL THEN 'load_run' ELSE 'range_run' END AS entry_type,
        id AS run_id,
        started_at,
        ended_at,
        data_domain,
        requested_source,
        answering_sources,
        identifiers_requested_raw,
        identifiers_duplicated,
        identifiers_attempted,
        loaded,
        no_op,
        not_resolved,
        no_data_for_date,
        unsupported_scheme,
        rejected,
        source_unavailable,
        rate_limited,
        storage_failure,
        assets_created,
        assets_adopted,
        duration_ms,
        as_of_date,
        range_from,
        range_to,
        range_concurrency,
        (rejected + source_unavailable + rate_limited + storage_failure) AS failed,
        CASE
            WHEN ended_at IS NULL THEN 'running'
            WHEN rejected + source_unavailable + rate_limited + storage_failure = 0
                THEN 'complete'
            WHEN rejected + source_unavailable + rate_limited + storage_failure
                 = identifiers_attempted
                THEN 'failed'
            ELSE 'partial'
        END AS child_status
    FROM reference_data_load_runs
),
run_entries AS (
    SELECT
        rr.entry_id,
        rr.entry_type,
        MIN(rr.started_at) AS started_at,
        CASE WHEN bool_or(rr.ended_at IS NULL) THEN NULL ELSE MAX(rr.ended_at) END AS ended_at,
        MAX(rr.data_domain) AS data_domain,
        MAX(rr.requested_source) AS requested_source,
        COALESCE((
            SELECT array_agg(DISTINCT source ORDER BY source)
            FROM run_rows x, unnest(x.answering_sources) AS src(source)
            WHERE x.entry_id = rr.entry_id
        ), '{}'::text[]) AS answering_sources,
        SUM(rr.identifiers_requested_raw)::integer AS identifiers_requested_raw,
        SUM(rr.identifiers_duplicated)::integer AS identifiers_duplicated,
        SUM(rr.identifiers_attempted)::integer AS identifiers_attempted,
        SUM(rr.loaded)::integer AS loaded,
        SUM(rr.no_op)::integer AS no_op,
        SUM(rr.not_resolved)::integer AS not_resolved,
        SUM(rr.no_data_for_date)::integer AS no_data_for_date,
        SUM(rr.unsupported_scheme)::integer AS unsupported_scheme,
        SUM(rr.rejected)::integer AS rejected,
        SUM(rr.source_unavailable)::integer AS source_unavailable,
        SUM(rr.rate_limited)::integer AS rate_limited,
        SUM(rr.storage_failure)::integer AS storage_failure,
        SUM(rr.assets_created)::integer AS assets_created,
        SUM(rr.assets_adopted)::integer AS assets_adopted,
        CASE
            WHEN bool_or(rr.ended_at IS NULL) THEN 'running'
            WHEN bool_or(rr.child_status = 'failed') THEN 'failed'
            WHEN bool_or(rr.child_status = 'partial') THEN 'partial'
            ELSE 'complete'
        END AS status,
        CASE
            WHEN bool_or(rr.ended_at IS NULL) THEN NULL
            ELSE (EXTRACT(EPOCH FROM (MAX(rr.ended_at) - MIN(rr.started_at))) * 1000)::bigint
        END AS duration_ms,
        CASE
            WHEN rr.entry_type = 'load_run' THEN MAX(rr.as_of_date)
            ELSE MIN(rr.range_from)
        END AS date_from,
        CASE
            WHEN rr.entry_type = 'load_run' THEN MAX(rr.as_of_date)
            ELSE MAX(rr.range_to)
        END AS date_to,
        CASE
            WHEN rr.entry_type = 'load_run' THEN 1
            ELSE COALESCE(
                (MAX(rr.range_to) - MIN(rr.range_from) + 1)::integer,
                COUNT(DISTINCT rr.as_of_date)::integer
            )
        END AS dates_requested,
        COUNT(*) FILTER (WHERE rr.ended_at IS NOT NULL)::integer AS dates_completed,
        COUNT(*) FILTER (WHERE rr.loaded > 0)::integer AS dates_with_loads,
        MAX(rr.range_concurrency) AS range_concurrency,
        array_agg(
            rr.run_id
            ORDER BY COALESCE(rr.as_of_date, rr.started_at::date), rr.started_at
        ) AS child_run_ids,
        NULL::text AS source_id,
        NULL::boolean AS before_enabled,
        NULL::boolean AS after_enabled,
        NULL::text AS actor
    FROM run_rows rr
    GROUP BY rr.entry_id, rr.entry_type
),
change_entries AS (
    SELECT
        c.id AS entry_id,
        'source_change'::text AS entry_type,
        c.changed_at AS started_at,
        c.changed_at AS ended_at,
        NULL::text AS data_domain,
        NULL::text AS requested_source,
        '{}'::text[] AS answering_sources,
        0::integer AS identifiers_requested_raw,
        0::integer AS identifiers_duplicated,
        0::integer AS identifiers_attempted,
        0::integer AS loaded,
        0::integer AS no_op,
        0::integer AS not_resolved,
        0::integer AS no_data_for_date,
        0::integer AS unsupported_scheme,
        0::integer AS rejected,
        0::integer AS source_unavailable,
        0::integer AS rate_limited,
        0::integer AS storage_failure,
        0::integer AS assets_created,
        0::integer AS assets_adopted,
        CASE WHEN c.after_enabled THEN 'source_enabled' ELSE 'source_disabled' END AS status,
        0::bigint AS duration_ms,
        NULL::date AS date_from,
        NULL::date AS date_to,
        0::integer AS dates_requested,
        0::integer AS dates_completed,
        0::integer AS dates_with_loads,
        NULL::integer AS range_concurrency,
        '{}'::uuid[] AS child_run_ids,
        c.source_id,
        c.before_enabled,
        c.after_enabled,
        c.actor
    FROM data_source_state_changes c
)
SELECT * FROM run_entries
UNION ALL
SELECT * FROM change_entries;
