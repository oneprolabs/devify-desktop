use super::*;
use serde_json::json;

#[test]
fn reads_detail_nested_in_the_api_envelope() {
    let payload = json!({
        "code": "user_not_found",
        "message": "failed",
        "data": {"detail": "User not found", "code": "user_not_found"}
    });
    assert_eq!(
        devify_error_message(&payload).as_deref(),
        Some("User not found")
    );
}

#[test]
fn reads_validation_field_errors_nested_in_the_envelope() {
    let payload = json!({
        "code": 400,
        "message": "failed",
        "data": {"title": ["不能为空。"], "period": ["周期无效。"]}
    });
    let message = devify_error_message(&payload).expect("message");
    assert!(message.contains("title：不能为空。"), "{message}");
    assert!(message.contains("period：周期无效。"), "{message}");
}

#[test]
fn falls_back_to_plain_top_level_errors() {
    let payload = json!({"detail": "Method \"GET\" not allowed."});
    assert_eq!(
        devify_error_message(&payload).as_deref(),
        Some("Method \"GET\" not allowed.")
    );
}

#[test]
fn ignores_empty_error_payloads() {
    assert_eq!(devify_error_message(&json!({})), None);
}

#[test]
fn falls_back_when_no_reason_is_present() {
    assert_eq!(
        devify_error_text(&json!({}), "服务端拒绝了请求"),
        "服务端拒绝了请求"
    );
}

#[test]
fn points_at_space_selection_when_the_space_is_gone() {
    assert_eq!(
        devify_friendly_error(reqwest::StatusCode::NOT_FOUND, "Space not found."),
        "当前 空间不存在或无权访问，请在「设置 / 账户」重新选择空间。"
    );
    assert_eq!(
        devify_friendly_error(reqwest::StatusCode::NOT_FOUND, "Event not found."),
        "Event not found."
    );
}
