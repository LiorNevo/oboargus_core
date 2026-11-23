-- Migration 0014: Authentication actors and per-owner workspace ownership.
--
-- Implements `specs/034-production-hardening/data-model.md` §8 and tasks
-- T005..T009. Additive and idempotent: new tables use IF NOT EXISTS, new
-- columns use ADD COLUMN IF NOT EXISTS, and constraints are added from
-- guard blocks. The one intentional constraint replacement is the
-- `automation_context` primary key; it is data-preserving and is skipped
-- when the key is already per-owner.
--
-- The startup bootstrap (Rust, after migrations) seeds the primary actor
-- and reassigns `dev-user`/NULL ownership to it, so owner columns that
-- cannot be resolved here stay nullable.

-- =============================================================================
-- 1. actors: Google identity → stable ownership id
-- =============================================================================

CREATE TABLE IF NOT EXISTS actors (
    id              UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    -- NULL only for the seeded primary actor before its first sign-in.
    -- PostgreSQL UNIQUE treats NULLs as distinct, which is what we need.
    google_sub      TEXT         NULL UNIQUE,
    email           TEXT         NOT NULL,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    last_login_at   TIMESTAMPTZ  NULL,

    CONSTRAINT actors_email_non_empty CHECK (email <> '')
);

-- Email comparison is case-insensitive; the startup bootstrap stores a
-- trimmed, lower-cased email, but this index keeps the invariant even if a
-- row is written by another path.
CREATE UNIQUE INDEX IF NOT EXISTS uq_actors_email_lower
    ON actors (lower(email));

-- =============================================================================
-- 2. load_range_groups: one durable row per requested range
-- =============================================================================

CREATE TABLE IF NOT EXISTS load_range_groups (
    id                  UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id            TEXT         NOT NULL,
    -- Client idempotency key. Unique per owner, not globally.
    client_group_id     UUID         NOT NULL,
    from_date           DATE         NOT NULL,
    to_date             DATE         NOT NULL,
    concurrency         INTEGER      NOT NULL,
    status              TEXT         NOT NULL DEFAULT 'running',
    -- Inclusive day count from the original request; never rewritten.
    dates_requested     INTEGER      NOT NULL,
    cancel_requested_at TIMESTAMPTZ  NULL,
    started_at          TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    ended_at            TIMESTAMPTZ  NULL,
    created_at          TIMESTAMPTZ  NOT NULL DEFAULT NOW(),

    CONSTRAINT load_range_groups_owner_client_unique
        UNIQUE (owner_id, client_group_id),
    CONSTRAINT load_range_groups_range_order
        CHECK (from_date <= to_date),
    CONSTRAINT load_range_groups_concurrency
        CHECK (concurrency BETWEEN 1 AND 32),
    CONSTRAINT load_range_groups_status
        CHECK (status IN (
            'running', 'complete', 'partial', 'failed', 'cancelled', 'interrupted'
        )),
    CONSTRAINT load_range_groups_dates_requested_positive
        CHECK (dates_requested > 0),
    CONSTRAINT load_range_groups_ended_order
        CHECK (ended_at IS NULL OR ended_at >= started_at)
);

CREATE INDEX IF NOT EXISTS idx_load_range_groups_owner_started
    ON load_range_groups (owner_id, started_at DESC);

-- =============================================================================
-- 3. reference_data_load_runs: owner + terminal status
-- =============================================================================

ALTER TABLE reference_data_load_runs
    ADD COLUMN IF NOT EXISTS owner_id TEXT NULL,
    ADD COLUMN IF NOT EXISTS terminal_status TEXT NOT NULL DEFAULT 'running';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'reference_data_load_runs_terminal_status'
    ) THEN
        ALTER TABLE reference_data_load_runs
            ADD CONSTRAINT reference_data_load_runs_terminal_status
            CHECK (terminal_status IN (
                'running', 'complete', 'partial', 'failed', 'interrupted', 'cancelled'
            ));
    END IF;
END $$;

-- Backfill terminal status from the existing counters for rows that already
-- ended. The counter partition matches the migration-0010 view: a failure is
-- rejected + source_unavailable + rate_limited + storage_failure.
UPDATE reference_data_load_runs
SET terminal_status = CASE
        WHEN rejected + source_unavailable + rate_limited + storage_failure = 0
            THEN 'complete'
        WHEN rejected + source_unavailable + rate_limited + storage_failure
             = identifiers_attempted
            THEN 'failed'
        ELSE 'partial'
    END
WHERE ended_at IS NOT NULL
  AND terminal_status = 'running';

-- Open rows cannot still be running after a restart: close them as
-- interrupted. GREATEST keeps the existing started_at <= ended_at check
-- satisfied even if the database clock moved backwards.
UPDATE reference_data_load_runs
SET terminal_status = 'interrupted',
    ended_at = GREATEST(started_at, NOW())
