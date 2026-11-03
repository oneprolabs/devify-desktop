//! Developer probe: resolve attachments for one conversation and optionally export a bundle.
//!
//! Usage:
//!   WX_ACCOUNT=<account> WX_SESSION=<username> \
//!     cargo run --quiet --example media_probe -- [days] [--export]
//!
//! Expects the decrypted cache produced by `examples/seed.rs` or the app.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use wechat_devify_bridge_lib::wechat::{decrypt, discover, media, query};

fn main() {
    let data = PathBuf::from(std::env::var("WX_APP_DATA").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/Library/Application Support/local.wechat.devifybridge")
    }));
    let scan = discover::scan();
    let account = std::env::var("WX_ACCOUNT")
        .ok()
        .and_then(|id| scan.accounts.iter().find(|account| account.id == id).cloned())
        .or_else(|| scan.accounts.first().cloned())
        .expect("没有发现微信账号");
    let root = PathBuf::from(&account.dir);
    let cache = decrypt::cache_dir(&data, &account.id);
    let sessions = query::sessions(&cache).expect("读取会话");
    let username = std::env::var("WX_SESSION").unwrap_or_else(|_| {
        sessions
            .iter()
            .find(|session| session.is_group)
            .map(|session| session.username.clone())
            .expect("没有群聊")
    });
    let days: i64 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(7);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let since = now - days * 86400;

    let raw = query::raw_messages_between(&cache, &username, since, now, 20000).expect("raw messages");
    let keys = media::resolve_image_keys(&root, &query::md5_hex(&username));
    println!("image keys: {:?}", keys.as_ref().map(|keys| &keys.keys));
    let report = media::resolve(&root, &username, &raw, keys.as_ref());
    println!(
        "session {} · {} messages · images {} videos {} files {} voices {} encrypted {} missing {}",
        username,
        raw.len(),
        report.counts.images,
        report.counts.videos,
        report.counts.files,
        report.counts.voices,
        report.counts.encrypted,
        report.counts.missing
    );
    for item in report.items.iter().filter(|item| item.kind == "image").take(5) {
        println!(
            "  img #{} size={:?} editable={} decoded={} preview={} sources={:?}",
            item.local_id, item.size, item.exists, item.decoded, item.preview.is_some(), item.sources
        );
    }
    for item in report.items.iter().filter(|item| item.kind != "image").take(12) {
        println!(
            "  #{} {} {:?} size={:?} exists={} encrypted={} thumb={} duration={:?} note={:?} rel={:?}",
            item.local_id,
            item.kind,
            item.name,
            item.size,
            item.exists,
            item.encrypted,
            item.thumb.is_some(),
            item.duration,
            item.note,
            item.rel_path
        );
    }

    let images: Vec<_> = report.items.iter().filter(|item| item.kind == "image").collect();
    let full = images
        .iter()
        .filter(|item| item.sources.iter().any(|path| !path.ends_with("_t.dat")))
        .count();
    println!("images with a full-size or bubble source: {full}/{}", images.len());

    if let Some(id) = std::env::args()
        .position(|arg| arg == "--open")
        .and_then(|index| std::env::args().nth(index + 1))
        .and_then(|value| value.parse::<i64>().ok())
    {
        if let Some(item) = report.items.iter().find(|item| item.local_id == id) {
            let paths: Vec<PathBuf> = item.sources.iter().map(|rel| root.join(rel)).collect();
            match media::open_image(&data, &account.id, &root, &paths) {
                Ok(path) => println!("opened {} ({} bytes)", path.display(), std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)),
                Err(error) => println!("open failed: {error}"),
            }
        }
    }

    if std::env::args().any(|arg| arg == "--export") {
        let display = sessions
            .iter()
            .find(|session| session.username == username)
            .map(|session| session.display.clone())
            .unwrap_or_else(|| username.clone());
        let out = media::export_bundle(&data, &root, &account.id, &username, &display, since, now)
            .expect("export");
        println!(
            "exported -> {} ({} copied, {} skipped, {} bytes)",
            out.directory, out.copied, out.skipped, out.bytes
        );
    }
}
