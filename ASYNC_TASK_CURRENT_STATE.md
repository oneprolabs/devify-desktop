# 客户端加载与刷新改造前对照附录

状态：改造前基线，核对起点 275c9a2，2026-11-19。下文四张图保留历史代码事实，不代表本轮改造后的实现；新行为见设计正文与实施记录。

目标流程与实施验收见[设计正文](ASYNC_TASK_DESIGN.md)。本附录供审查前后变化：本地消息、媒体、远端分页和浏览时的附带请求。IPC 中 `wechat_*` 也可能按账号分流 WhatsApp。HTTP 路径相对配置的服务地址，未确认的服务端能力不作已实现声明。

所有图统一为用户、客户端组（客户端／客户端数据库／客户端配置文件）与服务端。客户端内含界面、Tauri 命令和原生后台；没有把这些职责虚构为独立进程。

核对入口：[ChatWorkbench.tsx](src/ChatWorkbench.tsx)、[WeChatLocal.tsx](src/WeChatLocal.tsx)、[CloudRecords.tsx](src/CloudRecords.tsx)、[EvidenceDrawer.tsx](src/EvidenceDrawer.tsx)、[lib.rs](src-tauri/src/lib.rs)。改造前任务菜单没有全局状态输入，任务页仍独立定时刷新；页面快照最多 30 项、任务页快照和近期消息缓存在内存中，微信解密库、WhatsApp 快照及媒体已有落盘。

<!-- flow:flow-7 -->
## A.1 本地消息按需加载

现状：会话列表未分页；消息首次 100 条，更早消息按游标继续读取。

```mermaid
sequenceDiagram
    autonumber
    actor U as 用户
    box rgb(246, 247, 249) 客户端组
        participant C as 客户端
        participant D as 客户端数据库
        participant F as 客户端配置文件
    end
    participant S as 服务端
    Note over C,S: 当前实现，非目标方案；本图无服务端聊天 HTTP<br/>客户端包含 React 界面与 Tauri 命令，内存 Map 留在客户端，不冒充持久数据库
    C->>F: app_settings_get：读取全局配置
    C->>C: chat_accounts：发现本机微信／WhatsApp 账号
    loop 每个已连接账号
        C->>D: wechat_sessions(accountId)：读取全部已允许会话，当前无分页参数
        D-->>C: 全量账号列表，客户端合并排序
    end
    Note over C,D: 外层 ChatWorkbench 与内层 WeChatLocal 可能重复读取，当前无共享去重
    U->>C: 打开会话
    C->>C: 先看近期会话内存 Map，存在则先展示
    C->>D: wechat_messages(accountId, username, limit=100)
    D-->>C: 最新最多 100 条，与可复用内存缓存合并
    C-->>U: 展示消息
    opt 用户点加载更早
        U->>C: 加载更早
        C->>D: wechat_messages(limit=100, before, beforeId, beforeShard)
        D-->>C: 时间／本地 ID／分片游标对应一页，追加并保留位置
    end
    opt 关键词定位需补历史
        loop 额外最多 12 页，每页 100，找到或无下一页即止
            C->>D: wechat_messages，携带历史游标
            D-->>C: 下一页候选消息
        end
    end
    opt 显式跨会话搜索／整理工作内容
        U->>C: 搜索／收集
        C->>D: wechat_search 受 maxSessions 约束<br/>独立收集路径每页 500，每会话最多 5000
    end
    Note over C,D: 近期内存最多 10 会话／各 500 条，但当前打开列表可更多，未做 DOM 虚拟化<br/>关键词定位加首屏最多约 1300 条，focusMessageId 本身不触发该补页循环<br/>聊天源按账号分流微信解密副本或 WhatsApp 快照，不是一次取全历史
```

现状说明：

- 会话列表一次返回每账号全部已允许会话；消息正文首次最多 100 条。不是一次加载全部消息历史，也不是所有列表都分页。
- 普通历史加载由按钮触发，当前不是滚动到顶自动翻页；消息游标包含时间、本地 ID、分片，不能仅用时间戳代替。
- 关键词定位可能额外连续取最多 12 页，每页 100 条，加首屏最多约 1300 条；找到命中或没有下一页即停止。`focusMessageId` 本身不触发该关键词补页循环。
- 当前消息 DOM 渲染所有已加载消息，未采用可视窗口虚拟列表；不断加载历史会增加当前会话的 DOM 和内存。
- 近期缓存最多 10 个会话、每个 500 条，用于快速切回；当前打开会话加载的消息可能更多，此限制不是整个活动消息列表的总上限。
- “整理工作内容”的显式范围收集走另一条路径：每页 500、每会话最多 5000；它不是普通浏览首屏加载。
- 外层 ChatWorkbench 与内层 WeChatLocal 都可能读取会话，当前没有统一共享的会话请求去重。

<!-- /flow -->

<!-- flow:flow-8 -->
## A.2 头像与媒体加载

现状：附件按消息窗口解析；图片预览挂载后排队，不是严格可视区懒加载。

