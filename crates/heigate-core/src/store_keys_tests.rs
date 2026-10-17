use super::*;
use crate::store::Channel;

fn channel(store: &Store) -> Channel {
    let mut channel = Channel {
        name: "test".into(),
        api_key: "secret-1".into(),
        ..Default::default()
    };
    store.save(&mut channel).unwrap();
    channel
}

#[test]
fn migration_is_idempotent_and_never_serializes_secrets() {
    let store = Store::open_in_memory().unwrap();
    let ch = channel(&store);
    store.initialize_keys().unwrap();
    store.initialize_keys().unwrap();
    let keys = store.list_keys(ch.id).unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].secret, "secret-1");
    assert!(!serde_json::to_string(&keys).unwrap().contains("secret-1"));
}

#[test]
fn single_key_add_rejects_duplicates_and_preserves_ids() {
    let store = Store::open_in_memory().unwrap();
    let ch = channel(&store);
    let input = |secret: &str| KeyInput {
        name: "other".into(),
        secret: secret.into(),
    };
    assert!(store.add_key(ch.id, &input("***")).is_err());
    assert_eq!(store.list_keys(ch.id).unwrap().len(), 1);
    store.add_key(ch.id, &input("secret-2")).unwrap();
    assert!(store.add_key(ch.id, &input("secret-2")).is_err());
    store.add_key(ch.id, &input("secret-3")).unwrap();
    let keys = store.list_keys(ch.id).unwrap();
    assert_eq!(keys.len(), 3);
    store.delete_key(ch.id, keys[0].id).unwrap();
    assert_eq!(store.get(ch.id).unwrap().api_key, "secret-2");
    assert_eq!(store.list_keys(ch.id).unwrap()[1].id, keys[2].id);
    store.delete_key(ch.id, keys[1].id).unwrap();
    assert!(store.delete_key(ch.id, keys[2].id).is_err());
}

#[test]
fn legacy_edits_work_only_for_single_key() {
    let store = Store::open_in_memory().unwrap();
    let mut ch = channel(&store);
    ch.api_key = "replacement".into();
    store.save(&mut ch).unwrap();
    assert_eq!(store.list_keys(ch.id).unwrap()[0].secret, "replacement");
    store
        .add_key(
            ch.id,
            &KeyInput {
                name: "other".into(),
                secret: "secret-2".into(),
            },
        )
        .unwrap();
    ch.api_key = "ambiguous".into();
    assert!(store.save(&mut ch).is_err());
    assert_eq!(store.get(ch.id).unwrap().api_key, "replacement");
}

#[test]
fn key_edit_preserves_cooldown_and_replacement_increments_generation() {
    let store = Store::open_in_memory().unwrap();
    let ch = channel(&store);
    let key = store.list_keys(ch.id).unwrap().remove(0);
    let until = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    store
        .lock()
        .execute(
            "UPDATE channel_keys SET cooldown_until=? WHERE id=?",
            params![until, key.id],
        )
        .unwrap();
    store
        .patch_key(
            ch.id,
            key.id,
            &KeyPatch {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(store.list_keys(ch.id).unwrap()[0].cooldown_until.is_some());
    store
        .patch_key(
            ch.id,
            key.id,
            &KeyPatch {
                secret: Some("new-secret".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(store.list_keys(ch.id).unwrap()[0].generation, 1);
    assert_eq!(store.get(ch.id).unwrap().api_key, "new-secret");
}

#[test]
fn model_cooldown_is_scoped_to_the_channel_and_model() {
    let store = Store::open_in_memory().unwrap();
    let ch = channel(&store);
    store
        .add_key(
            ch.id,
            &KeyInput {
                name: "other".into(),
                secret: "secret-2".into(),
            },
        )
        .unwrap();
    let keys = store.list_keys(ch.id).unwrap();
    let until = (Utc::now() + chrono::Duration::minutes(2)).to_rfc3339();
    store
        .lock()
        .execute(
            "INSERT INTO channel_key_models(key_id,model,unavailable_until) VALUES(?,?,?)",
            params![keys[0].id, "model-a", until],
        )
        .unwrap();

    assert_eq!(
        store.model_cooling_key_ids(ch.id, "model-a").unwrap(),
        [keys[0].id].into_iter().collect()
    );
    assert!(store
        .model_cooling_key_ids(ch.id, "model-b")
        .unwrap()
        .is_empty());
}

#[test]
fn model_epoch_migration_preserves_existing_cooldowns_and_is_idempotent() {
    let store = Store::open_in_memory().unwrap();
    let ch = channel(&store);
    let key = store.list_keys(ch.id).unwrap()[0].id;
    let until = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    store.lock().execute_batch("DROP TABLE channel_key_models; CREATE TABLE channel_key_models(
        key_id INTEGER NOT NULL REFERENCES channel_keys(id) ON DELETE CASCADE,
        model TEXT NOT NULL, unavailable_until TEXT NOT NULL, rate_limit_count INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY(key_id,model));").unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO channel_key_models VALUES(?,?,?,?)",
            params![key, "gpt-a", until, 2],
        )
        .unwrap();
    for _ in 0..2 {
        store.initialize_keys().unwrap();
    }
    let saved: (String,i64,i64) = store.lock().query_row(
        "SELECT unavailable_until,rate_limit_count,generation FROM channel_key_models WHERE key_id=? AND model='gpt-a'",
        [key],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).unwrap();
    assert_eq!(saved, (until, 2, 0));
}
