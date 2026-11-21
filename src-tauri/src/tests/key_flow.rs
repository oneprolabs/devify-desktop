use super::*;

fn matched_account(
    account: &str,
    matched: usize,
    already: usize,
    selected: bool,
) -> AccountKeyMatch {
    AccountKeyMatch {
        account: account.to_string(),
        matched,
        already,
        total: matched + already,
        selected,
    }
}

#[test]
fn summary_names_the_selected_account() {
    let message = import_summary(
        Some("acct-a"),
        20,
        &[matched_account("acct-a", 18, 2, true)],
    );
    assert!(message.contains("acct-a"));
    assert!(message.contains("18 个数据库密钥已校验并保存"));
    assert!(message.contains("2 个数据库的现有密钥仍然有效"));
}

#[test]
fn summary_explains_an_account_mismatch() {
    let message = import_summary(
        Some("acct-a"),
        20,
        &[matched_account("acct-b", 20, 0, false)],
    );
    assert!(message.contains("属于账号 acct-b"));
    assert!(message.contains("当前选择的是 acct-a"));
}

#[test]
fn summary_reports_new_keys_even_when_the_selected_account_is_already_valid() {
    let message = import_summary(
        Some("acct-a"),
        20,
        &[
            matched_account("acct-a", 0, 20, true),
            matched_account("acct-b", 20, 0, false),
        ],
    );
    assert!(message.contains("属于账号 acct-b"));
    assert!(!message.contains("无需重复导入"));
}

#[test]
fn summary_reports_already_stored_keys_without_an_error_tone() {
    let message = import_summary(Some("acct-a"), 0, &[matched_account("acct-a", 0, 20, true)]);
    assert!(message.contains("无需重复导入"));
    assert!(!message.contains("没有通过"));
    // A wrong paste must not read as a silent success.
    assert!(message.contains("没有为账号 acct-a 新增密钥"));
    assert!(message.contains("与本机数据库不匹配"));
}

#[test]
fn summary_rejects_when_nothing_matches() {
    let message = import_summary(Some("acct-a"), 0, &[]);
    assert!(message.contains("没有通过账号 acct-a"));
    assert!(message.contains("重新运行提取器"));
}

fn fake_bundle(root: &Path, name: &str) -> PathBuf {
    let bundle = root.join(name);
    fs::create_dir_all(bundle.join("Contents").join("MacOS")).expect("create bundle");
    fs::write(bundle.join("Contents").join("Info.plist"), b"<plist/>").expect("write plist");
    bundle
}

#[test]
fn extractor_resolves_a_bundle_or_the_folder_that_holds_it() {
    let root = env::temp_dir().join(format!("wxkeys-extractor-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dist = root.join("dist");
    fs::create_dir_all(&dist).expect("create dist");
    let bundle = fake_bundle(&dist, EXTRACTOR_BUNDLE);

    assert_eq!(resolve_extractor(&bundle), Some(bundle.clone()));
    // The reference project hands out a `dist` folder, so the folder must work too.
    assert_eq!(resolve_extractor(&dist), Some(bundle.clone()));
    // A folder that is not a bundle and holds no bundle is rejected.
    assert_eq!(resolve_extractor(&root), None);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn only_system_or_user_applications_count_as_installed() {
    assert!(extractor_is_installed(Path::new(
        "/Applications/WeDataKeyExtractor.app"
    )));
    assert!(!extractor_is_installed(Path::new(
        "/tmp/build/dist/WeDataKeyExtractor.app"
    )));
}

#[test]
fn paths_are_quoted_for_the_administrator_copy() {
    assert_eq!(
        shell_single_quote(Path::new("/Applications/WeDataKeyExtractor.app")),
        "'/Applications/WeDataKeyExtractor.app'"
    );
    assert_eq!(
        shell_single_quote(Path::new("/tmp/it's here/WeData.app")),
        r#"'/tmp/it'\''s here/WeData.app'"#
    );
}

/// Manual check against the real machine: run with
/// `cargo test --lib real_extracted_passphrase -- --ignored --nocapture`.
#[test]
#[ignore = "requires local WeChat databases and an extracted passphrase file"]
fn real_extracted_passphrase_imports_and_is_idempotent() {
    let path = saved_key_path();
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing {}: {error}", path.to_string_lossy()));
    let candidates = keys::parse_candidates(&text).expect("passphrase file must parse");
    let scan = discover::scan();
    if scan.accounts.is_empty() {
        eprintln!("no local WeChat accounts; nothing to verify");
        return;
    }
    let selected = scan.accounts[0].id.clone();
    let mut store = KeyStore::default();

    let first = apply_candidates(&mut store, &scan, &candidates, Some(&selected));
    assert!(
        first.matched > 0,
        "passphrase must unlock at least one database"
    );
    assert_eq!(first.already, 0);

    let second = apply_candidates(&mut store, &scan, &candidates, Some(&selected));
    assert_eq!(second.matched, 0, "second import must not rewrite keys");
    assert!(
        second.already > 0,
        "stored keys must re-verify against page 1"
    );
    eprintln!(
        "first: matched={} accounts={:?}; second: already={}",
        first.matched,
        first
            .accounts
            .iter()
            .map(|entry| (&entry.account, entry.matched))
            .collect::<Vec<_>>(),
        second.already
    );
}

#[test]
fn clearing_keys_also_removes_the_decrypted_snapshot() {
    let root = env::temp_dir().join(format!("wxclear-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cache = decrypt::cache_dir(&root, "acct");
    std::fs::create_dir_all(cache.join("message")).unwrap();
    std::fs::create_dir_all(cache.join("session")).unwrap();
    std::fs::write(cache.join("_sync.json"), "{}").unwrap();
    std::fs::write(cache.join("message/message_0.db"), b"x").unwrap();
    std::fs::write(cache.join("session/session.db"), b"x").unwrap();

    // Plausible copy of another account must survive a scoped clear.
    let other = decrypt::cache_dir(&root, "other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("session.db"), b"x").unwrap();

    assert_eq!(clear_account_cache(&root, "acct").unwrap(), 2);
    assert!(!cache.exists());
    assert!(other.exists());

    let _ = std::fs::remove_dir_all(&root);
}
