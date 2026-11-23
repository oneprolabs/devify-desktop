//! Explicit app variants; an optimized development build is still a development app.
pub const DEVELOPMENT: bool = cfg!(feature = "dev-app");
pub const PRODUCTION_ID: &str = "local.wechat.devifybridge";
pub const DEVELOPMENT_ID: &str = "local.wechat.devifybridge.dev";
pub const IDENTIFIER: &str = if DEVELOPMENT {
    DEVELOPMENT_ID
} else {
    PRODUCTION_ID
};
pub const APP_NAME: &str = if DEVELOPMENT {
    "AIMyChats Dev"
} else {
    "AIMyChats"
};
pub const DEFAULT_SERVICE: &str = if DEVELOPMENT {
    "http://192.168.8.182:7000"
} else {
    "https://aimychats.com/"
};
// Both apps reuse the existing desktop login registration. The listener exists
// only during authorization; binding it prevents simultaneous browser attempts.
pub const AUTH_PORT: u16 = 47832;
pub const AGENT_BIND: &str = if DEVELOPMENT {
    "127.0.0.1:18766"
} else {
    "127.0.0.1:18765"
};
pub const SKILL_NAME: &str = if DEVELOPMENT {
    "aimychats-dev"
} else {
    "aimychats"
};

/// Side-effect-free packaging check; no Tauri runtime, credentials or data access.
pub fn metadata() -> serde_json::Value {
    serde_json::json!({"name":APP_NAME,"identifier":IDENTIFIER,"development":DEVELOPMENT,
        "version":env!("CARGO_PKG_VERSION"),"agent_bind":AGENT_BIND})
}

/// Refuse a mixed feature/config build before opening any app-owned data.
pub fn validate_identifier(actual: &str) -> Result<(), String> {
    if actual != IDENTIFIER {
        return Err(format!(
            "应用构建配置不一致：{APP_NAME} 需要标识 {IDENTIFIER}，实际为 {actual}。"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_identity_is_separate_but_existing_login_callback_is_shared() {
        assert!(validate_identifier(IDENTIFIER).is_ok());
        assert!(validate_identifier(if DEVELOPMENT {
            PRODUCTION_ID
        } else {
            DEVELOPMENT_ID
        })
        .is_err());
        assert_eq!(AUTH_PORT, 47832);
    }
}
