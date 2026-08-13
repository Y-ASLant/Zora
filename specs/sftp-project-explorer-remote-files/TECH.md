# SFTP 项目浏览器远程文件打开技术规格

## Context

产品行为见 `PRODUCT.md`。本技术方案把 SFTP 文件作为 Zora 原生文件来源接入项目浏览器和 editor/buffer 体系，而不是在 SFTP 视图里临时读取文本。

当前相关代码边界：

- `app/src/workspace/view/left_panel.rs:654` — `active_sftp_browser_view` 已按 pane group 查找 SFTP 浏览器视图。
- `app/src/workspace/view/left_panel.rs:693` — `open_sftp_browser` 已能在 SSH 会话打开时为当前 pane group 创建 `SftpBrowserView` 并切到 `ProjectExplorer`。
- `app/src/workspace/view.rs:4958` — `handle_left_panel_event` 处理项目浏览器、server file browser 和 SSH 管理器事件。
- `app/src/workspace/view.rs:5102` — `open_remote_file` 目前打开的是 remote-server 语义的 `RemotePath`，不能直接代表 SFTP 文件。
- `app/src/sftp_manager/sftp_backend.rs:16` — `SftpBackend` 已封装 SFTP 列目录、删除、重命名、上传、下载和目录传输。
- `app/src/sftp_manager/sftp_backend.rs:82` — `LiveSftpBackend` 持有 `zora_transport::Sftp`。
- `crates/zora_transport/src/sftp.rs:136` — `Sftp::read` 已能读取完整远程文件 bytes。
- `crates/zora_transport/src/sftp.rs:143` — `Sftp::write` 已能写入远程文件 bytes。
- `app/src/code/global_buffer_model.rs:198` — `GlobalBufferModel` 以 `BufferLocation -> FileId` 管理共享 buffer，是远程文件去重和保存状态的正确接入点。
- `app/src/code/global_buffer_model.rs:670` — 当前 `save` 走本地 `FileModel`，需要为 SFTP buffer 增加保存分支。

现有 `SftpBrowserView` 能承担第一版项目浏览器里的远程文件树，但长期形态应是统一的 `FileProvider` 边界：项目浏览器负责 UI，provider 负责本地、remote-server 或 SFTP 数据访问。

NyaTerm 的参考实现是“内置编辑器 + 外部编辑器临时文件 + watcher 自动上传”并行。它的 README 描述了 SFTP 浏览器、传输队列、远程文件编辑和 watcher-driven auto-upload；本方案只借鉴文件大小限制、内容指纹和外部打开兜底，不把外部编辑器 round-trip 作为 Zora 主路径。

## Proposed changes

### 1. 建立统一文件来源抽象

新增或收敛一个项目浏览器可消费的 provider trait。命名可以落在 `app/src/workspace/file_provider.rs` 或更贴近现有项目浏览器模块，核心能力如下：

```rust
trait ProjectFileProvider {
    fn identity(&self) -> ProjectFileProviderIdentity;
    fn capabilities(&self) -> ProjectFileProviderCapabilities;
    async fn list_dir(&self, path: &Path) -> Result<Vec<ProjectFileEntry>>;
    async fn stat(&self, path: &Path) -> Result<ProjectFileMetadata>;
    async fn read(&self, path: &Path, max_bytes: u64) -> Result<RemoteFileBytes>;
    async fn read_range(&self, path: &Path, offset: u64, len: u64) -> Result<Vec<u8>>;
    async fn write(
        &self,
        path: &Path,
        bytes: Vec<u8>,
        expected: Option<ProjectFileVersion>,
        mode: WriteMode,
    ) -> Result<ProjectFileWriteResult>;
    async fn rename(&self, old_path: &Path, new_path: &Path) -> Result<()>;
    async fn delete(&self, path: &Path) -> Result<()>;
}
```

第一版可以不一次性迁移本地项目浏览器，但 SFTP 新能力必须按 provider 思路设计，避免把远程打开逻辑写死在 `SftpBrowserView`。

`ProjectFileProviderIdentity` 必须包含 provider 类型和稳定连接身份。SFTP 至少包含 SSH node id、显示名、用户、host、port。它是 buffer 去重、缓存 key 和 UI 来源显示的基础。

`ProjectFileProviderCapabilities` 用于控制 UI 能力，而不是在 UI 内按 provider 类型散落判断。SFTP 第一版能力：

- `list_dir`
- `read`
- `write`
- `rename`
- `delete`
- `upload`
- `download`

SFTP 第一版不声明：

- `watch`
- `git_status`
- `workspace_search`
- `language_index`

