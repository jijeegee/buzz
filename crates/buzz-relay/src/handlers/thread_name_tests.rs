use super::ingest::{ingest_event, IngestAuth};
use buzz_core::kind::{KIND_STREAM_MESSAGE, KIND_THREAD_NAME};
use nostr::{EventBuilder, Keys, Kind, Tag};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires isolated Postgres with schema/schema.sql"]
async fn thread_name_round_trip_preserves_reply_counts_and_channel_access() {
    let url = crate::test_support::database_url();
    let state = crate::state::tests::test_state_with_database_url(&url).await;
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let host = format!("thread-name-{}.test", Uuid::new_v4());
    state.db.ensure_configured_community(&host).await.unwrap();
    state.db.ensure_future_partitions(1, true).await.unwrap();
    let tenant = crate::tenant::bind_community(&state.db, &host)
        .await
        .unwrap();
    let owner = Keys::generate();
    let outsider = Keys::generate();
    let channel = Uuid::new_v4();
    let other = Uuid::new_v4();
    for id in [channel, other] {
        sqlx::query("INSERT INTO channels(community_id,id,name,visibility,channel_type,created_by) VALUES($1,$2,$2::text,'private','stream',$3)")
            .bind(tenant.community().as_uuid()).bind(id).bind(owner.public_key().as_bytes().as_slice()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO channel_members(community_id,channel_id,pubkey,role) VALUES($1,$2,$3,'owner')")
            .bind(tenant.community().as_uuid()).bind(id).bind(owner.public_key().as_bytes().as_slice()).execute(&pool).await.unwrap();
    }
    let auth = |keys: &Keys| IngestAuth::Nip42 {
        pubkey: keys.public_key(),
        scopes: vec![buzz_auth::Scope::MessagesWrite],
        channel_ids: None,
        conn_id: Uuid::new_v4(),
    };
    let head = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "Task")
        .tags([Tag::parse(["h", &channel.to_string()]).unwrap()])
        .sign_with_keys(&owner)
        .unwrap();
    assert!(
        ingest_event(&state, &tenant, head.clone(), auth(&owner))
            .await
            .unwrap()
            .accepted
    );
    let name = |keys: &Keys, channel: Uuid, content: &str, marked: bool| {
        let mut target = vec!["e".to_string(), head.id.to_hex()];
        if marked {
            target.extend(["".into(), "reply".into()]);
        }
        EventBuilder::new(Kind::Custom(KIND_THREAD_NAME as u16), content)
            .tags([
                Tag::parse(["h", &channel.to_string()]).unwrap(),
                Tag::parse(target).unwrap(),
            ])
            .sign_with_keys(keys)
            .unwrap()
    };
    let named = name(&owner, channel, "작업 이름", false);
    assert!(
        ingest_event(&state, &tenant, named.clone(), auth(&owner))
            .await
            .unwrap()
            .accepted
    );
    let stored = state
        .db
        .get_event_by_id_for_event_write(tenant.community(), named.id.as_bytes())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.event.content, "작업 이름");
    for (event, actor) in [
        (
            name(&owner, channel, "가".repeat(21).as_str(), false),
            &owner,
        ),
        (name(&owner, channel, "marked", true), &owner),
        (name(&owner, other, "wrong channel", false), &owner),
        (name(&outsider, channel, "outsider", false), &outsider),
    ] {
        assert!(ingest_event(&state, &tenant, event, auth(actor))
            .await
            .is_err());
    }
    let cleared = name(&owner, channel, "", false);
    assert!(
        ingest_event(&state, &tenant, cleared.clone(), auth(&owner))
            .await
            .unwrap()
            .accepted
    );
    let counts: Option<(i32, i32)> = sqlx::query_as(
        "SELECT reply_count, descendant_count FROM thread_metadata WHERE community_id=$1 AND event_id=$2",
    )
    .bind(tenant.community().as_uuid())
    .bind(head.id.as_bytes().as_slice())
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(counts.unwrap_or_default(), (0, 0));
    for event in [named, cleared] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM thread_metadata WHERE community_id=$1 AND event_id=$2)",
        )
        .bind(tenant.community().as_uuid())
        .bind(event.id.as_bytes().as_slice())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!exists, "thread name must not be indexed as a reply");
    }
}
