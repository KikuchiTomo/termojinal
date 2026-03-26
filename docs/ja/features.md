# 機能一覧

Termojinal は macOS 向けの GPU アクセラレーテッドターミナルエミュレータで、AI コーディングエージェントと連携する開発者のために設計されている。

---

## GPU アクセラレーテッドレンダリング

wgpu + Metal による GPU レンダリング。大量出力のスクロールは滑らか、ウィンドウリサイズは即座に反映、半透明やブラーエフェクトもフルフレームレートで動作する。

---

## Daemon-Owned PTY

GUI はシンクライアント。デーモン（`termojinald`）が全 PTY セッションを所有するため、GUI を閉じてもシェルは生存する。

- `tm exit` -- GUI を切断。シェルは動作し続ける。
- `tm kill` -- シェルセッションを終了する。

---

## ワークスペース

単一ウィンドウ内の独立した環境。各ワークスペースは独自のタブ、ペイン、作業ディレクトリを持つ。

| 操作 | ショートカット |
|------|---------------|
| 新規ワークスペース | `Cmd+N` |
| ワークスペース N に切替 | `Cmd+1` 〜 `Cmd+9` |
| 次/前のワークスペース | `Cmd+Shift+]` / `[` |

---

## タブ

各ワークスペースに複数のタブを持てる。ターミナルセッションは独立。

| 操作 | ショートカット |
|------|---------------|
| 新規タブ | `Cmd+T` |
| タブを閉じる | `Cmd+W` |
| 次/前のタブ | `Cmd+Shift+}` / `{` |

タブタイトルはフォールバックチェーン: シェル設定タイトル、CWD ベース名、"Tab N"。`config.toml` の `[tab_bar]` でカスタマイズ可能。

---

## 分割ペイン

ペインを分割して複数のセッションを並べて表示。

| 操作 | ショートカット |
|------|---------------|
| 右に分割 | `Cmd+D` |
| 下に分割 | `Cmd+Shift+D` |
| 次/前のペイン | `Cmd+]` / `[` |
| ペインのズーム（フルスクリーン切替） | `Cmd+Shift+Enter` |
| ペインをタブに抽出 | `Cmd+Shift+T` |

- セパレーターをドラッグしてペインをリサイズ。
- タブをペインエリアにドラッグして新しい分割を作成（タブドラッグ分割）。
- `Cmd+Shift+T` でフォーカス中のペインを新しいタブに抽出（タブに複数ペインがある場合のみ）。

---

## Claudes Dashboard

`Cmd+Shift+C` -- 全ワークスペースの Claude Code セッションを管理する lazygit 風の2ペインインターフェース。ステータス確認、セッション切替、権限リクエスト管理が一箇所で行える。

---

## Quick Launch

`Cmd+O` -- ワークスペース・タブ・ペインを素早く切り替えるファジー検索オーバーレイ。入力でフィルタリング、Enter でジャンプ。

---

## サイドバー

`Cmd+B` で切替。各ワークスペースについて以下を表示:

- ワークスペース名とカラードット
- Git ブランチと変更状態
- アクティブなポート
- 未読アラートの通知ドット
- AI 権限リクエスト保留中の Allow Flow アクセントストライプ
- AI エージェントステータスインジケーター（Hooks ベース、ポーリングなし）

エージェントステータスのスタイル（設定可能）:

| スタイル | 挙動 |
|---------|------|
| `"pulse"` | ワークスペースドットがアニメーションで脈動（デフォルト） |
| `"color"` | 固定色（紫 = アクティブ、黄 = 権限待ち） |
| `"none"` | 非表示 |

サイドバー幅はドラッグ可能（デフォルト 240px、範囲 120-400px）。

---

## コマンドパレットとファイルファインダー

`Cmd+Shift+P` -- 2つのモードを持つパレットが開く。

### ファイルファインダーモード（デフォルト）

フォーカス中のペインの作業ディレクトリの内容を表示する。

- **入力**でプレフィックスフィルタリング
- **矢印キー**でナビゲーション、**Tab** でオートコンプリート
- **Enter**: ディレクトリなら `cd`、ファイルならそのファイルの親ディレクトリに `cd`
- **Shift+Enter**: エディタで開く（`$EDITOR`、フォールバックは `nvim`）
- **`..`** で親ディレクトリに移動
- 入力が空の状態で **Backspace**: 親ディレクトリへ移動（ルートの場合は閉じる）
- 入力中の **`/`**: サブディレクトリに移動（例: `src/` で `src` に入る）

### コマンドモード

先頭に **`>`** を入力するとコマンドモードに切り替わる。ビルトインアクションとカスタムコマンドをファジー検索。空の入力で Backspace を押すとファイルファインダーモードに戻る。

---

## Allow Flow

Claude Code が権限を必要とすると、`PermissionRequest` フック経由でリクエストをインターセプトし、サイドバーに表示する。どこからでも応答可能:

