-- 0016: Correct the range date-completion count for interrupted children.
--
-- An interrupted child date (started but never durably completed) is not a
-- completed date. The 0014 view counted any closed child; this replacement
-- excludes `terminal_status = 'interrupted'`. Idempotent and data-preserving.
--
-- Implements FR-017 of `specs/034-production-hardening/spec.md`.

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
        r.terminal_status,
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
        COUNT(*) FILTER (WHERE cr.ended_at IS NOT NULL AND cr.terminal_status <> 'interrupted')::integer AS dates_completed,
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
        CASE WHEN cr.ended_at IS NULL OR cr.terminal_status = 'interrupted' THEN 0 ELSE 1 END AS dates_completed,
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
