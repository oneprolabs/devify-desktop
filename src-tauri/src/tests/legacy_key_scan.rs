use super::*;

fn page(keys: &[(&str, &str)], more: bool) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "list": keys.iter().map(|(key, at)| serde_json::json!({"message_key": key, "occurred_at": at})).collect::<Vec<_>>(),
        "pagination": {"next": if more { serde_json::json!(2) } else { serde_json::Value::Null }},
    }))
}

#[test]
fn merges_pages_in_order() {
    let mut keys = ExistingUploadKeys::default();
    let finished = merge_legacy_pages(
        vec![
            page(
                &[
                    ("src:7", "2026-01-01T00:00:00Z"),
                    ("srv:9", "2026-01-01T00:00:00Z"),
                ],
                true,
            ),
            page(
                &[("local:a:8:2026-01-02T00:00:00Z", "2026-01-02T00:00:00Z")],
                false,
            ),
        ],
        &mut keys,
    )
    .unwrap();
    assert!(finished);
    assert!(keys.legacy.contains_key(&7));
    assert!(keys.stable.contains("srv:9"));
    assert!(keys.stable.contains("local:a:8:2026-01-02T00:00:00Z"));
}

#[test]
fn pages_prefetched_past_the_end_are_ignored() {
    // Page 2 repeats `src:7` with another time; a sequential scan never reads it.
    let mut keys = ExistingUploadKeys::default();
    let finished = merge_legacy_pages(
        vec![
            page(&[("src:7", "2026-01-01T00:00:00Z")], false),
            page(&[("src:7", "2025-01-01T00:00:00Z")], true),
        ],
        &mut keys,
    )
    .unwrap();
    assert!(finished);
    assert_eq!(keys.legacy.get(&7), Some(&Some(1_767_225_600)));
}

#[test]
fn conflicting_legacy_ids_still_stop_the_upload() {
    let mut keys = ExistingUploadKeys::default();
    let error = merge_legacy_pages(
        vec![
            page(&[("src:7", "2026-01-01T00:00:00Z")], true),
            page(&[("src:7", "2026-01-01T00:00:05Z")], false),
        ],
        &mut keys,
    )
    .unwrap_err();
    assert_eq!(error, "云端存在冲突的旧消息标识，无法安全上传。");
}

#[test]
fn a_missing_pagination_block_is_not_guessed() {
    let mut keys = ExistingUploadKeys::default();
    let error =
        merge_legacy_pages(vec![Ok(serde_json::json!({"list": []}))], &mut keys).unwrap_err();
    assert_eq!(error, "云端消息分页信息不完整，无法安全校验旧消息。");
}
