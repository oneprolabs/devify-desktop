//! Source-level guard: new WeChat reads must stay behind the install gate.

const LIB: &str = include_str!("lib.rs");

fn body_of(name: &str) -> &'static str {
    let start = LIB
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let rest = &LIB[start..];
    let end = rest[1..].find("\nfn ").or_else(|| rest[1..].find("\nasync fn ")).unwrap_or(rest.len() - 1);
    &rest[..end + 1]
}

#[test]
fn decrypt_account_is_only_called_through_the_gate() {
    let calls: Vec<usize> = LIB.match_indices("decrypt::decrypt_account(").map(|(at, _)| at).collect();
    assert_eq!(calls.len(), 1, "decrypt_account must only be called from read_new_wechat_data");
    let gate = LIB.find("fn read_new_wechat_data(").unwrap();
    assert!(calls[0] > gate && calls[0] - gate < 800);
    let body = body_of("read_new_wechat_data");
    let check = body.find("require_installed(connectors::ConnectorKind::Wechat").unwrap();
    assert!(check < body.find("decrypt::decrypt_account(").unwrap());
}

#[test]
fn key_import_checks_installation_before_parsing() {
    let body = body_of("import_key_text");
    let check = body.find("require_connector_installed(app, connectors::ConnectorKind::Wechat)?").unwrap();
    assert!(check < body.find("keys::parse_candidates").unwrap());
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
