use super::*;

#[test]
fn upload_preview_distinguishes_same_local_id_across_shards() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    for (shard, timestamp, body) in [(0, 100, "first"), (1, 200, "second")] {
        let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db")))
            .unwrap();
        db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,{timestamp},'{body}',1);")).unwrap();
    }
    let result = prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &["person".into()],
        None,
        None,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(result.preview.message_count, 2);
    assert_ne!(
        result.messages[0]["message_key"],
        result.messages[1]["message_key"]
    );
}

#[test]
fn pagination_keeps_same_id_and_second_from_different_shards() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    for shard in [0, 1] {
        let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db")))
            .unwrap();
        db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT); INSERT INTO {table} VALUES (1,100,'test');")).unwrap();
    }
    let first = query::messages(dir.path(), "person", 1, None).unwrap();
    let second = query::messages(
        dir.path(),
        "person",
        1,
        Some(query::MessageCursor::from_message(&first[0])),
    )
    .unwrap();
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].source_shard, second[0].source_shard);
}

#[test]
fn legacy_key_is_reused_only_for_matching_message() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    for (shard, timestamp, server_id) in [(0, 100, 101), (1, 200, 202)] {
        let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db")))
            .unwrap();
        db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,{server_id},{timestamp},'test',1);")).unwrap();
    }
    let mut history = LegacyUploadKeys::new();
    history.insert(
        "person".into(),
        ExistingUploadKeys {
            legacy: HashMap::from([(1, Some(200))]),
            stable: HashSet::new(),
        },
    );
    let prepare = |since, until| {
        prepare_upload_with_voice(
            dir.path(),
            dir.path(),
            dir.path(),
            "account",
            "7",
            "https://example.test",
            &["person".into()],
            since,
            until,
            &BTreeMap::new(),
            &settings::VoiceUploadSettings::default(),
            "text",
            &history,
        )
        .unwrap()
    };
    let both = prepare(None, None);
    let one = prepare(Some(100), Some(150));
    assert_eq!(both.messages[0]["message_key"], "srv:101");
    assert_eq!(both.messages[1]["message_key"], "src:1");
    assert_eq!(
        one.messages[0]["message_key"],
        both.messages[0]["message_key"]
    );
}

#[test]
fn duplicate_server_ids_still_stop_upload() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    for (shard, timestamp) in [(0, 100), (1, 200)] {
        let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db")))
            .unwrap();
        db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT); INSERT INTO {table} VALUES (1,101,{timestamp},'test');")).unwrap();
    }
    let result = prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &["person".into()],
        None,
        None,
        &BTreeMap::new(),
    );
    assert!(matches!(result, Err(error) if error.contains("服务端消息编号存在重复")));
}

#[test]
fn ambiguous_legacy_message_is_rejected_instead_of_misidentified() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    for shard in [0, 1] {
        let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db")))
            .unwrap();
        db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,100,'test',1);")).unwrap();
    }
    let mut history = LegacyUploadKeys::new();
    history.insert(
        "person".into(),
        ExistingUploadKeys {
            legacy: HashMap::from([(1, Some(100))]),
            stable: HashSet::new(),
        },
    );
    let result = prepare_upload_with_voice(
        dir.path(),
        dir.path(),
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &["person".into()],
        None,
        None,
        &BTreeMap::new(),
        &settings::VoiceUploadSettings::default(),
        "text",
        &history,
    );
    assert!(matches!(result, Err(error) if error.contains("旧消息与多个本地记录匹配")));
}

#[test]
fn colliding_media_remains_attached_to_its_own_message() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    for (shard, timestamp, server_id) in [(0, 100, 101), (1, 200, 202)] {
        let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db")))
            .unwrap();
        db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,{server_id},{timestamp},'<msg><img /></msg>',3);")).unwrap();
    }
    let result = prepare_upload_with_voice(
        dir.path(),
        dir.path(),
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &["person".into()],
        None,
        None,
        &BTreeMap::new(),
        &settings::VoiceUploadSettings::default(),
        "images",
        &LegacyUploadKeys::new(),
    )
    .unwrap();
    let image_keys: HashSet<_> = result
        .parts
        .iter()
        .filter(|p| p["kind"] == "image")
        .filter_map(|p| p["message_key"].as_str())
        .collect();
    assert_eq!(image_keys, HashSet::from(["srv:101", "srv:202"]));
}

