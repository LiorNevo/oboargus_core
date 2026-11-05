-- 0011_normalized_facts.sql
--
-- Feature 027 replaces the `value TEXT` reference-data convention with:
--   * a machine-readable fact catalog (the data dictionary);
--   * one append-only `temporal_facts` envelope;
--   * typed scalar payload tables;
--   * a typed `market_observations` store for prices.
--
-- This is a clean-schema change. The PoC database is recreated rather than
-- migrated, so the legacy tables are dropped without a backfill.

-- ---------------------------------------------------------------------------
-- Remove the legacy reference-data surface and its string value column.
-- ---------------------------------------------------------------------------

DROP TABLE IF EXISTS data_source_reconciliation_audits CASCADE;
DROP TABLE IF EXISTS asset_reference_data CASCADE;
DROP TABLE IF EXISTS issuer_reference_data CASCADE;
DROP TABLE IF EXISTS portfolio_reference_data CASCADE;

-- ---------------------------------------------------------------------------
-- The data dictionary.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS fact_types (
    fact_type          TEXT PRIMARY KEY,
    display_name       TEXT NOT NULL,
    description        TEXT NOT NULL,
    storage_kind       TEXT NOT NULL,
    aggregation        TEXT NOT NULL,
    unit               TEXT NULL,
    observation_kind   TEXT NULL,
    interval_seconds   INTEGER NULL,
    schema_version     INTEGER NOT NULL DEFAULT 1,
    wire_schema        JSONB NOT NULL DEFAULT '{}'::jsonb,
    active             BOOLEAN NOT NULL DEFAULT TRUE,
    CONSTRAINT fact_types_storage_kind CHECK (storage_kind IN (
        'text', 'decimal', 'integer', 'boolean', 'date', 'timestamp',
        'market_observation')),
    CONSTRAINT fact_types_non_empty CHECK (
        btrim(fact_type) <> '' AND btrim(display_name) <> ''),
    CONSTRAINT fact_types_observation_shape CHECK (
        (storage_kind = 'market_observation') = (observation_kind IS NOT NULL)),
    CONSTRAINT fact_types_interval_non_negative CHECK (
        interval_seconds IS NULL OR interval_seconds >= 0)
);

CREATE TABLE IF NOT EXISTS fact_type_fields (
    fact_type      TEXT NOT NULL REFERENCES fact_types(fact_type) ON DELETE CASCADE,
    ordinal        INTEGER NOT NULL,
    field_name     TEXT NOT NULL,
    data_type      TEXT NOT NULL,
    nullable       BOOLEAN NOT NULL,
    unit           TEXT NULL,
    precision      INTEGER NULL,
    scale          INTEGER NULL,
    min_value      NUMERIC NULL,
    max_value      NUMERIC NULL,
    allowed_values TEXT[] NULL,
    aggregates     TEXT[] NOT NULL DEFAULT '{none}'::text[],
    description    TEXT NOT NULL,
    PRIMARY KEY (fact_type, field_name),
    UNIQUE (fact_type, ordinal),
    CONSTRAINT fact_type_fields_non_empty CHECK (
        btrim(field_name) <> '' AND btrim(description) <> '')
);

CREATE TABLE IF NOT EXISTS fact_type_subjects (
    fact_type    TEXT NOT NULL REFERENCES fact_types(fact_type) ON DELETE CASCADE,
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('asset', 'issuer', 'portfolio')),
    PRIMARY KEY (fact_type, subject_kind)
);

