<div align="center">

<img src="assets/zap-logo.svg" alt="Zora" width="128" />

# Zora

[English](./README.md) · [日本語](./README.ja.md)

<sub><i>目前基于 <a href="https://github.com/warpdotdev/warp">Warp</a>,后续将独立演进。</i></sub>

</div>

Zora 是一个开放、本地优先的终端,带一等公民的 AI 与 Agent 体验。接入任意 AI 提供商、接入任意 CLI Agent、在终端内管理 SSH 主机 —— 密钥、历史与 Agent 状态默认留在本地。

## 相比官方 Warp 多出的功能

- **无强制云端** —— 不需要账号、登录、Drive 同步或云端 Agent 历史。
- **BYOP 自定义 AI 提供商** —— 任意 OpenAI 兼容端点,以及 OpenAI / Anthropic / Gemini / DeepSeek / Ollama 等原生协议,密钥仅存本地。
- **第三方 CLI Agent 接入** —— DeepSeek-TUI / Codex CLI / Claude Code / Google Antigravity(`agy`)接入 Block 与通知中心。
- **内置 SSH 主机管理器** —— 在终端内管理主机、配置与会话,集成 tmux。
- **内置 SFTP 文件浏览器** —— 浏览和管理远程文件,支持多选、拖拽上传,以及带进度显示和取消能力的文件与目录传输。
- **可编辑系统提示词** —— 基于 minijinja 模板,客户端实时渲染。
- **渲染优化** —— Markdown 管线优化;CJK 软换行 caret 与加粗子像素修复。
- **多语言界面** —— 原生英文 / 简体中文 / 日语,社区可扩展。
- **隐私默认值** —— Cloud Agent / Computer Use / Referral / 遥测默认关闭。

## 从 OpenWarp 或 Warp 迁移过来

如果你在项目改名 Zora 之前就一直在用(那时还叫 **OpenWarp**),
或者你是从上游 **Warp** 切过来的,参见
[docs/migrate-from-warp.zh-CN.md](docs/migrate-from-warp.zh-CN.md) 把设置带过来。

## 从源码构建

在仓库根目录执行跨平台构建入口：

```shell
make build
```

准备发布时可显式指定版本号：

```shell
make build RELEASE_TAG=v2026.08.03.1
```

如需释放 Cargo 构建缓存占用的磁盘空间，执行：

```shell
make clean
```

`make clean` 实际执行 `cargo clean`。它可能释放大量磁盘空间，但下一次构建会重新编译依赖；不会删除已生成的安装包。Windows 安装包的依赖和手动排障方式见 [script/windows/README.md](script/windows/README.md)。

## 后续计划

见 [docs/roadmap.zh-CN.md](docs/roadmap.zh-CN.md)。

## 鸣谢

- [Warp](https://github.com/warpdotdev/warp) —— Zora 所基于的上游开源终端代码库与核心平台。
- [Zap](https://github.com/zerx-lab/zap) —— 相关开源项目，Zora 的部分功能与实现思路来自或参考了该项目。

Zora 是一个独立应用。上述项目仍分别属于各自的上游/来源项目；许可证和第三方声明请以本仓库的相关文件为准。
