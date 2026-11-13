//! Limited WAL replay for decrypted databases.
//!
//! This intentionally implements only the *restricted* recovery path: frames are accepted
//! only when the frame salt matches the WAL header and the page HMAC verifies, and only the
//! prefix up to the last commit frame is applied. SQLite's rolling checksum chain is not
//! validated, so a replayed database may still miss the newest committed transaction. The
//! status string says so, and callers must not present this as a complete recovery.

use serde::Serialize;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::crypto;

const WAL_HEADER_LEN: usize = 32;
const FRAME_HEADER_LEN: usize = 24;

#[derive(Serialize, Clone, Debug)]
pub struct WalOutcome {
    pub present: bool,
    pub frames_read: u64,
    pub frames_applied: u64,
    pub final_pages: Option<u64>,
    pub status: String,
    /// Always false for the restricted replay path.
    pub complete: bool,
}

impl WalOutcome {
    fn new(status: impl Into<String>) -> Self {
        WalOutcome {
            present: true,
            frames_read: 0,
            frames_applied: 0,
            final_pages: None,
            status: status.into(),
            complete: false,
        }
    }
}

fn be_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]])
}

/// Replay the committed prefix of `wal_path` into the already-decrypted database file.
pub fn apply(db: &mut File, wal_path: &Path, enc_key: &[u8], mac_key: &[u8]) -> WalOutcome {
    let file = match File::open(wal_path) {
        Ok(file) => file,
        Err(error) => return WalOutcome::new(format!("无法打开 WAL：{error}")),
    };
    let mut reader = BufReader::new(file);

    let mut header = [0u8; WAL_HEADER_LEN];
    if reader.read_exact(&mut header).is_err() {
        return WalOutcome::new("WAL 头部不完整，已跳过");
    }
    let magic = be_u32(&header, 0);
    if magic != 0x377f_0682 && magic != 0x377f_0683 {
        return WalOutcome::new(format!("WAL magic 0x{magic:08x} 不受支持"));
    }
    let page_size = be_u32(&header, 8) as usize;
    if page_size != crypto::PAGE_SIZE {
        return WalOutcome::new(format!("WAL 页大小 {page_size} 不等于 4096，未重放"));
    }
    let wal_salt1 = be_u32(&header, 16);
    let wal_salt2 = be_u32(&header, 20);

    struct Frame {
        pgno: u32,
        page: Vec<u8>,
        commit_pages: u32,
    }

    let mut frames: Vec<Frame> = Vec::new();
    let mut last_commit: Option<usize> = None;
    let mut stop_reason: Option<String> = None;

    loop {
        let mut frame_header = [0u8; FRAME_HEADER_LEN];
        match reader.read_exact(&mut frame_header) {
            Ok(()) => {}
            Err(_) => break,
        }
        let mut page = vec![0u8; page_size];
        if reader.read_exact(&mut page).is_err() {
            stop_reason = Some("WAL 尾部帧不完整（截断）".to_string());
            break;
        }
        let pgno = be_u32(&frame_header, 0);
        let db_size = be_u32(&frame_header, 4);
        let salt1 = be_u32(&frame_header, 8);
        let salt2 = be_u32(&frame_header, 12);

        if salt1 != wal_salt1 || salt2 != wal_salt2 {
            stop_reason = Some("帧 salt 与 WAL 头不一致（疑似上一代残留帧），候选前缀到此结束".to_string());
            break;
        }
        if pgno == 0 {
            stop_reason = Some("帧页码为 0，格式不合法".to_string());
            break;
        }
        if pgno == 1 {
            stop_reason = Some("遇到第 1 页的 WAL 帧，受限重放不支持该布局".to_string());
            break;
        }
        if !crypto::verify_page(&page, mac_key, pgno) {
            stop_reason = Some(format!("第 {pgno} 页 HMAC 校验失败，停止重放"));
            break;
        }
        if db_size != 0 {
            last_commit = Some(frames.len());
        }
        frames.push(Frame {
            pgno,
            page,
            commit_pages: db_size,
        });
    }

    let frames_read = frames.len() as u64;
    let Some(commit_index) = last_commit else {
        let mut outcome = WalOutcome::new(if frames.is_empty() {
            "WAL 中没有可重放的帧".to_string()
        } else {
            "WAL 中没有已提交帧，未重放任何内容".to_string()
        });
        outcome.frames_read = frames_read;
        return outcome;
    };

    let final_pages = frames[commit_index].commit_pages as u64;
    let mut applied = 0u64;
    for frame in &frames[..=commit_index] {
        let offset = (frame.pgno as u64 - 1) * crypto::PAGE_SIZE as u64;
        if db.seek(SeekFrom::Start(offset)).is_err() {
            return WalOutcome::new("写入解密库时定位失败");
        }
        let plain = if frame.pgno == 1 {
            // Unreachable: page-1 frames stop the prefix above.
            continue;
        } else {
            match crypto::decrypt_page(&frame.page, enc_key, frame.pgno) {
                Ok(plain) => plain,
                Err(error) => return WalOutcome::new(error),
            }
        };
        if db.write_all(&plain).is_err() {
            return WalOutcome::new("写入解密库失败");
        }
        applied += 1;
    }

    if final_pages > 0 {
        let target = final_pages * crypto::PAGE_SIZE as u64;
        if db.set_len(target).is_err() {
            return WalOutcome::new("按 dbsize 调整解密库大小时失败");
        }
    }
    let _ = db.flush();

    let mut outcome = WalOutcome::new(match stop_reason {
        Some(reason) => format!(
            "已回填 {applied} 帧（受限恢复；{reason}；未校验 WAL 校验和，可能缺少最新事务）"
        ),
        None => format!(
            "已回填 {applied} 帧（受限恢复；未校验 WAL 校验和，可能缺少最新事务）"
        ),
    });
    outcome.frames_read = frames_read;
    outcome.frames_applied = applied;
    outcome.final_pages = Some(final_pages);
    outcome
}