#[test]
fn upload_reads_image_from_account_not_database_cache() {
    let cache = tempfile::tempdir().unwrap();
    let account = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cache.path().join("session")).unwrap();
    std::fs::create_dir_all(cache.path().join("message")).unwrap();
    let sessions = rusqlite::Connection::open(cache.path().join("session/session.db")).unwrap();
    sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
    let timestamp = 1_762_000_000i64;
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    let db = rusqlite::Connection::open(cache.path().join("message/message_0.db")).unwrap();
    db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,101,{timestamp},'<msg><img /></msg>',3);")).unwrap();
    let month = chrono::DateTime::from_timestamp(timestamp, 0)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%Y-%m")
        .to_string();
    let image_dir = account
        .path()
        .join("cache")
        .join(month)
        .join("Message")
        .join(hex::encode(md5::Md5::digest(b"person")))
        .join("Thumb");
    std::fs::create_dir_all(&image_dir).unwrap();
    let bytes = b"\xff\xd8\xfffixture";
    std::fs::write(image_dir.join(format!("1_{timestamp}_thumb.jpg")), bytes).unwrap();
    let result = prepare_upload_with_voice(
        cache.path(),
        cache.path(),
        account.path(),
        "account",
        "7",
        "https://example.test",
        &["person".into()],
        None,
        None,
        &BTreeMap::new(),
        &settings::VoiceUploadSettings::default(),
        "images",
        &LegacyUploadKeys::new(),
    )
    .unwrap();
    let part = result.parts.iter().find(|p| p["kind"] == "image").unwrap();
    let path = part["upload_item_path"].as_str().unwrap();
    assert!(result
        .media_files
        .iter()
        .any(|(name, content)| name == path && content == bytes));
}

#[test]
fn colliding_image_uses_wechat_resource_identity() {
    let cache = tempfile::tempdir().unwrap();
    let account = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cache.path().join("session")).unwrap();
    std::fs::create_dir_all(cache.path().join("message")).unwrap();
    let sessions = rusqlite::Connection::open(cache.path().join("session/session.db")).unwrap();
    sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    for (shard, timestamp, server_id) in [(0, 100, 101), (1, 200, 202)] {
        let db =
            rusqlite::Connection::open(cache.path().join(format!("message/message_{shard}.db")))
                .unwrap();
        db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,{server_id},{timestamp},'<msg><img length=\"10\" /></msg>',3);")).unwrap();
    }
    let resource =
        rusqlite::Connection::open(cache.path().join("message/message_resource.db")).unwrap();
    resource.execute_batch("CREATE TABLE MessageResourceInfo (message_svr_id INTEGER, message_local_id INTEGER, message_create_time INTEGER, packed_info BLOB);").unwrap();
    resource
        .execute(
            "INSERT INTO MessageResourceInfo VALUES (101, 1, 100, ?1)",
            [b"abc12345678901234567890123456789".as_slice()],
        )
        .unwrap();
    let month = chrono::DateTime::from_timestamp(100, 0)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%Y-%m")
        .to_string();
    let image_dir = account
        .path()
        .join("msg/attach")
        .join(hex::encode(md5::Md5::digest(b"person")))
        .join(month)
        .join("Img");
    std::fs::create_dir_all(&image_dir).unwrap();
    let bytes = b"\xff\xd8\xfffixture";
    std::fs::write(
        image_dir.join("abc12345678901234567890123456789.dat"),
        bytes,
    )
    .unwrap();
    let result = prepare_upload_with_voice(
        cache.path(),
        cache.path(),
        account.path(),
        "account",
        "7",
        "https://example.test",
        &["person".into()],
        None,
        None,
        &BTreeMap::new(),
        &settings::VoiceUploadSettings::default(),
        "images",
        &LegacyUploadKeys::new(),
    )
    .unwrap();
    let first = result
        .parts
        .iter()
        .find(|p| p["kind"] == "image" && p["message_key"] == "srv:101")
        .unwrap();
    assert!(first["upload_item_path"].is_string());
    let second = result
        .parts
        .iter()
        .find(|p| p["kind"] == "image" && p["message_key"] == "srv:202")
        .unwrap();
    assert_eq!(second["binding_state"], "missing");
}