```mermaid
sequenceDiagram
    autonumber
    actor U as 用户
    box rgb(246, 247, 249) 客户端组
        participant C as 客户端
        participant D as 客户端数据库
        participant F as 客户端配置文件
    end
    participant S as 服务端
    Note over C,S: 当前实现，本图不调用业务 HTTP；媒体文件访问由客户端内部处理<br/>头像／图片不是严格可视区懒加载，不将文件／内存混称数据库
    C->>D: wechat_avatars(usernames)：外层前 200 会话，内层缺失每批 120
    D-->>C: 微信头像数据；WhatsApp 当前返回空列表
    alt 微信会话
        C->>C: 已加载消息时间窗口变化，防抖 350ms
        C->>D: wechat_message_media(accountId, username, since, until)<br/>后端解析整个窗口，最多读 20000 条消息
        D-->>C: 媒体索引，非全部附件内容
        opt 图片组件挂载且无预览缓存
            C->>C: IPC wechat_media_preview(accountId, localId, paths)<br/>组件内队列最多 3 个，生成／复用本地预览文件
            C->>C: convertFileSrc：WebView 读取本地文件资源
            C-->>U: 展示预览
        end
        opt 用户打开原图／播放或转写语音
            U->>C: 打开媒体
            C->>C: wechat_media_open_image／wechat_voice_audio／wechat_voice_transcribe<br/>本地解码、读取与本地转写进程，缓存产物，不是云端转写 API
        end
    else WhatsApp 会话
        C->>C: 当前跳过微信附件解析，不能宣称已实现相同媒体支持
    end
    Note over C,D: 更早消息扩大窗口，可能重复扫描；首屏 100 不是附件扫描上限<br/>当前没有 IntersectionObserver 可视门槛，只有视频封面使用 loading=lazy
```

现状说明：头像不是严格的可视区懒加载；图片预览在组件挂载时排队生成，当前没有 IntersectionObserver 可视区门槛。只有视频封面使用浏览器 `loading="lazy"`。因此“消息分页”“附件按消息窗口解析”“图片可视区懒加载”是三个不同概念。翻到更早消息会扩大附件扫描时间范围；当前不是仅扫描新增一页。微信附件解析后端窗口读取上限为 20000 条，不能把消息首屏 100 条视为该扫描上限。

<!-- /flow -->

<!-- flow:flow-9 -->
## A.3 远端聊天分页

现状：会话每页 25，消息每页 100；普通打开先探测总数再读最新页。

```mermaid
sequenceDiagram
    autonumber
    actor U as 用户
    box rgb(246, 247, 249) 客户端组
        participant C as 客户端
        participant D as 客户端数据库
        participant F as 客户端配置文件
    end
    participant S as 服务端
    Note over C,S: 当前实现，客户端含聊天页／证据抽屉与 Tauri 命令<br/>已有 HTTP 分页，尚未接统一持久页面缓存；本图不写客户端数据库／配置文件
    U->>C: 进入远端聊天或搜索，搜索防抖 300ms
    C->>S: devify_conversations：GET /api/v1/spaces/{space_id}/conversations<br/>query: page=当前页、page_size=25、search=keyword
    S-->>C: 当前页会话与总数
    U->>C: 打开远端会话
    opt 普通打开，无指定页或 focusMessageId
        C->>S: devify_conversation_messages<br/>GET /api/v1/spaces/{space_id}/conversations/{conversation_id}/messages<br/>query: page=1、page_size=1，探测 total
        S-->>C: 总数，用于计算最新页
    end
    C->>S: 同一 messages 路径：page=目标页、page_size=100<br/>可附 focus_message_id；指定页／焦点跳过总数探测
    S-->>C: 目标页最多 100 条与分页信息
    C-->>U: 展示当前页，不取全部历史
    opt 证据会话不在已加载列表
        C->>S: devify_conversation_detail<br/>GET /api/v1/spaces/{space_id}/conversations/{conversation_id}
        S-->>C: 直接返回会话详情
    end
    opt 入口需要精确消息详情
        C->>S: devify_message_detail<br/>GET /api/v1/spaces/{space_id}/conversations/{conversation_id}/messages/{message_id}
        S-->>C: 单条详情
    end
    opt 证据抽屉点击更早
        U->>C: 加载更早
        C->>S: 同一 messages 路径：page=前一页、page_size=100
        S-->>C: 一页，客户端前置合并并保留位置
    end
    Note over C,S: 普通打开通常两次消息 HTTP；一次 IPC 不等同一次 HTTP
```

现状说明：搜索防抖 300ms；远端会话每页 25、消息每页 100，首次普通打开先用 `page_size=1` 获取总数，再读取最新页，通常产生两次消息 HTTP 请求。已有指定页或 `focusMessageId` 时跳过这个探测。证据抽屉同样每次 100，带焦点读取目标页，“更早”按钮请求前一页并前置合并；目标会话不在已加载列表时通过详情接口直接读取。

