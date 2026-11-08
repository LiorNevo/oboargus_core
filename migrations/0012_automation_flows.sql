-- Migration 0012: Low-code automation flow engine.
--
-- Implements `specs/030-automation-workflow-engine/data-model.md`.
-- Additive and idempotent. No existing table is altered.

CREATE TABLE IF NOT EXISTS automation_flows (
    id                          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id                    TEXT NOT NULL,
    name                        TEXT NOT NULL,
    description                 TEXT NOT NULL DEFAULT '',
    tags                        TEXT[] NOT NULL DEFAULT '{}'::text[],
    status                      TEXT NOT NULL DEFAULT 'active',
    current_draft_id            UUID NULL,
    current_published_version_id UUID NULL,
    draft_revision              BIGINT NOT NULL DEFAULT 1,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at                  TIMESTAMPTZ NULL,
    CONSTRAINT automation_flows_name_non_empty CHECK (name <> ''),
    CONSTRAINT automation_flows_status CHECK (status IN ('active', 'archived')),
    CONSTRAINT automation_flows_draft_revision_positive CHECK (draft_revision > 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_automation_flows_owner_name
    ON automation_flows (owner_id, lower(name))
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_automation_flows_owner_updated
    ON automation_flows (owner_id, updated_at DESC)
    WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS automation_flow_versions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flow_id         UUID NOT NULL REFERENCES automation_flows(id) ON DELETE CASCADE,
    version_number  INTEGER NULL,
    status          TEXT NOT NULL,
    flow_json       JSONB NOT NULL,
    change_note     TEXT NULL,
    created_by      TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    published_by    TEXT NULL,
    published_at    TIMESTAMPTZ NULL,
    CONSTRAINT automation_flow_versions_status
        CHECK (status IN ('draft', 'published', 'archived')),
    CONSTRAINT automation_flow_versions_publish_shape CHECK (
        (status = 'draft' AND version_number IS NULL AND published_at IS NULL)
        OR
        (status IN ('published', 'archived') AND version_number IS NOT NULL AND published_at IS NOT NULL)
    ),
    CONSTRAINT automation_flow_versions_version_positive
        CHECK (version_number IS NULL OR version_number > 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_automation_flow_versions_one_draft
    ON automation_flow_versions (flow_id)
    WHERE status = 'draft';

CREATE UNIQUE INDEX IF NOT EXISTS uq_automation_flow_versions_number
    ON automation_flow_versions (flow_id, version_number)
    WHERE version_number IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_automation_flow_versions_flow_status
    ON automation_flow_versions (flow_id, status, version_number DESC);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'automation_flows_current_draft_fk'
    ) THEN
        ALTER TABLE automation_flows
            ADD CONSTRAINT automation_flows_current_draft_fk
            FOREIGN KEY (current_draft_id)
            REFERENCES automation_flow_versions(id)
            ON DELETE SET NULL;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'automation_flows_current_published_fk'
    ) THEN
        ALTER TABLE automation_flows
            ADD CONSTRAINT automation_flows_current_published_fk
            FOREIGN KEY (current_published_version_id)
            REFERENCES automation_flow_versions(id)
            ON DELETE SET NULL;
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS automation_triggers (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flow_id             UUID NOT NULL REFERENCES automation_flows(id) ON DELETE CASCADE,
    flow_version_id     UUID NULL REFERENCES automation_flow_versions(id) ON DELETE CASCADE,
    node_id             TEXT NULL,
    kind                TEXT NOT NULL,
    enabled             BOOLEAN NOT NULL DEFAULT TRUE,
    repeat_kind         TEXT NULL,
    interval_seconds    INTEGER NULL,
    cron_expression     TEXT NULL,
    timezone            TEXT NOT NULL DEFAULT 'UTC',
    overlap_policy      TEXT NOT NULL DEFAULT 'skip',
    missed_policy       TEXT NOT NULL DEFAULT 'run_once',
    next_occurrence_at  TIMESTAMPTZ NULL,
    last_occurrence_at  TIMESTAMPTZ NULL,
    created_by          TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT automation_triggers_kind CHECK (kind IN ('inject', 'api')),
    CONSTRAINT automation_triggers_repeat_kind
        CHECK (repeat_kind IS NULL OR repeat_kind IN ('interval', 'cron')),
    CONSTRAINT automation_triggers_overlap
        CHECK (overlap_policy IN ('skip', 'queue', 'cancel_previous')),
    CONSTRAINT automation_triggers_missed
        CHECK (missed_policy IN ('run_once', 'skip')),
    CONSTRAINT automation_triggers_inject_shape CHECK (
        kind <> 'inject'
        OR (
            flow_version_id IS NOT NULL
            AND node_id IS NOT NULL
            AND repeat_kind IN ('interval', 'cron')
            AND next_occurrence_at IS NOT NULL
        )
    ),
    CONSTRAINT automation_triggers_repeat_shape CHECK (
        repeat_kind IS NULL
        OR (repeat_kind = 'interval' AND interval_seconds IS NOT NULL AND interval_seconds > 0)
        OR (repeat_kind = 'cron' AND cron_expression IS NOT NULL AND cron_expression <> '')
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_automation_triggers_inject_node
    ON automation_triggers (flow_version_id, node_id)
    WHERE kind = 'inject';

CREATE INDEX IF NOT EXISTS idx_automation_triggers_due
    ON automation_triggers (enabled, next_occurrence_at)
    WHERE kind = 'inject' AND enabled;

CREATE INDEX IF NOT EXISTS idx_automation_triggers_flow
    ON automation_triggers (flow_id, kind);

CREATE TABLE IF NOT EXISTS automation_runs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flow_id             UUID NOT NULL REFERENCES automation_flows(id),
    flow_version_id     UUID NOT NULL REFERENCES automation_flow_versions(id),
    trigger_id          UUID NULL REFERENCES automation_triggers(id) ON DELETE SET NULL,
    trigger_kind        TEXT NOT NULL,
    trigger_node_id     TEXT NULL,
    actor_id            TEXT NULL,
    status              TEXT NOT NULL,
    inputs              JSONB NOT NULL DEFAULT '{}'::jsonb,
    outputs             JSONB NOT NULL DEFAULT '{}'::jsonb,
    summary             JSONB NOT NULL DEFAULT '{}'::jsonb,
    error               JSONB NULL,
    idempotency_key     TEXT NULL,
    parent_run_id       UUID NULL REFERENCES automation_runs(id),
    cancel_requested_at TIMESTAMPTZ NULL,
    queued_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at          TIMESTAMPTZ NULL,
    ended_at            TIMESTAMPTZ NULL,
    heartbeat_at        TIMESTAMPTZ NULL,
    message_count       BIGINT NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT automation_runs_trigger_kind
        CHECK (trigger_kind IN ('manual', 'schedule', 'api')),
    CONSTRAINT automation_runs_status CHECK (
        status IN ('queued', 'running', 'succeeded', 'partial', 'failed', 'cancelled', 'interrupted')
    ),
    CONSTRAINT automation_runs_time_order
        CHECK (ended_at IS NULL OR started_at IS NULL OR ended_at >= started_at),
    CONSTRAINT automation_runs_message_count_non_negative CHECK (message_count >= 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_automation_runs_trigger_idempotency
    ON automation_runs (trigger_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND trigger_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS uq_automation_runs_manual_idempotency
    ON automation_runs (flow_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND trigger_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_automation_runs_flow_created
    ON automation_runs (flow_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_automation_runs_active
    ON automation_runs (status, queued_at)
    WHERE status IN ('queued', 'running');

CREATE TABLE IF NOT EXISTS automation_node_runs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id              UUID NOT NULL REFERENCES automation_runs(id) ON DELETE CASCADE,
    node_id             TEXT NOT NULL,
    node_type_id        TEXT NOT NULL,
    node_type_version   INTEGER NOT NULL,
    attempt             INTEGER NOT NULL DEFAULT 1,
    status              TEXT NOT NULL,
    started_at          TIMESTAMPTZ NULL,
    ended_at            TIMESTAMPTZ NULL,
    duration_ms         BIGINT NULL,
    input_message       JSONB NULL,
    output_messages     JSONB NULL,
    output_truncated    BOOLEAN NOT NULL DEFAULT FALSE,
    messages_in         BIGINT NOT NULL DEFAULT 0,
    messages_out        BIGINT NOT NULL DEFAULT 0,
    error               JSONB NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT automation_node_runs_status CHECK (
        status IN ('pending', 'ready', 'running', 'succeeded', 'failed', 'skipped', 'cancelled')
    ),
    CONSTRAINT automation_node_runs_attempt_positive CHECK (attempt >= 1),
    CONSTRAINT automation_node_runs_type_version_positive CHECK (node_type_version > 0),
    CONSTRAINT automation_node_runs_counts_non_negative
        CHECK (messages_in >= 0 AND messages_out >= 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_automation_node_runs_attempt
    ON automation_node_runs (run_id, node_id, attempt);

CREATE INDEX IF NOT EXISTS idx_automation_node_runs_run_started
    ON automation_node_runs (run_id, started_at);

CREATE TABLE IF NOT EXISTS automation_messages (
    id              BIGSERIAL PRIMARY KEY,
    run_id          UUID NOT NULL REFERENCES automation_runs(id) ON DELETE CASCADE,
    node_id         TEXT NULL,
    direction       TEXT NOT NULL,
    output_port     TEXT NULL,
    msg_id          TEXT NOT NULL,
    ts              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    payload         JSONB NULL,
    size_bytes      INTEGER NOT NULL DEFAULT 0,
    truncated       BOOLEAN NOT NULL DEFAULT FALSE,
    error           JSONB NULL,
    CONSTRAINT automation_messages_direction CHECK (direction IN ('in', 'out')),
    CONSTRAINT automation_messages_size_non_negative CHECK (size_bytes >= 0)
);

CREATE INDEX IF NOT EXISTS idx_automation_messages_run_ts
    ON automation_messages (run_id, ts);

CREATE INDEX IF NOT EXISTS idx_automation_messages_run_node_ts
    ON automation_messages (run_id, node_id, ts);

CREATE INDEX IF NOT EXISTS idx_automation_messages_run_msg
    ON automation_messages (run_id, msg_id);

CREATE TABLE IF NOT EXISTS automation_run_logs (
    id          BIGSERIAL PRIMARY KEY,
    run_id      UUID NOT NULL REFERENCES automation_runs(id) ON DELETE CASCADE,
    node_id     TEXT NULL,
    ts          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    level       TEXT NOT NULL,
    message     TEXT NOT NULL,
    fields      JSONB NULL,
    CONSTRAINT automation_run_logs_level
        CHECK (level IN ('trace', 'debug', 'info', 'warn', 'error', 'notice')),
    CONSTRAINT automation_run_logs_message_non_empty CHECK (message <> '')
);

CREATE INDEX IF NOT EXISTS idx_automation_run_logs_run_ts
    ON automation_run_logs (run_id, ts);

CREATE INDEX IF NOT EXISTS idx_automation_run_logs_run_node_ts
    ON automation_run_logs (run_id, node_id, ts);

CREATE TABLE IF NOT EXISTS automation_context (
    scope               TEXT NOT NULL,
    scope_id            TEXT NOT NULL,
    key                 TEXT NOT NULL,
    value               JSONB NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_by_run_id   UUID NULL REFERENCES automation_runs(id) ON DELETE SET NULL,
    PRIMARY KEY (scope, scope_id, key),
    CONSTRAINT automation_context_scope CHECK (scope IN ('flow', 'global')),
    CONSTRAINT automation_context_key_non_empty CHECK (key <> '')
);

CREATE INDEX IF NOT EXISTS idx_automation_context_scope_updated
    ON automation_context (scope, scope_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS automation_audit_events (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id        TEXT NULL,
    action          TEXT NOT NULL,
    target_type     TEXT NOT NULL,
    target_id       UUID NOT NULL,
    flow_id         UUID NULL,
    run_id          UUID NULL,
    outcome         TEXT NOT NULL,
    metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT automation_audit_events_outcome CHECK (outcome IN ('success', 'failure')),
    CONSTRAINT automation_audit_events_action_non_empty CHECK (action <> '')
);

CREATE INDEX IF NOT EXISTS idx_automation_audit_flow_created
    ON automation_audit_events (flow_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_automation_audit_target
    ON automation_audit_events (target_type, target_id);

CREATE INDEX IF NOT EXISTS idx_automation_audit_actor_created
    ON automation_audit_events (actor_id, created_at DESC);
