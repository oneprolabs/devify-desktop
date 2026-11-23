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
pub const AUTH_PORT: u16 = if DEVELOPMENT { 47833 } else { 47832 };
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

/// A distinct callback must be configured by the development server. Never grab the
/// production port or rewrite a server-issued OAuth redirect without its approval.
pub fn validate_auth_redirect(raw: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|_| "登录回调配置无效。".to_string())?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port() != Some(AUTH_PORT)
        || url.path() != "/auth/callback"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(if DEVELOPMENT {
            "当前服务尚未配置开发版浏览器登录，请使用账号密码登录。".into()
        } else {
            "服务端浏览器登录回调配置无效，请联系管理员。".into()
        });
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_and_callback_cannot_fall_back_to_the_other_app() {
        assert!(validate_identifier(IDENTIFIER).is_ok());
        assert!(validate_identifier(if DEVELOPMENT {
            PRODUCTION_ID
        } else {
            DEVELOPMENT_ID
        })
        .is_err());
        assert!(
            validate_auth_redirect(&format!("http://127.0.0.1:{AUTH_PORT}/auth/callback")).is_ok()
        );
        let other = if DEVELOPMENT { 47832 } else { 47833 };
        for uri in [
            format!("http://127.0.0.1:{other}/auth/callback"),
            format!("http://example.com:{AUTH_PORT}/auth/callback"),
            format!("http://127.0.0.1:{AUTH_PORT}/other"),
        ] {
            assert!(validate_auth_redirect(&uri).is_err());
        }
    }
}
