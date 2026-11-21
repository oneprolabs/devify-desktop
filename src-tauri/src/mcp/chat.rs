//! Uniform, read-only platform dispatch behind the local Agent bridge.
use super::{safe_account_id, AccountArgs, DigestArgs, MessagesArgs, SearchArgs};
use crate::wechat::{decrypt, query};
use crate::{
    config_runtime, entitlements,
    settings::{self, AppSettings},
    whatsapp,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

struct Context {
    account_id: String,
    cache: PathBuf,
    allowed: HashSet<String>,
    settings: AppSettings,
    app_data: PathBuf,
    whatsapp: bool,
}

fn authorized_settings(app_data: &Path) -> Result<AppSettings, String> {
    let settings = settings::load(app_data)?;
    let base = settings.devify.resolved_base_url()?;
    crate::desktop_credentials::load(app_data, &base).map_err(|_| "请先登录 AIMyChats。")?;
    if !entitlements::grants(app_data, &base, crate::capabilities::BRIDGE_MCP)
        && !entitlements::agent_access(app_data, &base)
    {
        return Err("当前账户未开通本机智能体读取能力。".into());
    }
    Ok(settings)
}

fn configured_accounts(settings: &AppSettings) -> Vec<String> {
    let mut ids: Vec<_> = settings
        .chat
        .allowed_usernames
        .keys()
        .chain(settings.runtime.sources.keys())
        .filter(|id| !config_runtime::browse_names(settings, id).is_empty())
        .cloned()
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn source_allowed(
    settings: &AppSettings,
    account: &str,
    current_whatsapp: Option<&str>,
) -> Result<bool, String> {
    safe_account_id(account)?;
    let is_whatsapp = whatsapp::is_account(account);
    if is_whatsapp {
        if !settings.connectors.whatsapp_enabled {
            return Err("WhatsApp 已断开，请重新连接。".into());
        }
        if current_whatsapp != Some(account) {
            return Err("WhatsApp 登录账号已变化，请重新完成连接向导。".into());
        }
    } else if !settings.connectors.wechat_enabled {
        return Err("微信已断开，请重新连接。".into());
    }
    Ok(is_whatsapp)
}

pub(super) fn accounts_json(app_data: &Path) -> Result<Value, String> {
    let settings = authorized_settings(app_data)?;
    let current = whatsapp::account();
    let ids = configured_accounts(&settings);
    let accounts: Vec<_> = ids
        .iter()
        .filter_map(|id| {
            let wa = source_allowed(&settings, id, current.as_ref().map(|a| a.id.as_str())).ok()?;
            Some(
                json!({"account_id": id, "platform": if wa {"whatsapp"} else {"wechat"},
            "selected_conversations": config_runtime::browse_names(&settings, id).len()}),
            )
        })
        .take(33)
        .collect();
    Ok(
        json!({"accounts": accounts.iter().take(32).collect::<Vec<_>>(), "truncated": accounts.len() > 32}),
    )
}

fn context(app_data: &Path, account_id: Option<String>) -> Result<Context, String> {
    let settings = authorized_settings(app_data)?;
    let account_id = account_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| settings.active_account_id.clone());
    let wa = if whatsapp::is_account(&account_id) {
        whatsapp::account()
    } else {
        None
    };
    let is_whatsapp = source_allowed(&settings, &account_id, wa.as_ref().map(|a| a.id.as_str()))?;
    let allowed = config_runtime::browse_names(&settings, &account_id)
        .into_iter()
        .collect();
    Ok(Context {
        cache: decrypt::cache_dir(app_data, &account_id),
        account_id,
        allowed,
        settings,
        app_data: app_data.into(),
        whatsapp: is_whatsapp,
    })
}

fn sessions(
    context: &Context,
    limit: usize,
    offset: usize,
) -> Result<Vec<query::SessionItem>, String> {
    if context.whatsapp {
        return whatsapp::sessions_page(&context.app_data, &context.allowed, limit, offset);
    }
    query::sessions_page(&context.cache, Some(&context.allowed), Some(limit), offset)
}

fn ensure_allowed(context: &Context, username: &str) -> Result<(), String> {
    if !context.whatsapp && !query::is_chat_session(&context.cache, username) {
        return Err("公众号、服务号和系统会话不在 MCP 查看范围内。".into());
    }
    if !context.allowed.contains(username) {
        return Err("该会话不在聊天白名单中。".into());
    }
    if context.whatsapp {
        // Do not mistake newsletter/status/system rows for visible conversations.
        let only = HashSet::from([username.to_string()]);
        if whatsapp::sessions_page(&context.app_data, &only, 1, 0)?.is_empty() {
            return Err("未找到可访问的会话。".into());
        }
    }
    Ok(())
}

pub(super) fn policy_json(app_data: &Path, args: AccountArgs) -> Result<Value, String> {
    let context = context(app_data, args.account_id)?;
    let sessions = sessions(&context, 101, 0)?;
    Ok(json!({
        "account_id": context.account_id,
        "whitelist_count": context.allowed.len(),
        "platform": if context.whatsapp {"whatsapp"} else {"wechat"},
        "sessions": sessions.iter().take(100).collect::<Vec<_>>(),
        "has_more": sessions.len() > 100,
        "automation_enabled": context.settings.automation.enabled
    }))
}

pub(super) fn sessions_json(app_data: &Path, args: AccountArgs) -> Result<Value, String> {
    let context = context(app_data, args.account_id)?;
    let limit = args.limit.unwrap_or(100).clamp(1, 500);
    let offset = args.offset.unwrap_or(0).min(100_000);
    let page = sessions(&context, limit + 1, offset)?;
    Ok(
        json!({"account_id": context.account_id, "platform": if context.whatsapp {"whatsapp"} else {"wechat"},
        "sessions": page.iter().take(limit).collect::<Vec<_>>(), "next_offset": if page.len() > limit { Some(offset + limit) } else { None }}),
    )
}

pub(super) fn messages_json(app_data: &Path, args: MessagesArgs) -> Result<Value, String> {
    let context = context(app_data, args.account_id)?;
    ensure_allowed(&context, &args.username)?;
    let limit = args.limit.unwrap_or(100).clamp(1, 500);
    let cursor = args.before.map(|time| match args.before_id {
        Some(id) => query::MessageCursor {
            time,
            local_id: id,
            source_shard: args.before_shard.unwrap_or_default(),
        },
        None => query::MessageCursor::before_time(time),
    });
    let messages = if context.whatsapp {
        whatsapp::messages(&context.app_data, &args.username, limit, cursor)?
    } else {
        query::messages(&context.cache, &args.username, limit, cursor)?
    };
    let has_more = messages.len() == limit;
    Ok(json!({
        "account_id": context.account_id,
        "platform": if context.whatsapp {"whatsapp"} else {"wechat"},
        "username": args.username,
        "messages": messages,
        "has_more": has_more,
        "next_cursor": messages.last().map(|m| json!({"before": m.time, "before_id": m.local_id, "before_shard": m.source_shard}))
    }))
}

pub(super) fn search_json(app_data: &Path, args: SearchArgs) -> Result<Value, String> {
    let context = context(app_data, args.account_id)?;
    if context.whatsapp {
        let allowed = if args.groups_only.unwrap_or(false) {
            context
                .allowed
                .iter()
                .filter(|id| id.ends_with("@g.us"))
                .cloned()
                .collect()
        } else {
            context.allowed.clone()
        };
        let report = whatsapp::search(
            &context.app_data,
            &args.keyword,
            args.max_sessions.unwrap_or(200).clamp(1, 200),
            Some(&allowed),
        )?;
        return serde_json::to_value(report).map_err(|error| error.to_string());
    }
    let report = query::search_filtered(
        &context.cache,
        &args.keyword,
        args.groups_only.unwrap_or(false),
        args.max_sessions.unwrap_or(200).clamp(1, 200),
        Some(&context.allowed),
    )?;
    serde_json::to_value(report).map_err(|error| error.to_string())
}

pub(super) fn digest_json(app_data: &Path, args: DigestArgs) -> Result<Value, String> {
    if args.usernames.is_empty() || args.usernames.len() > 100 {
        return Err("usernames 必须包含 1 到 100 个会话。".into());
    }
    let context = context(app_data, args.account_id)?;
    for username in &args.usernames {
        ensure_allowed(&context, username)?;
    }
    if context.whatsapp {
        let selected: HashSet<_> = args.usernames.iter().cloned().collect();
        let sessions = whatsapp::sessions_page(&context.app_data, &selected, 100, 0)?;
        let mut pages = Vec::new();
        for username in &args.usernames {
            let mut page = whatsapp::messages_between(
                &context.app_data,
                username,
                args.since,
                args.until.map(|end| end.saturating_add(1)),
                800,
            )?;
            // WhatsApp text type 0 maps to the shared rule extractor's text type 1.
            for message in &mut page {
                message.kind = if message.kind == 0 { 1 } else { 0 };
            }
            pages.push((username.clone(), page));
        }
        return serde_json::to_value(query::digest_from_pages(
            &sessions,
            &args.usernames,
            pages,
            args.since,
            args.until,
        )?)
        .map_err(|error| error.to_string());
    }
    let digest = query::digest_filtered(
        &context.cache,
        &args.usernames,
        args.since,
        args.until,
        Some(&context.allowed),
    )?;
    serde_json::to_value(digest).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_have_independent_gates_and_whatsapp_identity_is_checked() {
        let mut settings = AppSettings::default();
        settings.connectors.wechat_enabled = false;
        settings.connectors.whatsapp_enabled = true;
        let wa = "12345678901@s.whatsapp.net";
        assert_eq!(source_allowed(&settings, wa, Some(wa)).unwrap(), true);
        assert!(source_allowed(&settings, wa, Some("10987654321@s.whatsapp.net")).is_err());
        assert!(source_allowed(&settings, "wx_test", Some(wa)).is_err());
        settings.connectors.wechat_enabled = true;
        settings.connectors.whatsapp_enabled = false;
        assert_eq!(source_allowed(&settings, "wx_test", None).unwrap(), false);
        assert!(source_allowed(&settings, wa, Some(wa)).is_err());
        assert!(source_allowed(&settings, "../outside", None).is_err());
    }

    #[test]
    fn discovery_and_reads_share_the_completed_configuration() {
        let mut settings = AppSettings::default();
        settings
            .chat
            .allowed_usernames
            .insert("wx_test".into(), vec!["new_draft".into()]);
        settings
            .runtime
            .sources
            .entry("wx_test".into())
            .or_default()
            .draft = true;
        assert!(configured_accounts(&settings).is_empty());
        settings.runtime.sources.get_mut("wx_test").unwrap().active =
            Some(json!({"usernames":["previously_allowed"]}));
        assert_eq!(
            config_runtime::browse_names(&settings, "wx_test"),
            vec!["previously_allowed"]
        );
        assert_eq!(configured_accounts(&settings), vec!["wx_test"]);
        settings.chat.allowed_usernames.insert(
            "12345678901@s.whatsapp.net".into(),
            vec!["group@g.us".into()],
        );
        assert_eq!(configured_accounts(&settings).len(), 2);
    }

    #[test]
    fn shared_digest_keeps_evidence_and_rejects_unlisted_pages() {
        let session: query::SessionItem = serde_json::from_value(json!({
            "username":"group@g.us", "display":"项目群", "last_time":100,
            "is_group":true,"member_count":null,"preview":null,"unread":null
        }))
        .unwrap();
        let message: query::MessageItem = serde_json::from_value(json!({
            "local_id":1,"source_shard":"whatsapp","time":100,"sender":"tester",
            "sender_name":"测试成员","kind":1,"text":"决定明天确认合同","is_self":false
        }))
        .unwrap();
        let report = query::digest_from_pages(
            &[session],
            &["group@g.us".into()],
            vec![
                ("group@g.us".into(), vec![message.clone()]),
                (
                    "hidden@g.us".into(),
                    vec![query::MessageItem {
                        text: "不应读取的内容".into(),
                        ..message
                    }],
                ),
            ],
            Some(100),
            Some(100),
        )
        .unwrap();
        assert_eq!(report.messages, 1);
        assert_eq!(report.sessions, 1);
        assert!(report.markdown.contains("测试成员"));
        assert!(report.markdown.contains("决定明天确认合同"));
        assert!(!report.markdown.contains("不应读取的内容"));
        assert!(report.markdown.contains("规则"));
    }
}
