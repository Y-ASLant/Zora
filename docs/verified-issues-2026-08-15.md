# Zora 已验证问题清单（2026-08-15）

## 验证范围

本文件验证上一轮分析中列出的安全、可靠性、架构和发布流程风险。证据仅来自当前仓库源码、配置和已导出的日志，不把历史记忆或推断当作事实源。

结论：前述问题中，以下 10 项均能在当前代码或配置中找到直接证据。两个结论需要收窄表述：

- “交互式 SSH 缺少 keepalive/timeout 策略”已确认；“它就是某次 SSH 卡死的唯一根因”未被源码单独证明。
- “TerminalModel 锁风险暴露面大”已确认；“当前一定存在死锁”未复现。

## P1：SFTP 默认接受任意服务端公钥

**状态：真实存在。**

`app/src/sftp_manager/sftp_ops.rs:140-158` 中，SFTP 连接通过 `SftpSession::connect_with_policy(...)` 创建，并显式传入 `ServerKeyPolicy::AcceptAny`。

`crates/zora_transport/src/session.rs:22-34` 定义 `ServerKeyPolicy`，默认策略也是 `AcceptAny`。`crates/zora_transport/src/session.rs:50-68` 中，`AcceptAny` 的 verifier 为 `Arc::new(|_| true)`，即任何服务端公钥都会通过。

**影响：** SFTP 浏览、下载、上传、删除等远端文件操作无法确认连接到的主机身份。VPN、局域网或跳板链路中间人可以伪装目标主机。

**建议：** 默认改为 `KnownHosts` 或 TOFU/pinning；首次连接展示 fingerprint，用户确认后持久化。

## P1：SSH 测试连接跳过 host key 校验

**状态：真实存在。**

`crates/warp_ssh_manager/src/ssh_command.rs:96-112` 的 key auth 测试参数包含 `StrictHostKeyChecking=no`。`crates/warp_ssh_manager/src/ssh_command.rs:287-310` 的 password auth 测试参数也包含 `StrictHostKeyChecking=no`。

`app/src/ssh_manager/server_view.rs:823-881` 的 `on_test_connection()` 构造 `SshServerInfo` 后调用 `warp_ssh_manager::ssh_command::test_connection(&server, password).await`。

**影响：** 测试连接只能证明远端可达，不能证明远端身份可信。UI 若把该结果展示为“连接成功”，用户可能误以为目标主机已通过身份验证。

**建议：** 将结果拆成 reachability 与 identity verification；默认校验 known_hosts；如允许跳过，必须在 UI 中显式标记为不安全。

## P1：MCP CLI server 环境变量明文写入 SQLite

**状态：真实存在。**

`app/src/settings_view/mcp_servers/edit_page.rs:603-624` 将 `cli_server.static_env_vars` 收集为 `HashMap<String, String>`，序列化成 JSON 后通过 `ModelEvent::UpsertMCPServerEnvironmentVariables` 持久化。

`app/src/persistence/sqlite.rs:3570-3586` 将 `environment_variables: String` upsert 到 `mcp_environment_variables` 表。

`crates/persistence/src/schema.rs:198-203` 定义 `mcp_environment_variables.environment_variables -> Text`。

`app/src/ai/mcp/mod.rs:635-645` 运行时从 SQLite 读回该字段并写回 `cli_server.static_env_vars`。

`app/src/ai/mcp/mod_test.rs:20-28`、`app/src/ai/mcp/mod_test.rs:434-440` 的测试夹具也使用 `API_KEY` / `SOME_SECRET` 作为静态环境变量，说明该字段承载秘密是符合现有使用模型的。

**影响：** MCP server 的 API key、token、数据库 URL 等秘密可能以明文 JSON 存在本地 SQLite 中。

**建议：** 把环境变量区分为 public/secret；secret 值存 secure storage，SQLite 只保存变量名、引用和非敏感元数据。

## P1：Agent conversation token 与 sidecar 状态明文持久化

**状态：真实存在。**

`crates/persistence/src/model.rs:976-1020` 明确说明 `AgentConversationData` 序列化到 `agent_conversations.conversation_data`，字段包含：

- `server_conversation_token`
- `forked_from_server_conversation_token`
- `artifacts_json`
- `compaction_state_json`
- `byop_repair_state_json`
- `cli_subagent_block_snapshots_json`

