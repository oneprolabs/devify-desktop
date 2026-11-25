//! Local image/document attachments for the desktop reader, not the backup or Agent API.
use super::{clean, columns, core_data_time, data_dir, open};
use base64::Engine;
use rusqlite::{params_from_iter, Connection, ToSql};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const DOCUMENT_BYTES: u64 = 128 * 1024 * 1024;
const READ_ERROR: &str = "无法读取此 WhatsApp 附件。";
const SAVE_ERROR: &str = "无法保存此 WhatsApp 文件。";
const IMAGE_ERROR: &str = "无法解析此 WhatsApp 图片。";
const TOO_LARGE: &str = "WhatsApp 附件超过本机读取上限。";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Available,
    ThumbnailOnly,
    Unavailable,
    Denied,
    Unsupported,
}

impl Status {
    fn error(self) -> String {
        match self {
            Self::Denied => "没有权限读取此附件",
            Self::Unsupported => "此 WhatsApp 版本的附件暂不支持",
            _ => "附件在本机不可用",
        }
        .into()
    }
}

fn source_error(error: io::Error) -> Status {
    match error.kind() {
        io::ErrorKind::NotFound => Status::Unavailable,
        io::ErrorKind::PermissionDenied => Status::Denied,
        _ => Status::Unsupported,
    }
}

#[derive(Serialize)]
pub(crate) struct Description {
    kind: Kind,
    status: Status,
    name: Option<String>,
    file_type: Option<String>,
    size: Option<u64>,
    can_open: bool,
    has_thumbnail: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Image,
    Video,
    File,
}

pub(super) struct Attachment {
    pub(super) time: i64,
    kind: Kind,
    supported: bool,
    original: Option<String>,
    thumbnail: Option<String>,
    title: Option<String>,
    size: Option<u64>,
}

// Query only IDs on the current page, or the one message selected for an action.
pub(super) fn lookup(
    conn: &Connection,
    chat: &str,
    ids: &[i64],
) -> Result<HashMap<i64, Attachment>, String> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    if ids.len() > 100 {
        return Err(READ_ERROR.into());
    }
    let message_columns = columns(conn, "ZWAMESSAGE");
    let media_columns = columns(conn, "ZWAMEDIAITEM");
    let has = |name: &str| media_columns.iter().any(|column| column == name);
    let joined = has("Z_PK") && message_columns.iter().any(|column| column == "ZMEDIAITEM");
    let col = |name: &str| {
        if joined && has(name) {
            format!("a.{name}")
        } else {
            "NULL".to_string()
        }
    };
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT m.Z_PK, m.ZMESSAGEDATE, m.ZMESSAGETYPE, {}, {}, {}, {}
         FROM ZWAMESSAGE m {}
         WHERE m.ZCHATSESSION = (SELECT Z_PK FROM ZWACHATSESSION WHERE ZCONTACTJID = ? LIMIT 1)
           AND m.ZMESSAGETYPE IN (1,2,8) AND m.Z_PK IN ({placeholders})",
        col("ZMEDIALOCALPATH"),
        col("ZTHUMBNAILLOCALPATH"),
        col("ZTITLE"),
        col("ZFILESIZE"),
        if joined {
            "LEFT JOIN ZWAMEDIAITEM a ON a.Z_PK = m.ZMEDIAITEM"
        } else {
            ""
        },
    );
    let mut stmt = conn.prepare(&sql).map_err(|_| READ_ERROR)?;
    let params = std::iter::once(&chat as &dyn ToSql).chain(ids.iter().map(|id| id as &dyn ToSql));
    let rows = stmt
        .query_map(params_from_iter(params), |row| {
            let kind = match row.get::<_, i64>(2)? {
                1 => Kind::Image,
                2 => Kind::Video,
                _ => Kind::File,
            };
            Ok((
                row.get(0)?,
                Attachment {
                    time: core_data_time(row.get(1)?),
                    kind,
                    supported: joined
                        && (has("ZMEDIALOCALPATH")
                            || (kind != Kind::File && has("ZTHUMBNAILLOCALPATH"))),
                    original: row
                        .get::<_, Option<String>>(3)
                        .ok()
                        .flatten()
                        .filter(|s| !s.is_empty()),
                    thumbnail: row
                        .get::<_, Option<String>>(4)
                        .ok()
                        .flatten()
                        .filter(|s| !s.is_empty()),
                    title: row.get::<_, Option<String>>(5).ok().flatten(),
                    size: row
                        .get::<_, Option<i64>>(6)
                        .ok()
                        .flatten()
                        .filter(|&size| size > 0)
                        .map(|size| size as u64),
                },
            ))
        })
        .map_err(|_| READ_ERROR)?;
    rows.collect::<rusqlite::Result<HashMap<_, _>>>()
        .map_err(|_| READ_ERROR.into())
}

