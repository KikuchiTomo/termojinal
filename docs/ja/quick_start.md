# クイックスタート

Termojinal は macOS 向けの GPU アクセラレーテッドターミナルエミュレータ。

## インストール

### Homebrew（推奨）

```bash
brew tap KikuchiTomo/termojinal
brew install termojinal              # CLI ツール + デーモン
brew install --cask termojinal-app   # GUI アプリ (Termojinal.app → /Applications)
brew services start termojinal       # デーモン起動 (Cmd+` ホットキー)
```

### ソースから

```bash
git clone https://github.com/KikuchiTomo/termojinal.git
cd termojinal
make install && make app
open target/release/Termojinal.app
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

## コマンドパレット (Cmd+Shift+P)

コマンドパレットはデフォルトで**ファイルファインダーモード**で開く。

### ファイルファインダーモード

現在の作業ディレクトリのファイルとディレクトリを探索する。

- **矢印キー**でナビゲーション、**Tab** でオートコンプリート
- **Enter**: ディレクトリなら `cd`、ファイルならそのファイルの親ディレクトリに `cd`
- **Shift+Enter**: エディタで開く（`$EDITOR`、フォールバックは `nvim`）
- **`..`** で親ディレクトリに移動
- 入力が空の状態で **Backspace**: 親ディレクトリへ移動（ルートの場合は閉じる）
- 入力中の **`/`**: サブディレクトリに移動（例: `src/` で `src` に入る）

### コマンドモード

先頭に **`>`** を入力するとコマンドモードに切り替わる。ビルトインアクションとカスタムコマンドをファジー検索できる。

## キーバインド

| 操作 | ショートカット |
|------|---------------|
| コマンドパレット | Cmd+Shift+P |
| Quick Launch | Cmd+O |
| Claudes Dashboard | Cmd+Shift+C |
| Quick Terminal | Ctrl+\` |
| 右に分割 | Cmd+D |
| 下に分割 | Cmd+Shift+D |
| 次/前のペイン | Cmd+] / Cmd+[ |
| ペインのズーム | Cmd+Shift+Enter |
| ペインをタブに抽出 | Cmd+Shift+T |
| 新規タブ | Cmd+T |
| タブを閉じる | Cmd+W |
| 次/前のタブ | Cmd+Shift+} / { |
| 新規ワークスペース | Cmd+N |
| ワークスペース切替 | Cmd+1 〜 Cmd+9 |
| サイドバー切替 | Cmd+B |
| ディレクトリツリー切替 | Cmd+Shift+E |
| 検索 | Cmd+F |
| フォントサイズ | Cmd+= / Cmd+- |
| Option+クリック | URL やパスを `open` で開く |
| 終了 | Cmd+Q |

すべてのキーバインドはカスタマイズ可能。[configuration.md](configuration.md) を参照。

## Allow Flow（AI 権限管理）

Claude Code が権限を必要とすると、Termojinal が通知とヒントバーを表示する。どこからでも応答できる:

| キー | 動作 |
|------|------|
| y | リクエストを1件許可 |
| n | リクエストを1件拒否 |
| Y | 保留中のリクエストをすべて許可 |
| N | 保留中のリクエストをすべて拒否 |
| a / A | 許可してルールを記憶（永続） |
| Esc | ヒントバーを閉じる |

## カスタムコマンド

コマンドは stdio 経由の JSON で通信するスクリプト。`~/.config/termojinal/commands/` に配置し、コマンドパレットからアクセスする（`>` でコマンドモードに切り替え）。

プロトコルの詳細は [command.md](command.md) を参照。

## 設定

`~/.config/termojinal/config.toml` を編集して、フォント、カラー、サイドバー、ステータスバーなどをカスタマイズ。

設定リファレンスは [configuration.md](configuration.md) を参照。

## 関連ドキュメント

- [機能一覧](features.md)
- [設定リファレンス](configuration.md)
- [カスタムコマンドと JSON API](command.md)
