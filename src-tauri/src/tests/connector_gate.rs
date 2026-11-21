//! Source-level guards: invasive capture stays behind the install gate, while ordinary local
//! database reads and trusted key imports remain available without a signed app bundle.

const LIB: &str = include_str!("../lib.rs");

fn body_of(name: &str) -> &'static str {
    let start = LIB
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let rest = &LIB[start..];
    let end = rest[1..].find("\nfn ").or_else(|| rest[1..].find("\nasync fn ")).unwrap_or(rest.len() - 1);
    &rest[..end + 1]
}

#[test]
fn decrypt_account_is_not_blocked_by_install_detection() {
    let calls: Vec<usize> = LIB.match_indices("decrypt::decrypt_account(").map(|(at, _)| at).collect();
    assert_eq!(calls.len(), 1, "decrypt_account must only be called from read_new_wechat_data");
    let gate = LIB.find("fn read_new_wechat_data(").unwrap();
    assert!(calls[0] > gate && calls[0] - gate < 800);
    let body = body_of("read_new_wechat_data");
    assert!(!body.contains("require_installed("));
}

#[test]
fn key_import_does_not_require_installation_detection() {
    let body = body_of("import_key_text");
    assert!(!body.contains("require_connector_installed("));
    assert!(body.contains("keys::parse_candidates"));
}

#[test]
fn capture_prepare_checks_installation() {
    let body = body_of("wechat_capture_prepare");
    let check = body.find("require_connector_installed(&app").unwrap();
    assert!(check < body.find("capture::prepare(").unwrap());
}

#[test]
fn every_key_source_goes_through_import_key_text() {
    for name in ["wechat_import_keys", "wechat_import_saved_key", "wechat_windows_key_scan", "wechat_capture_start"] {
        assert!(body_of(name).contains("import_key_text("), "{name} bypasses import_key_text");
    }
}

#[test]
fn detection_command_is_registered() {
    let handler = &LIB[LIB.find("generate_handler![").unwrap()..];
    assert!(handler.contains("connector_detect,"));
}