WHERE ended_at IS NULL
  AND terminal_status = 'running';

CREATE INDEX IF NOT EXISTS idx_reference_data_load_runs_owner_started
    ON reference_data_load_runs (owner_id, started_at DESC);

-- =============================================================================
-- 4. automation_runs: owner inherited from the flow
-- =============================================================================

ALTER TABLE automation_runs
    ADD COLUMN IF NOT EXISTS owner_id TEXT NULL;

UPDATE automation_runs r
SET owner_id = f.owner_id
FROM automation_flows f
WHERE r.flow_id = f.id
  AND r.owner_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_automation_runs_owner_created
    ON automation_runs (owner_id, created_at DESC);

-- =============================================================================
-- 5. automation_context: per-owner key
-- =============================================================================

ALTER TABLE automation_context
    ADD COLUMN IF NOT EXISTS owner_id TEXT NULL;

-- Flow-scoped rows belong to the flow's owner.
UPDATE automation_context c
SET owner_id = f.owner_id
FROM automation_flows f
WHERE c.owner_id IS NULL
  AND c.scope = 'flow'
  AND c.scope_id = f.id::text;

-- Global (including scope_id = 'workspace') rows are not shared across
-- actors. The placeholder is reassigned to the primary actor by the startup
-- bootstrap.
UPDATE automation_context
SET owner_id = 'dev-user'
WHERE owner_id IS NULL;

ALTER TABLE automation_context
    ALTER COLUMN owner_id SET NOT NULL;

-- Replace the primary key with the per-owner key. This is the one
-- documented constraint replacement; both the old and new keys are unique
-- over the same rows after the deterministic backfill above, so no data is
-- lost or duplicated. The DO block is a no-op when the migration is re-run.
DO $$
DECLARE
    pk_name TEXT;
    pk_cols TEXT[];
BEGIN
    SELECT c.conname, array_agg(a.attname ORDER BY k.ord)
      INTO pk_name, pk_cols
      FROM pg_constraint c
      JOIN LATERAL unnest(c.conkey) WITH ORDINALITY AS k(attnum, ord) ON TRUE
      JOIN pg_attribute a
        ON a.attrelid = c.conrelid
       AND a.attnum = k.attnum
     WHERE c.conrelid = 'automation_context'::regclass
       AND c.contype = 'p'
     GROUP BY c.conname;

    IF pk_cols IS DISTINCT FROM
       ARRAY['owner_id', 'scope', 'scope_id', 'key']::text[] THEN
        IF pk_name IS NOT NULL THEN
            EXECUTE format(
                'ALTER TABLE automation_context DROP CONSTRAINT %I',
                pk_name
            );
        END IF;
        ALTER TABLE automation_context
            ADD PRIMARY KEY (owner_id, scope, scope_id, key);
    END IF;
END $$;

-- =============================================================================
-- 6. data_load_history: group-row ranges, ungrouped runs, source changes
-- =============================================================================
--
-- The column list is the migration-0010 list with `owner_id` appended at the
-- end, which is required for CREATE OR REPLACE VIEW. Rangeless children from
-- before this migration remain readable through the per-group fallback below.

