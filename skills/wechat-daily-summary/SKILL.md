---
name: wechat-daily-summary
description: 通过 wxq 从白名单聊天记录生成每日工作摘要草稿，保留规则抽取的原文证据与免责声明。
---

# WeChat Daily Summary

用于用户要求生成每日工作摘要、日报草稿或 Devify 投递前的内容审阅时。

## 前置条件

- 桌面应用「AIMyChats」正在运行。
- `wxq` 已通过「Agent 集成 → 工具接入」页的提示词安装（来源 `/agent/wxq`）；若不可用，停止并让用户重新复制提示词。

## 标准流程

1. `wxq policy`——确认当前账号、白名单会话数量与自动同步状态。
2. `wxq sessions`——只用输出中出现的 `username`。
3. `wxq digest --chat USERNAME [--chat USERNAME ...] --since 2026-10-20 --until 2026-10-27`——生成规则抽取草稿。时间也接受 Unix 秒；`--until` 只给日期时按当天 23:59:59 处理。
4. 需要补充上下文时，用 `wxq messages --chat USERNAME --limit 100 --before TS` 取原文。
5. 输出“事实/原文证据”“待办”“决定”“风险与问题”“时间线”“待确认项”几个部分。

## 输出边界

- `wxq digest` 是确定性规则抽取，不是本地 AI 结论；必须保留输出末尾的免责声明（“不是 AI 总结”）。
- 不确定的负责人、截止时间和因果关系标记为“待确认”，并给出可回查的会话与时间。
- 不主动调用 SMTP、Devify 或其他外部投递能力；本服务只读。投递由用户在桌面应用里显式触发。
- 只有用户明确要求时，才把摘要交给其他系统继续处理。

## 备注

同样的数据也能从 MCP 工具 `wechat_digest`（`http://127.0.0.1:18765/sse`）取得，字段与 `wxq digest --json` 一致。
