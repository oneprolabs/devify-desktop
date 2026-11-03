//! Developer probe: decode WeChat voice blobs from the decrypted cache to WAV.
//!
//! Usage:
//!   WX_APP_DATA=<app data dir> cargo run --quiet --example voice_probe -- [count]
//!
//! It reads `message/media_0.db` from the existing decrypted cache (run `examples/seed.rs`
//! first), writes the first `count` clips to `/tmp/aimychats-voice/`, and reports duration.

use std::fs;
use std::path::PathBuf;

use wechat_devify_bridge_lib::wechat::{decrypt, discover, voice};

fn main() {
    let data = PathBuf::from(std::env::var("WX_APP_DATA").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/Library/Application Support/local.wechat.devifybridge")
    }));
    let count: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(5);

    let scan = discover::scan();
    let account = scan.accounts.first().expect("没有发现微信账号");
    let cache = decrypt::cache_dir(&data, &account.id);
    let media = cache.join("message/media_0.db");
    println!("account: {}", account.id);
    println!("media db: {} ({})", media.display(), media.is_file());

    // Read ids first, then the blobs through the shared helper.
    let conn = rusqlite::Connection::open_with_flags(
        &media,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("打开 media_0.db");
    let mut stmt = conn.prepare("SELECT svr_id FROM VoiceInfo").expect("VoiceInfo");
    let ids: Vec<i64> = stmt
        .query_map([], |row| row.get::<_, i64>(0))
        .expect("query")
        .flatten()
        .take(count)
        .collect();
    let blobs = voice::read_voice_blobs(&cache, &ids).expect("read blobs");
    println!("rows: {} / requested {}", blobs.len(), ids.len());

    let out = PathBuf::from("/tmp/aimychats-voice");
    fs::create_dir_all(&out).expect("mkdir");
    for (id, blob) in blobs.iter().take(count) {
        match voice::decode_pcm(blob) {
            Ok(pcm) => {
                let wav = voice::pcm_to_wav(&pcm, 24000);
                let path = out.join(format!("{id}.wav"));
                fs::write(&path, &wav).expect("write wav");
                println!(
                    "{id}: silk {} bytes -> {} samples ({:.2}s) -> {}",
                    blob.len(),
                    pcm.len(),
                    pcm.len() as f64 / 24000.0,
                    path.display()
                );
            }
            Err(error) => println!("{id}: decode failed: {error}"),
        }
    }
}
