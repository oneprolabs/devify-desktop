---
name: aimychats
description: 通过 AIMyChats 统一只读接口读取微信、WhatsApp 聊天、当前空间已有待办、日报／周报／月报、主题订阅和已生成主题报告。用户要求查看聊天、我的待办或已有报告时使用；不触发生成或编辑。
---

# AIMyChats

用 `wxq` 通过 AIMyChats 读取当前允许访问的本机聊天记录及当前空间已有云端结果。不要直接读取 SQLite 文件、扫描微信目录或猜测数据库结构。

## 当前支持范围

Skills 只连接 AIMyChats 的统一本机接口。本机聊天当前支持微信和 WhatsApp；已有待办、报告和主题由客户端向当前服务发起云端远程读取，复用桌面登录，不要求另外登录或交出 access/refresh token。平台选择、账号校验和读取权限由 AIMyChats 负责。不要直接读取任何平台的数据库。

## 前置条件

1. 桌面应用「AIMyChats」正在运行（会自带只读服务，默认 `http://127.0.0.1:18765`）。
2. `wxq` 已通过 AIMyChats「智能体集成」页安装；本 Skill 也从同一服务的 `/agent/skills` 下载。优先使用安装时给出的完整本机读取入口，不要求它出现在 PATH 中。若该入口不可执行，停止并让用户在该页重新安装 Skills，不要自行构建或改用其它方式读取数据库。
3. 需要更新到最新时，在「智能体集成」重新安装 Skills 即可；已存在的 `SKILL.md` 会先备份。

服务地址可用 `--endpoint` 或 `WECHAT_AGENT_ENDPOINT` 覆盖（旧名 `WECHAT_MCP_ENDPOINT` 仍兼容）；服务始终要求 Bearer Token，`wxq` 默认读取桌面应用生成的 `<应用数据目录>/agent-token`，也可用 `--token` 或 `WECHAT_AGENT_TOKEN` 覆盖。

## 账号与平台

读取聊天时先执行 `wxq accounts --json` 自动发现当前可读取的平台和账号。后续每次读取都传入该结果中的 `--account ID`，不要猜测 ID。用户未指定平台时逐个账号检索，结果标明平台、账号、会话；不要把不同账号的同名聊天混在一起。

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

1. 读取聊天时先用 `wxq policy --account ID` 或 `wxq sessions --account ID --limit 100 --offset 0 --json` 确认范围，只使用输出里出现过的 `username`。
2. 会话不在白名单、或服务返回 401/400 时立即停止，把原文错误展示给用户，不要改用其他方式读取。
3. 需要历史消息时用 `--before` + `--before-id` 翻页（命令末尾会直接打印这两个值），不要一次请求过大的 `--limit`。
4. 明确区分原文、规则抽取和 Agent 自己的推断。
5. 不把聊天原文发送到其他服务，除非用户明确要求。
6. 聊天白名单为空时停止聊天读取，提示用户在桌面应用「导入管理」页配置；这不等于当前空间没有云端待办或报告，云端结果使用独立的空间和权限校验。

## 备注

`wxq` 只读，不解密、不写入、不发邮件；每次请求都由本机服务重新读取客户端设置并校验访问范围。`digest.todo` 是聊天规则素材，不是应用已生成的“我的待办”。若环境只支持 MCP 客户端而不便运行 shell，可用同一服务的 MCP 工具
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


## 读取已有待办与云端报告

先用安装时绑定的本机入口执行 `wxq health`，再执行 `wxq cloud-context --json`，发现 **当前** `space_id`、服务地址和接口限制。下面的 SPACE 和 ID 必须使用真实返回值，不猜 ID，不传服务端 URL。聊天来源 `--account` 不是云端空间。退出登录、切换服务／账号／空间后，重新发现上下文；不要使用旧结果或自行切换环境。

所有云端命令只读，数据来自当前客户端已登录的服务。只有 HTTP GET 数据查询，没有生成、完成待办、创建主题、上传、订阅或调度修改。空列表是成功但无结果；queued / partial / failed 是已有结果状态，不得自动重试生成；401 / 403 / 404 必须原样报告，不能当作空列表。

