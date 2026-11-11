//! Single source of truth for the capability keys that a subscription can grant.
//!
//! The list is intentionally client-side: the client needs a stable vocabulary to render
//! "what this plan includes", while the server remains the authority for which keys a plan
//! actually carries. A key the server sends that is missing here still works — the registry
//! only supplies labels, grouping and the free baseline.
//!
//! Naming: `domain.capability`, with `attachments.*` acting as a tier rather than a toggle.
//! Limits and quotas live in [`LIMITS`] / [`QUOTAS`] and are numbers, not booleans.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Capability {
    pub key: &'static str,
    /// Grouping used by settings and upgrade prompts.
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Part of the free baseline: still granted when a paid entitlement lapses.
    pub free: bool,
}

const fn cap(
    key: &'static str,
    group: &'static str,
    label: &'static str,
    description: &'static str,
    free: bool,
) -> Capability {
    Capability {
        key,
        group,
        label,
        description,
        free,
    }
}

/// Every capability the client knows how to gate.
pub const CAPABILITIES: &[Capability] = &[
    // ── 本机：读取自己的数据，不产生服务端成本 ──────────────────────────────
    cap(
        "local.wechat_scan",
        "local",
        "扫描微信账号",
        "发现本机微信账号与数据库位置。",
        true,
    ),
    cap(
        "local.wechat_keys",
        "local",
        "获取密钥",
        "通过内置扫描、外部工具或手动导入拿到解密密钥。",
        true,
    ),
    cap(
        "local.wechat_decrypt",
        "local",
        "解密快照",
        "把微信数据库解密到本机缓存，不上传。",
        true,
    ),
    cap(
        "local.wechat_read",
        "local",
        "浏览会话与消息",
        "读取白名单会话、分页消息与搜索。",
        true,
    ),
    cap(
        "local.wechat_media",
        "local",
        "图片解密与预览",
        "解密缩略图与原图，仅在本机打开。",
        true,
    ),
    cap(
        "local.voice_transcribe",
        "local",
        "语音转写",
        "用本机 whisper 系 CLI 转写语音，不联网。",
        true,
    ),
    cap(
        "local.cache_clear",
        "local",
        "清理本机缓存",
        "删除解密快照、图片缓存与转写缓存。",
        true,
    ),
    // ── 入库：服务端存储 + 解析成本 ─────────────────────────────────────────
    cap(
        "ingest.upload",
        "ingest",
        "手动上传",
        "把所选会话、日期或消息打包上传到 空间。",
        false,
    ),
    cap(
        "ingest.auto",
        "ingest",
        "自动备份",
        "按间隔或定时自动增量上传。",
        false,
    ),
    cap(
        "ingest.attachments.text",
        "ingest",
        "仅文本",
        "上传聊天文本，不包含任何附件字节。",
        false,
    ),
    cap(
        "ingest.attachments.images",
        "ingest",
        "文本 + 图片",
        "在上传包中附带图片。",
        false,
    ),
    cap(
        "ingest.attachments.all",
        "ingest",
        "全部附件",
        "附带图片、视频、文件与语音音频。",
        false,
    ),
    cap(
        "ingest.voice_transcript",
        "ingest",
        "上传语音转写",
        "把本机转写结果作为文本随包上传。",
        false,
    ),
    // ── 云端：读取与生成分开，模型调用另有配额 ───────────────────────────────
    cap(
        "cloud.records",
        "cloud",
        "云端记录",
        "浏览已上传的会话与原始消息。",
        false,
    ),
    cap(
        "cloud.events",
        "cloud",
        "事件查看",
        "读取事件列表、详情与证据。",
        false,
    ),
    cap(
        "cloud.events.write",
        "cloud",
        "事件编辑",
        "修改标题、正文、状态与完成标记。",
        false,
    ),
    cap(
        "cloud.events.relate",
        "cloud",
        "事件关联",
        "建立或解除事件之间的关系。",
        false,
    ),
    cap(
        "cloud.events.progress",
        "cloud",
        "事件进展",
        "补充、移出或撤销时间线进展。",
        false,
    ),
    cap(
        "cloud.topics",
        "cloud",
        "主题订阅",
        "创建和管理跨会话的主题订阅。",
        false,
    ),
    cap(
        "cloud.reports",
        "cloud",
        "报告查看",
        "查看服务端生成的日报、周报与月报。",
        false,
    ),
    cap(
        "cloud.reports.generate",
        "cloud",
        "按需生成报告",
        "在客户端发起一次报告生成任务。",
        false,
    ),
    cap(
        "cloud.ask",
        "cloud",
        "问答",
        "跨会话检索并让模型回答，产生模型调用。",
        false,
    ),
    // ── 本机智能：成本在本机，按功能开关而不是计量 ───────────────────────────
    cap(
        "ai.local_digest",
        "ai",
        "本地快速整理",
        "关键词归类风险 / 决定 / 待办，不联网。",
        true,
    ),
    cap(
        "ai.local_agent",
        "ai",
        "本地 Agent 配置",
        "选择、测试本机 CLI Agent 并读取模型列表。",
        false,
    ),
    cap(
        "ai.agent_summary",
        "ai",
        "AI 总结",
        "把原文交给本机 Agent 生成总结，原文可能离开本机。",
        false,
    ),
    cap(
        "ai.work_digest",
        "ai",
        "整理工作内容",
        "聊天页的工作台入口：按选区整理成可复制的工作内容。",
        false,
    ),
    cap(
        "ai.daily_report",
        "ai",
        "自动日报",
        "每天定点在本机生成日报并投递。",
        false,
    ),
    // ── 桥接：把本机数据交给外部 Agent ──────────────────────────────────────
    cap(
        "bridge.agent_cli",
        "bridge",
        "只读 CLI 与 Skills",
        "下载 wxq 并把三份 SKILL.md 安装到指定目录。",
        false,
    ),
    cap(
        "bridge.mcp",
        "bridge",
        "MCP 只读服务",
        "通过本机回环地址向其它 Agent 暴露白名单内的读取接口。",
        false,
    ),
    cap(
        "bridge.export",
        "bridge",
        "导出",
        "导出聊天、报告或工作台内容（当前占位）。",
        false,
    ),
    // ── 计划中：先占位，默认关闭，避免套餐承诺不可用能力 ─────────────────────
    cap(
        "ingest.whatsapp",
        "planned",
        "WhatsApp 接入",
        "尚未接入，仅记录「通知我」。",
        false,
    ),
    cap(
        "ingest.email",
        "planned",
        "邮件接入",
        "尚未接入，仅记录「通知我」。",
        false,
    ),
];

