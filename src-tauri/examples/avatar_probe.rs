//! Developer probe: decrypt a real local WeChat account and print what the avatar store looks
//! like (schema, row count, image sizes, a few MIME headers).
//!
//! Usage:
//!   WX_PASS=<64 hex> cargo run --quiet --example avatar_probe

use std::fs;

use wechat_devify_bridge_lib::wechat::{crypto, decrypt, discover, keys};

fn main() {
    let pass = std::env::var("WX_PASS").unwrap_or_else(|_| {
        eprintln!("请通过 WX_PASS 提供 64 位十六进制口令");
        std::process::exit(2);
    });

    let candidates = keys::parse_candidates(&pass).expect("解析口令");
    let scan = discover::scan();
    let account = scan.accounts.first().expect("没有找到账号");
    println!("账号 {}", account.id);
    for db in account.dbs.iter().filter(|d| d.rel.starts_with("head_image/")) {
        println!("头像库 {}（{} 字节，加密 {}）", db.rel, db.size, db.encrypted);
    }

    let mut store = keys::KeyStore::default();
    for db in account.dbs.iter().filter(|d| d.encrypted) {
        let data = fs::read(&db.path).expect("读取数据库首页");
        if data.len() < crypto::PAGE_SIZE {
            continue;
        }
        if let Some((key, label)) = keys::resolve_db_key(&data[..crypto::PAGE_SIZE], &candidates) {
            let salt = crypto::read_salt(&data).unwrap_or_default();
            keys::store_key(&mut store, &account.id, &db.rel, &key, &salt, &label);
        }
    }

    let work = std::env::temp_dir().join("wx-avatar-probe");
    let _ = fs::remove_dir_all(&work);
    let outcomes = decrypt::decrypt_account(&work, account, &store, false, true);
    for outcome in &outcomes {
        if outcome.rel.starts_with("head_image/") {
            println!("解密 {} → {} {:?}", outcome.rel, outcome.status, outcome.error);
        }
    }

    let cache = decrypt::cache_dir(&work, &account.id);
    let path = cache.join("head_image/head_image.db");
    let conn = rusqlite::Connection::open(&path).expect("打开头像库");
    let mut tables: Vec<String> = Vec::new();
    {
        let mut stmt = conn.prepare("select name, sql from sqlite_master where type='table'").unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)))
            .unwrap();
        for row in rows.flatten() {
            println!("\n表 {}: {}", row.0, row.1.unwrap_or_default());
            tables.push(row.0);
        }
    }

    for table in &tables {
        let count: i64 = conn
            .query_row(&format!("select count(*) from \"{table}\""), [], |row| row.get(0))
            .unwrap_or(-1);
        println!("\n{table}: {count} 行");
        if table != "head_image" {
            continue;
        }
        let mut stmt = conn
            .prepare("select username, length(image_buffer) from head_image order by length(image_buffer) desc limit 8")
            .unwrap();
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))).unwrap();
        for row in rows.flatten() {
            println!("  {} → {} 字节", row.0, row.1);
        }
        let sample: Option<(String, Vec<u8>)> = conn
            .query_row(
                "select username, image_buffer from head_image where image_buffer is not null order by length(image_buffer) desc limit 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        if let Some((username, bytes)) = sample {
            let head: Vec<String> = bytes.iter().take(12).map(|b| format!("{b:02x}")).collect();
            println!("样本 {username} 前 12 字节 {}", head.join(" "));
        }
    }
}
