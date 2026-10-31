# WeChat Devify Agent Skills

这三个 Skill 由桌面应用 **AImyChats** 直接提供，不需要从仓库构建或手动复制：

- `wechat-chat-records`：白名单聊天记录读取与检索。
- `wechat-work-exploration`：跨会话探索、项目重点和 timeline。
- `wechat-daily-summary`：每日工作摘要草稿。

## 一条提示词安装

打开 AImyChats 的 **Agent 集成** 页，切到 **工具接入**：

1. 选择当前工具，确认 CLI 目录（如 `~/.local/bin`）和 Skills 目录（如 `~/.claude/skills`）。
2. 复制页面生成的提示词，粘贴到该工具的会话中，允许它执行 shell。
3. Agent 会从本机服务下载 `wxq` 和三个 `SKILL.md`，然后运行 `wxq health` / `wxq policy` 验证。

页面不会展示或使用 MCP。所有下载都在本机回环地址上完成，应用关闭后服务停止。

## 下载接口

| 用途 | 地址 |
|---|---|
| 清单 | `http://127.0.0.1:18765/agent/manifest` |
| 只读 CLI | `http://127.0.0.1:18765/agent/wxq` |
| Skills 清单 | `http://127.0.0.1:18765/agent/skills` |
| 单个 Skill | `http://127.0.0.1:18765/agent/skills/<name>` |
| 安装脚本 | `http://127.0.0.1:18765/agent/install.sh?bin=<目录>&skills=<目录>` |

服务地址由 `WECHAT_AGENT_BIND` 决定（默认 `127.0.0.1:18765`，旧名 `WECHAT_MCP_BIND` 仍兼容）；绑定非本机地址时必须同时设置 `WECHAT_AGENT_TOKEN`，下载请求也要带
`Authorization: Bearer <token>`。

## 手动安装（可选）

```bash
curl -fsSL 'http://127.0.0.1:18765/agent/install.sh?bin=~/.local/bin&skills=~/.claude/skills' | sh
```

脚本会先备份已存在的 `SKILL.md`，再写入新内容，最后运行 `wxq health`。只读约束不变：`wxq` 不读数据库、不解密、不发邮件，每次请求都由服务端重新校验白名单。