pub(super) struct LocalFile {
    file: File,
    path: PathBuf,
    metadata: Metadata,
}

pub(super) fn resolve(container: &Path, relative: &str) -> Result<LocalFile, Status> {
    let relative = Path::new(relative);
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(Status::Unsupported);
    }
    let candidates: &[(&str, &Path)] = if let Ok(tail) = relative.strip_prefix("Message/Media") {
        &[("Message/Media", tail)]
    } else if let Ok(tail) = relative.strip_prefix("Media") {
        &[("Message/Media", tail), ("Media", tail)]
    } else {
        return Err(Status::Unsupported);
    };
    for (prefix, tail) in candidates {
        let relative = Path::new(prefix).join(tail);
        let file = match open_within(container, &relative) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(source_error(error)),
        };
        let metadata = file.metadata().map_err(source_error)?;
        if !metadata.is_file() {
            return Err(Status::Unsupported);
        }
        return Ok(LocalFile {
            file,
            path: container.join(relative),
            metadata,
        });
    }
    Err(Status::Unavailable)
}

#[cfg(unix)]
fn open_within(container: &Path, relative: &Path) -> io::Result<File> {
    use std::{
        ffi::CString,
        os::unix::{
            ffi::OsStrExt,
            fs::OpenOptionsExt,
            io::{AsRawFd, FromRawFd},
        },
    };
    let mut directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(container)?;
    let mut parts = relative.components().peekable();
    while let Some(Component::Normal(part)) = parts.next() {
        let part = CString::new(part.as_bytes())?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if parts.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                libc::O_NONBLOCK
            };
        // SAFETY: Each name is one NUL-terminated component; the directory FD stays open.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), part.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new descriptor owned exclusively by this File.
        directory = unsafe { File::from_raw_fd(fd) };
        if parts.peek().is_none() {
            return Ok(directory);
        }
    }
    Err(io::ErrorKind::InvalidInput.into())
}

#[cfg(not(unix))]
fn open_within(_: &Path, _: &Path) -> io::Result<File> {
    Err(io::ErrorKind::Unsupported.into())
}

impl Attachment {
    fn original(&self, container: &Path) -> Result<LocalFile, Status> {
        if !self.supported {
            return Err(Status::Unsupported);
        }
        resolve(
            container,
            self.original.as_deref().ok_or(Status::Unavailable)?,
        )
    }

    fn thumbnail(&self, container: &Path) -> Result<LocalFile, Status> {
        if let Some(path) = self.thumbnail.as_deref() {
            match resolve(container, path) {
                Err(Status::Unavailable) => {}
                result => return result,
            }
        }
        // Some versions omit the thumbnail column value for a sibling .thumb file.
        let path =
            Path::new(self.original.as_deref().ok_or(Status::Unavailable)?).with_extension("thumb");
        resolve(container, path.to_str().ok_or(Status::Unsupported)?)
    }

    fn file_type(&self) -> Option<String> {
        self.original
            .as_deref()
            .and_then(file_extension)
            .or_else(|| self.title.as_deref().and_then(file_extension))
    }

    fn name(&self) -> Option<String> {
        self.title.as_deref().and_then(safe_name).or_else(|| {
            Path::new(self.original.as_deref()?)
                .file_name()?
                .to_str()
                .and_then(safe_name)
        })
    }