/// The WhatsApp prepare step must produce the same envelope the WeChat one does: one
/// conversation record, one message per row with a stable key, and a coverage cursor.
#[test]
fn whatsapp_prepare_builds_the_shared_archive_envelope() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("ChatStorage.sqlite");
    {
        let conn = rusqlite::Connection::open(&source).unwrap();
        conn.execute_batch(
                r#"
                CREATE TABLE ZWACHATSESSION (Z_PK INTEGER PRIMARY KEY, ZCONTACTJID VARCHAR, ZPARTNERNAME VARCHAR,
                    ZLASTMESSAGEDATE TIMESTAMP, ZLASTMESSAGETEXT VARCHAR, ZUNREADCOUNT INTEGER);
                CREATE TABLE ZWAMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCHATSESSION INTEGER, ZISFROMME INTEGER,
                    ZMESSAGETYPE INTEGER, ZMESSAGEDATE TIMESTAMP, ZFROMJID VARCHAR, ZTEXT VARCHAR, ZMEDIAITEM INTEGER,
                    ZSTANZAID VARCHAR, ZGROUPEVENTTYPE INTEGER);
                CREATE TABLE ZWAGROUPMEMBER (ZCHATSESSION INTEGER, ZMEMBERJID VARCHAR, ZCONTACTNAME VARCHAR,
                    ZFIRSTNAME VARCHAR, ZISACTIVE INTEGER);
                CREATE TABLE ZWAPROFILEPUSHNAME (ZJID VARCHAR, ZPUSHNAME VARCHAR);
                CREATE TABLE ZWAMEDIAITEM (Z_PK INTEGER PRIMARY KEY, ZTITLE VARCHAR, ZVCARDNAME VARCHAR,
                    ZLATITUDE FLOAT, ZLONGITUDE FLOAT);
                INSERT INTO ZWACHATSESSION VALUES (1, '120363000000000001@g.us', '项目群', 816590732, NULL, 0);
                INSERT INTO ZWAMESSAGE VALUES (1, 1, 0, 0, 816590670, '8613800000000@s.whatsapp.net', '你好', NULL, 'STANZA-A', 0);
                INSERT INTO ZWAMESSAGE VALUES (2, 1, 1, 1, 816590671, '8615652425800@s.whatsapp.net', '看这个', 1, 'STANZA-B', 0);
                INSERT INTO ZWAMESSAGE VALUES (3, 1, 0, 10, 816590672, '8613800000000@s.whatsapp.net', NULL, NULL, 'STANZA-C', 0);
                INSERT INTO ZWAGROUPMEMBER VALUES (1, '8613800000000@s.whatsapp.net', '', '', 1);
                "#,
            )
            .unwrap();
    }
    std::env::set_var("WECHATIFY_WHATSAPP_DB", &source);
    let app_data = root.path().join("app-data");
    std::fs::create_dir_all(&app_data).unwrap();
    let account = "8615652425800@s.whatsapp.net";
    let prepared = prepare_whatsapp_upload(
        &app_data,
        account,
        "space-1",
        "https://example.test",
        &["120363000000000001@g.us".to_string()],
        None,
        None,
        &BTreeMap::new(),
    );
    std::env::remove_var("WECHATIFY_WHATSAPP_DB");
    let prepared = prepared.unwrap();
    assert_eq!(prepared.conversations.len(), 1);
    assert_eq!(prepared.conversations[0]["kind"], "group");
    assert_eq!(prepared.conversations[0]["self_sender_key"], account);
    // The empty type-10 row is not uploadable; the image keeps its label as text.
    assert_eq!(prepared.messages.len(), 2);
    assert_eq!(prepared.messages[0]["message_key"], "srv:STANZA-A");
    assert_eq!(prepared.messages[0]["direction"], "incoming");
    assert_eq!(prepared.messages[1]["direction"], "outgoing");
    assert_eq!(prepared.messages[1]["message_key"], "srv:STANZA-B");
    assert_eq!(prepared.parts[1]["text"], "[图片]");
    assert_eq!(prepared.preview.message_count, 2);
    assert_eq!(prepared.preview.fingerprint.len(), 64);
    let covered = prepared.covered.get("120363000000000001@g.us").unwrap();
    assert_eq!(covered.local_id, 2);
    assert_eq!(covered.boundary_count, Some(1));
    assert_eq!(prepared.latest_position, Some((1794897871, 2)));
}

