-- 0017_exchange_calendars.sql
--
-- Exchange-calendar reference data, effective-dated asset assignments, and the
-- calendar-aware load counter. Calendars are not temporal facts: they describe
-- venues, not assets, issuers, or portfolios.
--
-- Implements spec 035 (market-data coverage and exchange calendars).

CREATE TABLE IF NOT EXISTS exchange_calendars (
    mic              TEXT        PRIMARY KEY,
    name             TEXT        NOT NULL,
    time_zone        TEXT        NOT NULL,
    coverage_from    DATE        NOT NULL,
    coverage_to      DATE        NOT NULL,
    source           TEXT        NOT NULL,
    provenance       TEXT        NOT NULL,
    version          INTEGER     NOT NULL DEFAULT 1,
    active           BOOLEAN     NOT NULL DEFAULT TRUE,
    updated_by       TEXT        NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT exchange_calendars_mic_format CHECK (mic ~ '^[A-Z0-9]{3,4}$'),
    CONSTRAINT exchange_calendars_name_non_empty CHECK (btrim(name) <> ''),
    CONSTRAINT exchange_calendars_time_zone_non_empty CHECK (btrim(time_zone) <> ''),
    CONSTRAINT exchange_calendars_source_non_empty CHECK (btrim(source) <> ''),
    CONSTRAINT exchange_calendars_provenance_non_empty CHECK (btrim(provenance) <> ''),
    CONSTRAINT exchange_calendars_coverage_order CHECK (coverage_from <= coverage_to),
    CONSTRAINT exchange_calendars_version_positive CHECK (version >= 1)
);

CREATE TABLE IF NOT EXISTS exchange_calendar_weekdays (
    mic          TEXT     NOT NULL REFERENCES exchange_calendars(mic) ON DELETE CASCADE,
    iso_weekday  SMALLINT NOT NULL,
    open_local   TIME     NOT NULL,
    close_local  TIME     NOT NULL,
    PRIMARY KEY (mic, iso_weekday),
    CONSTRAINT exchange_calendar_weekdays_iso_weekday CHECK (iso_weekday BETWEEN 1 AND 7),
    CONSTRAINT exchange_calendar_weekdays_order CHECK (open_local < close_local)
);

CREATE TABLE IF NOT EXISTS exchange_calendar_exceptions (
    id              UUID        PRIMARY KEY,
    mic             TEXT        NOT NULL REFERENCES exchange_calendars(mic) ON DELETE CASCADE,
    exception_date  DATE        NOT NULL,
    kind            TEXT        NOT NULL,
    open_local      TIME        NULL,
    close_local     TIME        NULL,
    reason          TEXT        NULL,
    source          TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT exchange_calendar_exceptions_kind CHECK (kind IN ('closure', 'session', 'early_close')),
    CONSTRAINT exchange_calendar_exceptions_shape CHECK (
        (kind = 'closure' AND open_local IS NULL AND close_local IS NULL)
        OR (kind = 'session' AND open_local IS NOT NULL AND close_local IS NOT NULL
            AND open_local < close_local)
        OR (kind = 'early_close' AND close_local IS NOT NULL
            AND (open_local IS NULL OR open_local < close_local))
    ),
    CONSTRAINT exchange_calendar_exceptions_source_non_empty CHECK (btrim(source) <> ''),
    CONSTRAINT exchange_calendar_exceptions_unique_date UNIQUE (mic, exception_date)
);

CREATE INDEX IF NOT EXISTS idx_exchange_calendar_exceptions_mic_date
    ON exchange_calendar_exceptions (mic, exception_date);

CREATE TABLE IF NOT EXISTS exchange_calendar_audit (
    id          UUID        PRIMARY KEY,
    mic         TEXT        NOT NULL,
    action      TEXT        NOT NULL,
    actor_id    TEXT        NOT NULL,
    summary     TEXT        NOT NULL,
    payload     JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_exchange_calendar_audit_mic_created
    ON exchange_calendar_audit (mic, created_at DESC);

CREATE TABLE IF NOT EXISTS asset_calendar_assignments (
    id          UUID        PRIMARY KEY,
    asset_id    UUID        NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    mic         TEXT        NOT NULL REFERENCES exchange_calendars(mic),
    valid_from  DATE        NOT NULL,
    valid_to    DATE        NULL,
    source      TEXT        NOT NULL,
    created_by  TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT asset_calendar_assignments_range_order CHECK (valid_to IS NULL OR valid_to > valid_from),
    CONSTRAINT asset_calendar_assignments_source_non_empty CHECK (btrim(source) <> '')
);

CREATE INDEX IF NOT EXISTS idx_asset_calendar_assignments_asset_from
    ON asset_calendar_assignments (asset_id, valid_from);
CREATE INDEX IF NOT EXISTS idx_asset_calendar_assignments_mic
    ON asset_calendar_assignments (mic);

-- The calendar-aware per-date counter is additive.
ALTER TABLE reference_data_load_runs
    ADD COLUMN IF NOT EXISTS calendar_closed INTEGER NOT NULL DEFAULT 0;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'reference_data_load_runs_calendar_closed_non_negative'
    ) THEN
        ALTER TABLE reference_data_load_runs
            ADD CONSTRAINT reference_data_load_runs_calendar_closed_non_negative
            CHECK (calendar_closed >= 0);
    END IF;
