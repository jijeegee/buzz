use super::*;

fn preset(id: &str, avatar_url: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "display_name": id,
        "avatar_url": avatar_url,
        "system_prompt": "A customized built-in prompt",
        "is_builtin": true,
        "created_at": "before",
        "updated_at": "before"
    })
}

fn read_records(path: &std::path::Path) -> Vec<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn refresh_builtin_agent_avatars_updates_seeded_presets_and_preserves_customizations() {
    use sha2::{Digest as _, Sha256};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("presets.json");
    let old_fizz = "data:image/png;base64,old-fizz";
    let old_honey = "data:image/png;base64,old-honey";
    let fizz_hash = hex::encode(Sha256::digest(old_fizz.as_bytes()));
    let honey_hash = hex::encode(Sha256::digest(old_honey.as_bytes()));
    let legacy_avatars = [
        LegacyBuiltInAvatar {
            persona_id: "builtin:fizz",
            data_url_sha256: fizz_hash.as_str(),
            sanitized_media_sha256: "",
        },
        LegacyBuiltInAvatar {
            persona_id: "builtin:honey",
            data_url_sha256: honey_hash.as_str(),
            sanitized_media_sha256: "",
        },
    ];
    let mut fizz = preset("builtin:fizz", old_fizz);
    fizz["future_preset_field"] = serde_json::json!("preserved");
    let records = serde_json::json!([
        fizz,
        preset("builtin:honey", "data:image/png;base64,user-customized"),
        // Same seeded avatar, different preset id: not a built-in, untouched.
        preset("custom:fizz", old_fizz),
    ]);
    std::fs::write(&path, serde_json::to_vec_pretty(&records).unwrap()).unwrap();

    refresh_builtin_agent_avatars_in_file(&path, &legacy_avatars, "after");

    let migrated = read_records(&path);
    let new_fizz = crate::managed_agents::built_in_persona_avatar_url("builtin:fizz").unwrap();
    assert_eq!(migrated[0]["avatar_url"], new_fizz);
    assert_eq!(migrated[0]["updated_at"], "after");
    assert_eq!(migrated[0]["future_preset_field"], "preserved");
    assert_eq!(migrated[0]["system_prompt"], "A customized built-in prompt");
    assert_eq!(
        migrated[1]["avatar_url"],
        "data:image/png;base64,user-customized"
    );
    assert_eq!(migrated[1]["updated_at"], "before");
    assert_eq!(migrated[2]["avatar_url"], old_fizz);
    assert_eq!(migrated[2]["updated_at"], "before");

    let once = std::fs::read(&path).unwrap();
    refresh_builtin_agent_avatars_in_file(&path, &legacy_avatars, "later");
    assert_eq!(std::fs::read(&path).unwrap(), once, "second run is a no-op");
}

/// Agent records (no `id`, linked by `persona_id`) are never matched: an
/// agent keeps the avatar it has.
#[test]
fn refresh_builtin_agent_avatars_ignores_agent_shaped_records() {
    use sha2::{Digest as _, Sha256};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("presets.json");
    let old_fizz = "data:image/png;base64,old-fizz";
    let fizz_hash = hex::encode(Sha256::digest(old_fizz.as_bytes()));
    let legacy_avatars = [LegacyBuiltInAvatar {
        persona_id: "builtin:fizz",
        data_url_sha256: fizz_hash.as_str(),
        sanitized_media_sha256: "",
    }];
    let records = serde_json::json!([{
        "name": "fizz-agent",
        "pubkey": "fizz-agent",
        "persona_id": "builtin:fizz",
        "avatar_url": old_fizz,
        "updated_at": "before"
    }]);
    let bytes = serde_json::to_vec_pretty(&records).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    refresh_builtin_agent_avatars_in_file(&path, &legacy_avatars, "after");

    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn current_builtin_agent_avatars_do_not_match_legacy_hashes() {
    use sha2::{Digest as _, Sha256};

    for legacy in LEGACY_BUILTIN_AVATARS {
        let current =
            crate::managed_agents::built_in_persona_avatar_url(legacy.persona_id).unwrap();
        assert_ne!(
            hex::encode(Sha256::digest(current.as_bytes())),
            legacy.data_url_sha256
        );
    }
}

#[test]
fn legacy_avatar_match_checks_every_known_generation_for_a_preset() {
    use sha2::{Digest as _, Sha256};

    let prior_avatar = "data:image/png;base64,prior-fizz";
    let prior_hash = hex::encode(Sha256::digest(prior_avatar.as_bytes()));
    let legacy_avatars = [
        LegacyBuiltInAvatar {
            persona_id: "builtin:fizz",
            data_url_sha256: "older-fizz-hash",
            sanitized_media_sha256: "older-upload-hash",
        },
        LegacyBuiltInAvatar {
            persona_id: "builtin:fizz",
            data_url_sha256: prior_hash.as_str(),
            sanitized_media_sha256: "prior-upload-hash",
        },
    ];

    let by_id = serde_json::json!({ "id": "builtin:fizz", "avatar_url": prior_avatar });
    assert_eq!(
        legacy_avatar_match(&by_id, &legacy_avatars).as_deref(),
        Some("builtin:fizz")
    );

    let by_slug = serde_json::json!({ "slug": "builtin:fizz", "avatar_url": prior_avatar });
    assert_eq!(
        legacy_avatar_match(&by_slug, &legacy_avatars),
        None,
        "only the preset `id` is matched"
    );

    let other_id = serde_json::json!({ "id": "builtin:honey", "avatar_url": prior_avatar });
    assert_eq!(legacy_avatar_match(&other_id, &legacy_avatars), None);
}

#[test]
fn refresh_builtin_agent_avatars_updates_uploaded_media_urls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("presets.json");
    let media_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let different_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let legacy_avatars = [LegacyBuiltInAvatar {
        persona_id: "builtin:fizz",
        data_url_sha256: "not-a-data-url-hash",
        sanitized_media_sha256: media_sha256,
    }];
    let matching_url = format!("https://relay.example/media/{media_sha256}.png?download=1");
    let custom_url = format!("https://relay.example/media/{different_sha256}.png");
    let embedded_hash_url = format!("https://relay.example/media/avatar-{media_sha256}.png");
    let records = serde_json::json!([
        preset("builtin:fizz", &matching_url),
        preset("builtin:fizz", &custom_url),
        preset("builtin:fizz", &embedded_hash_url),
    ]);
    std::fs::write(&path, serde_json::to_vec_pretty(&records).unwrap()).unwrap();

    refresh_builtin_agent_avatars_in_file(&path, &legacy_avatars, "after");

    let migrated = read_records(&path);
    let new_fizz = crate::managed_agents::built_in_persona_avatar_url("builtin:fizz").unwrap();
    assert_eq!(migrated[0]["avatar_url"], new_fizz);
    assert_eq!(migrated[0]["updated_at"], "after");
    assert_eq!(migrated[1]["avatar_url"], custom_url);
    assert_eq!(migrated[1]["updated_at"], "before");
    assert_eq!(migrated[2]["avatar_url"], embedded_hash_url);
    assert_eq!(migrated[2]["updated_at"], "before");
}
