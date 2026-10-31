//! Developer probe: decrypt a real local WeChat account and print what the query layer sees.
//!
//! Usage:
//!   WX_PASS=<64 hex> cargo run --quiet --example probe -- [keyword] [max_sessions]
//!
//! It writes the decrypted copy to a temporary directory, prints group/session/search/digest
//! diagnostics, then removes that copy. Original databases are only read.

use std::fs;

use wechat_devify_bridge_lib::wechat::{crypto, decrypt, discover, keys, query};

fn main() {
    let pass = std::env::var("WX_PASS").unwrap_or_else(|_| {
        eprintln!("请通过 WX_PASS 提供 64 位十六进制口令");
        std::process::exit(2);
    });
    let keyword = std::env::args().nth(1).unwrap_or_else(|| "会议".to_string());
    let max_sessions: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    let candidates = keys::parse_candidates(&pass).expect("解析口令");
    let scan = discover::scan();
    println!("账号目录：{:?}", scan.roots);
    for account in &scan.accounts {
        println!("账号 {}：{} 个库，加密 {}", account.id, account.db_count, account.encrypted_count);
    }
    let account = scan.accounts.first().expect("没有找到账号");

    let mut store = keys::KeyStore::default();
    let mut matched = 0;
    for db in account.dbs.iter().filter(|d| d.encrypted) {
        let data = fs::read(&db.path).expect("读取数据库首页");
        if data.len() < crypto::PAGE_SIZE {
            continue;
        }
        if let Some((key, label)) = keys::resolve_db_key(&data[..crypto::PAGE_SIZE], &candidates) {
            let salt = crypto::read_salt(&data).unwrap_or_default();
            keys::store_key(&mut store, &account.id, &db.rel, &key, &salt, &label);
            matched += 1;
        } else {
            println!("未解锁：{}", db.rel);
        }
    }
    println!("解锁 {matched}/{} 个加密库", account.encrypted_count);

    // Reuse one cache directory so repeated probes skip decryption (startup sync makes it cheap).
    let work = std::env::temp_dir().join("wx-probe-cache");
    if std::env::var("WX_PROBE_CLEAN").is_ok() {
        let _ = fs::remove_dir_all(&work);
    }
    let outcomes = decrypt::decrypt_account(&work, account, &store, false, true);
    for outcome in &outcomes {
        println!("解密 {:<28} {:<7} 页={:?} {}", outcome.rel, outcome.status, outcome.pages, outcome.error.clone().unwrap_or_default());
    }

    let cache = decrypt::cache_dir(&work, &account.id);
    let sessions = query::sessions(&cache).expect("读取会话");
    let groups: Vec<_> = sessions.iter().filter(|s| s.is_group).collect();
    println!("\n会话 {}，群聊 {}", sessions.len(), groups.len());
    for s in sessions.iter().take(20) {
        println!(
            "  {:<40} {:<24} group={} 成员={:?} 未读={:?} 预览={:?}",
            s.display, s.username, s.is_group, s.member_count, s.unread, s.preview
        );
    }
    println!("\n群聊 Top 10：");
    for s in groups.iter().take(10) {
        println!("  {:<40} {:<34} 成员={:?} 未读={:?}", s.display, s.username, s.member_count, s.unread);
    }

    let report = query::search(&cache, &keyword, true, max_sessions).expect("搜索");
    println!(
        "\n搜索「{keyword}」（仅群聊）：扫描 {}，跳过 {}，命中 {}",
        report.scanned,
        report.skipped,
        report.hits.len()
    );
    for hit in report.hits.iter().take(10) {
        println!("  {:<40} 命中 {}{}", hit.display, hit.hits, if hit.truncated { "+（截断）" } else { "" });
        for snippet in hit.snippets.iter().take(2) {
            println!("      [{}] {}", snippet.time, snippet.text);
        }
    }

    let focus: Vec<String> = if report.hits.is_empty() {
        groups.iter().take(3).map(|s| s.username.clone()).collect()
    } else {
        report
            .hits
            .iter()
            .filter(|h| h.is_group)
            .take(3)
            .map(|h| h.username.clone())
            .collect()
    };
    if !focus.is_empty() {
        let picks = focus;
        let digest = query::digest(&cache, &picks, None, None).expect("整理");
        println!(
            "\n整理：会话 {}，消息 {}，待办 {}，决定 {}，问题 {}，提问 {}，数据 {}",
            digest.sessions, digest.messages, digest.todo, digest.decisions, digest.issues, digest.questions, digest.data
        );
        println!("{}", digest.markdown.chars().take(2000).collect::<String>());
    }

    if std::env::var("WX_PROBE_CLEAN").is_ok() {
        let _ = fs::remove_dir_all(&work);
    }
}