    pub(super) fn describe(&self, container: &Path) -> Description {
        let mut has_thumbnail = false;
        let (status, local_size) = match self.original(container) {
            Ok(file) => {
                has_thumbnail = self.kind == Kind::Video && self.thumbnail(container).is_ok();
                (Status::Available, Some(file.metadata.len()))
            }
            Err(Status::Unavailable) if self.kind != Kind::File => {
                let status = match self.thumbnail(container) {
                    Ok(_) => {
                        has_thumbnail = true;
                        Status::ThumbnailOnly
                    }
                    Err(error) => error,
                };
                (status, None)
            }
            Err(error) => (error, None),
        };
        let file_type = self.file_type();
        Description {
            kind: self.kind,
            status,
            name: (self.kind == Kind::File).then(|| self.name()).flatten(),
            size: self.size.or(local_size),
            can_open: self
                .original
                .as_deref()
                .and_then(file_extension)
                .is_some_and(|ext| self.can_open(&ext)),
            has_thumbnail,
            file_type,
        }
    }

    fn can_open(&self, ext: &str) -> bool {
        match self.kind {
            Kind::Image => false,
            Kind::Video => matches!(ext, "mp4" | "mov" | "m4v" | "3gp" | "webm"),
            Kind::File => document_can_open(ext),
        }
    }
}

fn safe_name(name: &str) -> Option<String> {
    let name = clean(name);
    if name.is_empty()
        || name.starts_with('.')
        || name.len() > 200
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return None;
    }
    Some(name)
}

fn file_extension(path: &str) -> Option<String> {
    let ext = Path::new(path).extension()?.to_str()?;
    (!ext.is_empty() && ext.len() <= 12 && ext.bytes().all(|c| c.is_ascii_alphanumeric()))
        .then(|| ext.to_ascii_lowercase())
}

fn document_can_open(ext: &str) -> bool {
    matches!(
        ext,
        "pdf"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "txt"
            | "csv"
            | "rtf"
            | "odt"
            | "ods"
            | "odp"
            | "jpg"
            | "jpeg"
            | "png"
            | "heic"
            | "webp"
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Preview,
    Image,
    Open,
    Save,
}

pub(crate) fn perform(
    data: &Path,
    scope: &Path,
    account: &str,
    chat: &str,
    id: i64,
    time: i64,
    action: Action,
    downloads: Option<&Path>,
    recheck: impl FnOnce() -> Result<tokio::sync::RwLockReadGuard<'static, ()>, String>,
) -> Result<String, String> {
    let container = data_dir().ok_or(READ_ERROR)?;
    let attachment = lookup(&*open(data)?, chat, &[id])?
        .remove(&id)
        .filter(|attachment| attachment.time == time)
        .ok_or_else(|| Status::Unavailable.error())?;
    let output = match action {
        Action::Preview | Action::Image
            if attachment.kind == Kind::Image
                || (attachment.kind == Kind::Video && matches!(action, Action::Preview)) =>
        {
            let cache = scope.join("whatsapp-image-cache").join(account);
            let path = attachment_image(
                &attachment,
                &container,
                &cache,
                &format!("{chat}\0{id}\0{time}"),
                matches!(action, Action::Preview),
            )?;
            let mut bytes = Vec::new();
            File::open(path)
                .and_then(|file| file.take(IMAGE_BYTES + 1).read_to_end(&mut bytes))
                .map_err(|_| IMAGE_ERROR)?;
            if bytes.len() as u64 > IMAGE_BYTES {
                return Err(TOO_LARGE.into());
            }
            // Scope the bytes to this authorized response rather than vending a replayable asset URL.
            let data = format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            );
            let _identity = recheck()?;
            return Ok(data);
        }
        Action::Open | Action::Save
            if attachment.kind == Kind::File
                || (attachment.kind == Kind::Video && matches!(action, Action::Open)) =>
        {
            let source = attachment.original(&container).map_err(Status::error)?;
            let ext = source.path.to_str().and_then(file_extension);
            let mut name = attachment.name().unwrap_or_else(|| "WhatsApp-file".into());
            if let Some(ext) = ext.as_deref() {
                if file_extension(&name).as_deref() != Some(ext) {
                    name.push('.');
                    name.push_str(ext);
                }
            }
            if matches!(action, Action::Open) {
                if !ext.is_some_and(|ext| attachment.can_open(&ext)) {
                    return Err(if attachment.kind == Kind::File {
                        "此文件类型只能保存，不能直接打开。".into()
                    } else {
                        Status::Unsupported.error()
                    });
                }
                let folder = scope.join("whatsapp-open").join(account);
                let (copy, folder) = document_copy(source, &folder, &name)?;
                let identity = recheck()?;
                let child = Command::new("/usr/bin/open")
                    .arg(&copy)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|_| READ_ERROR)?;
                drop(identity);
                if !wait_for(child) {
                    return Err("无法打开此 WhatsApp 文件。".into());
                }
                // shortcut: external apps may keep files open; add eviction if staged copies grow.
                let _ = folder.keep();
                return Ok(String::new());
            }
            let folder = downloads.ok_or(SAVE_ERROR)?.join("AIMyChats");
            let (copy, folder) = document_copy(source, &folder, &name)?;
            let _identity = recheck()?;
            let _ = folder.keep();
            copy
        }
        _ => return Err(Status::Unsupported.error()),
    };
    Ok(output.to_string_lossy().into_owned())
}