#[test]
fn whatsapp_upload_filters_by_message_selection() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("ChatStorage.sqlite");
    {
        let conn = rusqlite::Connection::open(&source).unwrap();
        conn.execute_batch(
                r#"
                CREATE TABLE ZWACHATSESSION (Z_PK INTEGER PRIMARY KEY, ZCONTACTJID VARCHAR, ZPARTNERNAME VARCHAR,
                    ZLASTMESSAGEDATE TIMESTAMP, ZLASTMESSAGETEXT VARCHAR, ZUNREADCOUNT INTEGER);
                CREATE TABLE ZWAMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCHATSESSION INTEGER, ZISFROMME INTEGER,
                    ZMESSAGETYPE INTEGER, ZMESSAGEDATE TIMESTAMP, ZFROMJID VARCHAR, ZTEXT VARCHAR, ZMEDIAITEM INTEGER,
                    ZSTANZAID VARCHAR);
                INSERT INTO ZWACHATSESSION VALUES (1, '8613800000000@s.whatsapp.net', '张杭', 816590671, NULL, 0);
                INSERT INTO ZWAMESSAGE VALUES (1, 1, 0, 0, 816590670, '8613800000000@s.whatsapp.net', '一', NULL, 'A');
                INSERT INTO ZWAMESSAGE VALUES (2, 1, 0, 0, 816590671, '8613800000000@s.whatsapp.net', '二', NULL, 'B');
                "#,
            )
            .unwrap();
    }
    std::env::set_var("WECHATIFY_WHATSAPP_DB", &source);
    let app_data = root.path().join("app-data");
    std::fs::create_dir_all(&app_data).unwrap();
    let selection = BTreeMap::from([(
        "8613800000000@s.whatsapp.net".to_string(),
        vec![settings::MessageRef {
            local_id: 2,
            time: 1794897871,
            source_shard: "whatsapp".into(),
        }],
    )]);
    let prepared = prepare_whatsapp_upload(
        &app_data,
        "8615652425800@s.whatsapp.net",
        "space-1",
        "https://example.test",
        &["8613800000000@s.whatsapp.net".to_string()],
        None,
        None,
        &selection,
    );
    std::env::remove_var("WECHATIFY_WHATSAPP_DB");
    let prepared = prepared.unwrap();
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.parts[0]["text"], "二");
}

#[test]
fn manual_batch_plan_preserves_every_selected_message() {
    let refs = (0..2301)
        .map(|n| settings::MessageRef {
            local_id: n,
            time: n / 3,
            source_shard: format!("message_{}.db", n % 2),
        })
        .collect::<Vec<_>>();
    let scope = BTreeMap::from([("chat".to_string(), refs.clone())]);
    let batches = super::manual_batch_plan(&scope, 1000).unwrap();
    assert_eq!(
        batches
            .iter()
            .map(|batch| batch.values().map(Vec::len).sum::<usize>())
            .collect::<Vec<_>>(),
        vec![1000, 1000, 301]
    );
    assert_eq!(
        batches
            .into_iter()
            .flat_map(|batch| batch.into_values().flatten())
            .collect::<Vec<_>>(),
        refs
    );
    let duplicate = BTreeMap::from([("chat".to_string(), vec![refs[0].clone(), refs[0].clone()])]);
    assert!(super::manual_batch_plan(&duplicate, 1000).is_err());
}

