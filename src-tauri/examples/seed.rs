//! Developer seed: unlock a real local WeChat account exactly like the UI does, then stop.
//!
//! Usage:
//!   WX_PASS=<64 hex> cargo run --quiet --example seed
//!
//! It writes the verified keys to the app key store and decrypts into the app cache dir,
//! so the running app can open the viewer without a manual paste. Originals are only read.

use std::fs;
use std::path::PathBuf;

use wechat_devify_bridge_lib::wechat::{crypto, decrypt, discover, keys, query};

fn main() {
    let pass = std::env::var("WX_PASS").unwrap_or_else(|_| {
        eprintln!("请通过 WX_PASS 提供 64 位十六进制口令或密钥");
        std::process::exit(2);
    });

    let data = PathBuf::from(std::env::var("WX_APP_DATA").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/Library/Application Support/local.wechat.devifybridge")
    }));
    fs::create_dir_all(&data).expect("创建应用数据目录");

    let candidates = keys::parse_candidates(&pass).expect("解析口令");
    let store_file = keys::store_path(&data);
    let mut store = keys::load(&store_file);
    let scan = discover::scan();

    for account in &scan.accounts {
        let mut matched = 0usize;
        for db in account.dbs.iter().filter(|d| d.encrypted) {
            let Ok(bytes) = fs::read(&db.path) else { continue };
            if bytes.len() < crypto::PAGE_SIZE {
                continue;
            }
            if let Some((key, label)) = keys::resolve_db_key(&bytes[..crypto::PAGE_SIZE], &candidates) {
                let salt = crypto::read_salt(&bytes).unwrap_or_default();
                keys::store_key(&mut store, &account.id, &db.rel, &key, &salt, &label);
                matched += 1;
            } else {
                println!("未解锁：{}", db.rel);
            }
        }
        println!("{} 解锁 {matched}/{} 个加密库", account.id, account.encrypted_count);
    }
    keys::save(&store_file, &store).expect("写入密钥库");
    println!("密钥库：{}", store_file.display());

    for account in &scan.accounts {
        let outcomes = decrypt::decrypt_account(&data, account, &store, false, true);
        let ok = outcomes
            .iter()
            .filter(|o| o.status == "ok" || o.status == "cached")
            .count();
        println!("{} 解密 {ok}/{} 个库", account.id, outcomes.len());
        let cache = decrypt::cache_dir(&data, &account.id);
        match query::sessions(&cache) {
            Ok(sessions) => {
                let groups = sessions.iter().filter(|s| s.is_group).count();
                println!("  会话 {}（群聊 {}）", sessions.len(), groups);
            }
            Err(err) => println!("  读取会话失败：{err}"),
        }
    }
}