fn attachment_image(
    attachment: &Attachment,
    container: &Path,
    cache: &Path,
    message: &str,
    preview: bool,
) -> Result<PathBuf, String> {
    let render = |source: LocalFile| {
        let key = hex::encode(Sha256::digest(format!(
            "{message}\0{}\0{}\0{:?}\0{preview}",
            source.path.display(),
            source.metadata.len(),
            source.metadata.modified()
        )));
        image(source, cache, &key, preview)
    };
    if attachment.kind == Kind::Video {
        return render(attachment.thumbnail(container).map_err(Status::error)?);
    }
    let original = attachment
        .original(container)
        .map_err(Status::error)
        .and_then(render);
    if preview && original.is_err() {
        if let Ok(thumbnail) = attachment.thumbnail(container) {
            if let Ok(path) = render(thumbnail) {
                return Ok(path);
            }
        }
    }
    original
}

fn copy_bounded(source: LocalFile, output: &mut File, limit: u64) -> Result<(), String> {
    if source.metadata.len() > limit {
        return Err(TOO_LARGE.into());
    }
    let bytes = io::copy(&mut source.file.take(limit + 1), output).map_err(|_| READ_ERROR)?;
    if bytes > limit {
        return Err(TOO_LARGE.into());
    }
    Ok(())
}

fn document_copy(
    source: LocalFile,
    parent: &Path,
    name: &str,
) -> Result<(PathBuf, tempfile::TempDir), String> {
    fs::create_dir_all(parent).map_err(|_| SAVE_ERROR)?;
    let folder = tempfile::Builder::new()
        .prefix("WhatsApp-")
        .tempdir_in(parent)
        .map_err(|_| SAVE_ERROR)?;
    let path = folder.path().join(name);
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| SAVE_ERROR)?;
    copy_bounded(source, &mut output, DOCUMENT_BYTES)?;
    Ok((path, folder))
}

fn image(source: LocalFile, cache: &Path, key: &str, preview: bool) -> Result<PathBuf, String> {
    fs::create_dir_all(cache).map_err(|_| IMAGE_ERROR)?;
    let target = cache.join(format!("{key}.png"));
    if target.is_file() {
        return Ok(target);
    }
    let scratch = tempfile::tempdir_in(cache).map_err(|_| IMAGE_ERROR)?;
    let input = scratch.path().join("source");
    let output = scratch.path().join("image.png");
    copy_bounded(
        source,
        &mut File::create(&input).map_err(|_| IMAGE_ERROR)?,
        IMAGE_BYTES,
    )?;
    let mut properties = tempfile::tempfile().map_err(|_| IMAGE_ERROR)?;
    let probe = Command::new("/usr/bin/sips")
        .args(["-g", "pixelWidth", "-g", "pixelHeight"])
        .arg(&input)
        .stdout(properties.try_clone().map_err(|_| IMAGE_ERROR)?)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| IMAGE_ERROR)?;
    if !wait_for(probe) {
        return Err(IMAGE_ERROR.into());
    }
    use std::io::{Seek, SeekFrom};
    properties
        .seek(SeekFrom::Start(0))
        .map_err(|_| IMAGE_ERROR)?;
    let mut text = String::new();
    properties
        .take(4096)
        .read_to_string(&mut text)
        .map_err(|_| IMAGE_ERROR)?;
    let dimension = |key: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(key)?.trim().parse::<u64>().ok())
    };
    let area = dimension("pixelWidth:")
        .zip(dimension("pixelHeight:"))
        .and_then(|(w, h)| w.checked_mul(h));
    if !area.is_some_and(|pixels| (1..=32_000_000).contains(&pixels)) {
        return Err(IMAGE_ERROR.into());
    }
    let mut process = Command::new("/usr/bin/sips");
    process.args(["-s", "format", "png"]);
    if preview {
        process.args(["-Z", "480"]);
    }
    let result = process
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| IMAGE_ERROR)?;
    if !wait_for(result)
        || !output
            .metadata()
            .is_ok_and(|meta| meta.is_file() && meta.len() <= IMAGE_BYTES)
    {
        return Err(IMAGE_ERROR.into());
    }
    fs::rename(output, &target).map_err(|_| IMAGE_ERROR)?;
    Ok(target)
}

