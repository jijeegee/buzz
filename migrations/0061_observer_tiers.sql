-- Per-owner observer telemetry tier overrides. Owners without a row use the
-- relay's configured default tier (BUZZ_OBSERVER_DEFAULT_TIER); agents inherit
-- their owner's tier.
CREATE TABLE observer_tiers (
    community_id UUID NOT NULL REFERENCES communities(id),
    owner_pubkey BYTEA NOT NULL CHECK (length(owner_pubkey) = 32),
    tier TEXT NOT NULL CHECK (tier IN ('free', 'standard', 'premium')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, owner_pubkey)
);

SELECT attach_community_write_fence('observer_tiers');