`crates/persistence/src/schema.rs:10-16` 定义 `agent_conversations.conversation_data -> Text`。

`app/src/ai/agent/conversation.rs:3031-3070` 在更新 conversation state 时把这些字段写入 `AgentConversationData`。

`app/src/ai/agent/api.rs:404-414` 请求参数直接使用 `conversation.server_conversation_token`。

**影响：** 本地 SQLite 泄露时，server conversation token 和多类恢复 sidecar 状态会一起泄露。token 的服务端权限边界需另行审计，但“token 明文入库并被后续请求使用”已确认。

**建议：** bearer-like token 迁入 secure storage；SQLite 中只保存无敏感恢复状态和 secure-storage 引用。

## P1：交互式 SSH 路径缺少明确 keepalive/timeout 策略

**状态：真实存在，根因归因需谨慎。**

`app/src/workspace/view.rs:5204-5357` 的 `open_ssh_terminal()` 通过 `warp_ssh_manager::build_ssh_command_line(&server_for_connection)` 生成交互式 SSH 命令，并交给 terminal 执行。

`crates/warp_ssh_manager/src/ssh_command.rs:30-59` 的 `build_ssh_args()` 只拼接：

- `ssh`
- 可选 `-p <port>`
- 可选 `-i <key_path>`
- `user@host` 或 `host`

该交互式路径没有 `ConnectTimeout`、`ServerAliveInterval`、`ServerAliveCountMax` 等参数。

对照：`crates/zora_transport/src/session.rs:131-135` 的 SFTP russh config 设置了 `inactivity_timeout: 30s` 和 `keepalive_interval: 15s`。

已导出的 SSH 日志中，SFTP `list_dir` 能成功完成，而交互式 `ssh` 最终以 `ExitCode(255)` 退出；这与“两个远端通道策略不一致”一致，但不能单独证明某次卡死唯一由 keepalive 缺失导致。

**影响：** VPN/NAT/堡垒机清理空闲 TCP 时，交互式 SSH 和 SFTP 的表现可能分裂：文件面板可用，终端会话半开或延迟失败。

**建议：** 为交互式 SSH 加统一连接策略，并与 SFTP 共享 host-key、timeout、keepalive 参数定义。

## P1：日志导出是原样打包，未做统一脱敏

**状态：真实存在。**

`app/src/workspace/view.rs:5590-5674` 的 `collect_log_bundle_extras()` 会把 `mcp/*.log` 加入导出包。

`crates/warp_logging/src/native.rs:355-423` 的 `write_log_bundle_zip_inner()` 对主日志、轮转日志和 extra files 使用 `copy(&mut source, &mut zip_writer)` 原样写入 zip。

`crates/warp_logging/src/lib.rs:37-59` 的 `diagnostic_text_preview()` 只转义换行、制表符和控制字符，并截断长度；没有 secret redaction。

`app/src/terminal/view.rs:13540-13546`、`app/src/terminal/model/terminal_model.rs:2851-2857`、`app/src/ssh_manager/startup_command_injector.rs:44-49` 在诊断模式下会记录命令或 startup command 预览。

**影响：** 用户导出日志时可能带出命令、MCP stderr、协议调试片段或其它敏感内容。当前实现只保护 zip entry 名称，不保护文件内容。

**建议：** 导出前统一 redaction；默认不打包 MCP 子日志或增加二次确认；诊断模式 UI 明示风险。

## P2：Workspace / TerminalView 职责过宽，主 crate 还全局抑制 dead_code

**状态：真实存在，属于维护性风险。**

`app/src/workspace/action.rs:151-683` 的 `WorkspaceAction` 覆盖 tab、pane、SSH、SFTP、settings、日志导出、AI/prompt 等多类行为。例如 `OpenSshTerminal` 在 `app/src/workspace/action.rs:199-204`，`ExportLogsToPath` 在 `app/src/workspace/action.rs:279-280`。

`app/src/workspace/view.rs` 中同一 `Workspace` 实现承载 SSH 打开、SFTP 面板、日志导出、设置、工作流、prompt editor 等大量装配逻辑。`app/src/terminal/view.rs` 也承载 terminal、agent、shared session、SSH label、diagnostic command 执行等大量职责。

`app/src/lib.rs:1-4` 在主 crate 顶层启用 `#![allow(dead_code)]`，注释说明是为裁剪后遗留孤儿代码统一抑制 dead_code 告警。