fn wait_for(mut child: Child) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn documents_save_as_separate_copies_without_touching_the_source() {
        let root = tempfile::tempdir().unwrap();
        let media = root.path().join("Message/Media");
        fs::create_dir_all(&media).unwrap();
        let source = media.join("stored.pdf");
        fs::write(&source, b"fixture document").unwrap();
        let dest = root.path().join("downloads");
        let save = || {
            document_copy(
                resolve(root.path(), "Media/stored.pdf").ok().unwrap(),
                &dest,
                "Report.pdf",
            )
            .unwrap()
        };
        let (first, first_dir) = save();
        let (second, second_dir) = save();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"fixture document");
        assert_eq!(fs::read(&second).unwrap(), b"fixture document");
        fs::write(&second, b"changed export").unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"fixture document");
        drop((first_dir, second_dir));

        File::create(&source)
            .unwrap()
            .set_len(DOCUMENT_BYTES + 1)
            .unwrap();
        let oversized = resolve(root.path(), "Media/stored.pdf").ok().unwrap();
        assert_eq!(
            document_copy(oversized, &dest, "Report.pdf").unwrap_err(),
            TOO_LARGE
        );
        assert!(fs::read_dir(dest).unwrap().next().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn images_are_decoded_on_demand_and_bad_images_leave_no_cache_entry() {
        use base64::Engine;
        let root = tempfile::tempdir().unwrap();
        let media = root.path().join("Media");
        fs::create_dir(&media).unwrap();
        let source = media.join("photo.thumb");
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l2UAAAAASUVORK5CYII=")
            .unwrap();
        fs::write(&source, &png).unwrap();
        let cache = root.path().join("image-cache");
        let resolve_thumb = || resolve(root.path(), "Media/photo.thumb").ok().unwrap();
        let result = image(resolve_thumb(), &cache, "preview", true).unwrap();
        assert_eq!(result, cache.join("preview.png"));
        assert!(fs::read(&result).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(fs::read(&source).unwrap(), png);
        fs::write(&source, b"not an image").unwrap();
        assert!(image(resolve_thumb(), &cache, "broken", true).is_err());
        assert!(!cache.join("broken.png").exists());
        let attachment = Attachment {
            time: 100,
            kind: Kind::Image,
            supported: true,
            original: Some("Media/photo.png".into()),
            thumbnail: Some("Media/photo.thumb".into()),
            title: None,
            size: None,
        };
        fs::write(media.join("photo.png"), &png).unwrap();
        let original_preview =
            attachment_image(&attachment, root.path(), &cache, "100", true).unwrap();
        fs::write(&source, png).unwrap();
        assert_eq!(
            attachment_image(&attachment, root.path(), &cache, "100", true).unwrap(),
            original_preview
        );
        fs::remove_file(media.join("photo.png")).unwrap();
        let fallback = attachment_image(&attachment, root.path(), &cache, "100", true).unwrap();
        assert_ne!(fallback, original_preview);
        assert!(fs::read(fallback)
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