2026-11-19 只读补核对：服务端 f64da4424d08 已支持 page=last，test_api.py:406 有对应源码用例；当前两次请求是客户端数字 page 参数与计算逻辑造成的，不是服务端必须先探测。消息列表 preview 最多 280 字、单条详情才含完整 parts，当前云端聊天页只渲染 preview。目标改造见正文第三流程，以上现状时序保持按当前客户端代码描述。

<!-- /flow -->

<!-- flow:flow-10 -->
## A.4 浏览时的附带刷新

现状：本地更新、上传标记、事件与批次状态分别刷新，当前未统一调度。

```mermaid
sequenceDiagram
    autonumber
    actor U as 用户
    box rgb(246, 247, 249) 客户端组
        participant C as 客户端
        participant D as 客户端数据库
        participant F as 客户端配置文件
    end
    participant S as 服务端
    Note over C,S: 当前实现，非目标统一调度；以下为同一客户端内互相独立的入口<br/>页面隐藏仍可能工作；未使用客户端配置文件作高频网络刷新状态库
    loop 本地即时更新：进入立即一次，之后 10 至 30 秒
        C->>D: 微信 wechat_sync 增量解密／复用副本；WhatsApp 跳过解密<br/>随后 wechat_sessions 与 wechat_messages(limit=100)
        D-->>C: 会话与最新消息
    end
    Note over U,C: useEffect 回调变化可能重启并立即执行；同组件有锁，跨组件未统一<br/>约 400ms 后挂载隐藏原始聊天面板，也可能工作
    loop 上传标记：活跃时 5 秒，否则 60 秒
        C->>S: wechat_upload_state／verified_upload_identity<br/>GET /api/v1/auth/user，然后 GET /api/v1/auth/desktop/devices
        S-->>C: 当前用户与本设备有效登记（不是用户只能有一台 active 设备）
        opt 凭据需刷新
            C->>S: POST /api/v1/auth/desktop/session/refresh<br/>或 POST /api/v1/auth/token/refresh
        end
        opt 旧格式、缺少归属的上传历史
            C->>S: adopt_legacy_upload_history：GET /api/v1/chat/uploads/{batch_id}<br/>每批最多 8 并发，总预算 20 秒，仅接纳归属与终态匹配者
        end
        C->>D: 验证已确认覆盖；释放身份读锁后扫描本地位置
        D-->>C: synced／pending／local／unknown
    end
    opt 当前会话变化，原始聊天页签也可能触发
        loop 查关联会话，最多 100 页
            C->>S: devify_conversations：GET /api/v1/spaces/{space_id}/conversations<br/>query: search=username、page、page_size=100
            S-->>C: 候选页，匹配同平台／来源
        end
        loop devify_analysis_list(kind=events) 一次 IPC 内最多 100 页
            C->>S: GET /api/v1/spaces/{space_id}/events<br/>query: page、page_size=100、conversation_id
            S-->>C: 当前页事件，客户端汇总，未要求事件页签激活
        end
    end
    opt 备份提交后等解析：约每 5 秒
        C->>S: devify_upload_status：GET /api/v1/chat/uploads/{batch_id}
        S-->>C: 批次状态，另有原生后台核对入口
    end
    opt 任务页：重建时 2 秒／活跃 10 秒／其他 30 秒
        C->>S: devify_upload_list：GET /api/v1/spaces/{space_id}/chat/uploads<br/>query: page、page_size=pageSize
        S-->>C: 上传列表
        Note over C,S: 首次前后还重复核对：devify_current_user／devify_upload_identity<br/>GET /api/v1/auth/user 与 GET /api/v1/auth/desktop/devices<br/>app_settings_get 则只读客户端配置文件，无 HTTP
    end
    opt 设备登记／心跳：约 2 分钟及聚焦
        C->>S: devify_device_register：POST /api/v1/auth/desktop/devices<br/>随后 GET /api/v1/auth/desktop/devices 核验
        S-->>C: 有效登记；失败可能再触发 devify_auth_status
    end
    Note over C,D: 原生备份每轮结束再等 30 秒，与页面计时器独立<br/>固定 setInterval 未按完成后再计时／跨入口合并，存在重叠；在线阈值未验证
```

现状风险：即时更新 useEffect 依赖消息相关回调，变化可能重启并立即触发；同组件有同步锁，但页面隐藏不等于取消此轮询。上传标记固定 setInterval 缺少完成后再计时和跨入口合并；组件挂载约 400ms 后会挂载原始聊天面板，隐藏状态仍可能发起工作。聊天关联事件的读取未按当前页签限定；`devify_analysis_list` 一次 IPC 内部可连续拉最多 100 页，每页 100，不能把 IPC 一次等同于 HTTP 一次。本轮仅记录，不修改运行实现。

其他已有周期：设备登记/心跳约 2 分钟及聚焦；任务页列表刷新为重建时 2 秒、活跃时 10 秒、其他 30 秒；原生备份主循环每轮完成后再等 30 秒；这些不是同一个调度入口。

<!-- /flow -->

