-- 0001_initial_schema.sql
-- Schema for the 8 high-level entities defined in
-- specs/015-database-uml-domains/spec.md. See the spec for the entity
-- definitions, relationships, and invariants.

CREATE TABLE IF NOT EXISTS portfolios (
    id              UUID PRIMARY KEY,
    name            TEXT NOT NULL,
    purpose         TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by      TEXT NOT NULL,
    parent_id       UUID NULL REFERENCES portfolios(id),
    CONSTRAINT portfolios_no_self_parent CHECK (id <> parent_id)
);

CREATE INDEX IF NOT EXISTS idx_portfolios_parent_id ON portfolios(parent_id);

CREATE TABLE IF NOT EXISTS issuers (
    id                  UUID PRIMARY KEY,
    name                TEXT NOT NULL,
    issuer_type         TEXT NOT NULL,
    legal_jurisdiction  TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS assets (
    id                       UUID PRIMARY KEY,
    asset_type               TEXT NOT NULL,
    display_name             TEXT NOT NULL,
    currency                 TEXT NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    backed_by_portfolio_id   UUID NULL REFERENCES portfolios(id),
    issuer_id                UUID NULL REFERENCES issuers(id)
);

CREATE INDEX IF NOT EXISTS idx_assets_issuer_id ON assets(issuer_id);
CREATE INDEX IF NOT EXISTS idx_assets_backed_by ON assets(backed_by_portfolio_id);

CREATE TABLE IF NOT EXISTS positions (
    portfolio_id     UUID NOT NULL REFERENCES portfolios(id),
    asset_id         UUID NOT NULL REFERENCES assets(id),
    quantity         NUMERIC(38, 18) NOT NULL,
    as_of            TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (portfolio_id, asset_id)
);

CREATE TABLE IF NOT EXISTS events (
    id                  UUID PRIMARY KEY,
    event_type          TEXT NOT NULL,
    effective_date      DATE NOT NULL,
    quantity_delta      NUMERIC(38, 18) NOT NULL,
    recorded_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    portfolio_id        UUID NOT NULL REFERENCES portfolios(id),
    asset_id            UUID NOT NULL REFERENCES assets(id),
    to_portfolio_id     UUID NULL REFERENCES portfolios(id),
    CONSTRAINT events_non_zero_delta CHECK (quantity_delta <> 0),
    CONSTRAINT events_transfer_has_destination CHECK (event_type <> 'transfer' OR to_portfolio_id IS NOT NULL),
    CONSTRAINT events_transfer_not_self CHECK (event_type <> 'transfer' OR to_portfolio_id <> portfolio_id)
);

CREATE INDEX IF NOT EXISTS idx_events_portfolio_asset ON events(portfolio_id, asset_id);
CREATE INDEX IF NOT EXISTS idx_events_effective_date ON events(effective_date);

CREATE TABLE IF NOT EXISTS asset_reference_data (
    id              UUID PRIMARY KEY,
    asset_id        UUID NOT NULL REFERENCES assets(id),
    fact_type       TEXT NOT NULL,
    as_of_date      DATE NOT NULL,
    knowledge_date  DATE NOT NULL,
    value           TEXT NOT NULL,
    source          TEXT NOT NULL,
    CONSTRAINT asset_ref_data_dual_temporal UNIQUE (asset_id, fact_type, as_of_date, knowledge_date)
);

CREATE INDEX IF NOT EXISTS idx_asset_ref_data_lookup
    ON asset_reference_data(asset_id, fact_type, as_of_date, knowledge_date DESC);

CREATE TABLE IF NOT EXISTS issuer_reference_data (
    id              UUID PRIMARY KEY,
    issuer_id       UUID NOT NULL REFERENCES issuers(id),
    fact_type       TEXT NOT NULL,
    as_of_date      DATE NOT NULL,
    knowledge_date  DATE NOT NULL,
    value           TEXT NOT NULL,
    source          TEXT NOT NULL,
    CONSTRAINT issuer_ref_data_dual_temporal UNIQUE (issuer_id, fact_type, as_of_date, knowledge_date)
);

CREATE INDEX IF NOT EXISTS idx_issuer_ref_data_lookup
    ON issuer_reference_data(issuer_id, fact_type, as_of_date, knowledge_date DESC);

CREATE TABLE IF NOT EXISTS portfolio_reference_data (
    id              UUID PRIMARY KEY,
    portfolio_id    UUID NOT NULL REFERENCES portfolios(id),
    fact_type       TEXT NOT NULL,
    as_of_date      DATE NOT NULL,
    knowledge_date  DATE NOT NULL,
    value           TEXT NOT NULL,
    source          TEXT NOT NULL,
    CONSTRAINT portfolio_ref_data_dual_temporal UNIQUE (portfolio_id, fact_type, as_of_date, knowledge_date)
);

CREATE INDEX IF NOT EXISTS idx_portfolio_ref_data_lookup
    ON portfolio_reference_data(portfolio_id, fact_type, as_of_date, knowledge_date DESC);
