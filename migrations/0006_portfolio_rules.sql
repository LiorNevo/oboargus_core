-- 0006_portfolio_rules.sql
-- The shared rule catalog and its many-to-many link to portfolios.
--
-- Implements FR-029 of `specs/016-implement-domain-model/spec.md` and the
-- `Rule` entity defined by the predecessor
-- (`specs/015-database-uml-domains/spec.md` FR-035..FR-044, added 2026-10-20).
--
-- Design notes (see the specs for the full rationale):
--   * A rule is SHARED: it is owned by no portfolio and governs every
--     portfolio it is attached to. Attaching it to a second portfolio
--     neither copies nor forks it.
--   * `category` names the EFFECT the rule has on the portfolio's data
--     (`all_assets`, `portfolio_data`, `positions`, `events`, ...). The
--     vocabulary is OPEN (predecessor FR-037): no enum constraint.
--   * The effect vocabulary is stored trimmed and lowercased by the
--     application (FR-032); the CHECK below only rejects an empty category.
--   * The rule `name` is unique across the WHOLE catalog (FR-035): a shared
--     rule is referenced from many places, so it needs one stable name.
--   * `portfolio_rules` is a plain join table: the link carries no
--     attributes of its own (the category belongs to the shared rule).
--     `PRIMARY KEY (portfolio_id, rule_id)` enforces that a portfolio
--     carries a given rule at most once (FR-036).
--   * Rules are current-state configuration, NOT dual-temporal facts: no
--     `as_of_date` / `knowledge_date` columns (predecessor FR-041).
--   * ON DELETE CASCADE on both sides of the join: deleting a rule removes
--     it from every portfolio that used it; deleting a portfolio removes
--     its attachments but leaves the rules in the catalog for others.
--
-- The migration is idempotent (IF NOT EXISTS / CREATE INDEX IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS rules (
    id           UUID PRIMARY KEY,
    name         TEXT NOT NULL,
    category     TEXT NOT NULL,
    expression   TEXT NOT NULL,
    description  TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by   TEXT NOT NULL,
    CONSTRAINT rules_name_non_empty CHECK (btrim(name) <> ''),
    CONSTRAINT rules_category_non_empty CHECK (btrim(category) <> ''),
    CONSTRAINT rules_expression_non_empty CHECK (btrim(expression) <> ''),
    CONSTRAINT rules_unique_name UNIQUE (name)
);

CREATE TABLE IF NOT EXISTS portfolio_rules (
    portfolio_id UUID NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
    rule_id      UUID NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
    CONSTRAINT portfolio_rules_unique_pair PRIMARY KEY (portfolio_id, rule_id)
);

CREATE INDEX IF NOT EXISTS idx_portfolio_rules_rule_id
    ON portfolio_rules(rule_id);