**影响：** 局部功能修改容易穿过多个产品域；编译器无法帮助发现 app crate 的孤儿代码；review 需要人工覆盖更大的状态空间。

**建议：** 优先把 SSH/SFTP orchestration、日志导出、agent conversation persistence 从 `Workspace` / `TerminalView` 中切成深模块；逐步移除 app 级 `allow(dead_code)`。

## P2：TerminalModel 锁暴露面大

**状态：真实存在，未复现具体死锁。**

代表性调用点：

- `app/src/terminal/block_list_element.rs:3190-3196` 在布局/渲染路径中持有 `self.model.lock()`。
- `app/src/terminal/block_list_element.rs:4615-4626` 在键盘事件路径中短时间内两次 `self.model.lock()`。
- `app/src/terminal/input.rs:5692-5706` 在命令可执行性检查中读取 model。
- `app/src/terminal/input.rs:5750-5761` 在 shared session 代理执行路径中两次读取 model。
- `app/src/terminal/local_tty/event_loop.rs:234-245`、`370-376`、`398-402`、`475-476` 在 PTY event loop 中直接锁 `terminal`。

仓库规则已经把 `TerminalModel::lock()` 标为高优先级死锁风险；源码中的直接锁调用覆盖 UI、输入和 PTY 事件路径。

**影响：** 目前不能据此断言已有死锁，但风险面真实存在：未来改动若在持锁期间触发回调、writer、parser 或 view update，容易形成低频卡死。

**建议：** 建只读 snapshot API；明确 lock ordering；把 PTY/event-loop 写模型路径收敛为小接口。

## P2：发布流水线主要证明能产包，缺少发布级 smoke/test 证明

**状态：真实存在。**

`.github/workflows/zora_release.yml:176-178` 的 macOS check 步骤运行 `script/bundle --check-only`。

`.github/workflows/zora_release.yml:320-323` 的 Linux check 步骤运行 `script/bundle --check-only`。

`.github/workflows/zora_release.yml` 中搜索 `cargo test`、`cargo nextest`、`nextest` 没有发现发布 workflow 直接运行测试；该 workflow 主要调用 `script/bundle`。

`.github/workflows/zora_release.yml:349-370` 中 RPM 和 Arch 包构建设置 `continue-on-error: true`，失败不会阻塞已成功的 AppImage/deb 发布。

**影响：** release workflow 能证明 bundle 过程大体可执行，但不能证明产物可启动、CLI 可用、本地 terminal 可打开、设置页可访问或平台包集合完整。

**建议：** 在发布前补最小 smoke matrix：`zora --version`、应用启动、打开本地终端、打开设置；生成并校验产物 manifest。

## P2：Windows update_manager 测试挂住问题被 retry 绕过

**状态：真实存在。**

`.config/nextest.toml:33-39` 明确配置 Windows 上 `test(update_manager)` 重试 2 次；注释说明：

- 测试 flaky，常 timeout；
- evidence suggests 测试本身通过，但进程挂住；
- root cause 未弄清楚；
- retry 是 unblock devs 的 workaround。

**影响：** CI 可能把“断言曾经通过但进程生命周期异常”的问题重试成绿色，隐藏 updater 或子进程退出路径缺陷。

**建议：** 给该测试加 hang dump、child-process inventory 和超时现场采集；把 retry 作为临时止血而不是长期验证策略。

## 未纳入为已验证问题的部分

- 没有证明“某次 SSH 卡死的唯一根因就是 Zora 的 keepalive 缺失”。当前可确认的是交互式 SSH 与 SFTP 策略不一致，且日志症状支持该方向继续排查。
- 没有复现 `TerminalModel` 死锁。当前可确认的是直接锁调用覆盖面大，且仓库规则已把该 API 视为高风险。
- 没有审计 server conversation token 在服务端的权限、有效期和撤销语义。当前可确认的是 token 明文持久化并被后续请求使用。

## 建议修复顺序

1. SFTP host key 校验与 SSH 测试连接 host key 策略统一。
2. MCP env var 与 Agent conversation token 从 SQLite 明文迁移到 secure storage 引用模型。
3. 日志导出 redaction 与 MCP 日志二次确认。
4. 交互式 SSH 统一 timeout/keepalive 策略。
5. 发布 smoke 与 Windows update_manager hang 现场采集。
6. Workspace/TerminalView/TerminalModel 锁 API 的架构收敛。
