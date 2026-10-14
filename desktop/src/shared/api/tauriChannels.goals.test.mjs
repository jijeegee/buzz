import assert from "node:assert/strict";
import test from "node:test";

// Per-room goals switch: kind:39000 carries ["goals", "off"] only when
// disabled, and older backends omit `goals_enabled` entirely. Both the list
// and detail conversions must treat an absent field as on.

const { fromRawChannel, fromRawChannelDetail } = await import(
  "./tauriChannels.ts"
);

function rawChannel(overrides = {}) {
  return {
    id: "11111111-1111-4111-8111-111111111111",
    name: "general",
    channel_type: "stream",
    visibility: "open",
    description: "",
    topic: null,
    purpose: null,
    member_count: 1,
    member_pubkeys: [],
    last_message_at: null,
    archived_at: null,
    participants: [],
    participant_pubkeys: [],
    is_member: true,
    ttl_seconds: null,
    ttl_deadline: null,
    ...overrides,
  };
}

function rawDetail(overrides = {}) {
  return {
    ...rawChannel(overrides),
    created_by: "ab".repeat(32),
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    topic_set_by: null,
    topic_set_at: null,
    purpose_set_by: null,
    purpose_set_at: null,
    topic_required: false,
    max_members: null,
    nip29_group_id: null,
  };
}

test("absent goals_enabled means goals are on", () => {
  assert.equal(fromRawChannel(rawChannel()).goalsEnabled, true);
  assert.equal(fromRawChannelDetail(rawDetail()).goalsEnabled, true);
});

test("goals_enabled false turns room goals off", () => {
  assert.equal(
    fromRawChannel(rawChannel({ goals_enabled: false })).goalsEnabled,
    false,
  );
  assert.equal(
    fromRawChannelDetail(rawDetail({ goals_enabled: false })).goalsEnabled,
    false,
  );
});

test("goals_enabled true keeps room goals on", () => {
  assert.equal(
    fromRawChannel(rawChannel({ goals_enabled: true })).goalsEnabled,
    true,
  );
});
