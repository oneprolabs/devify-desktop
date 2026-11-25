# AIMyChats Agent Skills

只提供一个 **`aimychats` Skill**，由桌面应用直接安装，不需要从仓库构建或手动复制。
它统一负责发现可读取的平台与账号、查找会话、分页读取和检索原文。总结、日报、项目分析
是用户智能体拿到这些材料之后的不同处理方式，无需分别安装 Skills。
同一 Skill 还可只读查看当前空间已有待办、日报／周报／月报、主题订阅和已有主题报告；这是客户端复用登录发起的远程读取，不触发生成。先用 `aimychats-cli cloud-context --json` 发现当前空间，按列表 → 详情读取。主题报告最多最近 50 份，完整契约限制见 Skill。

## 安装与范围

打开 AIMyChats 的 **智能体集成** 页，为检测到的智能体点击 **安装 Skills**。安装后重新打开智能体会话即可使用，无需输入端口或 Token。

Skills 只连接 AIMyChats 的统一本机只读接口。先用 `aimychats-cli accounts --json` 发现可读取的平台和账号，再携带 `--account ID` 读取会话；目前支持微信与 WhatsApp，权限、平台适配与账号变化校验均由客户端负责。会话和消息均分页读取。

升级时，应用管理的旧 `wechat-*` Skills 会在新版全部写入后移到应用数据目录的 `legacy-skill-backups`，避免智能体重复发现。无应用标记的旧 Skills 保留并提示用户处理；旧下载地址与 MCP 工具名仍兼容。

## 下载接口

| 用途 | 地址 |
|---|---|
| 清单 | `http://127.0.0.1:18765/agent/manifest` |
| 只读 CLI | `http://127.0.0.1:18765/agent/aimychats-cli` |
| 旧版兼容下载 | `http://127.0.0.1:18765/agent/wxq` |
| Skills 清单 | `http://127.0.0.1:18765/agent/skills` |
| 单个 Skill | `http://127.0.0.1:18765/agent/skills/<name>` |
| 安装脚本 | `http://127.0.0.1:18765/agent/install.sh?bin=<目录>&skills=<目录>` |

服务地址由 `WECHAT_AGENT_BIND` 决定（默认 `127.0.0.1:18765`，旧名 `WECHAT_MCP_BIND` 仍兼容）。服务只在登录且当前套餐包含本机 Agent 集成时启动，退出登录即停止。所有请求（包括本机回环）都要带 `Authorization: Bearer <token>`；Token 由桌面应用生成在 `<应用数据目录>/agent-token`（0600），`aimychats-cli` 与安装脚本会自动读取，`WECHAT_AGENT_TOKEN` 可覆盖。

## 手动安装（可选）

```bash
TOKEN=$(cat "$HOME/Library/Application Support/local.wechat.devifybridge/agent-token")
curl -fsSL -H "Authorization: Bearer $TOKEN" 'http://127.0.0.1:18765/agent/install.sh?bin=~/.local/bin&skills=~/.claude/skills' | sh
```

新版清单在 `canonical_cli` 中发布品牌命令；旧 `cli` 字段保留。脚本会备份变更的 `SKILL.md`，写入带完整命令路径的新内容，最后运行 `aimychats-cli health`。脚本不会覆盖自选目录里不同内容的同名程序；更新请用应用内「更新 Skills」。只读约束不变：`aimychats-cli` 不读数据库、不解密、不发邮件，每次请求都由服务端重新校验白名单。
