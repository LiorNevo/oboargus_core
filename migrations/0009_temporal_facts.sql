-- 0009_temporal_facts.sql
--
-- Converts the three reference-data fact tables from a `DATE`-granular
-- dual-temporal model to the append-only instant model of
-- `specs/021-bitemporal-market-data`:
--
--   * `valid_at` / `known_at` are `TIMESTAMPTZ` (microsecond, UTC);
--   * `retracted` marks a belief that removes a value;
--   * `as_of_date` / `knowledge_date` become generated compatibility columns;
--   * `source` becomes part of the identity key, so two sources cannot
--     overwrite each other (FR-018).
--
-- The migration is guarded so a re-run is a no-op. See `data-model.md` §1
-- for the constraint text this implements.

DO $migration$
DECLARE
    target RECORD;
BEGIN
    FOR target IN
        SELECT *
        FROM (VALUES
            ('asset_reference_data', 'asset_id',
             'asset_ref_data_dual_temporal', 'idx_asset_ref_data_lookup',
             'asset_ref_data_temporal_identity', 'idx_asset_ref_data_known'),
            ('issuer_reference_data', 'issuer_id',
             'issuer_ref_data_dual_temporal', 'idx_issuer_ref_data_lookup',
             'issuer_ref_data_temporal_identity', 'idx_issuer_ref_data_known'),
            ('portfolio_reference_data', 'portfolio_id',
             'portfolio_ref_data_dual_temporal', 'idx_portfolio_ref_data_lookup',
             'portfolio_ref_data_temporal_identity', 'idx_portfolio_ref_data_known')
        ) AS v(tbl, subject_column, old_constraint, old_index, new_constraint, known_index)
    LOOP
        -- 1. Add the instant columns.
        EXECUTE format(
            'ALTER TABLE %I ADD COLUMN IF NOT EXISTS valid_at TIMESTAMPTZ',
            target.tbl);
        EXECUTE format(
            'ALTER TABLE %I ADD COLUMN IF NOT EXISTS known_at TIMESTAMPTZ',
            target.tbl);
        EXECUTE format(
            'ALTER TABLE %I ADD COLUMN IF NOT EXISTS retracted BOOLEAN NOT NULL DEFAULT FALSE',
            target.tbl);

        -- 2. Backfill from the legacy DATE columns, at midnight UTC, but only
        --    while those columns are still plain (not generated) columns.
        IF EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_name = target.tbl
              AND column_name = 'as_of_date'
              AND is_generated = 'NEVER'
        ) THEN
            EXECUTE format(
                'UPDATE %I SET valid_at = (as_of_date::timestamp AT TIME ZONE ''UTC'') WHERE valid_at IS NULL',
                target.tbl);
        END IF;

        IF EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_name = target.tbl
              AND column_name = 'knowledge_date'
              AND is_generated = 'NEVER'
        ) THEN
            EXECUTE format(
                'UPDATE %I SET known_at = (knowledge_date::timestamp AT TIME ZONE ''UTC'') WHERE known_at IS NULL',
                target.tbl);
        END IF;

        EXECUTE format('ALTER TABLE %I ALTER COLUMN valid_at SET NOT NULL', target.tbl);
        EXECUTE format('ALTER TABLE %I ALTER COLUMN known_at SET NOT NULL', target.tbl);

        -- 3. Drop the legacy identity key and lookup index. The old unique
        --    key omitted `source`, which is a lost-write hazard once two
        --    providers report the same fact.
        EXECUTE format('ALTER TABLE %I DROP CONSTRAINT IF EXISTS %I',
            target.tbl, target.old_constraint);
        EXECUTE format('DROP INDEX IF EXISTS %I', target.old_index);

        -- 4. Replace the DATE columns with generated compatibility columns.
        EXECUTE format('ALTER TABLE %I DROP COLUMN IF EXISTS as_of_date', target.tbl);
        EXECUTE format('ALTER TABLE %I DROP COLUMN IF EXISTS knowledge_date', target.tbl);
        EXECUTE format(
            'ALTER TABLE %I ADD COLUMN as_of_date DATE GENERATED ALWAYS AS ((valid_at AT TIME ZONE ''UTC'')::date) STORED',
            target.tbl);
        EXECUTE format(
            'ALTER TABLE %I ADD COLUMN knowledge_date DATE GENERATED ALWAYS AS ((known_at AT TIME ZONE ''UTC'')::date) STORED',
            target.tbl);

        -- 5. New identity key: source is part of the fact's identity.
        IF NOT EXISTS (
            SELECT 1 FROM pg_constraint
            WHERE conname = target.new_constraint
              AND conrelid = format('%I', target.tbl)::regclass
        ) THEN
            EXECUTE format(
                'ALTER TABLE %I ADD CONSTRAINT %I UNIQUE (%I, fact_type, valid_at, known_at, source)',
                target.tbl, target.new_constraint, target.subject_column);
        END IF;

        -- 6. Read paths: the "winner at (identity, valid_at)" walk and the
        --    "everything this subject knew as of K" walk.
        EXECUTE format(
            'CREATE INDEX IF NOT EXISTS idx_%s_winner ON %I (%I, fact_type, valid_at, known_at DESC, id DESC)',
            target.tbl, target.tbl, target.subject_column);
        EXECUTE format(
            'CREATE INDEX IF NOT EXISTS %I ON %I (%I, known_at DESC)',
            target.known_index, target.tbl, target.subject_column);
    END LOOP;
END
$migration$;