#[test]
fn manual_batches_resume_in_order_and_stop_on_failure() {
    let mut value = settings::AppSettings::default();
    value.active_account_id = "account".into();
    value.devify.active_space_id = Some("7".into());
    let owner = settings::UploadOwner {
        base_url: "https://example.invalid".into(),
        space_id: "7".into(),
        account_id: "account".into(),
        user_id: 1,
        device_id: "device".into(),
    };
    value.upload_queue = (0..3)
        .map(|index| settings::UploadJob {
            id: format!("batch-{index}"),
            account_id: "account".into(),
            space_id: "7".into(),
            owner: Some(owner.clone()),
            create_started: Some(false),
            status: "queued".into(),
            manual_group: Some("group".into()),
            manual_index: index,
            manual_total: 3,
            ..Default::default()
        })
        .collect();
    assert_eq!(
        super::next_manual_batch(&value, &owner).as_deref(),
        Some("batch-0")
    );
    let mut other_user = owner.clone();
    other_user.user_id = 2;
    assert!(super::next_manual_batch(&value, &other_user).is_none());
    assert_eq!(value.upload_queue[0].status, "queued");
    value.upload_queue[0].status = "running".into();
    value.upload_queue[0].manual_fingerprint = Some("saved".into());
    let mut value: settings::AppSettings =
        serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(
        super::next_manual_batch(&value, &owner).as_deref(),
        Some("batch-0")
    );
    value.upload_queue[0].create_started = Some(true);
    assert!(super::next_manual_batch(&value, &owner).is_none());
    value.upload_queue[0].status = "failed".into();
    assert!(super::next_manual_batch(&value, &owner).is_none());
    value.upload_queue[0].status = "completed".into();
    assert_eq!(
        super::next_manual_batch(&value, &owner).as_deref(),
        Some("batch-1")
    );
    value.upload_queue[1].status = "processing".into();
    value.upload_queue[1].batch_id = Some(1);
    assert!(super::next_manual_batch(&value, &owner).is_none());
    value.upload_queue.push(settings::UploadJob {
        id: "old-stopped".into(),
        account_id: "account".into(),
        space_id: "7".into(),
        status: "failed".into(),
        ..Default::default()
    });
    assert!(super::next_manual_batch(&value, &owner).is_none());
    value.devify.active_space_id = Some("8".into());
    assert!(super::next_manual_batch(&value, &owner).is_none());
}

#[test]
fn upload_preview_reflects_real_data_and_detects_changes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let session = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    session.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','测试会话');").unwrap();
    let message = rusqlite::Connection::open(dir.path().join("message/message_0.db")).unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    message.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,100,'one',1),(2,200,'two',1),(3,300,'three',1);")).unwrap();
    let names = vec!["person".to_string()];
    let first = prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &names,
        Some(100),
        Some(300),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(first.preview.message_count, 2);
    assert_eq!(first.preview.sessions[0].message_count, 2);
    let mut picks = BTreeMap::new();
    picks.insert(
        "person".into(),
        vec![settings::MessageRef {
            source_shard: String::new(),
            local_id: 2,
            time: 200,
        }],
    );
    let one = prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &names,
        Some(100),
        Some(300),
        &picks,
    )
    .unwrap();
    assert_eq!(one.preview.message_count, 1);
    assert_eq!(one.parts[0]["text"], "two");
    assert_ne!(one.preview.fingerprint, first.preview.fingerprint);
    picks.insert(
        "person".into(),
        vec![settings::MessageRef {
            source_shard: String::new(),
            local_id: 3,
            time: 300,
        }],
    );
    assert!(prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &names,
        Some(100),
        Some(300),
        &picks
    )
    .is_err());
    let unchanged = prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &names,
        Some(100),
        Some(300),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(first.preview.fingerprint, unchanged.preview.fingerprint);
    message
        .execute(
            &format!("UPDATE {table} SET message_content='edited' WHERE local_id=1"),
            [],
        )
        .unwrap();
    let edited = prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &names,
        Some(100),
        Some(300),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_ne!(first.preview.fingerprint, edited.preview.fingerprint);
    assert!(verify_upload_preview(
        Some(&first.preview.fingerprint),
        &unchanged.preview.fingerprint
    )
    .is_ok());
    assert!(verify_upload_preview(
        Some(&first.preview.fingerprint),
        &edited.preview.fingerprint
    )
    .is_err());
}

