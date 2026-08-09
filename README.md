<div align="center">

<img src="assets/zap-logo.svg" alt="Zora" width="128" />

# Zora

[简体中文](./README.zh-CN.md) · [日本語](./README.ja.md)

<sub><i>Currently based on <a href="https://github.com/warpdotdev/warp">Warp</a>; evolving independently going forward.</i></sub>

</div>

Zora is an open, local-first terminal with first-class AI and agent support. Plug in any AI provider, bring in any CLI agent, manage SSH hosts inside the terminal — with keys, history and agent state staying on your machine by default.

## What Zora adds over upstream Warp

- **No mandatory cloud** — no account, login, Drive sync or cloud agent history required.
- **BYOP AI providers** — any OpenAI-compatible endpoint, plus native OpenAI / Anthropic / Gemini / DeepSeek / Ollama protocols. Keys stay local.
- **Third-party CLI agents** — DeepSeek-TUI / Codex CLI / Claude Code / Google Antigravity (`agy`) wired into Blocks and the notification center.
- **Built-in SSH host manager** — manage hosts, configs and sessions inside the terminal, with tmux integration.
- **Built-in SFTP file browser** — browse and manage remote files, multi-select entries, drag and drop uploads, and track or cancel file and directory transfers.
- **Editable system prompts** — minijinja templates rendered on the client.
- **Rendering fixes** — tuned Markdown pipeline; CJK soft-wrap caret and bold subpixel fixes.
- **Localized UI** — English / Simplified Chinese / Japanese out of the box, community-extensible.
- **Privacy defaults** — Cloud Agent / Computer Use / Referral / telemetry off by default.

## Migrating from OpenWarp or Warp

If you used the project before it was renamed to Zora (formerly **OpenWarp**),
or are coming from upstream **Warp**, see
[docs/migrate-from-warp.md](docs/migrate-from-warp.md) to bring your settings
across.

## Build from source

From the repository root, use the platform-aware build wrapper:

```shell
make build
```

Set an explicit release version when preparing a release:

```shell
make build RELEASE_TAG=v2026.08.03.1
```

To reclaim Cargo build-cache space, run:

```shell
make clean
```

`make clean` runs `cargo clean`. It can free substantial disk space and forces the next build to recompile dependencies; it does not delete generated installers. Windows-specific installer prerequisites and manual troubleshooting instructions are in [script/windows/README.md](script/windows/README.md).

## Roadmap

See [docs/roadmap.md](docs/roadmap.md).

## Acknowledgements

- [Warp](https://github.com/warpdotdev/warp) — the upstream open-source terminal codebase and core platform on which Zora is built.
- [Zap](https://github.com/zerx-lab/zap) — a related open-source project from which some Zora features and implementation ideas are derived or adapted.

Zora is an independent application. The projects above remain their respective upstream/source projects; please refer to this repository's license and third-party notices for licensing details.