CREATE OR REPLACE VIEW data_load_history AS
WITH child_rows AS (
    SELECT
        r.id AS run_id,
        r.range_group_id,
        r.started_at,
        r.ended_at,
        r.data_domain,
        r.requested_source,
        r.answering_sources,
        r.identifiers_requested_raw,
        r.identifiers_duplicated,
        r.identifiers_attempted,
        r.loaded,
        r.no_op,
        r.not_resolved,
        r.no_data_for_date,
        r.unsupported_scheme,
        r.rejected,
        r.source_unavailable,
        r.rate_limited,
        r.storage_failure,
        r.assets_created,
        r.assets_adopted,
        r.as_of_date,
        r.range_from,
        r.range_to,
        r.range_concurrency,
        r.owner_id,
        CASE
            WHEN r.ended_at IS NULL THEN 'running'
            WHEN r.terminal_status <> 'running' THEN r.terminal_status
            WHEN r.rejected + r.source_unavailable + r.rate_limited
                 + r.storage_failure = 0 THEN 'complete'
            WHEN r.rejected + r.source_unavailable + r.rate_limited
                 + r.storage_failure = r.identifiers_attempted THEN 'failed'
            ELSE 'partial'
        END AS child_status
    FROM reference_data_load_runs r
),
group_children AS (
    SELECT
        cr.range_group_id AS group_id,
        MIN(cr.started_at) AS first_started_at,
        MAX(cr.ended_at) AS last_ended_at,
        bool_or(cr.ended_at IS NULL) AS has_open_child,
        bool_or(cr.child_status = 'interrupted') AS has_interrupted,
        bool_or(cr.child_status = 'cancelled') AS has_cancelled,
        bool_or(cr.child_status = 'failed') AS has_failed,
        bool_or(cr.child_status = 'partial') AS has_partial,
        MAX(cr.data_domain) AS data_domain,
        MAX(cr.requested_source) AS requested_source,
        COALESCE((
            SELECT array_agg(DISTINCT source ORDER BY source)
            FROM child_rows x, unnest(x.answering_sources) AS src(source)
            WHERE x.range_group_id = cr.range_group_id
        ), '{}'::text[]) AS answering_sources,
        SUM(cr.identifiers_requested_raw)::integer AS identifiers_requested_raw,
        SUM(cr.identifiers_duplicated)::integer AS identifiers_duplicated,
        SUM(cr.identifiers_attempted)::integer AS identifiers_attempted,
        SUM(cr.loaded)::integer AS loaded,
        SUM(cr.no_op)::integer AS no_op,
        SUM(cr.not_resolved)::integer AS not_resolved,
        SUM(cr.no_data_for_date)::integer AS no_data_for_date,
        SUM(cr.unsupported_scheme)::integer AS unsupported_scheme,
        SUM(cr.rejected)::integer AS rejected,
        SUM(cr.source_unavailable)::integer AS source_unavailable,
        SUM(cr.rate_limited)::integer AS rate_limited,
        SUM(cr.storage_failure)::integer AS storage_failure,
        SUM(cr.assets_created)::integer AS assets_created,
        SUM(cr.assets_adopted)::integer AS assets_adopted,
        COUNT(*) FILTER (WHERE cr.ended_at IS NOT NULL)::integer AS dates_completed,
        COUNT(*) FILTER (WHERE cr.loaded > 0)::integer AS dates_with_loads,
        MIN(cr.range_from) AS min_range_from,
        MAX(cr.range_to) AS max_range_to,
        COUNT(DISTINCT cr.as_of_date)::integer AS distinct_dates,
        MAX(cr.range_concurrency) AS range_concurrency,
        array_agg(
            cr.run_id
            ORDER BY COALESCE(cr.as_of_date, cr.started_at::date), cr.started_at
        ) AS child_run_ids,
        MAX(cr.owner_id) AS owner_id
    FROM child_rows cr
    WHERE cr.range_group_id IS NOT NULL
    GROUP BY cr.range_group_id
),
group_entries AS (
    SELECT
        g.id AS entry_id,
        'range_run'::text AS entry_type,
        g.started_at AS started_at,
        g.ended_at AS ended_at,
        gc.data_domain AS data_domain,
        gc.requested_source AS requested_source,
        COALESCE(gc.answering_sources, '{}'::text[]) AS answering_sources,
        COALESCE(gc.identifiers_requested_raw, 0) AS identifiers_requested_raw,
        COALESCE(gc.identifiers_duplicated, 0) AS identifiers_duplicated,
        COALESCE(gc.identifiers_attempted, 0) AS identifiers_attempted,
        COALESCE(gc.loaded, 0) AS loaded,
        COALESCE(gc.no_op, 0) AS no_op,
        COALESCE(gc.not_resolved, 0) AS not_resolved,
        COALESCE(gc.no_data_for_date, 0) AS no_data_for_date,
        COALESCE(gc.unsupported_scheme, 0) AS unsupported_scheme,
        COALESCE(gc.rejected, 0) AS rejected,
        COALESCE(gc.source_unavailable, 0) AS source_unavailable,
        COALESCE(gc.rate_limited, 0) AS rate_limited,
        COALESCE(gc.storage_failure, 0) AS storage_failure,
        COALESCE(gc.assets_created, 0) AS assets_created,
        COALESCE(gc.assets_adopted, 0) AS assets_adopted,
        g.status AS status,
        CASE
            WHEN g.ended_at IS NULL THEN NULL
            ELSE (EXTRACT(EPOCH FROM (g.ended_at - g.started_at)) * 1000)::bigint
        END AS duration_ms,
        g.from_date AS date_from,
        g.to_date AS date_to,
        g.dates_requested AS dates_requested,
        COALESCE(gc.dates_completed, 0) AS dates_completed,
        COALESCE(gc.dates_with_loads, 0) AS dates_with_loads,
        g.concurrency AS range_concurrency,
        COALESCE(gc.child_run_ids, '{}'::uuid[]) AS child_run_ids,
        NULL::text AS source_id,
        NULL::boolean AS before_enabled,
        NULL::boolean AS after_enabled,
        NULL::text AS actor,
        g.owner_id AS owner_id
    FROM load_range_groups g
    LEFT JOIN group_children gc ON gc.group_id = g.id
),
legacy_range_entries AS (
    -- Children whose group row predates 0014 (or was written by an older
    -- build) are still summarised by their range_group_id.
    SELECT
        gc.group_id AS entry_id,
        'range_run'::text AS entry_type,
        gc.first_started_at AS started_at,
        CASE WHEN gc.has_open_child THEN NULL ELSE gc.last_ended_at END AS ended_at,
        gc.data_domain AS data_domain,
        gc.requested_source AS requested_source,
        gc.answering_sources AS answering_sources,
        gc.identifiers_requested_raw AS identifiers_requested_raw,
        gc.identifiers_duplicated AS identifiers_duplicated,
        gc.identifiers_attempted AS identifiers_attempted,
        gc.loaded AS loaded,
        gc.no_op AS no_op,
        gc.not_resolved AS not_resolved,
        gc.no_data_for_date AS no_data_for_date,
        gc.unsupported_scheme AS unsupported_scheme,
        gc.rejected AS rejected,
        gc.source_unavailable AS source_unavailable,
        gc.rate_limited AS rate_limited,
        gc.storage_failure AS storage_failure,
        gc.assets_created AS assets_created,
        gc.assets_adopted AS assets_adopted,
        CASE
            WHEN gc.has_open_child THEN 'running'
            WHEN gc.has_interrupted THEN 'interrupted'
            WHEN gc.has_cancelled THEN 'cancelled'
            WHEN gc.has_failed THEN 'failed'
            WHEN gc.has_partial THEN 'partial'
            ELSE 'complete'
        END AS status,
        CASE
            WHEN gc.has_open_child THEN NULL
            ELSE (EXTRACT(EPOCH FROM (gc.last_ended_at - gc.first_started_at)) * 1000)::bigint
        END AS duration_ms,
        gc.min_range_from AS date_from,
        gc.max_range_to AS date_to,
        COALESCE(
            (gc.max_range_to - gc.min_range_from + 1)::integer,
            gc.distinct_dates
        ) AS dates_requested,
        gc.dates_completed AS dates_completed,
        gc.dates_with_loads AS dates_with_loads,
        gc.range_concurrency AS range_concurrency,
        gc.child_run_ids AS child_run_ids,
        NULL::text AS source_id,
        NULL::boolean AS before_enabled,
        NULL::boolean AS after_enabled,
        NULL::text AS actor,
        gc.owner_id AS owner_id
    FROM group_children gc
    LEFT JOIN load_range_groups g ON g.id = gc.group_id
    WHERE g.id IS NULL
),
load_run_entries AS (
    SELECT
        cr.run_id AS entry_id,
        'load_run'::text AS entry_type,
        cr.started_at AS started_at,
        cr.ended_at AS ended_at,
        cr.data_domain AS data_domain,
        cr.requested_source AS requested_source,
        cr.answering_sources AS answering_sources,
        cr.identifiers_requested_raw AS identifiers_requested_raw,
        cr.identifiers_duplicated AS identifiers_duplicated,
        cr.identifiers_attempted AS identifiers_attempted,
        cr.loaded AS loaded,
        cr.no_op AS no_op,
        cr.not_resolved AS not_resolved,
        cr.no_data_for_date AS no_data_for_date,
        cr.unsupported_scheme AS unsupported_scheme,
        cr.rejected AS rejected,
        cr.source_unavailable AS source_unavailable,
        cr.rate_limited AS rate_limited,
        cr.storage_failure AS storage_failure,
        cr.assets_created AS assets_created,
        cr.assets_adopted AS assets_adopted,
        cr.child_status AS status,
        CASE
            WHEN cr.ended_at IS NULL THEN NULL
            ELSE (EXTRACT(EPOCH FROM (cr.ended_at - cr.started_at)) * 1000)::bigint
        END AS duration_ms,
        cr.as_of_date AS date_from,
        cr.as_of_date AS date_to,
        1::integer AS dates_requested,
        CASE WHEN cr.ended_at IS NULL THEN 0 ELSE 1 END AS dates_completed,
        CASE WHEN cr.loaded > 0 THEN 1 ELSE 0 END AS dates_with_loads,
        NULL::integer AS range_concurrency,
        ARRAY[cr.run_id] AS child_run_ids,
        NULL::text AS source_id,
        NULL::boolean AS before_enabled,
        NULL::boolean AS after_enabled,
        NULL::text AS actor,
        cr.owner_id AS owner_id
    FROM child_rows cr
    WHERE cr.range_group_id IS NULL
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
        c.source_id AS source_id,
        c.before_enabled AS before_enabled,
        c.after_enabled AS after_enabled,
        c.actor AS actor,
        NULL::text AS owner_id
    FROM data_source_state_changes c
)
SELECT * FROM group_entries
UNION ALL
SELECT * FROM legacy_range_entries
UNION ALL
SELECT * FROM load_run_entries
UNION ALL
SELECT * FROM change_entries;
