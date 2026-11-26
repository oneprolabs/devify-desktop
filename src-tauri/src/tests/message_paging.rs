use super::*;
use rusqlite::Connection;

#[test]
fn summary_history_preserves_same_second_shards_and_rejects_unreadable_shards() {
    let username = "wxid_summary";
    let root = cache_with_messages(username, &[(1, 100)]);
    let dir = root.join("message");
    fs::copy(dir.join("message_0.db"), dir.join("message_1.db")).unwrap();
    let first = query::summary_messages(&root, username, 1, None).unwrap();
    assert_eq!(first[0].source_shard, "message_1.db");
    let second = query::summary_messages(
        &root,
        username,
        1,
        Some(query::MessageCursor::from_message(&first[0])),
    )
    .unwrap();
    assert_eq!(second[0].source_shard, "message_0.db");
    fs::write(dir.join("message_1.db"), b"not a database").unwrap();
    assert!(query::messages(&root, username, 100, None).is_ok());
    let result = query::summary_messages(&root, username, 100, None);
    let _ = fs::remove_dir_all(&root);
    assert!(
        result.is_err(),
        "A full summary must not advance over an unreadable shard"
    );
}

#[test]
fn summary_history_reads_full_text_without_display_placeholders() {
    let username = "wxid_summary_text";
    let root = cache_with_messages(username, &[(1, 100), (2, 101)]);
    let conn = Connection::open(root.join("message/message_0.db")).unwrap();
    let table = format!("Msg_{}", query::md5_hex(username));
    let text = format!("<msg><title>{}</title></msg>", "a".repeat(200));
    conn.execute(
        &format!("UPDATE {table} SET message_content=?1 WHERE local_id=1"),
        [&text],
    )
    .unwrap();
    conn.execute(
        &format!("UPDATE {table} SET message_content=NULL WHERE local_id=2"),
        [],
    )
    .unwrap();
    let rows = query::summary_messages(&root, username, 2, None).unwrap();
    assert_eq!(rows[0].text, "");
    assert_eq!(rows[1].text, text);
    conn.execute(
        &format!("UPDATE {table} SET message_content=x'FF' WHERE local_id=1"),
        [],
    )
    .unwrap();
    let result = query::summary_messages(&root, username, 2, None);
    drop(conn);
    let _ = fs::remove_dir_all(&root);
    assert!(result.is_err());
}

fn cache_with_messages(username: &str, rows: &[(i64, i64)]) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "wc-paging-test-{}-{}",
        std::process::id(),
        rows.len()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("message")).unwrap();
    let table = format!("Msg_{}", query::md5_hex(username));
    let conn = Connection::open(root.join("message").join("message_0.db")).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE \"{table}\" (
                local_id INTEGER PRIMARY KEY,
                server_id INTEGER,
                create_time INTEGER,
                message_content BLOB,
                local_type INTEGER,
                real_sender_id INTEGER
            );"
    ))
    .unwrap();
    for (local_id, time) in rows {
        conn.execute(
                &format!(
                    "INSERT INTO \"{table}\" (local_id, server_id, create_time, message_content, local_type, real_sender_id)
                     VALUES (?1, ?1, ?2, ?3, 1, NULL)"
                ),
                rusqlite::params![local_id, time, format!("消息 {local_id}").into_bytes()],
            )
            .unwrap();
    }
    root
}

#[test]
fn a_full_page_sharing_one_second_no_longer_aborts_the_upload() {
    let username = "123456789@chatroom";
    // 1200 messages, all inside the same second: the old timestamp cursor stopped at
    // 500 and the upload refused to continue.
    let rows: Vec<(i64, i64)> = (1..=1200).map(|id| (id, 1_700_000_000)).collect();
    let root = cache_with_messages(username, &rows);

    let messages = collect_upload_messages(&root, username, None, None, None).unwrap();
    assert_eq!(messages.len(), 1200, "每一条都必须读到");
    let ids: HashSet<i64> = messages.iter().map(|message| message.local_id).collect();
    assert_eq!(ids.len(), 1200, "不能重复");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn paging_continues_inside_a_second_that_straddles_the_page_boundary() {
    let username = "wxid_boundary";
    // Newest 200 share second 200, then 400 older ones share second 100: the cursor
    // lands mid-second in both cases.
    let mut rows: Vec<(i64, i64)> = (1..=200).map(|id| (id, 200)).collect();
    rows.extend((201..=600).map(|id| (id, 100)));
    let root = cache_with_messages(username, &rows);

    let messages = collect_upload_messages(&root, username, None, None, None).unwrap();
    assert_eq!(messages.len(), 600);
    let times: Vec<i64> = messages.iter().map(|message| message.time).collect();
    assert!(
        times.windows(2).all(|pair| pair[0] >= pair[1]),
        "保持从新到旧"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn since_stops_paging_without_dropping_the_boundary_second() {
    let username = "wxid_since";
    let rows: Vec<(i64, i64)> = (1..=900).map(|id| (id, 1_000 + id)).collect();
    let root = cache_with_messages(username, &rows);

    let messages = collect_upload_messages(&root, username, Some(1_500), None, None).unwrap();
    assert_eq!(messages.len(), 401, "只保留 1500 及以后的消息");
    assert!(messages.iter().all(|message| message.time >= 1_500));

    let _ = fs::remove_dir_all(&root);
}