-- ---------------------------------------------------------------------------
-- The append-only reference-fact envelope.
--
-- Exactly one subject reference is non-null. The generated subject columns
-- let every read use one index without a polymorphic application-side join.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS temporal_facts (
    id                UUID PRIMARY KEY,
    asset_id          UUID NULL REFERENCES assets(id),
    issuer_id         UUID NULL REFERENCES issuers(id),
    portfolio_id      UUID NULL REFERENCES portfolios(id),
    subject_kind      TEXT GENERATED ALWAYS AS (
                          CASE
                            WHEN asset_id IS NOT NULL THEN 'asset'
                            WHEN issuer_id IS NOT NULL THEN 'issuer'
                            ELSE 'portfolio'
                          END) STORED,
    subject_id        UUID GENERATED ALWAYS AS (
                          COALESCE(asset_id, issuer_id, portfolio_id)) STORED,
    fact_type         TEXT NOT NULL REFERENCES fact_types(fact_type),
    valid_at          TIMESTAMPTZ NOT NULL,
    known_at          TIMESTAMPTZ NOT NULL,
    source            TEXT NOT NULL,
    retracted         BOOLEAN NOT NULL DEFAULT FALSE,
    retraction_reason TEXT NULL,
    content_hash      TEXT NOT NULL,
    CONSTRAINT temporal_facts_one_subject CHECK (
        num_nonnulls(asset_id, issuer_id, portfolio_id) = 1),
    CONSTRAINT temporal_facts_source_non_empty CHECK (btrim(source) <> ''),
    CONSTRAINT temporal_facts_retraction CHECK (
        (retracted AND retraction_reason IS NOT NULL)
        OR (NOT retracted AND retraction_reason IS NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS temporal_facts_identity
    ON temporal_facts (subject_kind, subject_id, fact_type,
                       valid_at, known_at, source);
CREATE INDEX IF NOT EXISTS temporal_facts_resolve
    ON temporal_facts (subject_kind, subject_id, fact_type,
                       valid_at DESC, known_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS temporal_facts_as_known
    ON temporal_facts (subject_kind, subject_id, fact_type, known_at DESC);
CREATE INDEX IF NOT EXISTS temporal_facts_hash
    ON temporal_facts (subject_kind, subject_id, fact_type, valid_at, source, content_hash);

-- ---------------------------------------------------------------------------
-- Typed scalar payloads: one table per value type, one row per fact.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS fact_text_values (
    fact_id UUID PRIMARY KEY REFERENCES temporal_facts(id),
    value   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS fact_decimal_values (
    fact_id UUID PRIMARY KEY REFERENCES temporal_facts(id),
    value   NUMERIC(38,18) NOT NULL
);

CREATE TABLE IF NOT EXISTS fact_integer_values (
    fact_id UUID PRIMARY KEY REFERENCES temporal_facts(id),
    value   BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS fact_boolean_values (
    fact_id UUID PRIMARY KEY REFERENCES temporal_facts(id),
    value   BOOLEAN NOT NULL
);

CREATE TABLE IF NOT EXISTS fact_date_values (
    fact_id UUID PRIMARY KEY REFERENCES temporal_facts(id),
    value   DATE NOT NULL
);

CREATE TABLE IF NOT EXISTS fact_timestamp_values (
    fact_id UUID PRIMARY KEY REFERENCES temporal_facts(id),
    value   TIMESTAMPTZ NOT NULL
);

-- ---------------------------------------------------------------------------
-- Source capability: which source can provide which fact type.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS data_source_fact_types (
    source_id TEXT NOT NULL REFERENCES data_sources(source_id) ON DELETE CASCADE,
    fact_type TEXT NOT NULL REFERENCES fact_types(fact_type) ON DELETE CASCADE,
    PRIMARY KEY (source_id, fact_type)
);

-- ---------------------------------------------------------------------------
-- Market observations: typed and partitioned by valid time.
--
-- `kind` distinguishes an end-of-day bar from an intraday bar or a quote.
-- `interval_seconds` is 0 for eod/quote and > 0 for a bar.
-- `content_hash` makes redelivery idempotent without conflating it with a
-- later correction (which has the same valid_at but a different hash).
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS market_observations (
    id                 UUID NOT NULL,
    asset_id           UUID NOT NULL REFERENCES assets(id),
    kind               TEXT NOT NULL,
    interval_seconds   INTEGER NOT NULL,
    valid_at           TIMESTAMPTZ NOT NULL,
    source             TEXT NOT NULL,
    known_at           TIMESTAMPTZ NOT NULL,
    content_hash       TEXT NOT NULL,
    open               NUMERIC(38,18) NULL,
    high               NUMERIC(38,18) NULL,
    low                NUMERIC(38,18) NULL,
    close              NUMERIC(38,18) NULL,
    adjclose           NUMERIC(38,18) NULL,
    price              NUMERIC(38,18) NULL,
    volume             NUMERIC(38,0) NULL,
    currency           CHAR(3) NOT NULL,
    adjustment         TEXT NOT NULL,
    sequence           BIGINT NULL,
    retracted          BOOLEAN NOT NULL DEFAULT FALSE,
    retraction_reason  TEXT NULL,
    PRIMARY KEY (asset_id, kind, interval_seconds, valid_at, source, content_hash),
    CONSTRAINT market_observations_kind CHECK (kind IN ('eod', 'bar', 'quote')),
    CONSTRAINT market_observations_interval CHECK (
        (kind = 'bar' AND interval_seconds > 0)
        OR (kind IN ('eod', 'quote') AND interval_seconds = 0)),
    CONSTRAINT market_observations_source_non_empty CHECK (btrim(source) <> ''),
    CONSTRAINT market_observations_adjustment CHECK (
        adjustment IN ('adjusted', 'unadjusted', 'none')),
    CONSTRAINT market_observations_retraction CHECK (
        (retracted AND retraction_reason IS NOT NULL)
        OR (NOT retracted AND retraction_reason IS NULL)),
    CONSTRAINT market_observations_has_value CHECK (
        retracted OR num_nonnulls(price, close) > 0),
    CONSTRAINT market_observations_ohlc_bounds CHECK (
        (high IS NULL OR low IS NULL OR high >= low)
        AND (high IS NULL OR open IS NULL OR open <= high)
        AND (low IS NULL OR open IS NULL OR open >= low)
        AND (high IS NULL OR close IS NULL OR close <= high)
        AND (low IS NULL OR close IS NULL OR close >= low)),
    CONSTRAINT market_observations_volume_non_negative CHECK (
        volume IS NULL OR volume >= 0),
    CONSTRAINT market_observations_bar_shape CHECK (
        retracted
        OR kind <> 'bar'
        OR (open IS NOT NULL AND high IS NOT NULL AND low IS NOT NULL AND close IS NOT NULL)),
    CONSTRAINT market_observations_quote_shape CHECK (
        retracted OR kind <> 'quote' OR price IS NOT NULL)
) PARTITION BY RANGE (valid_at);

CREATE TABLE IF NOT EXISTS market_observations_default
    PARTITION OF market_observations DEFAULT;

CREATE INDEX IF NOT EXISTS market_observations_series
    ON market_observations (asset_id, kind, interval_seconds, source,
                            valid_at DESC, known_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS market_observations_as_known
    ON market_observations (asset_id, kind, interval_seconds, source,
                            valid_at DESC, known_at DESC);
CREATE INDEX IF NOT EXISTS market_observations_asset
    ON market_observations (asset_id, valid_at DESC);

-- ---------------------------------------------------------------------------
-- Reconciliation audit: records the chosen belief by id, not a copied payload.
-- `chosen_observation_id` has no FK because the observation primary key is
-- composite (it includes the partition key); the writer validates the id.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS fact_reconciliations (
    id                    UUID PRIMARY KEY,
    subject_kind          TEXT NOT NULL CHECK (subject_kind IN ('asset', 'issuer', 'portfolio')),
    subject_id            UUID NOT NULL,
    fact_type             TEXT NOT NULL REFERENCES fact_types(fact_type),
    valid_at              TIMESTAMPTZ NOT NULL,
    chosen_fact_id        UUID NULL REFERENCES temporal_facts(id),
    chosen_observation_id UUID NULL,
    policy                TEXT NOT NULL,
    sources_tried         TEXT[] NOT NULL,
    per_source_answers    JSONB NOT NULL,
    decided_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT fact_reconciliations_one_choice CHECK (
        num_nonnulls(chosen_fact_id, chosen_observation_id) <= 1),
    CONSTRAINT fact_reconciliations_two_sources CHECK (
        cardinality(sources_tried) >= 2)
);

CREATE INDEX IF NOT EXISTS fact_reconciliations_subject
    ON fact_reconciliations (subject_kind, subject_id, fact_type, valid_at DESC);