### 2. 扩展 SFTP 后端为字节级 provider

在 `SftpBackend` 增加字节级接口，而不是只加文本接口：

```rust
fn read_file(&self, path: &Path, max_bytes: u64) -> Result<RemoteFileBytes, SftpOpsError>;
fn read_file_range(
    &self,
    path: &Path,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>, SftpOpsError>;
fn write_file(
    &self,
    path: &Path,
    bytes: &[u8],
    expected: Option<RemoteFileVersion>,
    mode: WriteMode,
) -> Result<RemoteFileWriteResult, SftpOpsError>;
```

`LiveSftpBackend` 使用 `zora_transport::Sftp::stat/read/write` 实现。`InMemorySftpBackend` 同步实现，保证现有 `browser_integration_tests` 可以覆盖打开文件行为。

`read_file` 必须先 `stat` 检查大小。若文件超过 `max_bytes`，返回结构化 `TooLarge { size, max }`，不要先把完整文件读入内存后再报错。

如果 `russh_sftp::client::SftpSession` 支持 range read，应实现 `read_file_range`；若当前封装没有 range API，第一版可以读取受限 `max_bytes` 的完整内容作为嗅探和小文件打开路径，但接口要保留 range 能力，避免后续大文件预览重构公共 API。

### 3. 新增 RemoteFileService

新增 `RemoteFileService`，作为项目浏览器和 editor 之间的策略层。它不负责 UI 渲染，也不直接持有 view。

职责：

- 根据 provider identity、path、metadata 生成稳定 `RemoteFileLocation`。
- 执行打开策略：大小判断、扩展名判断、前缀 bytes 嗅探、编码判断。
- 管理内存 LRU 缓存。
- 处理 open/reload/save 的统一错误类型。
- 在保存前执行远程版本冲突检测。

建议设置默认值：

- `remote_files.auto_open_text_max_bytes = 8 * 1024 * 1024`
- `remote_files.text_cache_max_bytes = 64 * 1024 * 1024`
- `remote_files.sniff_bytes = 64 * 1024`
- `remote_files.external_auto_upload = false`

缓存 key：

```text
provider_identity + normalized_remote_path + size + mtime
```

缓存 value 保存：

- decoded text
- original bytes hash 可选
- encoding
- metadata
- loaded_at

缓存不得跨 provider identity 命中。同一 host 不同 SSH node、不同 user 或不同 port 也不能误复用。

### 4. 文本检测和编码

打开文件时按以下顺序判断：

1. `stat` 得到大小和文件类型。
2. 非普通文件直接返回 unsupported。
3. 大于自动打开阈值时返回 `OpenDecision::NeedsConfirmation`。
4. 读取前 `sniff_bytes`。
5. 先看 BOM：UTF-8、UTF-16LE、UTF-16BE。
6. 如果包含 NUL 或明显二进制控制字节，返回 `OpenDecision::Binary`。
7. 根据扩展名和 UTF-8 解码结果判断文本。
8. UTF-8 解码失败且无可支持编码时返回 `OpenDecision::UnknownEncoding`。

第一版不需要引入复杂编码自动检测。支持 UTF-8 和 UTF-16 BOM 即可；其它编码显示明确提示，允许用户下载或外部打开。

### 5. 接入 buffer/editor

不要把 SFTP 文件作为一次性 in-memory editor。应扩展现有 code buffer 模型，让 SFTP 文件成为 `BufferLocation` 的一种。

新增 location 形态示例：

```rust
pub enum BufferLocation {
    Local(PathBuf),
    Remote(RemotePath),
    Sftp(SftpRemotePath),
}

pub struct SftpRemotePath {
    pub node_id: String,
    pub display_host: String,
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub path: RemotePathString,
}
```

`SftpRemotePath` 的 `Eq/Hash` 必须包含稳定连接身份和路径，不能只用 path。

`GlobalBufferModel::open` 需要支持 `BufferLocation::Sftp`：

- 如果已有 loaded buffer，直接返回已有 handle。
- 如果没有，创建 buffer 并通过 `RemoteFileService` 异步加载内容。
- 成功加载后调用现有 buffer populate 流程。
- 加载失败发出与本地/remote buffer 一致的失败事件。

`GlobalBufferModel::save` 需要按 buffer source 分支：

- `Local` 继续走 `FileModel`。
- `Remote` 继续走 remote-server 现有逻辑。
- `Sftp` 走 `RemoteFileService::save_text`。

保存成功后更新 buffer base version 和远程 metadata；保存失败保留 dirty 内容。