| キー | 動作 |
|------|------|
| `y` | 1件許可 |
| `n` | 1件拒否 |
| `Y` | 保留中すべて許可 |
| `N` | 保留中すべて拒否 |
| `a` / `A` | 許可してルールを記憶（永続） |
| `Esc` | ヒントバーを閉じる |

他のツール用にカスタム検出パターンも追加可能:

```toml
[[allow_flow.patterns]]
tool = "My Deploy Tool"
action = "production deploy"
pattern = "Deploy to production\\? \\[y/N\\]"
yes_response = "y\n"
no_response = "n\n"
```

---

## Quick Terminal

`Cmd+`` ` -- 画面上部からスライドする Quake 風ドロップダウンターミナル。Termojinal がフォーカスされていなくても動作する（デーモンが必要）。

もう一度ホットキーを押すか Escape で閉じる。専用ワークスペースでセッションが維持される。

### 設定

| オプション | デフォルト | 説明 |
|-----------|-----------|------|
| `enabled` | `true` | 機能の有効化 |
| `hotkey` | `"ctrl+\`"` | グローバルホットキー |
| `animation` | `"slide_down"` | `slide_down`, `slide_up`, `fade`, `none` |
| `animation_duration_ms` | `200` | アニメーション速度 |
| `height_ratio` | `0.4` | 画面に対する高さの割合 |
| `width_ratio` | `1.0` | 画面に対する幅の割合 |
| `position` | `"center"` | `left`, `center`, `right` |
| `screen_edge` | `"top"` | `top`, `bottom` |
| `hide_on_focus_loss` | `false` | フォーカスを失った時に自動非表示 |
| `dismiss_on_esc` | `true` | Escape で非表示 |
| `window_level` | `"floating"` | `normal`, `floating`, `above_all` |
| `corner_radius` | `12.0` | 角丸の半径（ピクセル） |

---

## ディレクトリツリー

`Cmd+Shift+E` で切替。サイドバー内のファイルブラウザパネル。

`config.toml` で有効にする:

```toml
[directory_tree]
enabled = true
```

- Git 対応ルート検出（`auto`, `cwd`, `git_root`）
- 矢印キーでナビゲーション、プレフィックス検索でジャンプ
- ディレクトリをダブルクリックで `cd`
- ファイル上で `v` を押すとエディタで開く（`$EDITOR` または nvim）

---

## Time Travel（コマンド履歴）

OSC 133 シェル統合マーカーを使ってコマンド間をナビゲーション。

| 操作 | ショートカット |
|------|---------------|
| 前のコマンド | `Cmd+Up` |
| 次のコマンド | `Cmd+Down` |
| 最初のコマンド | `Cmd+Shift+Up` |
| 最後のコマンド | `Cmd+Shift+Down` |
| コマンドタイムライン | `Cmd+Shift+H` |

### 設定

```toml
[time_travel]
command_history = true
max_command_history = 10000
command_navigation = true
show_command_marker = true
show_command_position = true
timeline_ui = true
session_persistence = true
restore_on_startup = true
snapshots = true
max_snapshots_per_session = 50
```

---

## 検索

`Cmd+F` -- ターミナルのスクロールバックバッファを検索。大文字小文字を区別しない部分文字列マッチング。ハイライト色のカスタマイズ可能。

---

## ステータスバー

ウィンドウ下部のセッション情報バー。左右のセグメントにテンプレート変数を配置する。

### 利用可能な変数

`{user}`, `{host}`, `{cwd}`, `{cwd_short}`, `{git_branch}`, `{git_status}`, `{git_remote}`, `{git_worktree}`, `{git_stash}`, `{git_ahead}`, `{git_behind}`, `{git_dirty}`, `{git_untracked}`, `{ports}`, `{shell}`, `{pid}`, `{pane_size}`, `{font_size}`, `{workspace}`, `{workspace_index}`, `{tab}`, `{tab_index}`, `{time}`, `{date}`

### 例

```toml
[status_bar]
enabled = true

[[status_bar.left]]
content = "{user}@{host}"
fg = "#1A1A28"
bg = "#7AA2F7"

[[status_bar.right]]
content = "{time}"
fg = "#1A1A28"
bg = "#7AA2F7"
```

---

## インライン画像

3つのプロトコルをサポート: **Kitty Graphics Protocol**、**iTerm2 Inline Images**（OSC 1337）、**Sixel Graphics**。

---

## CJK・日本語入力

- CJK 全角文字を正しい倍幅でレンダリング
- インライン IME: カーソル位置で日本語入力の変換中テキストを表示（専用背景色 `preedit_bg`）
- 別の入力ウィンドウ不要 -- ターミナル内で直接日本語を入力

---

## カラー絵文字

macOS Core Text によるレンダリング。ネイティブ macOS アプリと同じ絵文字表示。

---

## テーマと外観

- デフォルトテーマ: **Catppuccin Mocha**
- カスタムテーマ: `~/.config/termojinal/themes/<name>.toml`
- macOS の外観に連動したダーク/ライト自動切替
- ウィンドウの不透明度、太字の明るさ、薄暗い不透明度のカスタマイズ
- フォントサイズズーム: `Cmd+=` / `Cmd+-`

