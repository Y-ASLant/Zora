# Zora Transport 技术规格

## Context

当前 `crates/zap_sftp` 是同步 `ssh2` 薄封装，入口在 `src/session.rs`、`src/sftp.rs`、`src/file.rs` 和 `src/types.rs`。`app/src/sftp_manager/sftp_ops.rs` 负责认证解析、目录递归和 32 KiB 文件复制；`app/src/sftp_manager/sftp_backend.rs` 又把协议操作转换成 UI 类型。`app/src/sftp_manager/browser.rs` 在 `transfers: Vec<TransferTask>` 中维护任务，并用 `AtomicU64` 只保存后台进度，完成前没有稳定的 UI 刷新通道。传输面板位于 `transfer_panel.rs`，现有动作只有取消，没有暂停、恢复和重试。

本次新增的 `crates/zora_transport` 按 NyaTerm 当前后端方向重建：crate 使用 Rust 2024 edition、Tokio 异步运行时、`russh` 0.62 和 `russh-sftp` 2.4。UI 仍通过 WarpUI 后台执行器调用同步适配层，但网络和磁盘传输本身不再由 app 自己实现。

## Proposed changes

### 1. 新 crate 和协议边界

- 新增 `crates/zora_transport`，模块拆分为 `error`、`types`、`session`、`remote_fs`、`sftp`、`transfer`。
- `RemoteFs` trait 负责 home、list/stat、读写、创建、删除、重命名、符号链接、递归上传下载等协议无关操作。
- `Sftp` 使用 `russh::client::Handle` 建立 SSH 会话，并通过 SFTP subsystem 创建 `russh_sftp::client::SftpSession`。远程文件和目录接口全部是 async，句柄可以安全地在后台任务之间共享。
- 当前迁移先实现 SFTP 主路径；命令通道和 SCP fallback 不在本次 app 接入范围内，协议能力不足时返回结构化的 `Unsupported`，不伪造成功。
- 生产 API 不暴露 app 的 WarpUI 类型；协议层只返回远程条目、元数据和传输快照。测试提供本地目录实现，供 UI 集成测试复用。

### 2. 传输控制器

- `TransferController` 使用共享状态和条件变量实现 `Running/Paused/Cancelled` 协作控制；每个读写块前检查状态。
- `TransferRegistry` 为任务分配 ID，保存活跃控制器，并暴露快照查询、pause/resume/cancel/retry 所需的句柄。
- `TransferEvent` 至少包含 started、progress、paused、resumed、completed、cancelled、failed；事件携带任务 ID、方向、源/目标、总字节、已传字节、目录子任务计数和错误。
- 文件传输使用固定临时后缀，完成后以重命名/备份恢复策略提交；失败时清理，清理失败的信息保留在错误中。
- 目录传输先收集文件清单和总大小，再用父任务汇总子文件进度；路径校验集中在 transport 层，避免递归复制绕过安全规则。

### 3. app 迁移

- `app/Cargo.toml` 改为依赖 `zora_transport`，根工作区依赖表加入同名路径依赖；删除 `zap_sftp` 依赖和 crate 文件。
- `app/src/sftp_manager/sftp_ops.rs` 只保留 SSH repository/secret store 到 transport session 的适配和 UI 错误转换；文件复制、递归和临时文件逻辑迁移到 `zora_transport`。
- `app/src/sftp_manager/sftp_backend.rs` 改为 `RemoteFs` 适配器，并继续提供本地测试后端；UI 的 `FileEntry` 只承担展示字段转换。
- `SftpBrowserView` 通过 `SftpBackend` 适配 `zora_transport::Sftp`，传输任务保存 transport controller 和可取消的进度刷新 future，不再直接依赖 `zap_sftp` 或在 UI 内实现块复制。
- 传输面板新增暂停/恢复/重试/取消动作、速度/字节进度和目录汇总；使用现有 WarpUI `Timer` 定时消费 transport 快照并 `ctx.notify()`，后台线程不直接触碰 UI。
- 文件列表和右键菜单新增目录上传/下载入口、批量任务入口和明确的重试/冲突状态；按钮沿用现有 WarpUI 主题，危险操作继续使用现有确认对话框模式。

### 4. 迁移顺序

1. 添加 `zora_transport` 类型、错误、会话和 SFTP 操作，先通过 crate 单元测试。
2. 添加 `RemoteFs` 与传输控制器，测试暂停/恢复/取消、临时文件提交、目录汇总和安全路径。
3. 替换 app 依赖和协议调用，保持现有 UI 测试可编译。
4. 接入快照轮询和任务控制，再加入目录操作和冲突流程。
5. 删除 `zap_sftp`，运行格式化、crate 测试和 `cargo check`。

## End-to-end flow

```text
SftpBrowserAction
        │
        ▼
SftpBrowserView ──后台执行器──> RemoteFs
        ▲                         │
        │ Timer + snapshot        ▼
TransferPanel <────────── TransferController
                                  │
                                  ▼
                         SFTP 或 SSH/SCP 后端
```

## Testing and validation

- `zora_transport` 单元测试覆盖：权限/文件类型解析、远程路径规范化、非法路径拒绝、控制器状态转换、暂停等待、取消唤醒、重试状态和目录进度汇总。
- 本地目录后端测试覆盖：文件/目录列举、递归上传下载、覆盖提交、取消后的临时文件清理和 Unicode 文件名。
- app SFTP UI 测试覆盖：加载/空状态、导航历史、搜索、删除/重命名/创建目录、批量上传冲突、传输面板暂停/恢复/取消/重试动作。
- 交付前执行 `cargo check`；若环境支持，再执行 `cargo nextest run -p zora_transport` 和相关 app 测试。
- 手工验证至少包括一个密码 SSH 服务器和一个密钥 SSH 服务器，单文件与目录上传/下载、暂停恢复、取消、失败重试、覆盖确认和关闭 pane。

## Risks and mitigations

- `russh`/`russh-sftp` 版本与 NyaTerm 的 vendor fork 可能有小幅 API 差异：依赖固定到当前 crates.io 版本，并把协议层限制在 `zora_transport`。
- 服务器公钥策略目前提供 `ServerKeyPolicy`；app 兼容旧行为暂时使用 `AcceptAny`，后续应接入 known_hosts/首次信任确认 UI。
- 远程服务器对 rename overwrite 的支持不一致：使用临时文件、备份名称和失败恢复，并明确报告无法提交的临时文件。
- SSH/SCP 兼容后端的目录列表能力依赖远程 shell：能力不足时返回 `Unsupported`，不把不完整结果当成成功。
- 旧测试直接依赖 app 的 `SftpBackend`：先保留测试用本地后端，再逐步把协议 trait 替换为 `RemoteFs`，避免一次性破坏 UI 测试。
