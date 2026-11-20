# 客户端生命周期与异步任务设计

状态：第一阶段已按本文件实现，分层自动回归与代码复核已通过；新版原生客户端连接开发环境的完整端到端验收尚未完成（2026-11-19）。第一、第二流程保留用户的绿色“已审核”标记；其余流程是本轮实现复核，不代替用户验收。实现证据与测试边界见 [实施记录](ASYNC_TASK_IMPLEMENTATION.md)。

阅读入口：[HTML 预览](ASYNC_TASK_PREVIEW.html)。以下九个流程顺序为登录 → 配置 → 浏览 → 自动任务 → 暂停 → 注销 → 来源隔离 → 异常恢复 → 清理。所有接口方法、路径和依赖都写在时序图中。改造前的四张图留在 [对照附录](ASYNC_TASK_CURRENT_STATE.md)，明确是历史基线。

## 1. 第一阶段的边界

客户端保存本机配置，服务端只保存幂等副本。用户配置时选择会话、首次时间范围及备份周期；配置完成后后台自动执行，预览不是上传前置，不新增聊天内单独上传流程。微信与 WhatsApp 共用现有统一备份周期，选择和首次范围按来源保存。旧版关闭／手动设置不会在升级时自动开启。

首期建立的是自动备份任务体系，不建设通用 DAG 或分布式调度。长转写、批量导出和 Agent 工作沿用现有功能，没有冒充已全部接入持久恢复。多设备独立上传及跨设备去重继续在 [Issue #56](https://github.com/oneprolabs/devify-desktop/issues/56) 跟踪；远端备份主动删除见 [Issue #55](https://github.com/oneprolabs/devify-desktop/issues/55)。

## 2. 统一实体、存储和简单规则

所有图只用 **用户｜客户端组〔客户端、客户端数据库、客户端配置文件〕｜服务端**。客户端是同一个 Tauri 进程的 React、IPC 和后台代码，没有虚构独立客户端 API／Worker 进程。

客户端数据库中的 `client-tasks.sqlite` 是配置、用户 profile、草稿／active、提交请求、任务、暂停和检查点的共同权威。设置与任务在同一事务内提交，`synchronous=FULL`，没有 JSON 与任务库各一份权威的问题。JSON 只用于旧版导入和可读配置镜像；发现已有 SQLite 损坏时不会回退旧 JSON。已访问元数据缓存也放在这份库的 `read_cache`，有独立数量和容量上限。

客户端配置文件只表示凭据、密钥、配置镜像等文件。原始聊天库只读；解密副本、WhatsApp 快照、图片／语音缓存和原始上传 ZIP 属于本地文件，不塞进配置请求。密钥按服务用户隔离、0600 和原子写入；本次没有宣称静态加密或操作系统凭据库已经完成。

每次保存先本地持久化，再按顺序上报。请求一旦入队就冻结 key 和 body；响应只确认自身。最后完成向导是新请求；服务端确认 completed 后才启用对应来源的新任务。已有 active 仍用于浏览，draft／pending 停止该来源新包。消息检查点只在服务端 completed 后推进，不把接收归档误当解析完成。

网络、读库和压缩不持有身份读锁。常规本地列表／消息读取最多两个；上传状态读取也受同一预算约束，聊天页隐藏后停止轮询，同一页面不重叠扫描。后台准备与上传由一个协调器串行，各来源最多一个未完成包；配置提交与只读远端核对每 30 秒独立运行，不等待本地解密／转写完成。本机执行才显示并转动任务圆环，等待云端、排队和暂停只显示普通任务图标。图片和头像进入可视区或一屏范围才激活。


<!-- flow:flow-1 -->
## 3. 启动与登录

审核：已审核（用户确认，2026-11-19）。

目标：先确认登录与本机配置归属，再允许浏览和后台任务；登录不等上传结束。

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
    U->>C: 启动客户端／登录
    C->>C: 取得数据目录的操作系统实例锁<br/>锁随进程结束释放，残留 lock 文件不阻止启动
    C->>D: app_settings_get：读取服务地址与未完成注销标记
    opt 上次注销未完成
        C->>F: 先清除旧登录凭据
        C->>D: 完成注销清理；失败保留标记并停止恢复登录
    end
    C->>F: 读取当前服务的本地登录凭据
    alt 已有可用凭据
        opt access 即将到期，刷新请求合并为一个在途
            C->>S: POST /api/v1/auth/desktop/session/refresh<br/>或 POST /api/v1/auth/token/refresh
            S-->>C: 新凭据／明确失效／暂不可用
            C->>F: 仅在原登录代次仍有效时保存
        end
    else 用户开始登录
        alt 浏览器授权
            C->>S: GET /api/v1/auth/desktop/config
            S-->>C: 授权地址、换码路径、回调地址
            C-->>U: 打开系统浏览器授权
            U->>C: 本机回调，校验 state、PKCE、登录代次
            C->>S: POST 服务配置的 token_path
            S-->>C: 当前用户与设备的登录凭据
        else 密码登录
            C->>S: POST /api/v1/auth/login
            S-->>C: 登录凭据
        end
        C->>F: 短事务保存凭据，不持锁等待网络
    end
    par 独立身份读取
        C->>S: GET /api/v1/auth/user<br/>同用户短时缓存，供账户显示
        S-->>C: 用户身份
    and 独立空间读取
        C->>S: GET /api/v1/spaces
        S-->>C: 可访问空间，随后选择有效空间
    and 独立权限读取
        C->>S: GET /api/v1/me/entitlements
        S-->>C: 权限快照；按服务用户隔离，旧登录结果不得写入
    end
    opt 需要设备登记或续约
        C->>S: POST /api/v1/auth/desktop/devices<br/>沿用已有注册／心跳，合并同时触发
        S-->>C: 当前用户的设备登记
    end
    C->>D: 按服务＋用户＋空间切换本地配置 profile<br/>新用户无配置时使用空选择、关闭自动备份；旧用户切回恢复自己的配置
    C-->>U: 进入配置检查，或显示准确的登录失败与重试入口
    Note over C,S: 三项独立读取并发；设备／空间后续写入遵循真实依赖<br/>网络暂不可用不删除凭据，401 与迟到结果按原登录代次处理<br/>SQLite、短凭据事务与长上传之间没有读锁依赖
```

<!-- /flow -->

<!-- flow:flow-wizard -->
## 4. 本机配置、平台向导与任务交接

审核：已审核（用户确认，2026-11-19）。

目标：客户端是配置权威；每次有效保存先本地提交，再上报，完成向导与远端确认后自动交接。

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
    C->>D: 读取当前 profile、来源完成标记、active 与 draft／pending
    alt 本机已有已完成配置
        C->>C: 复用本地配置，不下载或合并云端设置
    else 尚未配置
        C-->>U: 进入微信／WhatsApp 向导，保留草稿
    end
    U->>C: 选择会话、首次范围（默认最近 7 天）与统一备份周期
    C->>D: app_settings_save：同一事务保存候选、不可变请求与 mutation_id<br/>草稿不覆盖 last completed active；同来源新批次等待
    C-->>U: 按钮 loading／防重复，页面保持可响应
    loop 按顺序提交当前用户空间的 pending，确认前不发后一个
        C->>S: PUT /api/v1/spaces/{space_id}/desktop/sync-config<br/>schema_version、device_id、mutation_id、local_revision、target、payload<br/>payload 只含 setup_state、enabled、会话 ID／首次范围、周期和附件偏好
        S->>S: 校验用户、空间、active 设备、字段与 2 MiB 上限<br/>同键同内容返回原回执；同键异内容 409；副本与回执同一事务
        S-->>C: 原 mutation_id、local_revision、target、saved
        alt 匹配的保存确认
            C->>D: 只移除本次 pending；更晚修改仍待提交<br/>draft 只更新草稿，completed 发布 active 与完成标记
        else 断网／超时／服务端拒绝
            C->>D: 保留原键和完整请求，不把本地保存冒充同步成功
            C-->>U: 说明本地已保存、尚待同步；调度器有界重试
        end
    end
    U->>C: connector_setup_state：最后一步保存并完成<br/>completed 使用新请求键，不能复用 draft 的键
    C->>D: 记录来源完成候选；确认后 ready，已有 key／文件不能代替完成
    C->>C: 唤醒唯一后台调度器；无需预览或再点上传
    Note over C,S: 无 base_revision、云端冲突选择、GET sync-config 或 applied<br/>关闭／收窄立即中止未发送的旧候选；未完成重配置只停止该来源新批次<br/>首次选择、周期和全部 ID 持久保存，不从当前页推导同步谁
    Note over C,F: F 只保存凭据、密钥及 JSON 配置镜像；配置／任务的权威都在 SQLite<br/>两平台使用同一备份周期，选择集和首次范围分别保存；旧手动／关闭设置不自动开启
```

<!-- /flow -->

<!-- flow:flow-cache -->
## 5. 本地会话浏览与按需备份读取

审核：实现复核，待用户验收。

目标：默认只读有数量上限的本地记录；打开具体会话且确认本地缺失，才请求远端备份。

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
    U->>C: 进入聊天
    C->>D: app_settings_get／chat_accounts：读取完整选择与来源摘要
    par 独立来源读取，原生最多两个常规读任务
        C->>D: chat_sessions：微信 SQL 按选择过滤，limit 50、offset
    and
        C->>D: chat_sessions：WhatsApp SQL 按选择过滤，limit 50、offset
    end
    D-->>C: 有界会话摘要；可用上次本地元数据保留缺失会话入口
    C-->>U: 显示列表；首次打开最近活动的第一条，返回保留当前聊天<br/>不预取其他会话正文、事件／报告／整页图片
    opt 用户加载更多
        U->>C: 下一页
        C->>D: 只读下一页；驻留列表最多 200 个摘要
    end
    U->>C: 切换指定会话（或首次默认选择）
    C->>D: wechat_messages：按源账号分流两平台<br/>首次／下一页最多 100 条，时间＋ID＋分片游标
    alt 本地读取成功
        D-->>C: 当前消息页，驻留消息窗口最多 200 条
        C-->>U: 展示本地记录
    else 本地权限不足、损坏或读取错误
        D-->>C: 明确错误，不冒充缺失
        C-->>U: 显示原因与处理入口，不自动读远端
    else 已确认本地无该段记录
        D-->>C: LOCAL_CHAT_MISSING
        C->>S: GET /api/v1/spaces/{space_id}/desktop/conversation<br/>platform、source_account_id、source_conversation_id 精确查询
        S-->>C: 唯一会话 ID／未找到／归属或权限错误
        opt 找到可访问的准确会话
            C->>S: GET /api/v1/spaces/{space_id}/conversations/{id}/messages<br/>page=last、page_size=100，之后只读用户打开的页
            S-->>C: 摘要与媒体引用，不是全部正文／图片
            C-->>U: 标明正在查看云端备份
            opt 用户展开完整消息
                C->>S: GET /api/v1/spaces/{space_id}/conversations/{id}/messages/{message_id}
                S-->>C: 完整 parts
            end
            opt 图片进入可视区或一屏范围
                C->>S: 原生鉴权 GET /api/v1/spaces/{space_id}/chat-media/{sha256}<br/>或 /media/{item_id}；最多两个请求，每张 8 MiB 接收上限
                S-->>C: 图片字节／明确超限；凭据不进入 URL
            end
        end
    end
    Note over C,D: chat_sessions 已替代历史 wechat_sessions，所有注册与调用方同步迁移<br/>配置与最后已访问摘要在 SQLite；read_cache 最多 200 项／20 MiB<br/>本地图片和头像也按可视区触发，不因缓存命中提前注入真实地址
    Note over C,S: 通用 JSON 接收上限 2 MiB；响应超限明确停止<br/>服务端没有专用缩略图／字节分页，不能宣称所有大图片或长正文均已解决<br/>远端 404 可隐藏权限，不断言原记录已删除；迟到的旧身份结果不采用
```

<!-- /flow -->

<!-- flow:flow-2 -->
## 6. 自动备份与任务菜单

审核：实现复核，待用户验收。

目标：配置完成后由后台自动调度，按来源轮转，以多会话小批次上传，任务页只展示本地状态。

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
    C->>D: 唯一调度器读取 completed active、pending、暂停与上次执行位置
    C->>C: 登录／授权有效，来源未重配置，已到期且网络允许才准备新包<br/>来源轮转；同来源一个流水线，错过周期合并，不堆积时间片
    C->>D: 核对该来源尚未完成的已知批次
    opt 有已知远端批次
        C->>S: GET /api/v1/chat/uploads/{batch_id}<br/>核对用户、空间、设备、平台、源账号与摘要
        S-->>C: processing／completed／failed
        C->>D: 只有 completed 才事务提交该包各会话检查点<br/>失败保留原证据，用户可重试原任务
    end
    alt 来源仍有云端处理中批次
        C->>C: 本轮等待，不继续准备更多包
    else 可以执行新的到期增量
        C->>D: 每轮最多检查 8 个会话，读取有界的 oldest-first 页<br/>一个包可含多个会话，合计最多 200 条
        opt 微信本地副本需要更新
            C->>C: 在阻塞线程按已有密钥增量解密；不修改原始聊天库
        end
        opt 本批微信消息需要兼容旧标识
            C->>S: POST /api/v1/spaces/{space_id}/desktop/message-keys<br/>只查询本批 conversation＋候选 key，禁止全量下载历史
            S-->>C: 当前来源命中的旧 key／时间
        end
        C->>C: 生成包，附件合计预算 32 MiB、ZIP 64 MiB<br/>同批不混用户／空间／平台／源账号；原包先落盘并校验摘要
        C->>D: 原子记录任务、精确选择、检查点候选、create_key 与 digest<br/>发送前已持久保存 create_started
        C->>S: POST /api/v1/spaces/{space_id}/chat/uploads<br/>固定 idempotency_key、archive_sha256、archive_size、device_id、来源、scheduled
        S-->>C: 新批次或同请求原批次；已有幂等结果先于新上传配额
        C->>D: 先保存 batch_id，再传输
        C->>S: PUT /api/v1/chat/uploads/{batch_id}/archive<br/>原 ZIP，不在响应丢失后换包换键
        S-->>C: 归档接收结果
        C->>S: POST /api/v1/chat/uploads/{batch_id}/complete
        S-->>C: processing／completed；后台只核对，不提前推进游标
    end
    C-->>U: task-runtime 本地快照／事件，菜单只在本机执行时转圈<br/>排队／暂停／云端处理中隐藏圆环；打开任务页不另起轮询
    opt 用户明确查看云端历史
        U->>C: 查看云端历史／下一页
        C->>S: GET /api/v1/spaces/{space_id}/chat/uploads?page=N&page_size=20
        S-->>C: 仅当前页
    end
    Note over C,S: CPU 准备与上传串行，每来源最多一个未完成批次；两平台公平推进<br/>临时失败按来源退避，服务端明确失败等用户重试，不无限重建相同坏包<br/>本次实现常规备份任务，不宣称长转写、导出、Agent 等已全部可断点恢复
```

<!-- /flow -->

<!-- flow:flow-5 -->
## 7. 暂停、恢复与失败重试

审核：实现复核，待用户验收。

目标：暂停是可选控制，持久保留；已发送批次仍可核对，不把云端处理误称为本机正在执行。

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
    U->>C: task_runtime_pause(paused=true)
    C->>D: 持久保存当前 profile 的暂停状态
    C->>C: 取消代次失效；停止新准备／发送，真实执行结束后才释放资源
    C-->>U: 显示已暂停，任务菜单静态；登录与浏览继续可用
    opt 已有批次正在服务端处理
        C->>S: GET /api/v1/chat/uploads/{batch_id}
        S-->>C: 完成状态；本机只对账，不申请服务端暂停
        C->>D: 已确认完成才保存进度
    end
    U->>C: task_runtime_pause(paused=false)
    C->>D: 清除暂停标记；普通登录／重启不替用户恢复
    C->>C: 唤醒调度器，重新核对归属、配置、网络与周期
    opt 某批次明确失败，用户点击重试
        U->>C: task_runtime_retry(job_id)，防重复按钮
        C->>S: GET /api/v1/chat/uploads/{batch_id}<br/>先确认原批次归属
        C->>S: POST /api/v1/chat/uploads/{batch_id}/retry<br/>只重试原任务，不重新选择或换幂等键
        S-->>C: 已安排／错误
        C->>D: 保存等待核对状态；尚未发送的本机失败只唤醒原候选
    end
    Note over C,S: 服务端没有本阶段专用暂停 API；本机暂停不能撤回已发送的解析<br/>暂停跨重启保留；失败、等待与执行分别显示，错误不靠永久 loading 掩盖
```

<!-- /flow -->

<!-- flow:flow-3 -->
## 8. 注销与重新登录

审核：实现复核，待用户验收。

目标：注销只等短落盘与凭据清理，不等上传、压缩或长 HTTP；旧任务结果不能改写新登录。

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
    U->>C: devify_logout
    C->>C: 立即使当前上传取消代次失效
    C->>D: 先记录 logout_pending，保存未知请求／原归档证据<br/>把正在执行的本机任务记为中断
    C->>F: 短身份事务清除本地凭据，推进凭据代次
    C->>D: 清除 logout_pending；配置、原聊天和备份不删除
    C-->>U: 已退出，可立即开始新的登录
    par 尽力撤销旧会话，不阻塞界面
        C->>S: DELETE /api/v1/auth/desktop/session/refresh<br/>只使用捕获的旧 device_id／session，有超时
        S-->>C: 成功／失败；都不恢复旧凭据
    and 旧工作可能迟到
        C->>C: 每次发送前、写结果前重查原 owner／token／代次<br/>迟到成功不能写入新 profile 或确认新配置
    end
    U->>C: 登录另一账号
    C->>D: 按新服务用户空间激活独立 profile
    C-->>U: 清空旧用户任务快照，旧 context／revision 事件不采用
    Note over C,F: 凭据文件与 SQLite 无跨存储事务；若进程死在注销中间<br/>下次启动先完成清理再读取凭据，失败停止恢复，不假报已退出<br/>进程内锁随退出消失，磁盘任务记录不是登录锁
```

<!-- /flow -->

<!-- flow:flow-key-scope -->
## 9. 登录用户、来源账号与本地密钥

审核：实现复核，待用户验收。

目标：切换 AIMyChats 用户或本机聊天账号时，分别校验来源，不继承旧用户的配置、密钥或明文副本。

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
    U->>C: 从 A 切换到 B，或切换本机微信／WhatsApp 账号
    C->>D: 切换服务＋user_id＋空间的 profile<br/>选择、周期、完成标记、进度与暂停分别保存
    C->>F: 定位服务用户隔离的 key 文件与明文副本目录<br/>新用户无权沿用 A 的 key／解密结果
    alt 旧版本数据有已核验 owner 证据
        C->>F: 只为对应用户记录不可反向改写的 legacy 数据归属<br/>原文件保留，不盲目复制给首次登录者
    else 归属未知或新用户
        C->>C: 使用新的用户目录，保留旧数据待原用户处理
    end
    C->>C: 校验当前来源账号与配置 owner<br/>WhatsApp 本机账号不匹配则等待重新连接，不冒用旧 account_id
    alt 已有该用户已完成来源配置及有效本机权限
        C->>C: 只读当前来源，按其范围调度
    else 尚未完成／授权或密钥缺失
        C-->>U: 指引完成该来源向导；不重置另一平台
    end
    Note over C,S: 无 key 上传接口，配置协议拒绝秘密／路径／额外字段<br/>数据库密钥不是客户端专属加密材料；复制有效 key 仍可能解密原库<br/>本次实现目录隔离、0600 与原子写入，没有声称已实现 Keychain／硬件绑定
    Note over C,F: 语音缓存、图片预览与本地摘要也随用户目录隔离<br/>旧登录异步准备使用事先冻结的 private_data，不临时解析成新用户目录
```

<!-- /flow -->

<!-- flow:flow-4 -->
## 10. 异常退出、断电与重启恢复

审核：实现复核，待用户验收。

目标：每步以最后持久证据恢复，未知上传先核对原请求，不盲目生成新任务或提前推进进度。

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
    U->>C: 强制结束进程／断电后重新启动
    C->>C: OS 实例锁重新取得，不依赖删除残留锁文件
    C->>D: SQLite 回滚未提交事务；验证版本和状态<br/>损坏／不支持时保留现场，不回退旧 JSON 重传
    C->>F: 如有未完成注销，先清凭据，再进入正常登录
    C->>D: 登录后读取当前 profile 的配置 pending、暂停、任务与检查点
    opt 配置请求结果未知
        C->>S: PUT /api/v1/spaces/{space_id}/desktop/sync-config<br/>重试同 mutation_id、完整原请求
        S-->>C: 原保存回执；不能重新覆盖更新的副本
    end
    alt 已有 batch_id
        C->>S: GET /api/v1/chat/uploads/{batch_id}
        S-->>C: 上传中／处理中／完成／明确失败
    else 新任务保留了原 ZIP、digest 和 create_key
        C->>C: 校验原包摘要，不重读全部历史重建不同原包
        C->>S: POST /api/v1/spaces/{space_id}/chat/uploads<br/>重放同幂等请求，找回原批次
        S-->>C: 原 batch_id
    else 旧版本只有未知发送证据
        C->>S: GET /api/v1/spaces/{space_id}/chat/uploads/recovery<br/>device_id、source_account_id、idempotency_key 或 archive_sha256
        S-->>C: 精确原批次、archive_received 或无法确定
        Note over C,S: 兼容旧服务器只作有界旧查询，核对不完就等待<br/>绝不因超时断言未发送，不换键盲传
    end
    opt 原批次仍缺归档／提交，且当前来源允许继续
        C->>S: PUT /api/v1/chat/uploads/{batch_id}/archive<br/>POST /api/v1/chat/uploads/{batch_id}/complete
    end
    C->>D: completed 核对后事务应用各会话检查点一次<br/>processing／失败／未知不推进；暂停状态不自动清除
    Note over C,D: 配置、任务、发送意图与进度共用 SQLite FULL 同步事务<br/>原 ZIP 先 fsync 再记录引用，孤儿包可清理；未知任务引用包必须保留<br/>普通缓存可丢弃，完成标记与检查点不是缓存
```

<!-- /flow -->

<!-- flow:flow-6 -->
## 11. 空闲清理与容量保护

审核：实现复核，待用户验收。

目标：空闲时自动回收已确认历史与可重建缓存，始终保留未知任务、原包与用户配置。

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
    C->>C: 调度回合结束，确认没有本机上传／准备正在占用资源
    C->>D: 有界维护，最多每分钟一次<br/>已验证完成且无待应用效果的历史：保留最近 500 条或 30 天
    C->>D: read_cache 写入时控制最多 200 项、20 MiB<br/>活动配置／全部选择／暂停／检查点／未确认请求不走 TTL
    C->>C: 按数据库引用核对归档目录，每轮最多检查 100 项<br/>已完成无引用包可删；未知包／处理中包保留，孤儿包等一天
    C->>C: 新 ZIP 最多 64 MiB，待确认归档总预算 512 MiB<br/>满额只等待释放，不删除不确定证据来继续上传
    opt 任务元数据库大于 4096 页且空闲页超过四分之一
        C->>D: 空闲时 VACUUM，只处理任务元数据库
    end
    Note over C,F: 不删除原微信／WhatsApp 库、keys、当前配置、有效草稿、暂停状态或未确认归档<br/>清理与原包持久引用串行，避免边写边删；清理失败保留数据，不影响登录<br/>远端备份主动删除另见 Issue 55，本地清理不调用远端删除接口
```

<!-- /flow -->


## 12. 实施定位与验证边界

存储与恢复见 `task_store.rs`、`upload_archive.rs`、`instance_lock.rs`；调度见 `task_runtime.rs`；配置请求、profile 与任务 IPC 见 `config_runtime.rs`、`runtime_commands.rs`；界面使用 `taskRuntime.ts` 的共享快照和事件。通用会话入口已更名 `chat_sessions`，SQL 过滤和数量限制在两平台原生查询中完成。原始系统授权、逐库密钥校验、官方微信恢复流程继续沿用已实现逻辑。

服务端新增配置副本、精确批次恢复、本批消息标识与准确会话查询，见 [配置协议与服务端改动](CLIENT_CONFIG_SYNC_API_DESIGN.md)。控制端本轮没有新增设置编辑页，也不能反向覆盖本机配置。开发栈按既有专用解析／分析队列部署，不能只重启默认 worker 却让专用队列无人消费。

数量分页不能自动证明所有异常体积都安全：通用远端 JSON 接收有 2 MiB 上限，原生来源字段仍依赖现有解析器；专用缩略图、服务端字节分页、任意超长正文批量读取与完整 OS 凭据库仍是后续能力。不会把暂不可用、权限不足或数据库损坏说成记录已删除。测试使用独立本地源库和原生生成的合成压缩包，保留真实用户登录、密钥和聊天；不把前端 IPC 隔离测试说成重新走过真实 macOS 授权。