---

## カラー付きコピー

テキスト選択中に `Cmd+C` で RTF 形式でクリップボードにコピーし、ターミナルの色を保持する。選択がない場合、`Cmd+C` は Ctrl+C をターミナルに送信する。

---

## Option+クリック

Option を押しながらテキストをクリック: URL はデフォルトブラウザで開き、ファイルパスは macOS の `open` で開く。

---

## Brew 更新チェッカー

起動時に Homebrew（formula と cask）で更新を確認。新しいバージョンがあれば通知を表示。

---

## カスタムコマンド

コマンドパレットから Termojinal を拡張するスクリプト（`>` でコマンドモードに切り替え）。stdin/stdout 上の行区切り JSON で通信。

### ディレクトリ構造

```
~/.config/termojinal/commands/my-command/
├── command.toml    # マニフェスト
└── run.sh          # エントリポイント
```

### インタラクティブ UI の種類

| タイプ | 説明 |
|--------|------|
| `fuzzy` | フィルタリング可能な単一選択リスト |
| `multi` | チェックボックス付き複数選択 |
| `confirm` | はい/いいえダイアログ |
| `text` | 補完候補付きテキスト入力 |
| `info` | 進捗メッセージ |
| `done` | 完了シグナル（通知付きオプション） |
| `error` | エラーメッセージ |

### バンドルコマンド

`hello-world`, `start-review`, `switch-worktree`, `kill-merged`, `clone-and-open`, `run-agent`

### SDK

型付き Deno SDK（`@termojinal/sdk`）。ヘルパー: `fuzzy()`, `multi()`, `confirm()`, `text()`, `info()`, `done()`, `error()`。

プロトコルの詳細は [command.md](command.md) を参照。

---

## MCP サーバー

Claude Code にワークスペース制御を提供 -- タブ作成、ターミナルコンテンツ読み取り、権限の承認。

Claude Code の MCP 設定に追加:

```json
{
  "mcpServers": {
    "termojinal": {
      "command": "termojinal-mcp"
    }
  }
}
```

---

## 通知

macOS Notification Center 経由のデスクトップ通知。コマンド完了、Allow Flow リクエスト、カスタムコマンドで発火。

```toml
[notifications]
enabled = true
sound = false
```

---

## コマンド署名

Ed25519 でコマンドを暗号署名して信頼性を検証。

1. 鍵ペア生成: `termojinal-sign --generate-key`
2. 署名: `termojinal-sign path/to/command.toml <secret-key-hex>`
3. 検証済みコマンドはパレットにチェックマーク付きで表示。

---

## 全キーバインド

| キー | アクション |
|------|-----------|
| `Cmd+Shift+P` | コマンドパレット |
| `Cmd+O` | Quick Launch |
| `Cmd+Shift+C` | Claudes Dashboard |
| `Cmd+`` ` | Quick Terminal（グローバル） |
| `Cmd+D` | 右に分割 |
| `Cmd+Shift+D` | 下に分割 |
| `Cmd+]` / `[` | 次/前のペイン |
| `Cmd+Shift+Enter` | ペインのズーム |
| `Cmd+Shift+T` | ペインをタブに抽出 |
| `Cmd+T` | 新規タブ |
| `Cmd+W` | タブを閉じる |
| `Cmd+Shift+}` / `{` | 次/前のタブ |
| `Cmd+N` | 新規ワークスペース |
| `Cmd+1`〜`9` | ワークスペース切替 |
| `Cmd+Shift+]` / `[` | 次/前のワークスペース |
| `Cmd+B` | サイドバー切替 |
| `Cmd+Shift+E` | ディレクトリツリー切替 |
| `Cmd+F` | 検索 |
| `Cmd+=` / `-` | フォントサイズ拡大/縮小 |
| `Cmd+A` | 全選択 |
| `Cmd+C` / `V` | コピー/ペースト |
| `Cmd+K` | スクロールバッククリア |
| `Cmd+L` | 画面クリア |
| `Cmd+Up` / `Down` | 前/次のコマンド |
| `Cmd+Shift+Up` / `Down` | 最初/最後のコマンド |
| `Cmd+Shift+H` | コマンドタイムライン |
| `Cmd+,` | 設定を開く |
| `Cmd+Q` | 終了 |
| Option+クリック | URL / パスを開く |

すべてのキーバインドは `~/.config/termojinal/keybindings.toml` でカスタマイズ可能。**normal**、**global**、**alternate_screen** の3レイヤーに対応。

設定の詳細は [configuration.md](configuration.md) を参照。

---

## 関連ドキュメント

- [クイックスタート](quick_start.md) -- インストールと最初のステップ
- [設定リファレンス](configuration.md) -- config.toml の完全リファレンス
- [カスタムコマンドと JSON API](command.md) -- コマンドプロトコル仕様
