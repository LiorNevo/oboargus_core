-- 0003_issuer_parent.sql
-- Add `parent_id` to the `issuers` table to support the issuer DAG
-- (predecessor spec Assumption 20, FR-027, Invariant 8).
--
-- Idempotent: checks column existence before adding.

-- Add the nullable parent_id column referencing the same table
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'issuers' AND column_name = 'parent_id'
    ) THEN
        ALTER TABLE issuers ADD COLUMN parent_id UUID NULL REFERENCES issuers(id);
    END IF;
END
$$;

-- Prevent self-loop (id = parent_id)
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.table_constraints
        WHERE table_name = 'issuers' AND constraint_name = 'issuers_no_self_loop'
    ) THEN
        ALTER TABLE issuers ADD CONSTRAINT issuers_no_self_loop CHECK (id <> parent_id);
    END IF;
END
$$;

-- Index for efficient child lookup (list_children_issuers)
CREATE INDEX IF NOT EXISTS issuers_parent_id_idx ON issuers(parent_id);
