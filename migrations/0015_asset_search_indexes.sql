-- 0015_asset_search_indexes.sql
--
-- Asset search keeps the leading-wildcard `ILIKE '%q%'` predicate. These
-- trigram GIN indexes make that predicate indexable.
--
-- The migration fails fast when `pg_trgm` is unavailable; the deployment then
-- follows the documented prefix-search fallback. Both statements are
-- idempotent.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX IF NOT EXISTS idx_assets_display_name_trgm
    ON assets USING gin (display_name gin_trgm_ops);

CREATE INDEX IF NOT EXISTS idx_asset_identifiers_value_trgm
    ON asset_identifiers USING gin (value gin_trgm_ops);