END $$;

-- The 0007 checks are inline and therefore unnamed. Replace them by matching
-- their definitions so a fresh database and an upgraded one converge.
DO $$
DECLARE
    constraint_name TEXT;
BEGIN
    FOR constraint_name IN
        SELECT conname
        FROM pg_constraint
        WHERE conrelid = 'reference_data_load_runs'::regclass
          AND contype = 'c'
          AND (
              pg_get_constraintdef(oid) LIKE '%identifiers_attempted = loaded%'
              OR pg_get_constraintdef(oid)
                 LIKE '%identifiers_requested_raw - identifiers_duplicated%'
          )
    LOOP
        EXECUTE format(
            'ALTER TABLE reference_data_load_runs DROP CONSTRAINT %I',
            constraint_name
        );
    END LOOP;
END $$;

-- calendar_closed is not a fetch attempt, so the attempted partition still
-- excludes it. The requested identity must account for every closed line.
ALTER TABLE reference_data_load_runs
    ADD CONSTRAINT reference_data_load_runs_attempted_partition CHECK (
        identifiers_attempted = loaded + no_op + no_data_for_date
            + rejected + source_unavailable + rate_limited + storage_failure
            + unsupported_scheme
    ),
    ADD CONSTRAINT reference_data_load_runs_requested_identity CHECK (
        not_resolved + identifiers_attempted + calendar_closed
            = identifiers_requested_raw - identifiers_duplicated
    );

-- Recreate the history view from 0016 with the per-date closed counter and a
-- fully-closed date count. Calendar-closed lines never enter the attempted
-- buckets, so the existing status logic keeps them out of `failed`.
-- Dropped first because the new columns are not appended at the end.
DROP VIEW IF EXISTS data_load_history;
CREATE VIEW data_load_history AS
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
        r.calendar_closed,
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
        r.terminal_status,
        CASE
            WHEN r.ended_at IS NULL THEN 'running'
            WHEN r.terminal_status <> 'running' THEN r.terminal_status
            WHEN r.rejected + r.source_unavailable + r.rate_limited
                 + r.storage_failure = 0 THEN 'complete'
            WHEN r.rejected + r.source_unavailable + r.rate_limited
                 + r.storage_failure = r.identifiers_attempted THEN 'failed'
            ELSE 'partial'
        END AS child_status,
        (
            r.calendar_closed > 0
            AND r.identifiers_attempted = 0
            AND r.not_resolved = 0
            AND r.loaded = 0
            AND r.no_op = 0
            AND r.no_data_for_date = 0
            AND r.rejected = 0
            AND r.source_unavailable = 0
            AND r.rate_limited = 0
            AND r.storage_failure = 0
            AND r.unsupported_scheme = 0
        ) AS fully_closed
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
        SUM(cr.calendar_closed)::integer AS calendar_closed,
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
        COUNT(*) FILTER (WHERE cr.ended_at IS NOT NULL AND cr.terminal_status <> 'interrupted')::integer AS dates_completed,
        COUNT(*) FILTER (WHERE cr.loaded > 0)::integer AS dates_with_loads,
        COUNT(*) FILTER (WHERE cr.fully_closed)::integer AS dates_fully_closed,
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
        COALESCE(gc.calendar_closed, 0) AS calendar_closed,
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
        COALESCE(gc.dates_fully_closed, 0) AS dates_fully_closed,
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
        gc.calendar_closed AS calendar_closed,
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
        gc.dates_fully_closed AS dates_fully_closed,
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
        cr.calendar_closed AS calendar_closed,
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
        CASE WHEN cr.ended_at IS NULL OR cr.terminal_status = 'interrupted' THEN 0 ELSE 1 END AS dates_completed,
        CASE WHEN cr.loaded > 0 THEN 1 ELSE 0 END AS dates_with_loads,
        CASE WHEN cr.fully_closed THEN 1 ELSE 0 END AS dates_fully_closed,
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
        0::integer AS calendar_closed,
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
        0::integer AS dates_fully_closed,
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
