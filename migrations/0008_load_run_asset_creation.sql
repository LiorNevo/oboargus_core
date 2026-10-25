-- Migration 0008: asset-creation counters on reference-data load runs.
--
-- Implements FR-016 of `specs/018-load-reference-data/spec.md`.
--
-- The two additive counters record the asset-creation side effect of a load
-- run. They are NOT outcome buckets: an identifier that caused an asset to be
-- created still ends in one of the nine outcomes (usually `loaded`). The
-- fetch-attempted partition and the `not_resolved + attempted = unique`
-- partition therefore stay exactly as migration 0007 defined them.

ALTER TABLE reference_data_load_runs
    ADD COLUMN IF NOT EXISTS assets_created INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS assets_adopted INTEGER NOT NULL DEFAULT 0;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'reference_data_load_runs_assets_created_non_negative'
    ) THEN
        ALTER TABLE reference_data_load_runs
            ADD CONSTRAINT reference_data_load_runs_assets_created_non_negative
            CHECK (assets_created >= 0);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'reference_data_load_runs_assets_adopted_non_negative'
    ) THEN
        ALTER TABLE reference_data_load_runs
            ADD CONSTRAINT reference_data_load_runs_assets_adopted_non_negative
            CHECK (assets_adopted >= 0);
    END IF;
END
$$;
