//! Locate WeChat 4.x local data without hardcoding container or version names.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

use super::crypto;

#[derive(Serialize, Clone, Debug)]
pub struct DbFile {
    /// Path relative to `db_storage`, using forward slashes.
    pub rel: String,
    pub path: String,
    pub size: u64,
    pub encrypted: bool,
    pub salt: Option<String>,
    pub has_wal: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Account {
    pub id: String,
    pub dir: String,
    pub db_count: usize,
    pub encrypted_count: usize,
    pub total_bytes: u64,
    /// Newest write time of this account's databases (unix seconds). WeChat touches only the
    /// account that is signed in right now, so this is how the UI finds the current account
    /// without asking the user to pick one.
    pub modified_at: Option<u64>,
    pub dbs: Vec<DbFile>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ScanResult {
    pub roots: Vec<String>,
    pub accounts: Vec<Account>,
    pub skipped: Vec<String>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Containers/directories that may hold WeChat data. Discovery, not a fixed path list.
fn candidate_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(home) = home() {
        let containers = home.join("Library/Containers");
        if containers.is_dir() {
            if let Ok(entries) = fs::read_dir(&containers) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if name.contains("wechat") || name.contains("weixin") || name.contains("tencent") {
                        bases.push(entry.path());
                    }
                }
            }
        }
        // WeChat 4.x keeps everything inside its container, so the old `~/Documents` layout is
        // only worth looking at when no container exists. Checking it unconditionally makes
        // macOS ask for「文稿」access the app never actually needs.
        if bases.is_empty() {
            for rel in ["Documents/xwechat_files", "Documents/WeChat Files", "Documents/WeChat"] {
                let candidate = home.join(rel);
                if candidate.is_dir() {
                    bases.push(candidate);
                }
            }
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let candidate = PathBuf::from(appdata).join("Tencent");
        if candidate.is_dir() {
            bases.push(candidate);
        }
    }
    bases.sort();
    bases.dedup();
    bases
}

/// Depth-limited search for `db_storage` directories.
///
/// Symlinks are never followed. WeChat's container `Data` folder contains links named
/// `Desktop`, `Downloads`, `Movies`, `Music` and `Pictures` that point at the real home
/// folders; stat-ing one of them is enough for macOS to ask the user for access to that folder,
/// which this app has no use for. `DirEntry::file_type` reports the link itself, so the walk
/// stays inside the container.
fn find_db_storage(base: &Path, out: &mut Vec<PathBuf>) {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth == 0 {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            if path.file_name().map(|n| n == "db_storage").unwrap_or(false) {
                out.push(path);
                continue;
            }
            walk(&path, depth - 1, out);
        }
    }
    walk(base, 7, out);
}

fn collect_dbs(db_storage: &Path) -> Vec<DbFile> {
    let mut dbs = Vec::new();
    let mut stack = vec![db_storage.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".db") || name.ends_with("-wal.db") || name.ends_with("-shm.db") {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if size < crypto::PAGE_SIZE as u64 {
                continue;
            }
            let header = read_header(&path, crypto::SALT_LEN);
            let salt = crypto::read_salt(&header);
            let rel = path
                .strip_prefix(db_storage)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| name.clone());
            let wal = PathBuf::from(format!("{}-wal", path.to_string_lossy()));
            dbs.push(DbFile {
                rel,
                path: path.to_string_lossy().to_string(),
                size,
                encrypted: salt.is_some(),
                salt: salt.map(hex::encode),
                has_wal: wal.exists(),
            });
        }
    }
    dbs.sort_by(|a, b| a.rel.cmp(&b.rel));
    dbs
}

fn modified_secs(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_secs())
}

fn read_header(path: &Path, len: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; len];
    match fs::File::open(path).and_then(|mut f| f.read_exact(&mut buf).map(|_| ())) {
        Ok(()) => buf,
        Err(_) => Vec::new(),
    }
}

/// Scan every discoverable WeChat account directory.
pub fn scan() -> ScanResult {
    let bases = candidate_bases();
    let mut storage_dirs = Vec::new();
    for base in &bases {
        find_db_storage(base, &mut storage_dirs);
    }
    storage_dirs.sort();
    storage_dirs.dedup();

    let mut accounts = Vec::new();
    let mut skipped = Vec::new();
    for storage in storage_dirs {
        let Some(account_dir) = storage.parent() else {
            continue;
        };
        let id = account_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if id.is_empty() || id == "all_users" {
            skipped.push(storage.to_string_lossy().to_string());
            continue;
        }
        let dbs = collect_dbs(&storage);
        if dbs.is_empty() {
            skipped.push(format!("{}（没有可读的 .db）", storage.to_string_lossy()));
            continue;
        }
        // The WAL file is written even more often than the database it belongs to, so both
        // count when deciding which account was used last.
        let modified_at = dbs
            .iter()
            .filter_map(|db| {
                let db_time = modified_secs(Path::new(&db.path));
                let wal_time = modified_secs(Path::new(&format!("{}-wal", db.path)));
                db_time.max(wal_time)
            })
            .max();
        accounts.push(Account {
            id,
            dir: account_dir.to_string_lossy().to_string(),
            db_count: dbs.len(),
            encrypted_count: dbs.iter().filter(|d| d.encrypted).count(),
            total_bytes: dbs.iter().map(|d| d.size).sum(),
            modified_at,
            dbs,
        });
    }
    accounts.sort_by(|a, b| a.id.cmp(&b.id));
    ScanResult {
        roots: bases.iter().map(|b| b.to_string_lossy().to_string()).collect(),
        accounts,
        skipped,
    }
}
