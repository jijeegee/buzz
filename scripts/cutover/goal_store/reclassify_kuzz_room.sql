-- One-off, after the import: puts the Kuzz room's 33 layer 2 goals under six
-- bundles. Moves keep every `updated_at`; a bundle takes the newest of its
-- goals, so nothing looks recently touched.
\set community '''0eaa0496-74c9-4d86-bccc-c938743932ab'''
\set channel '''3bc405e6-3f14-44fa-a0cb-20f8ba8d879a'''

CREATE TEMP TABLE bundles (id text PRIMARY KEY, title text NOT NULL, order_key bigint NOT NULL);
INSERT INTO bundles VALUES
    ('g_b1a0c3d5e7f90101', '계정·로그인·가입', 1),
    ('g_b1a0c3d5e7f90102', '대화·그룹·친구', 2),
    ('g_b1a0c3d5e7f90103', '에이전트·실행기', 3),
    ('g_b1a0c3d5e7f90104', '앱 성능·업데이트·푸시', 4),
    ('g_b1a0c3d5e7f90105', '서버·보안', 5),
    ('g_b1a0c3d5e7f90106', '브랜드·공개', 6);

CREATE TEMP TABLE moves (goal text PRIMARY KEY, bundle text NOT NULL REFERENCES bundles (id));
INSERT INTO moves VALUES
    ('g_173910a340800f48', 'g_b1a0c3d5e7f90101'),
    ('g_730c1914f80dd6ff', 'g_b1a0c3d5e7f90101'),
    ('g_f7d2a51d10155926', 'g_b1a0c3d5e7f90101'),
    ('g_bfdba208833d29dc', 'g_b1a0c3d5e7f90101'),
    ('g_7a8a717e4ab717ef', 'g_b1a0c3d5e7f90102'),
    ('g_888aa8c1e2531e6c', 'g_b1a0c3d5e7f90102'),
    ('g_166f35ffa7c73118', 'g_b1a0c3d5e7f90102'),
    ('g_bba4d7d071d87361', 'g_b1a0c3d5e7f90102'),
    ('g_86a0ee4dda5e782b', 'g_b1a0c3d5e7f90102'),
    ('g_407d414d004a96b4', 'g_b1a0c3d5e7f90102'),
    ('g_4446dfb3fe53d342', 'g_b1a0c3d5e7f90102'),
    ('g_d5126f443f6e4f9e', 'g_b1a0c3d5e7f90102'),
    ('g_f44b3810351a989c', 'g_b1a0c3d5e7f90102'),
    ('g_e69d2016ad54d3b4', 'g_b1a0c3d5e7f90103'),
    ('g_b17bbc8f84fe4b50', 'g_b1a0c3d5e7f90103'),
    ('g_417d17ced4e47455', 'g_b1a0c3d5e7f90103'),
    ('g_3485b4f9e036682e', 'g_b1a0c3d5e7f90103'),
    ('g_f25d6201a24264cf', 'g_b1a0c3d5e7f90103'),
    ('g_1d43a815301e3a71', 'g_b1a0c3d5e7f90103'),
    ('g_f9c94c68c26bd3e7', 'g_b1a0c3d5e7f90103'),
    ('g_9fb79a63fd9c1703', 'g_b1a0c3d5e7f90103'),
    ('g_fd661a63f5baf65c', 'g_b1a0c3d5e7f90104'),
    ('g_5c44739c8db99369', 'g_b1a0c3d5e7f90104'),
    ('g_0968016767780023', 'g_b1a0c3d5e7f90104'),
    ('g_0a33611b69d35eab', 'g_b1a0c3d5e7f90104'),
    ('g_64bd778cad46f96e', 'g_b1a0c3d5e7f90104'),
    ('g_4c36400e2b299556', 'g_b1a0c3d5e7f90104'),
    ('g_fe3f825e6e2fcfc8', 'g_b1a0c3d5e7f90105'),
    ('g_f9432f4c55badb9a', 'g_b1a0c3d5e7f90105'),
    ('g_ad9cb2ed3881e184', 'g_b1a0c3d5e7f90105'),
    ('g_9a55d41ffc0becdd', 'g_b1a0c3d5e7f90106'),
    ('g_87ec3e06a3d66ac9', 'g_b1a0c3d5e7f90106'),
    ('g_66bc05bd61062140', 'g_b1a0c3d5e7f90106');

INSERT INTO goals (community_id, channel_id, id, parent_id, title, status, order_key, updated_at, created_at)
SELECT root.community_id, root.channel_id, b.id, root.id, b.title, 'open', b.order_key, newest.at, newest.at
FROM bundles b
JOIN goals root
    ON root.community_id = :community::uuid AND root.channel_id = :channel::uuid
    AND root.parent_id IS NULL
JOIN LATERAL (
    SELECT max(g.updated_at) AS at
    FROM goals g JOIN moves m ON m.goal = g.id
    WHERE g.community_id = root.community_id AND g.channel_id = root.channel_id
        AND m.bundle = b.id
) newest ON true;

UPDATE goals g SET parent_id = m.bundle
FROM moves m
JOIN goals root ON root.community_id = :community::uuid AND root.channel_id = :channel::uuid
    AND root.parent_id IS NULL
WHERE g.community_id = root.community_id AND g.channel_id = root.channel_id AND g.id = m.goal
    AND g.parent_id = root.id;

-- A goal the list above doesn't name, or one it names that is gone, stops the
-- whole file: the room changed since this list was written.
DO $$
DECLARE
    moved bigint;
    left_over bigint;
BEGIN
    SELECT count(*) INTO moved
    FROM goals g JOIN moves m ON m.goal = g.id AND g.parent_id = m.bundle;
    SELECT count(*) INTO left_over
    FROM goals g JOIN goals p
        ON p.community_id = g.community_id AND p.channel_id = g.channel_id AND p.id = g.parent_id
    WHERE p.parent_id IS NULL AND g.community_id = '0eaa0496-74c9-4d86-bccc-c938743932ab'
        AND g.channel_id = '3bc405e6-3f14-44fa-a0cb-20f8ba8d879a'
        AND NOT EXISTS (SELECT 1 FROM bundles b WHERE b.id = g.id);
    IF moved <> (SELECT count(*) FROM moves) OR left_over <> 0 THEN
        RAISE EXCEPTION 'room changed: % of % moved, % left under layer 1',
            moved, (SELECT count(*) FROM moves), left_over;
    END IF;
END $$;
