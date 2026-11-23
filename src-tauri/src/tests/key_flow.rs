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

fn encrypt_fixture_page(
    page: &[u8; crypto::PAGE_SIZE],
    page_number: u32,
    key: &[u8; crypto::KEY_LEN],
    salt: &[u8; crypto::SALT_LEN],
) -> Vec<u8> {
    use aes::Aes256;
    use cbc::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
    use hmac::{Hmac, Mac};
    use sha2::Sha512;

    let iv = [0x19; crypto::IV_LEN];
    let start = if page_number == 1 { crypto::SALT_LEN } else { 0 };
    let end = crypto::PAGE_SIZE - crypto::RESERVE_LEN;
    let cipher = cbc::Encryptor::<Aes256>::new(key.into(), (&iv).into());
    let ciphertext = cipher.encrypt_padded_vec_mut::<NoPadding>(&page[start..end]);
    let mac_key = crypto::derive_mac_key(key, salt).unwrap();
    let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
    mac.update(&ciphertext);
    mac.update(&iv);
    mac.update(&page_number.to_le_bytes());

    let mut encrypted = Vec::with_capacity(crypto::PAGE_SIZE);
    if page_number == 1 {
        encrypted.extend_from_slice(salt);
    }
    encrypted.extend_from_slice(&ciphertext);
    encrypted.extend_from_slice(&iv);
    encrypted.extend_from_slice(&mac.finalize().into_bytes());
    encrypted
}

#[test]
fn synthetic_key_import_and_snapshot_retry_preserve_last_good_copy() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("synthetic.db");
    let data = temp.path().join("app-data");
    let account_id = "fixture-account";
    let rel = "message/message_0.db";
    let passphrase = [0x42; crypto::KEY_LEN];
    let salt = [0x71; crypto::SALT_LEN];
    let key = crypto::derive_enc_key(&passphrase, &salt).unwrap();

    let mut plain = [[0; crypto::PAGE_SIZE]; 3];
    plain[0][..crypto::SALT_LEN].copy_from_slice(crypto::SQLITE_HEADER);
    plain[0][crypto::SALT_LEN] = 1;
    plain[1][0] = 2;
    plain[2][0] = 3;
    let encrypted: Vec<u8> = plain
        .iter()
        .enumerate()
        .flat_map(|(index, page)| encrypt_fixture_page(page, index as u32 + 1, &key, &salt))
        .collect();
    let original = &encrypted[..2 * crypto::PAGE_SIZE];
    fs::write(&source, original).unwrap();
    let mut scan = ScanResult {
        roots: Vec::new(),
        accounts: vec![discover::Account {
            id: account_id.into(),
            dir: temp.path().to_string_lossy().into(),
            db_count: 1,
            encrypted_count: 1,
            total_bytes: original.len() as u64,
            modified_at: None,
            dbs: vec![discover::DbFile {
                rel: rel.into(),
                path: source.to_string_lossy().into(),
                size: original.len() as u64,
                encrypted: true,
                salt: Some(hex::encode(salt)),
                has_wal: false,
            }],
        }],
        skipped: Vec::new(),
    };

    let mut store = KeyStore::default();
    let wrong_passphrase = serde_json::json!({ "passphrase": "81".repeat(32) });
    let invalid = keys::parse_candidates(&wrong_passphrase.to_string()).unwrap();
    let rejected = apply_candidates(&mut store, &scan, &invalid, Some(account_id));
    assert_eq!(rejected.matched, 0);
    assert!(store.keys.is_empty());

    let right_passphrase = serde_json::json!({ "passphrase": hex::encode(passphrase) });
    let valid = keys::parse_candidates(&right_passphrase.to_string()).unwrap();
    let imported = apply_candidates(&mut store, &scan, &valid, Some(account_id));
    assert_eq!(imported.matched, 1);
    let key_path = keys::store_path(&data);
    keys::save(&key_path, &store).unwrap();
    let mut store = keys::load(&key_path);
    let repeated = apply_candidates(&mut store, &scan, &valid, Some(account_id));
    assert_eq!((repeated.matched, repeated.already), (0, 1));

    let account = &mut scan.accounts[0];
    let cache = decrypt::cache_dir(&data, account_id);
    let cache_file = cache.join(rel);
    let initial = decrypt::decrypt_account(&data, account, &store, false, true);
    assert_eq!(initial[0].status, "ok");
    assert_eq!(initial[0].pages, 2);
    let last_good = plain[..2].concat();
    assert_eq!(fs::read(&cache_file).unwrap(), last_good);
    assert_eq!(fs::read(&source).unwrap(), original);

    for (damaged, failed_page) in [(&[][..], 1), (&original[..crypto::PAGE_SIZE + 91], 2)] {
        fs::write(&source, damaged).unwrap();
        account.dbs[0].size = damaged.len() as u64;
        let failure = decrypt::decrypt_account(&data, account, &store, false, true);
        assert_eq!(failure[0].status, "failed");
        assert_eq!(failure[0].failed_page, Some(failed_page));
        assert_eq!(fs::read(&cache_file).unwrap(), last_good);
        assert!(!cache_file.with_extension("db.part").exists());
    }

    // Older builds could record a partial source as an up-to-date cache entry.
    let meta_path = cache.join("_sync.json");
    let mut old_meta: serde_json::Value = serde_json::from_slice(&fs::read(&meta_path).unwrap()).unwrap();
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    let nanos = modified.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
    old_meta["entries"][rel]["src_size"] = serde_json::json!(account.dbs[0].size);
    old_meta["entries"][rel]["src_mtime"] = serde_json::json!(nanos);
    fs::write(&meta_path, old_meta.to_string()).unwrap();
    let stale = decrypt::decrypt_account(&data, account, &store, false, true);
    assert_eq!(stale[0].status, "failed");
    assert_eq!(stale[0].failed_page, Some(2));
    assert_eq!(fs::read(&cache_file).unwrap(), last_good);

    fs::write(&source, &encrypted).unwrap();
    account.dbs[0].size = encrypted.len() as u64;
    let retry = decrypt::decrypt_account(&data, account, &store, false, true);
    assert_eq!(retry[0].status, "ok");
    assert_eq!(retry[0].pages, 3);
    assert_eq!(fs::read(&cache_file).unwrap(), plain.concat());
}