#[test]
fn upload_package_carries_conversation_avatars() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    std::fs::create_dir_all(dir.path().join("head_image")).unwrap();
    let session = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    session
        .execute_batch(
            "CREATE TABLE Session (username TEXT, display_name TEXT); \
                 INSERT INTO Session VALUES ('person','测试会话');",
        )
        .unwrap();
    let message = rusqlite::Connection::open(dir.path().join("message/message_0.db")).unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    message
            .execute_batch(&format!(
                "CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); \
                 INSERT INTO {table} VALUES (1,100,'hello',1);"
            ))
            .unwrap();
    let head = rusqlite::Connection::open(dir.path().join("head_image/head_image.db")).unwrap();
    head.execute_batch(
            "CREATE TABLE head_image (username TEXT PRIMARY KEY, md5 TEXT, image_buffer BLOB, update_time INTEGER);",
        )
        .unwrap();
    let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
    head.execute(
        "INSERT INTO head_image VALUES ('person','md5',?1,0)",
        [&png],
    )
    .unwrap();

    let prepared = prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &["person".to_string()],
        Some(0),
        Some(1000),
        &BTreeMap::new(),
    )
    .unwrap();

    assert_eq!(prepared.avatar_files.len(), 1);
    assert!(prepared.avatar_files[0].0.starts_with("media/avatars/"));
    assert!(prepared.avatar_files[0].0.ends_with(".png"));
    assert_eq!(prepared.avatar_files[0].1, png);
    assert!(prepared
        .avatars
        .iter()
        .any(|row| row["scope"] == "conversation" && row["conversation_id"] == "person"));
}

#[test]
fn upload_package_includes_cached_voice_transcript_without_audio_bytes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let session = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    session
            .execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','测试会话');")
            .unwrap();
    let message = rusqlite::Connection::open(dir.path().join("message/message_0.db")).unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    message
            .execute_batch(&format!(
                "CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,4242,100,'<msg><voicemsg voicelength=\\\"2300\\\" /></msg>',34);"
            ))
            .unwrap();
    let media = rusqlite::Connection::open(dir.path().join("message/media_0.db")).unwrap();
    media
            .execute_batch("CREATE TABLE VoiceInfo (svr_id INTEGER, voice_data BLOB); INSERT INTO VoiceInfo VALUES (4242, X'02');")
            .unwrap();
    voice::save_transcripts(
        dir.path(),
        &BTreeMap::from([(
            voice::transcript_key("account", 4242),
            voice::TranscriptEntry {
                text: "明天下午三点开会".into(),
                engine: "test".into(),
                model: "fixture".into(),
                created_at: 1,
            },
        )]),
    )
    .unwrap();

    let prepared = prepare_upload_with_voice(
        dir.path(),
        dir.path(),
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &["person".into()],
        None,
        None,
        &BTreeMap::new(),
        &settings::VoiceUploadSettings {
            auto_transcribe: true,
            upload_transcripts: true,
        },
        "images",
        &LegacyUploadKeys::new(),
    )
    .unwrap();

    assert!(prepared
        .parts
        .iter()
        .any(|part| { part["kind"] == "audio" && part["binding_state"] == "missing" }));
    assert!(prepared.parts.iter().any(|part| {
        part["kind"] == "text"
            && part["part_key"] == "voice-transcript-4242"
            && part["text"] == "明天下午三点开会"
            && part["source_header"]["engine"] == "test"
    }));
    assert!(prepared.voice_pending.is_empty());
}

