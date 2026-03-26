# クイックスタート

Termojinal は macOS 向けの GPU アクセラレーテッドターミナルエミュレータです。

## インストール

### Homebrew（推奨）

```bash
brew install KikuchiTomo/tap/termojinal
brew services start termojinal   # デーモンを起動
```

### ソースから

```bash
git clone https://github.com/KikuchiTomo/termojinal.git
cd termojinal
make install   # Rust ツールチェーンのインストール + 依存取得
make build     # リリースビルド
make app       # Termojinal.app を作成
```

## 初回起動

`/Applications`（Homebrew）または `target/release/Termojinal.app`（ソースビルド）から `Termojinal.app` を開く。

初回起動時に macOS が通知の許可を求めてくる。Claude Code の権限プロンプトやコマンド完了通知を受け取るために許可すること。

## Claude Code のセットアップ

```bash
tm setup
```

このコマンド一つで以下が完了する:
- `~/.config/termojinal/` の作成
- Claude Code の通知・権限フックのインストール
- バンドルコマンドのリンク

## キーバインド

| 操作 | ショートカット |
|------|---------------|
| コマンドパレット | Cmd+Shift+P |
| Quick Launch | Cmd+O |
| Claudes Dashboard | Cmd+Shift+C |
| Quick Terminal | Cmd+\` |
| 右に分割 | Cmd+D |
| 下に分割 | Cmd+Shift+D |
| 次のペイン | Cmd+] |
| 前のペイン | Cmd+[ |
| ペインのズーム | Cmd+Shift+Enter |
| ペインをタブに抽出 | Cmd+Shift+T |
| 新規タブ | Cmd+T |
| タブを閉じる | Cmd+W |
| 次/前のタブ | Cmd+Shift+} / { |
| 新規ワークスペース | Cmd+N |
| ワークスペース切替 | Cmd+1 〜 Cmd+9 |
| サイドバー切替 | Cmd+B |
| 検索 | Cmd+F |
| フォントサイズ | Cmd+= / Cmd+- |
| Option+クリック | URL をブラウザで開く / パスを `open` で開く |
| 終了 | Cmd+Q |

すべてのキーバインドは `~/.config/termojinal/keybindings.toml` でカスタマイズ可能。

## Allow Flow（AI 権限管理）

Claude Code がファイル編集やシェルコマンド実行などの権限を必要とすると、Termojinal が通知とヒントバーを表示する。どこからでも応答できる:

| キー | 動作 |
|------|------|
| y | リクエストを1件許可 |
| n | リクエストを1件拒否 |
| Y | 保留中のリクエストをすべて許可 |
| N | 保留中のリクエストをすべて拒否 |
| a / A | 許可してルールを記憶（永続） |
| Esc | ヒントバーを閉じる |

## カスタムコマンド

コマンドは stdio 経由の JSON で通信するスクリプト。`~/.config/termojinal/commands/` に配置し、コマンドパレットからアクセスする。

プロトコルの詳細は [command.md](command.md) を参照。

## 設定

`~/.config/termojinal/config.toml` を編集して、フォント、カラー、サイドバー、ステータスバーなどをカスタマイズ。

設定リファレンスは [configuration.md](configuration.md) を参照。

## Claudes Dashboard

`Cmd+Shift+C` で開く。lazygit 風の2ペインインターフェースで、全ワークスペースの Claude Code セッションを一覧表示する。セッションのステータス確認、切替、権限管理が一箇所で行える。

## Quick Launch

`Cmd+O` で開く。ファジー検索オーバーレイで、ワークスペース・タブ・ペインを素早く切り替えられる。入力でフィルタリング、Enter でジャンプ。

## アーキテクチャ

Termojinal は **Daemon-owned PTY** モデルを採用している。GUI はシンクライアントであり、GUI を閉じても（`tm exit`）シェルはデーモンで生存する。`tm kill` でシェルを終了する。

| バイナリ | 用途 |
|---------|------|
| `Termojinal.app` | シンクライアント GUI（wgpu + Metal + winit） |
| `termojinald` | セッションデーモン（PTY 所有、グローバルホットキー、永続化） |
| `tm` | CLI ツール（セットアップ、通知、Allow Flow） |
| `termojinal-mcp` | Claude Code 統合用の MCP サーバー |
| `termojinal-sign` | Ed25519 コマンド署名ツール |

## 関連ドキュメント

- [設定リファレンス](configuration.md)
- [カスタムコマンドと JSON API](command.md)
