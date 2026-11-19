//! Keep source keys and decrypted copies separate for each service login.
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize, PartialEq)]
struct Principal {
    base: String,
    user: i64,
}

pub fn root(data: &Path) -> PathBuf {
    let Ok(value) = crate::settings::load(data) else {
        return data.join("unavailable-source");
    };
    let Ok(base) = value.devify.resolved_base_url() else {
        return data.join("unavailable-source");
    };
    let Ok(credentials) = crate::desktop_credentials::load(data, &base) else {
        // Source commands are authenticated separately. This directory cannot expose a
        // previously signed-in user's decrypted copy while signed out.
        return data.join("signed-out-source");
    };
    let Some(user) = crate::access_token_claim(&credentials.access, "user_id")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
    else {
        return data.join("unavailable-source");
    };
    let principal = Principal {
        base: base.clone(),
        user,
    };
    let marker = data.join("legacy-source-owner.json");
    let legacy_owner = fs::read(&marker)
        .ok()
        .and_then(|raw| serde_json::from_slice::<Principal>(&raw).ok());
    if legacy_owner.as_ref() == Some(&principal) {
        return data.into();
    }
    // Migrate only when an existing verified owner proves the old data belongs to
    // this login. Never guess ownership from the selected local WeChat account.
    let hash = hex::encode(sha2::Sha256::digest(format!("{base}\0{user}").as_bytes()));
    let scoped = data.join("source-users").join(hash);
    if scoped.exists() {
        return scoped;
    }
    let known = value
        .sync
        .owners
        .values()
        .chain(value.upload_queue.iter().filter_map(|j| j.owner.as_ref()))
        .any(|o| o.user_id == user && crate::settings::same_service_url(&o.base_url, &base));
    if !marker.exists() && known {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        if let Ok(mut file) = options.open(&marker) {
            if serde_json::to_vec(&principal)
                .ok()
                .is_some_and(|raw| file.write_all(&raw).and_then(|_| file.sync_all()).is_ok())
            {
                return data.into();
            }
        } else if fs::read(&marker)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Principal>(&raw).ok())
            .as_ref()
            == Some(&principal)
        {
            return data.into();
        }
    }
    let _ = fs::create_dir_all(&scoped);
    scoped
}
