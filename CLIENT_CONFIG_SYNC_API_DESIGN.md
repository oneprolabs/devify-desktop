# 客户端配置上报与服务端实现

状态：第一阶段已实现于客户端 `feat/durable-client-tasks` 和开发服务端 `feat/desktop-task-runtime`。开发 API 挂载代码自动加载，迁移已应用；不是生产环境发布。完整调用、参数和异常分支统一放在[生命周期时序图](ASYNC_TASK_DESIGN.md)及[离线 HTML](ASYNC_TASK_PREVIEW.html#flow-wizard)中。

## 权威和归属

本机 `client-tasks.sqlite` 是配置、profile、提交记录、任务和检查点的共同权威。服务端仅保存白名单配置副本，不提供云端恢复／冲突合并，也不通过配置副本授予本机来源权限。已完成配置启动复用本地，没有修改就不请求配置接口。

副本按认证用户、可上传空间、已授权设备和平台／来源账号隔离。会话 ID、首次备份范围、周期及附件偏好可上报；正文、媒体、密钥、凭据、路径、权限、暂停覆盖和执行游标均不在协议内。原有空间 workflow-config 的服务端处理规则不变。

## 配置事务与确认

每次有效修改先在本地事务保存候选和不可变提交请求，再串行执行 `PUT /api/v1/spaces/{space_id}/desktop/sync-config`。请求包括 schema_version=1、device_id、mutation_id、local_revision、target 和 payload；不是整份 app-settings.json。2 MiB 上限，未知字段拒绝；原始 JSON 解析保留会话映射键，不做 camelCase 转换。

服务端在一个事务内锁定已授权设备、查找回执、更新目标副本并保存回执。相同归属及 mutation_id 的同内容重放返回原结果；异内容返回 409。确认原样带回 mutation_id、local_revision、target 和 saved=true。local_revision 只关联本机请求，不参与多端排序。回执当前保留，不使用会导致旧请求重新生效的自动过期策略。

客户端只删除与确认对应的提交记录；响应丢失、重启都重放同一 key 和 body，后续修改排在它之后。当前实现不合并已持久化提交，最多保留 128 个待提交请求，达到上限会阻止继续堆积并提示恢复连接。网络失败显示本地已保存、等待同步，后台继续有界重试。

草稿和最近完成配置各自保存。完成向导产生新的 completed 请求，确认后发布 active 并允许该来源自动备份。draft／pending 阻止对应来源领取新包，浏览仍使用上一份 active。关闭或收窄范围使未发送工作失效；不删除已经发送批次的核对证据。服务／空间切换在本地事务中先归档旧 profile，恢复目标 profile，不能把旧空间的选择上报给新空间。

## 其余必要服务端能力

自动上传只查询本批的旧消息键：`POST /api/v1/spaces/{space_id}/desktop/message-keys` 最多 8 个会话、合计 1000 个键、256 KiB。按来源 namespace 查别名及可证明归属的规范消息键，不再下载全量远端聊天历史去查重。

本地明确缺失时，`GET /api/v1/spaces/{space_id}/desktop/conversation` 按平台、来源账号和会话 ID 精确定位。只返回已授权空间的匹配项；namespace 不匹配返回空，歧义返回 409。之后复用现有 page=last、每页最多 100 条、消息详情及媒体接口；权限／读库故障不被当作删除。媒体仍使用现有原文件接口，专用缩略图不在本轮。

上传结果不明时，`GET /api/v1/spaces/{space_id}/chat/uploads/recovery` 按认证用户、空间、设备、来源账号以及幂等键或归档摘要精确查找，两个条件只能传一个；歧义保留原任务并返回 409。客户端恢复原始 ZIP，不重新读取变化后的聊天来冒充原请求。已有批次幂等命中先于新上传配额检查，且重放不创建新来源记录。

## 开发环境与验证

服务端新增 DesktopBackupConfig、DesktopConfigReceipt 及 spaces/0003 迁移；本次使用既有开发容器和标准测试数据库。发现开发数据库缺少主分支的前置迁移，一并应用必要的 desktop-auth、ingestion worker-started-at 和 analysis 迁移；未应用与本功能无关的 billing／feedback 待迁移。通用 worker 和 scheduler 已重启，compose 中原已声明的上传／分析专用 worker 已启动，三个 worker 均响应 Celery ping。

验证包含配置回执重放／异内容拒绝、草稿与完成隔离、未知字段与权限、恢复归属、配额下幂等重放，以及 Rust 实际生成的微信／WhatsApp ZIP 经真实后端解析入库，再读取精确会话、消息和消息键。准确结果和测试边界见[实施记录](ASYNC_TASK_IMPLEMENTATION.md)。没有使用用户真实聊天或重置密钥／登录。

第二阶段仍见 [Issue #56](https://github.com/oneprolabs/devify-desktop/issues/56)：设备独立配置和上传，服务端跨设备消息去重。没有实现主设备、心跳占有、共享配置版本比对或双向同步。
