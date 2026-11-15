-- Migration 0013: Morpheus agent chat.
--
-- Implements specs/032-morpheus-agent-chat/data-model.md.
-- Additive and idempotent. No existing table is altered.

CREATE TABLE IF NOT EXISTS morpheus_conversations (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id                TEXT NOT NULL,
    client_conversation_id  UUID NOT NULL,
    title                   TEXT NOT NULL DEFAULT '',
    next_message_seq        BIGINT NOT NULL DEFAULT 1,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT morpheus_conversations_owner_non_empty CHECK (owner_id <> ''),
    CONSTRAINT morpheus_conversations_title_length CHECK (char_length(title) <= 120),
    CONSTRAINT morpheus_conversations_seq_positive CHECK (next_message_seq > 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_conversations_client
    ON morpheus_conversations (owner_id, client_conversation_id);
CREATE INDEX IF NOT EXISTS idx_morpheus_conversations_owner_updated
    ON morpheus_conversations (owner_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS morpheus_messages (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id     UUID NOT NULL REFERENCES morpheus_conversations(id) ON DELETE CASCADE,
    seq                 BIGINT NOT NULL,
    role                TEXT NOT NULL,
    status              TEXT NOT NULL,
    content             TEXT NOT NULL DEFAULT '',
    client_message_id   TEXT NULL,
    error_code          TEXT NULL,
    error_message       TEXT NULL,
    context_trimmed     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at        TIMESTAMPTZ NULL,
    CONSTRAINT morpheus_messages_role CHECK (role IN ('user', 'agent')),
    CONSTRAINT morpheus_messages_status CHECK (
        status IN ('queued', 'streaming', 'complete', 'stopped', 'failed')
    ),
    CONSTRAINT morpheus_messages_seq_positive CHECK (seq > 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_messages_seq
    ON morpheus_messages (conversation_id, seq);
CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_messages_client
    ON morpheus_messages (conversation_id, client_message_id)
    WHERE client_message_id IS NOT NULL AND role = 'user';
CREATE INDEX IF NOT EXISTS idx_morpheus_messages_conversation_seq
    ON morpheus_messages (conversation_id, seq DESC);

CREATE TABLE IF NOT EXISTS morpheus_turns (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id     UUID NOT NULL REFERENCES morpheus_conversations(id) ON DELETE CASCADE,
    user_message_id     UUID NOT NULL REFERENCES morpheus_messages(id) ON DELETE CASCADE,
    agent_message_id    UUID NOT NULL REFERENCES morpheus_messages(id) ON DELETE CASCADE,
    attempt             INTEGER NOT NULL DEFAULT 1,
    retry_of            UUID NULL REFERENCES morpheus_turns(id) ON DELETE SET NULL,
    status              TEXT NOT NULL,
    surface_session_id  TEXT NULL,
    step_count          INTEGER NOT NULL DEFAULT 0,
    provider_calls      INTEGER NOT NULL DEFAULT 0,
    observation_count   INTEGER NOT NULL DEFAULT 0,
    approve_all_turn    BOOLEAN NOT NULL DEFAULT FALSE,
    limit_reached       TEXT NULL,
    error_code          TEXT NULL,
    error_message       TEXT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at          TIMESTAMPTZ NULL,
    ended_at            TIMESTAMPTZ NULL,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT morpheus_turns_status CHECK (
        status IN ('queued', 'running', 'awaiting_approval', 'awaiting_observation',
                   'awaiting_action_result', 'completed', 'stopped', 'failed')
    ),
    CONSTRAINT morpheus_turns_attempt_positive CHECK (attempt > 0),
    CONSTRAINT morpheus_turns_step_count_non_negative CHECK (step_count >= 0),
    CONSTRAINT morpheus_turns_provider_calls_non_negative CHECK (provider_calls >= 0),
    CONSTRAINT morpheus_turns_observation_count_non_negative CHECK (observation_count >= 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_turns_attempt
    ON morpheus_turns (user_message_id, attempt);
CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_turns_agent_message
    ON morpheus_turns (agent_message_id);
CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_turns_active_conversation
    ON morpheus_turns (conversation_id)
    WHERE status IN ('running', 'awaiting_approval', 'awaiting_observation',
                     'awaiting_action_result');
CREATE INDEX IF NOT EXISTS idx_morpheus_turns_conversation_created
    ON morpheus_turns (conversation_id, created_at);
CREATE INDEX IF NOT EXISTS idx_morpheus_turns_status
    ON morpheus_turns (status);

CREATE TABLE IF NOT EXISTS morpheus_observations (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    turn_id             UUID NOT NULL REFERENCES morpheus_turns(id) ON DELETE CASCADE,
    status              TEXT NOT NULL DEFAULT 'requested',
    surface_session_id  TEXT NULL,
    width               INTEGER NULL,
    height              INTEGER NULL,
    image               BYTEA NULL,
    image_mime          TEXT NULL,
    image_bytes         INTEGER NOT NULL DEFAULT 0,
    dom_hash            TEXT NULL,
    mask_manifest       JSONB NULL,
    elements            JSONB NULL,
    element_count       INTEGER NOT NULL DEFAULT 0,
    mask_count          INTEGER NOT NULL DEFAULT 0,
    requested_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    captured_at         TIMESTAMPTZ NULL,
    expires_at          TIMESTAMPTZ NOT NULL,
    deleted_at          TIMESTAMPTZ NULL,
    CONSTRAINT morpheus_observations_status CHECK (
        status IN ('requested', 'received', 'consumed', 'expired', 'cancelled')
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_observations_pending
    ON morpheus_observations (turn_id)
    WHERE status = 'requested';
CREATE INDEX IF NOT EXISTS idx_morpheus_observations_turn
    ON morpheus_observations (turn_id, requested_at DESC);
CREATE INDEX IF NOT EXISTS idx_morpheus_observations_expiry
    ON morpheus_observations (expires_at)
    WHERE image IS NOT NULL;

CREATE TABLE IF NOT EXISTS morpheus_approval_requests (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    turn_id         UUID NOT NULL REFERENCES morpheus_turns(id) ON DELETE CASCADE,
    action_kind     TEXT NOT NULL,
    action_id       UUID NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending',
    decision_mode   TEXT NULL,
    context_hash    TEXT NOT NULL,
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    decided_at      TIMESTAMPTZ NULL,
    CONSTRAINT morpheus_approval_requests_kind CHECK (action_kind IN ('tool', 'ui_action')),
    CONSTRAINT morpheus_approval_requests_status CHECK (
        status IN ('pending', 'approved', 'denied', 'expired')
    ),
    CONSTRAINT morpheus_approval_requests_mode CHECK (
        decision_mode IS NULL OR decision_mode IN ('individual', 'turn', 'standing')
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_approval_action
    ON morpheus_approval_requests (action_kind, action_id);
CREATE INDEX IF NOT EXISTS idx_morpheus_approval_turn_status
    ON morpheus_approval_requests (turn_id, status);
CREATE INDEX IF NOT EXISTS idx_morpheus_approval_pending_expiry
    ON morpheus_approval_requests (expires_at)
    WHERE status = 'pending';

CREATE TABLE IF NOT EXISTS morpheus_steps (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    turn_id         UUID NOT NULL REFERENCES morpheus_turns(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    kind            TEXT NOT NULL,
    status          TEXT NOT NULL,
    summary         TEXT NOT NULL DEFAULT '',
    ref_id          UUID NULL,
    payload         JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ NULL,
    CONSTRAINT morpheus_steps_kind CHECK (
        kind IN ('reasoning', 'tool', 'ui_action', 'observation', 'answer', 'notice')
    ),
    CONSTRAINT morpheus_steps_status CHECK (
        status IN ('started', 'completed', 'failed', 'skipped')
    ),
    CONSTRAINT morpheus_steps_seq_positive CHECK (seq > 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_steps_seq
    ON morpheus_steps (turn_id, seq);

CREATE TABLE IF NOT EXISTS morpheus_tool_calls (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    turn_id             UUID NOT NULL REFERENCES morpheus_turns(id) ON DELETE CASCADE,
    provider_call_id    TEXT NOT NULL,
    tool_id             TEXT NOT NULL,
    tool_version        INTEGER NOT NULL,
    arguments           JSONB NOT NULL DEFAULT '{}'::jsonb,
    arguments_summary   TEXT NOT NULL DEFAULT '',
    risk                TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'proposed',
    result_summary      TEXT NULL,
    result_truncated    BOOLEAN NOT NULL DEFAULT FALSE,
    error_code          TEXT NULL,
    error_message       TEXT NULL,
    approval_id         UUID NULL REFERENCES morpheus_approval_requests(id) ON DELETE SET NULL,
    started_at          TIMESTAMPTZ NULL,
    completed_at        TIMESTAMPTZ NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT morpheus_tool_calls_risk CHECK (risk IN ('read', 'write', 'destructive')),
    CONSTRAINT morpheus_tool_calls_status CHECK (
        status IN ('proposed', 'awaiting_approval', 'approved', 'denied', 'running',
                   'succeeded', 'failed', 'cancelled', 'expired')
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_tool_calls_provider
    ON morpheus_tool_calls (turn_id, provider_call_id);
CREATE INDEX IF NOT EXISTS idx_morpheus_tool_calls_turn
    ON morpheus_tool_calls (turn_id, created_at);

CREATE TABLE IF NOT EXISTS morpheus_ui_actions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    turn_id         UUID NOT NULL REFERENCES morpheus_turns(id) ON DELETE CASCADE,
    observation_id  UUID NULL REFERENCES morpheus_observations(id) ON DELETE SET NULL,
    kind            TEXT NOT NULL,
    target_ref      TEXT NULL,
    target_label    TEXT NOT NULL DEFAULT '',
    value_summary   TEXT NULL,
    state_hash      TEXT NULL,
    status          TEXT NOT NULL DEFAULT 'proposed',
    result_summary  TEXT NULL,
    error_code      TEXT NULL,
    approval_id     UUID NULL REFERENCES morpheus_approval_requests(id) ON DELETE SET NULL,
    started_at      TIMESTAMPTZ NULL,
    completed_at    TIMESTAMPTZ NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT morpheus_ui_actions_kind CHECK (
        kind IN ('navigate', 'click', 'type', 'select', 'press', 'scroll', 'submit')
    ),
    CONSTRAINT morpheus_ui_actions_status CHECK (
        status IN ('proposed', 'awaiting_approval', 'approved', 'denied', 'running',
                   'succeeded', 'failed', 'rejected', 'cancelled', 'expired')
    )
);

CREATE INDEX IF NOT EXISTS idx_morpheus_ui_actions_turn
    ON morpheus_ui_actions (turn_id, created_at);
CREATE INDEX IF NOT EXISTS idx_morpheus_ui_actions_observation
    ON morpheus_ui_actions (observation_id);

CREATE TABLE IF NOT EXISTS morpheus_standing_approvals (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id          TEXT NOT NULL,
    tool_grants       JSONB NOT NULL DEFAULT '{}'::jsonb,
    ui_action_kinds   TEXT[] NOT NULL DEFAULT '{}'::text[],
    granted_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at        TIMESTAMPTZ NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_standing_active
    ON morpheus_standing_approvals (actor_id)
    WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS morpheus_control_leases (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id            TEXT NOT NULL,
    surface_session_id  TEXT NOT NULL,
    conversation_id     UUID NOT NULL REFERENCES morpheus_conversations(id) ON DELETE CASCADE,
    turn_id             UUID NULL REFERENCES morpheus_turns(id) ON DELETE SET NULL,
    heartbeat_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at          TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_morpheus_control_lease_actor
    ON morpheus_control_leases (actor_id);