### 6. 保存和冲突检测

打开 SFTP 文件时记录：

- remote size
- remote mtime
- 可选 content hash
- loaded content version

保存流程：

1. 保存前 `stat` 当前远程文件。
2. 如果当前远程 `size/mtime` 与打开记录一致，允许保存。
3. 如果不一致，返回 `SaveConflict`，由 editor/Workspace 显示冲突对话框。
4. 用户选择覆盖时，用 `WriteMode::Force` 保存。
5. 用户选择重新加载时，重新读取远程内容并替换 buffer。
6. 用户选择另存为时，走远程路径选择或输入流程，写入新路径。

写入策略优先使用同目录临时文件提交：

```text
<filename>.zora-tmp-<nonce>
write temp
stat temp
rename temp -> target
```

如果目标服务器不支持覆盖 rename，降级为直接写入，并在错误中标明降级原因。临时文件清理失败要记录日志并在必要时显示路径。

### 7. 项目浏览器打开流程

在 SFTP 文件树的双击/Enter 打开路径里，不直接读文件或创建 editor，而是发事件给 workspace：

```rust
SftpBrowserEvent::OpenPath {
    provider_identity,
    path,
    entry_metadata,
}
```

`LeftPanelView` 接收后转发：

```rust
LeftPanelEvent::OpenProviderFile {
    provider_identity,
    path,
    metadata,
}
```

`Workspace` 调用 `RemoteFileService::decide_open`：

- `OpenDecision::Directory` -> 项目浏览器导航。
- `OpenDecision::Text` -> `open_code(CodeSource::RemoteProviderFile { ... })`。
- `OpenDecision::NeedsConfirmation` -> 显示大文件打开对话框。
- `OpenDecision::Binary` -> 显示下载/外部打开对话框。
- `OpenDecision::Unsupported` -> 显示错误。

这保证所有 provider 未来都复用同一打开策略，而不是 SFTP 专用逻辑散落在视图里。

### 8. 外部打开

外部打开不是主路径，但要有明确实现边界：

- 小文本文件默认不走外部打开。
- 用户显式选择外部打开时，下载到 app 临时目录。
- 临时路径包含 provider identity hash 和时间戳，避免不同服务器同名文件覆盖。
- 默认只打开外部编辑器，不自动上传。
- 后续若启用自动上传，复用 NyaTerm 的内容指纹思路：只在内容 hash 变化时触发上传，避免编辑器 metadata-only save 造成无意义写回。

### 9. 设置

新增设置项应进入现有 settings 系统，并在设置 UI 中放到“功能 / SSH / 远程文件”相关区域：

- 自动在 Zora 内打开文本文件最大大小，默认 `8 MiB`。
- 远程文本缓存总量，默认 `64 MiB`。
- 大文件预览读取大小，默认 `1 MiB`。
- 外部打开保存后自动上传，默认关闭。

设置值需要最小/最大限制：

- 自动打开阈值：`1 MiB` 到 `64 MiB`。
- 缓存总量：`0 MiB` 到 `512 MiB`；`0` 表示禁用缓存。
- 大文件预览大小：`256 KiB` 到 `8 MiB`。

### 10. 实施顺序

1. 添加 provider identity、capabilities 和远程文件打开决策类型，不改变 UI。
2. 扩展 `SftpBackend` / `LiveSftpBackend` / `InMemorySftpBackend` 的字节级 read/write/stat 能力。
3. 添加 `RemoteFileService` 的文本嗅探、大小限制和缓存单元测试。
4. 给 `BufferLocation` / `GlobalBufferModel` 添加 SFTP buffer location 和只读打开能力。
5. 从 SFTP 项目浏览器双击文件接到 workspace 打开 flow。
6. 添加保存能力和冲突检测。
7. 添加大文件、二进制、未知编码和外部打开对话框。
8. 补设置 UI、缓存限制和手工验证。

## End-to-end flow

```text
SSH Manager
    │ open ssh terminal
    ▼
LeftPanelView::open_sftp_browser
    │
    ▼
Project Explorer
    │ double click remote file
    ▼
SftpBrowserEvent::OpenPath
    │
    ▼
Workspace
    │
    ▼
RemoteFileService::decide_open
    ├── directory ───────────> SFTP provider navigate
    ├── small text ──────────> GlobalBufferModel::open(BufferLocation::Sftp)
    ├── large text ──────────> confirmation dialog
    ├── binary/unknown ──────> download / external open dialog
    └── error ───────────────> non-destructive error state
```

保存流程：

