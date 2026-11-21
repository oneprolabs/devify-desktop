//! Lifecycle and shared state of the signed-in local Agent bridge.
use crate::{
    app_data, desktop_credentials, devify_access_token_async, entitlements, mcp, settings,
};
use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

#[derive(Clone, Serialize)]
pub(crate) struct McpStatus {
    pub(crate) running: bool,
    pub(crate) address: String,
    endpoint: String,
    local_only: bool,
    pub(crate) error: Option<String>,
    /// Set when the listener is intentionally down (not signed in, no bridge capability,
    /// connector off). Kept apart from `error` so the UI can explain instead of alarm.
    pub(crate) reason: Option<String>,
}

struct RunningService {
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Clone)]
pub(crate) struct McpState {
    pub(crate) status: Arc<Mutex<McpStatus>>,
    token: Option<String>,
    task: Arc<tokio::sync::Mutex<Option<RunningService>>>,
}

impl McpState {
    pub(crate) fn new(bind: &str, token: Option<String>) -> Self {
        let local_only = !mcp::requires_token_for_ui(bind);
        Self {
            status: Arc::new(Mutex::new(McpStatus {
                running: false,
                endpoint: format!("http://{bind}/sse"),
                address: bind.to_string(),
                local_only,
                error: None,
                reason: None,
            })),
            token,
            task: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

pub(crate) fn access_state(state: &State<McpState>) -> Result<McpStatus, String> {
    state
        .status
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "读取本机服务状态失败。".to_string())
}

/// Why the listener is down when it is not an error.
fn local_service_idle_reason(data: &Path) -> String {
    let Ok(settings) = settings::load(data) else {
        return "无法读取本机设置，本机 Agent 服务不会启动。".to_string();
    };
    if !settings.connectors.wechat_enabled && !settings.connectors.whatsapp_enabled {
        return "所有平台连接均已断开，本机 Agent 服务不会启动。".to_string();
    }
    let Ok(base_url) = settings.devify.resolved_base_url() else {
        return "服务端地址无效，本机 Agent 服务不会启动。".to_string();
    };
    if desktop_credentials::load(data, &base_url).is_err() {
        return "未登录，本机 Agent 服务不会启动。".to_string();
    }
    "当前套餐未包含本地 Agent 集成，本机服务不会启动。".to_string()
}

/// Start or stop the loopback listener so it exists only while the account is signed in and
/// its plan carries a bridge capability. Called after login, logout, entitlement refresh,
/// settings changes and on the periodic tick, so the port tracks state instead of the app's
/// lifetime.
pub(crate) async fn reconcile_local_service(app: &AppHandle) {
    let state = app.state::<McpState>();
    let data = app_data(app);
    let allowed = mcp::bridge_listen_allowed(&data);
    let mut slot = state.task.lock().await;

    if allowed {
        if slot.is_some() {
            if let Ok(mut status) = state.status.lock() {
                status.reason = None;
            }
            return;
        }
        let Some(token) = state.token.clone() else {
            if let Ok(mut status) = state.status.lock() {
                status.running = false;
                status.reason = Some("无法创建本机访问 Token，本机服务不会启动。".to_string());
            }
            return;
        };
        let bind = mcp::configured_bind();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let status = state.status.clone();
        let start_data = data.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let result = mcp::start(
                start_data,
                bind,
                Some(token),
                Some(ready_tx),
                Some(shutdown_rx),
            )
            .await;
            if let Err(error) = result {
                if let Ok(mut status) = status.lock() {
                    status.running = false;
                    status.error = Some(error);
                }
            }
        });
        match tokio::time::timeout(Duration::from_secs(5), ready_rx).await {
            Ok(Ok(Ok(()))) => {
                *slot = Some(RunningService {
                    shutdown: shutdown_tx,
                    handle,
                });
                if let Ok(mut status) = state.status.lock() {
                    status.running = true;
                    status.error = None;
                    status.reason = None;
                }
            }
            Ok(Ok(Err(_))) => {
                // `start` already reported the bind error through the ready channel and the
                // spawned task; nothing else to record here.
            }
            Ok(Err(_)) | Err(_) => {
                if let Ok(mut status) = state.status.lock() {
                    status.running = false;
                    status.error = Some("本机服务启动超时。".to_string());
                }
            }
        }
    } else {
        if let Some(running) = slot.take() {
            let _ = running.shutdown.send(());
            let _ = tokio::time::timeout(Duration::from_secs(5), running.handle).await;
        }
        if let Ok(mut status) = state.status.lock() {
            status.running = false;
            status.error = None;
            status.reason = Some(local_service_idle_reason(&data));
        }
    }
}

/// Refresh the snapshot after a login and then reconcile, so a restored session picks up a
/// plan change without waiting for the frontend's first refresh.
pub(crate) async fn refresh_entitlements_then_reconcile(app: &AppHandle) {
    let data = app_data(app);
    if let Ok(settings) = settings::load(&data) {
        if let Ok(base_url) = settings.devify.resolved_base_url() {
            if let Ok(token) = devify_access_token_async(&data, &base_url).await {
                let _ = entitlements::refresh(&data, &base_url, &token).await;
            }
        }
    }
    reconcile_local_service(app).await;
}