/// Numeric ceilings a plan may carry. Values are unit-suffixed by the key itself.
pub const LIMITS: &[&str] = &[
    "limits.accounts",
    "limits.ingest.conversations",
    "limits.ingest.file_mb",
    "limits.ingest.batch_mb",
    "limits.cloud.spaces",
    "limits.cloud.topics",
    "limits.retention_days",
];

/// Periodically resetting counters a plan may carry.
pub const QUOTAS: &[&str] = &[
    "quotas.ingest.messages_per_month",
    "quotas.ingest.uploads_per_day",
    "quotas.cloud.ask_per_day",
    "quotas.cloud.reports_per_month",
    "quotas.ai.agent_runs_per_day",
];

pub fn get(key: &str) -> Option<&'static Capability> {
    CAPABILITIES.iter().find(|item| item.key == key)
}

// Guard call sites: `entitlements::require(&data, &base, capabilities::CLOUD_ASK)`.
pub const LOCAL_VOICE_TRANSCRIBE: &str = "local.voice_transcribe";
pub const INGEST_UPLOAD: &str = "ingest.upload";
pub const INGEST_AUTO: &str = "ingest.auto";
pub const CLOUD_ASK: &str = "cloud.ask";
pub const CLOUD_REPORTS_GENERATE: &str = "cloud.reports.generate";
pub const AI_LOCAL_AGENT: &str = "ai.local_agent";
pub const AI_AGENT_SUMMARY: &str = "ai.agent_summary";
pub const AI_WORK_DIGEST: &str = "ai.work_digest";
pub const AI_DAILY_REPORT: &str = "ai.daily_report";
pub const BRIDGE_AGENT_CLI: &str = "bridge.agent_cli";
pub const BRIDGE_MCP: &str = "bridge.mcp";
pub const BRIDGE_EXPORT: &str = "bridge.export";

/// Keys that survive an expired entitlement: the local-only baseline.
pub fn free_keys() -> Vec<&'static str> {
    CAPABILITIES
        .iter()
        .filter(|item| item.free)
        .map(|item| item.key)
        .collect()
}

pub fn label(key: &str) -> &str {
    get(key).map(|item| item.label).unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_grouped() {
        let mut keys: Vec<&str> = CAPABILITIES.iter().map(|item| item.key).collect();
        let total = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), total, "能力键不能重复");
        assert!(CAPABILITIES.iter().all(|item| !item.group.is_empty()));
    }

    #[test]
    fn free_baseline_is_local_only() {
        let free = free_keys();
        assert!(free.contains(&"local.wechat_read"));
        assert!(free.contains(&"ai.local_digest"));
        assert!(!free.contains(&"ingest.upload"));
        assert!(!free.contains(&"cloud.ask"));
    }

    #[test]
    fn labels_fall_back_to_the_key() {
        assert_eq!(label("cloud.ask"), "问答");
        assert_eq!(label("server.only.key"), "server.only.key");
    }
}