| 用途 | CLI | MCP 工具 |
|---|---|---|
| 当前云端空间和限制 | `wxq cloud-context --json` | `aimychats_get_cloud_context` |
| 待办列表 | `wxq todos --space SPACE --category my_todo --status unfinished --page 1 --page-size 25 --json` | `aimychats_list_todos` |
| 待办详情 | `wxq todo --space SPACE --id ID --json` | `aimychats_get_todo` |
| 已有报告列表 | `wxq reports --space SPACE --page 1 --page-size 25 --json` | `aimychats_list_reports` |
| 已有报告详情 | `wxq report --space SPACE --id ID --json` | `aimychats_get_report` |
| 主题订阅列表 | `wxq topics --space SPACE --page 1 --page-size 25 --json` | `aimychats_list_topics` |
| 主题订阅配置详情 | `wxq topic --space SPACE --id ID --json` | `aimychats_get_topic` |
| 某主题已有报告 | `wxq topic-reports --space SPACE --id TOPIC_ID --json` | `aimychats_list_topic_reports` |

MCP 列表与详情同样传 `space_id`；详情和主题报告传 `id`。CLI 的 `--page-size` 对应 MCP `page_size`，`--category` 对应 `viewer_relevance`，`--keyword` 对应 `q`。其他参数保持同名。

### 筛选与分页

- 待办个人分类：`my_todo`（我的待办）／`related`（与我有关）／`other`（其他）。完成状态与分类独立：`unfinished` 包括 open 和 in_progress，`done` 是已完成；全部状态省略 `--status`。不要把已完成待办改算到 related。
- 待办可加 `--keyword TEXT` 搜索（最多 200 字），与分类、状态及分页组合。若服务端未确认支持筛选，停止并报告契约缺口，不下载所有历史做本地筛选。
- 报告列表只支持 `--status queued|ready|partial|failed`。当前接口**没有报告类型、日期或主题的服务端筛选**；按页检查标题和 period_start / period_end，不因为第一页没有匹配就声称不存在。无法完整查阅时，说明已检查的页数和剩余范围。
- 列表每页默认 25、最多 100。`next_page` 非空时，以同一空间和筛选传 `--page NEXT`；不要跟随返回的 `pagination.next` URL。没有用户要求，不自动拉完整历史。
- topics 服务端返回有大小限制的完整数组，本机再分页，返回 `pagination_mode: local_over_server_array`；不是服务端分页。超过客户端 2 MiB 响应限制会明确失败，不静默截断。
- topic-reports 服务端最多返回最近 **50** 份，不接受 page。`history_complete: false` 时不能声称是完整历史。需要更早结果时，逐页浏览 reports，核对 **`source_info.subscription_id`**；不得根据同名标题猜主题归属。

### 列表 → 详情示例

- “我的未完成待办”：先 cloud-context → todos（my_todo + unfinished）→ 使用列表里的 id 调用 todo。保留 viewer_relevance、status、content、evidence、note_ids 等服务端字段；证据引用不代表已读取证据原文，不自行展开整个历史。
- “已经生成的上周周报”：先 reports 分页定位标题和实际 reporting period → report 获取 `item.body`、`item.sections` 和生成状态。类型以服务端字段为准；旧报告只有标题时说明依据标题识别，不伪造结构化类型。
- “合同跟进主题的报告”：先 topics 找到真实主题 ID → topic 读取订阅配置 → topic-reports 找到该主题报告 ID → report 获取完整正文。订阅 instruction/period 是配置，不能当作已生成报告正文；报告归属以 source_info.subscription_id 为准。

## 服务端契约限制（客户端不修改服务端）

已核对 oneprolabs/devify main `5e5264e02cd091ad6d56d96748afd961cc1e39fd`：GET `/api/v1/topics/{id}/reports` 可读取已有主题报告，但最多 50 份；topics 列表没有服务端分页；reports 只有 status 分页筛选，没有类型／日期／subscription_id 筛选，旧报告没有结构化类型。以上限制必须保留在回答中；若部署版本返回 404 或缺少关联字段，报告“不支持／无法确认”，不要猜测新路径，不触发 `/run` 的 POST。