#[test]
fn upload_preview_reads_a_shared_second_past_the_page_boundary() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("session")).unwrap();
    std::fs::create_dir_all(dir.path().join("message")).unwrap();
    let session = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
    session
        .execute_batch(
            "CREATE TABLE Session (username TEXT); INSERT INTO Session VALUES ('person');",
        )
        .unwrap();
    let message = rusqlite::Connection::open(dir.path().join("message/message_0.db")).unwrap();
    let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
    message.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<501) INSERT INTO {table} SELECT x,100,'text',1 FROM n;")).unwrap();
    // 501 messages inside one second: the cursor must walk the whole second instead of
    // giving up with "分页遇到相同时间戳".
    let prepared = prepare_upload(
        dir.path(),
        "account",
        "7",
        "https://example.test",
        &["person".into()],
        None,
        None,
        &BTreeMap::new(),
    )
    .expect("分页必须读完整秒");
    assert_eq!(prepared.preview.message_count, 501);
    assert_eq!(prepared.messages.len(), 501);
    let ids: HashSet<_> = prepared
        .messages
        .iter()
        .map(|message| message["source_message_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids.len(), 501);
}

#[test]
fn upload_scope_uses_selected_accounts_own_allowlist() {
    let mut settings = settings::AppSettings::default();
    settings.active_account_id = "account-a".into();
    settings.devify.active_space_id = Some("7".into());
    settings
        .chat
        .allowed_usernames
        .insert("account-a".into(), vec!["chat-a".into()]);
    settings
        .chat
        .allowed_usernames
        .insert("account-b".into(), vec!["chat-b".into()]);
    assert!(validate_upload_scope(&settings, "account-b", "7", &["chat-b".into()]).is_ok());
    assert!(validate_upload_scope(&settings, "account-b", "7", &["chat-a".into()]).is_err());
    assert!(validate_upload_scope(&settings, "", "7", &["chat-b".into()]).is_err());
    assert!(validate_upload_scope(&settings, "account-b", "8", &["chat-b".into()]).is_err());
    settings
        .chat
        .allowed_usernames
        .insert("../other".into(), vec!["chat-b".into()]);
    assert!(validate_upload_scope(&settings, "../other", "7", &["chat-b".into()]).is_err());
}

#[test]
fn individual_selection_is_exact_and_rejects_missing_or_ambiguous_messages() {
    let message = |local_id, time, text: &str| MessageItem {
        server_id: 0,
        source_shard: String::new(),
        local_id,
        time,
        text: text.into(),
        sender: None,
        sender_name: None,
        kind: 1,
        is_self: false,
    };
    let all = vec![message(1, 100, "a"), message(2, 200, "b")];
    let refs = vec![settings::MessageRef {
        source_shard: String::new(),
        local_id: 2,
        time: 200,
    }];
    let selected = filter_upload_messages(all.clone(), Some(&refs)).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].text, "b");
    assert!(filter_upload_messages(
        all.clone(),
        Some(&[settings::MessageRef {
            source_shard: String::new(),
            local_id: 3,
            time: 300
        }])
    )
    .is_err());
    assert!(filter_upload_messages(
        vec![all[0].clone(), all[0].clone()],
        Some(&[settings::MessageRef {
            source_shard: String::new(),
            local_id: 1,
            time: 100
        }])
    )
    .is_err());
    assert!(filter_upload_messages(all, Some(&[])).is_err());
}

#[test]
fn upload_fingerprint_changes_with_content_and_scope() {
    let a = serde_json::json!({"account":"a", "messages":[{"id":1,"text":"hello"}]});
    let b = serde_json::json!({"account":"a", "messages":[{"id":1,"text":"changed"}]});
    let c = serde_json::json!({"account":"b", "messages":[{"id":1,"text":"hello"}]});
    assert_eq!(upload_fingerprint(&a), upload_fingerprint(&a));
    assert_ne!(upload_fingerprint(&a), upload_fingerprint(&b));
    assert_ne!(upload_fingerprint(&a), upload_fingerprint(&c));
}

#[test]
fn same_upload_cannot_be_retried_concurrently() {
    let state = UploadState::default();
    let first = begin_retry(&state, "job-1").unwrap();
    assert!(begin_retry(&state, "job-1").is_err());
    assert!(begin_retry(&state, "job-2").is_ok());
    drop(first);
    assert!(begin_retry(&state, "job-1").is_ok());
}
