use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};
use tauri::AppHandle;

const FILE: &str = "network-connection.json";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    #[default]
    Auto,
    SystemProxy,
    Direct,
}

pub(crate) fn load(root: &Path) -> io::Result<Mode> {
    match fs::read(root.join(FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Mode::Auto),
        Err(error) => Err(error),
    }
}

fn save(root: &Path, mode: Mode) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(&mut file, &mode).map_err(io::Error::other)?;
    file.as_file().sync_all()?;
    file.persist(root.join(FILE)).map_err(|error| error.error)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn network_connection_get(app: AppHandle) -> Result<Mode, String> {
    load(&crate::app_data(&app)).map_err(|error| format!("无法读取网络连接设置：{error}"))
}

#[tauri::command]
pub(crate) fn network_connection_set(app: AppHandle, mode: Mode) -> Result<Mode, String> {
    let root = crate::app_data(&app);
    save(&root, mode).map_err(|error| error.to_string())?;
    crate::http_client::connection_mode_changed(&root);
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_install_defaults_to_auto_and_each_mode_persists() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(load(root.path()).unwrap(), Mode::Auto);
        for mode in [Mode::Direct, Mode::SystemProxy, Mode::Auto] {
            save(root.path(), mode).unwrap();
            assert_eq!(load(root.path()).unwrap(), mode);
        }
    }
}
