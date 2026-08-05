<div align="center">

<img src="assets/zap-logo.svg" alt="Zap" width="128" />

# Zap

[English](./README.md) · [简体中文](./README.zh-CN.md)

<sub><i>現在は <a href="https://github.com/warpdotdev/warp">Warp</a> をベースにしていますが、今後は独自に進化していきます。</i></sub>

</div>

Zap はオープンでローカルファーストなターミナルで、AI と Agent をファーストクラスでサポートします。任意の AI プロバイダーを接続し、任意の CLI Agent を取り込み、ターミナル内で SSH ホストを管理 —— API キー・履歴・Agent の状態はデフォルトで自分のマシンに留まります。

## 公式 Warp に対して Zap が追加する機能

- **クラウド必須なし** —— アカウント、ログイン、Drive 同期、クラウド Agent 履歴のいずれも不要。
- **BYOP な AI プロバイダー** —— 任意の OpenAI 互換エンドポイントに加え、OpenAI / Anthropic / Gemini / DeepSeek / Ollama のネイティブプロトコル。API キーはローカルに保持。
- **サードパーティ CLI Agent** —— DeepSeek-TUI / Codex CLI / Claude Code / Google Antigravity (`agy`) を Block と通知センターに統合。
- **内蔵 SSH ホストマネージャー** —— ターミナル内でホスト・設定・セッションを管理、tmux と連携。
- **内蔵 SFTP ファイルブラウザー** —— リモートファイルの閲覧・管理、複数選択、ドラッグ＆ドロップによるアップロード、ファイルやディレクトリ転送の進捗確認・キャンセルに対応。
- **編集可能なシステムプロンプト** —— minijinja テンプレートをクライアント側でレンダリング。
- **レンダリング改善** —— Markdown パイプラインのチューニング、CJK ソフトラップ caret と太字サブピクセルの修正。
- **多言語 UI** —— 英語 / 簡体字中国語 / 日本語をデフォルト同梱、コミュニティで拡張可能。
- **プライバシー優先のデフォルト** —— Cloud Agent / Computer Use / Referral / テレメトリはデフォルトでオフ。

## OpenWarp / Warp からの移行

プロジェクトが Zap に改名される前から使っていた方(当時の名称は **OpenWarp**)、
または上流 **Warp** から乗り換える方は、
[docs/migrate-from-warp.ja.md](docs/migrate-from-warp.ja.md) を参照して設定を
引き継いでください。

## ソースからのビルド

リポジトリのルートで、プラットフォーム対応のビルド入口を実行します。

```shell
make build
```

リリース準備時はバージョンを明示できます。

```shell
make build RELEASE_TAG=v2026.08.03.1
```

Cargo のビルドキャッシュで使用したディスク領域を解放するには、次を実行します。

```shell
make clean
```

`make clean` は `cargo clean` を実行します。大量のディスク領域を解放できる一方、次回のビルドでは依存関係を再コンパイルします。生成済みインストーラーは削除しません。Windows インストーラーの前提条件と手動トラブルシューティングは [script/windows/README.md](script/windows/README.md) を参照してください。

## ロードマップ

[docs/roadmap.ja.md](docs/roadmap.ja.md) を参照してください。

## 謝辞

- [Warp](https://github.com/warpdotdev/warp) —— Zap がベースとしている上流のターミナル。
- [DeepSeek-TUI](https://github.com/Hmbown/DeepSeek-TUI) —— 深く統合された CLI Agent パートナー。
