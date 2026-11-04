---
name: wechat-chat-records
description: 通过 wxq 命令行读取 WeChat Devify 白名单内的微信群聊和单聊原文，支持会话列表与关键字检索；wxq 连接本机桌面应用暴露的只读 HTTP 接口。
---

# WeChat Chat Records

用 `wxq` 读取本机已解密缓存中的白名单聊天记录。不要直接读取 SQLite 文件、扫描微信目录或猜测数据库结构。

## 前置条件

1. 桌面应用「AIMyChats」正在运行（会自带只读服务，默认 `http://127.0.0.1:18765`）。
2. `wxq` 已通过 AIMyChats「Agent 集成 → 工具接入」页的一条提示词安装；本 Skill 也从同一服务的 `/agent/skills` 下载。若 `command -v wxq` 为空，停止并让用户重新复制该提示词，不要自行构建或改用其它方式读取数据库。
3. 需要更新到最新时，重新运行同一提示词即可；已存在的 `SKILL.md` 会先备份。

服务地址可用 `--endpoint` 或 `WECHAT_AGENT_ENDPOINT` 覆盖（旧名 `WECHAT_MCP_ENDPOINT` 仍兼容）；服务绑定非本机地址时，用 `--token` 或 `WECHAT_AGENT_TOKEN` 传 Bearer Token。

## 命令

| 用途 | 命令 |
|---|---|
| 检查服务 | `wxq health` |
| 账号、白名单数量与可见会话 | `wxq policy` |
| 只列白名单会话 | `wxq sessions` |
| 读会话原文 | `wxq messages --chat USERNAME [--limit 100] [--before TS --before-id ID]` |
| 跨白名单会话检索 | `wxq search --keyword KEY [--groups-only] [--max-sessions 200]` |

- `--limit` 范围 1–500，默认 100；输出按时间正序，`has_more` 为真时会打印继续翻页的命令。
- `--before` 接受 Unix 秒或 `2026-10-20` / `"2026-10-20 18:00"`；`--before-id` 是上一页最后一条的 `local_id`，翻页时一起带上，否则同一秒内的多条消息可能被跳过。
- 需要结构化数据时加 `--json`（全局参数，放在命令前或后都生效）。

## 工作规则

1. 先用 `wxq policy` 或 `wxq sessions` 确认范围，只使用输出里出现过的 `username`。
2. 会话不在白名单、或服务返回 401/400 时立即停止，把原文错误展示给用户，不要改用其他方式读取。
3. 需要历史消息时用 `--before` + `--before-id` 翻页（命令末尾会直接打印这两个值），不要一次请求过大的 `--limit`。
4. 明确区分原文、规则抽取和 Agent 自己的推断。
5. 不把聊天原文发送到其他服务，除非用户明确要求。
6. 白名单为空时停止读取，提示用户在桌面应用「聊天白名单」页配置。

## 备注

`wxq` 只读，不解密、不写入、不发邮件；每次请求都由服务端重新读取 `app-settings.json` 并校验白名单。若环境只支持 MCP 客户端而不便运行 shell，可用同一服务的 MCP 工具
`wechat_get_policy` / `wechat_list_sessions` / `wechat_get_messages` / `wechat_search_messages` / `wechat_digest`
（`http://127.0.0.1:18765/sse`），规则与上述完全一致。