```text
Editor save
    │
    ▼
GlobalBufferModel::save(file_id)
    │
    ▼
RemoteFileService::save_text
    │ stat current remote metadata
    ├── unchanged ─────> SftpProvider::write(temp + rename)
    ├── changed ───────> SaveConflict dialog
    └── failed ────────> keep dirty buffer + show retryable error
```

## Testing and validation

产品规格中的关键行为映射到以下验证：

- Behavior 1-6：项目浏览器 provider 切换测试。通过 SSH 管理器打开两个不同 node，断言每个 pane group 显示独立 SFTP root、路径和连接身份。
- Behavior 7-10：SFTP 浏览器交互测试。目录双击导航；文件双击调用打开决策；小文本、二进制和大文件走不同 decision。
- Behavior 11-13：buffer 去重测试。同一 provider/path 重复打开复用一个 buffer；不同 provider 同路径创建不同 buffer。
- Behavior 14-17：`RemoteFileService` 单元测试覆盖 UTF-8、UTF-8 BOM、UTF-16 BOM、NUL binary、未知扩展文本、未知扩展二进制和非法编码。
- Behavior 18：错误映射测试覆盖 permission denied、not found、timeout、too large、unsupported encoding。
- Behavior 19-23：保存与冲突测试。打开后远程 metadata 未变时保存成功；metadata 变化时返回冲突；覆盖保存、重新加载、另存为各自更新正确状态。
- Behavior 24-25：缓存测试。命中必须匹配 provider identity、path、size、mtime；超过缓存预算时 LRU 淘汰；设置为 0 时不缓存。
- Behavior 26-27：外部打开和传输队列手工验证。小文本内部打开不生成传输任务；显式下载/外部打开进入传输或临时下载流程。
- Behavior 28-29：取消、关闭和键盘路径手工验证。打开中取消、连接断开、关闭 dirty tab、Enter/Escape 焦点行为都保持稳定。
- Behavior 30：回归检查。独立 SFTP 面板不再作为主要入口，项目浏览器和旧入口不显示互相矛盾的远程路径。

交付前至少执行：

- `cargo fmt --check`
- `cargo check -p warp`
- 相关 `sftp_manager` / `global_buffer_model` / `RemoteFileService` 单元测试

若环境可用，再用一个密码 SSH 服务器和一个密钥 SSH 服务器手工验证：

- 打开 SSH 后项目浏览器自动显示远程目录。
- 双击 `/etc/hosts` 或小型配置文件进入 Zora editor。
- 修改后保存成功。
- 打开后远程手动修改文件，再保存触发冲突。
- 大于阈值的日志文件显示确认对话框。
- 二进制文件不进入文本编辑器。

## Risks and mitigations

- **风险：把 SFTP 文件硬塞进现有 `RemotePath` 会污染 remote-server 语义。**  
  缓解：新增 `BufferLocation::Sftp` 或等价 provider-backed location，明确区分 remote-server 和 SFTP。

- **风险：保存路径绕过 buffer model 会产生 dirty 状态、tab 去重和关闭确认不一致。**  
  缓解：所有内部打开的远程文本文件必须进入 `GlobalBufferModel`，保存也从该模型分发。

- **风险：大文件完整读取导致 UI 卡顿或内存占用过高。**  
  缓解：先 `stat`，再按阈值决策；嗅探只读前缀；缓存有总预算。

- **风险：远程文件在用户编辑期间被其它进程修改。**  
  缓解：保存前比较 `size/mtime`，冲突时要求用户选择，不盲写。

- **风险：不同服务器同路径文件缓存或 buffer 误复用。**  
  缓解：provider identity 是缓存 key 和 `BufferLocation::Sftp` 的一部分。

- **风险：编码检测过度复杂导致第一版不稳定。**  
  缓解：第一版只保证 UTF-8、UTF-8 BOM 和 UTF-16 BOM；其它编码明确提示，不静默损坏内容。

- **风险：外部编辑器自动上传覆盖用户不想覆盖的远程文件。**  
  缓解：外部自动上传默认关闭，后续启用时必须加内容指纹和冲突检测。

## Follow-ups

- 把本地项目浏览器、remote-server 文件树和 SFTP 文件树完全收敛到同一个 `ProjectFileProvider` trait。
- 为远程目录提供按 provider 能力声明的搜索。
- 为图片、Markdown、JSON、日志等常见远程文件增加专用预览器。
- 外部打开自动上传支持 watcher、内容指纹、冲突检测和用户可见状态。
- 如果 remote server 可用，优先使用 remote-server provider；不可用时自动降级到 SFTP provider。

