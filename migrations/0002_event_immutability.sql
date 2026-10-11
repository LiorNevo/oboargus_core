-- 0002_event_immutability.sql
-- Enforce the "events are immutable" invariant (predecessor FR-005 and
-- data-model.md § "events ... No UPDATE or DELETE permissions granted to
-- the application role") at the database role level.
--
-- This migration assumes the application connects as the role
-- `obolargus_app`. The role is created if it does not already exist;
-- adjust the role name to match the deployment's connection settings.
-- The migration is idempotent (uses DO blocks with existence checks).

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'obolargus_app') THEN
        CREATE ROLE obolargus_app WITH LOGIN;
    END IF;
END
$$;

-- Revoke any privileges that may have been granted by default, then
-- grant only INSERT and SELECT on the events table. UPDATE and DELETE
-- are NOT granted, so the application role cannot mutate events even
-- if a future bug tries to.
REVOKE ALL ON events FROM obolargus_app;
GRANT SELECT, INSERT ON events TO obolargus_app;
