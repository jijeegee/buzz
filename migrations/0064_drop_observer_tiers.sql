-- Remove per-owner observer telemetry tiers (0061). The relay-side tier
-- limits, policy endpoint and buzz-admin command were reverted; plans will be
-- designed again later. 0061 stays in the migration history because deployed
-- databases already applied it. Dropping the table also drops its
-- community_write_fence trigger.
DROP TABLE observer_tiers;
