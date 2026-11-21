---
name: aimychats
description: 通过 AIMyChats 统一本机接口查找和读取微信、WhatsApp 聊天记录，为用户的总结、日报、项目分析或其他处理提供原文依据。用户要求查看或分析 AIMyChats 中的聊天时使用。
---

# AIMyChats

用 `wxq` 通过 AIMyChats 读取当前允许访问的本机聊天记录。不要直接读取 SQLite 文件、扫描微信目录或猜测数据库结构。

## 当前支持范围

Skills 只连接 AIMyChats 的统一本机接口，当前支持微信和 WhatsApp。平台选择、账号校验和读取权限由 AIMyChats 负责。不要直接读取任何平台的数据库。

## 前置条件

1. 桌面应用「AIMyChats」正在运行（会自带只读服务，默认 `http://127.0.0.1:18765`）。
2. `wxq` 已通过 AIMyChats「智能体集成」页安装；本 Skill 也从同一服务的 `/agent/skills` 下载。优先使用安装时给出的完整本机读取入口，不要求它出现在 PATH 中。若该入口不可执行，停止并让用户在该页重新安装 Skills，不要自行构建或改用其它方式读取数据库。
3. 需要更新到最新时，在「智能体集成」重新安装 Skills 即可；已存在的 `SKILL.md` 会先备份。

服务地址可用 `--endpoint` 或 `WECHAT_AGENT_ENDPOINT` 覆盖（旧名 `WECHAT_MCP_ENDPOINT` 仍兼容）；服务始终要求 Bearer Token，`wxq` 默认读取桌面应用生成的 `<应用数据目录>/agent-token`，也可用 `--token` 或 `WECHAT_AGENT_TOKEN` 覆盖。

## 账号与平台

先执行 `wxq accounts --json` 自动发现当前可读取的平台和账号。后续每次读取都传入该结果中的 `--account ID`，不要猜测 ID。用户未指定平台时逐个账号检索，结果标明平台、账号、会话；不要把不同账号的同名聊天混在一起。

## 命令

| 用途 | 命令 |
|---|---|
| 检查服务 | `wxq health` |
| 账号、白名单数量与可见会话 | `wxq policy --account ID` |
| 分页列出允许会话 | `wxq sessions --account ID --limit 100 --offset 0 --json` |
| 读会话原文 | `wxq messages --account ID --chat USERNAME [--limit 100] [--before TS --before-id ID]` |
| 跨白名单会话检索 | `wxq search --account ID --keyword KEY [--groups-only] [--max-sessions 200]` |

- `--limit` 范围 1–500，默认 100；输出按时间正序，`has_more` 为真时会打印继续翻页的命令。
- `--before` 接受 Unix 秒或 `2026-10-20` / `"2026-10-20 18:00"`；`--before-id` 是上一页最后一条的 `local_id`，翻页时一起带上，否则同一秒内的多条消息可能被跳过。
- 需要结构化数据时加 `--json`（全局参数，放在命令前或后都生效）。

## 工作规则

1. 先用 `wxq policy --account ID` 或 `wxq sessions --account ID --limit 100 --offset 0 --json` 确认范围，只使用输出里出现过的 `username`。
2. 会话不在白名单、或服务返回 401/400 时立即停止，把原文错误展示给用户，不要改用其他方式读取。
3. 需要历史消息时用 `--before` + `--before-id` 翻页（命令末尾会直接打印这两个值），不要一次请求过大的 `--limit`。
4. 明确区分原文、规则抽取和 Agent 自己的推断。
5. 不把聊天原文发送到其他服务，除非用户明确要求。
6. 白名单为空时停止读取，提示用户在桌面应用「导入管理」页配置。

## 备注

`wxq` 只读，不解密、不写入、不发邮件；每次请求都由本机服务重新读取客户端设置并校验访问范围。若环境只支持 MCP 客户端而不便运行 shell，可用同一服务的 MCP 工具
`aimychats_list_accounts` / `aimychats_get_policy` / `aimychats_list_sessions` / `aimychats_get_messages` / `aimychats_search_messages` / `aimychats_digest`
（`http://127.0.0.1:18765/sse`），新工具统一使用 `aimychats_` 前缀；旧 `wechat_` 名称仅作兼容，规则与上述完全一致。

会话列表按 `next_offset` 继续分页；消息按 `next_cursor` 同时传 `--before`、`--before-id`、`--before-shard`，避免跨分片漏消息。默认每页 100 条，单页最多 500 条；不要一次拉取全部历史。`digest` 每个会话最多读取 800 条，遵守返回的截断提示。

## 按用户目的组合读取

只有这一套 AIMyChats 接入能力，不按平台或总结类型另装 Skills。

- 读某个聊天：先发现账号和会话，按名称确认后分页取消息。保留平台、账号、会话、时间和发言人。
- 总结或日报：读取用户指定的聊天与时间范围，再由当前智能体总结。可按“结论、事情经过、可能的待办”组织，忠实于原文，不虚构人物、时间或决定。
- 项目或跨聊天探索：先用 `search` 按项目名、人名、关键词定位候选会话，再读原文核对；跨平台时逐个账号读取，并保留每条证据所属的平台和会话。
- 可选规则整理：`wxq digest --account ID --chat USERNAME [--chat USERNAME ...] --since DATE --until DATE` 返回有限范围内的关键词规则素材，**不是 AI 总结**。不得将规则抽取冒充模型结论；最终总结和分析由当前智能体负责。

不要自行切换模型、开启上传、投递报告、修改备份设置或触发服务端分析。只读取 AIMyChats 允许访问的范围；其他操作需要用户明确要求和相应的真实接口支持。
