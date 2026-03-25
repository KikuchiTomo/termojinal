# Termojinal 設定リファレンス

Termojinal の設定に関する包括的なリファレンス。メイン設定ファイル、テーマファイル、キーバインド、全オプションを網羅する。

---

## 目次

- [設定ファイルの場所](#設定ファイルの場所)
- [完全リファレンス](#完全リファレンス)
  - [\[font\]](#font)
  - [\[window\]](#window)
  - [\[theme\]](#theme)
  - [\[sidebar\]](#sidebar)
  - [\[tab\_bar\]](#tab_bar)
  - [\[pane\]](#pane)
  - [\[search\]](#search)
  - [\[palette\]](#palette-コマンドパレット)
  - [\[status\_bar\]](#status_bar)
  - [\[notifications\]](#notifications)
  - [\[allow\_flow\]](#allow_flow)
  - [\[quick\_terminal\]](#quick_terminal)
- [キーバインド](#キーバインド)
- [カラーフォーマット](#カラーフォーマット)
- [テーマファイル](#テーマファイル)

---

## 設定ファイルの場所

Termojinal は以下の順序で設定ファイルを探す:

1. **プライマリ（XDG スタイル）:** `~/.config/termojinal/config.toml`
2. **フォールバック（macOS 標準）:** `~/Library/Application Support/termojinal/config.toml`

ディスク上に最初に見つかったパスが使われる。どちらのファイルも存在しない場合、ビルトインのデフォルトで起動する。

設定ファイルが見つかったがパースエラーがある場合、警告をログに出力してデフォルトにフォールバックする:

```
config parse error: expected `=`, found newline at line 5 column 1
```

設定ファイルのすべてのフィールドはオプション。オーバーライドしたい値だけ指定すればよく、未指定のフィールドはデフォルトが使われる。

---

## 完全リファレンス

### [font]

フォントレンダリングの設定。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `family` | String | `"monospace"` | フォントファミリー名。システムにインストールされた任意のフォントを使用可能（例: `"JetBrains Mono"`, `"Fira Code"`）。 |
| `size` | f32 | `14.0` | フォントサイズ（ピクセル）。 |
| `line_height` | f32 | `1.2` | フォントサイズに対する行の高さの倍率。 |
| `max_size` | f32 | `72.0` | Cmd+= ズームで到達可能な最大フォントサイズ。 |
| `size_step` | f32 | `1.0` | ズーム1ステップあたりのフォントサイズ増分（Cmd+= / Cmd+-）。 |

**例:**

```toml
[font]
family = "JetBrains Mono"
size = 13.0
line_height = 1.3
```

---

### [window]

ウィンドウの初期サイズと外観。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `width` | u32 | `960` | ウィンドウの初期幅（ピクセル）。 |
| `height` | u32 | `640` | ウィンドウの初期高さ（ピクセル）。 |
| `opacity` | f32 | `1.0` | ウィンドウ背景の不透明度。`0.0` = 完全に透明、`1.0` = 完全に不透明。 |
| `padding_x` | f32 | `1.0` | 水平パディング（セル幅単位）。 |
| `padding_y` | f32 | `0.5` | 垂直パディング（セル高さ単位）。 |

**例:**

```toml
[window]
width = 1200
height = 800
opacity = 0.92
```

---

### [theme]

ターミナルコンテンツ領域のカラー設定。ビルトインのデフォルトは Catppuccin Mocha パレットに準拠。

#### コアカラー

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `background` | String | `"#1E1E2E"` | ターミナルの背景色。 |
| `foreground` | String | `"#CDD6F4"` | デフォルトのテキスト色。 |
| `cursor` | String | `"#F5E0DC"` | カーソルの色。 |
| `selection_bg` | String | `"#45475A"` | 選択ハイライトの背景色。 |
| `preedit_bg` | String | `"#313244"` | IME 変換中の背景色。 |
| `search_highlight_bg` | String | `"#F9E2AF"` | 検索マッチのハイライト背景色。 |
| `search_highlight_fg` | String | `"#1E1E2E"` | 検索マッチのハイライト前景色。 |
| `bold_brightness` | f32 | `1.2` | 太字テキストの色に適用される明るさ倍率。 |
| `dim_opacity` | f32 | `0.6` | 薄い（faint）テキストに適用される不透明度。 |

#### ANSI 16色パレット

| フィールド | デフォルト | 説明 |
|---|---|---|
| `black` | `"#45475A"` | ANSI カラー 0（黒） |
| `bright_black` | `"#585B70"` | ANSI カラー 8（明るい黒 / ダークグレー） |
| `red` | `"#F38BA8"` | ANSI カラー 1（赤） |
| `bright_red` | `"#F38BA8"` | ANSI カラー 9（明るい赤） |
| `green` | `"#A6E3A1"` | ANSI カラー 2（緑） |
| `bright_green` | `"#A6E3A1"` | ANSI カラー 10（明るい緑） |
| `yellow` | `"#F9E2AF"` | ANSI カラー 3（黄） |
| `bright_yellow` | `"#F9E2AF"` | ANSI カラー 11（明るい黄） |
| `blue` | `"#89B4FA"` | ANSI カラー 4（青） |
| `bright_blue` | `"#89B4FA"` | ANSI カラー 12（明るい青） |
| `magenta` | `"#F5C2E7"` | ANSI カラー 5（マゼンタ） |
| `bright_magenta` | `"#F5C2E7"` | ANSI カラー 13（明るいマゼンタ） |
| `cyan` | `"#94E2D5"` | ANSI カラー 6（シアン） |
| `bright_cyan` | `"#94E2D5"` | ANSI カラー 14（明るいシアン） |
| `white` | `"#BAC2DE"` | ANSI カラー 7（白） |
| `bright_white` | `"#A6ADC8"` | ANSI カラー 15（明るい白） |

#### ダーク/ライトの自動切替

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `auto_switch` | bool | `false` | macOS の外観変更時にテーマを自動切替。 |
| `dark` | String | `""` | ダークモードで読み込むテーマファイル名（例: `"catppuccin-mocha"`）。 |
| `light` | String | `""` | ライトモードで読み込むテーマファイル名（例: `"catppuccin-latte"`）。 |

**例:**

```toml
[theme]
background = "#0D1117"
foreground = "#E6EDF3"
cursor = "#58A6FF"

auto_switch = true
dark = "github-dark"
light = "github-light"
```

---

### [sidebar]

サイドバーはワークスペース、タブ、git ブランチ情報、通知インジケーターを表示する。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `width` | f32 | `240.0` | サイドバーのデフォルト幅（ピクセル）。 |
| `min_width` | f32 | `120.0` | ドラッグリサイズ時の最小幅。 |
| `max_width` | f32 | `400.0` | ドラッグリサイズ時の最大幅。 |
| `bg` | String | `"#0D0D12"` | サイドバーの背景色。 |
| `active_entry_bg` | String | `"#1A1A24"` | アクティブなワークスペースエントリの背景色。 |
| `active_fg` | String | `"#F2F2F8"` | アクティブなワークスペース名のテキスト色。 |
| `inactive_fg` | String | `"#8C8C99"` | 非アクティブなワークスペース名のテキスト色。 |
| `dim_fg` | String | `"#666670"` | 補助情報テキスト（パス、メタデータ）の色。 |
| `git_branch_fg` | String | `"#5AB3D9"` | git ブランチラベルの色。 |
| `separator_color` | String | `"#333338"` | 水平セパレーターラインの色。 |
| `notification_dot` | String | `"#FF941A"` | 未読通知インジケータードットの色。 |
| `git_dirty_color` | String | `"#CCB34D"` | git 変更状態インジケーターの色。 |
| `allow_accent_color` | String | `"#4FC1FF"` | Allow Flow リクエスト保留中のワークスペースの左側ストライプアクセント色。 |
| `allow_hint_fg` | String | `"#7DC8FF"` | Allow Flow ヒントテキストの前景色。 |
| `top_padding` | f32 | `6.0` | 上部パディング（ピクセル）。 |
| `side_padding` | f32 | `6.0` | 左右パディング（ピクセル）。 |
| `entry_gap` | f32 | `4.0` | ワークスペースエントリ間の垂直ギャップ（ピクセル）。 |
| `info_line_gap` | f32 | `1.0` | ワークスペース名と情報行の間のギャップ（ピクセル）。 |

**例:**

```toml
[sidebar]
width = 260.0
bg = "#0A0A10"
active_entry_bg = "#1A1A28"
git_branch_fg = "#7AA2F7"
entry_gap = 16.0
```

---

### [tab_bar]

タブバーは現在のワークスペース内のタブを表示する。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `format` | String | `"{title\|cwd_base\|Tab {index}}"` | タブタイトルのフォーマット文字列。フォールバック構文: 最初の空でない値が使われる。 |
| `always_show` | bool | `false` | タブが1つだけでもタブバーを表示。 |
| `position` | String | `"top"` | タブバーの位置: `"top"` または `"bottom"`。 |
| `height` | f32 | `36.0` | タブバーの高さ（ピクセル）。 |
| `max_width` | f32 | `200.0` | 単一タブの最大幅（ピクセル）。 |
| `min_tab_width` | f32 | `60.0` | 単一タブの最小幅（ピクセル）。 |
| `new_tab_button_width` | f32 | `32.0` | "+" 新規タブボタンの幅（ピクセル）。 |
| `bg` | String | `"#1A1A1F"` | タブバーの背景色。 |
| `active_tab_bg` | String | `"#2E2E38"` | アクティブタブの背景色。 |
| `active_tab_fg` | String | `"#F2F2F8"` | アクティブタブのテキスト色。 |
| `inactive_tab_fg` | String | `"#8C8C99"` | 非アクティブタブのテキスト色。 |
| `accent_color` | String | `"#4D8CFF"` | アクティブタブの下線アクセント色。 |
| `separator_color` | String | `"#383840"` | タブ間のセパレーター色。 |
| `close_button_fg` | String | `"#808088"` | タブ閉じるボタンの色。 |
| `new_button_fg` | String | `"#808088"` | 新規タブボタン（"+"）の色。 |
| `padding_x` | f32 | `6.0` | タブ内の水平パディング（ピクセル）。 |
| `padding_y` | f32 | `6.0` | タブバー内の垂直パディング（ピクセル）。 |
| `accent_height` | u32 | `2` | アクティブタブの下線の太さ（ピクセル）。 |
| `bottom_border` | bool | `true` | タブバー下部にボーダーを表示。 |
| `bottom_border_color` | String | `"#2A2A34"` | 下部ボーダーの色。 |

#### タブフォーマット文字列

`format` フィールドは `|` で区切られたフォールバックチェーン構文を使う:

```
{title|cwd_base|Tab {index}}
```

- `title` -- シェルが設定したタブタイトル（OSC エスケープシーケンス経由）
- `cwd_base` -- カレントディレクトリのベース名
- `Tab {index}` -- 1始まりのタブインデックスを含むリテラルテキスト

チェーン内の最初の空でない値が表示される。

**例:**

```toml
[tab_bar]
format = "{cwd_base|Tab {index}}"
always_show = true
height = 38.0
accent_color = "#7AA2F7"
```

---

### [pane]

ペインの分割、ボーダー、スクロールバーの外観。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `separator_color` | String | `"#4D4D4D"` | ペイン間のラインの色。 |
| `focus_border_color` | String | `"#3399FFCC"` | フォーカス中のペインのボーダー色（アルファ対応）。 |
| `separator_width` | u32 | `2` | ペインセパレーターのライン幅（ピクセル）。 |
| `focus_border_width` | u32 | `2` | フォーカス中のペインのボーダー幅（ピクセル）。 |
| `separator_tolerance` | f32 | `4.0` | ペインセパレーターのドラッグ時のマウスヒット許容範囲（ピクセル）。 |
| `scrollbar_thumb_opacity` | f32 | `0.5` | スクロールバーサムの不透明度（`0.0`〜`1.0`）。 |
| `scrollbar_track_opacity` | f32 | `0.1` | スクロールバートラックの不透明度（`0.0`〜`1.0`）。 |

**例:**

```toml
[pane]
separator_color = "#2A2A38"
focus_border_color = "#7AA2F7CC"
separator_width = 1
scrollbar_thumb_opacity = 0.4
```

---

### [search]

検索バーオーバーレイの外観。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `bar_bg` | String | `"#262633F2"` | 検索バーの背景色（アルファで半透明に対応）。 |
| `input_fg` | String | `"#F2F2F2"` | 検索入力テキストの色。 |
| `border_color` | String | `"#4D4D66"` | 検索バーのボーダー色。 |

**例:**

```toml
[search]
bar_bg = "#1A1A28F0"
input_fg = "#E8E8F0"
border_color = "#3A3A50"
```

---

### [palette]（コマンドパレット）

コマンドパレットは wgpu を使い、SDF 角丸矩形とフロストガラスブラーエフェクトでレンダリングされる。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `bg` | String | `"#1F1F29F2"` | パレットの背景色（アルファ対応）。 |
| `border_color` | String | `"#4D4D66"` | 角丸矩形のボーダー色。 |
| `input_fg` | String | `"#F2F2F2"` | 入力テキストの色。 |
| `separator_color` | String | `"#40404D"` | 入力と結果の間のセパレーターライン色。 |
| `command_fg` | String | `"#CCCCD1"` | コマンド名のテキスト色。 |
| `selected_bg` | String | `"#383852"` | 選択中のコマンドの背景色。 |
| `description_fg` | String | `"#808088"` | コマンド説明のテキスト色。 |
| `overlay_color` | String | `"#00000080"` | パレット背後に描画される暗いオーバーレイの色。 |
| `max_height` | f32 | `400.0` | パレットの最大高さ（ピクセル）。 |
| `max_visible_items` | usize | `10` | スクロール前の最大表示コマンド数。 |
| `width_ratio` | f32 | `0.6` | ウィンドウ幅に対するパレット幅の割合（`0.0`〜`1.0`）。 |
| `corner_radius` | f32 | `12.0` | SDF 角丸矩形の角丸半径（ピクセル）。 |
| `blur_radius` | f32 | `20.0` | フロストガラスエフェクトの Gaussian ブラー半径（ピクセル）。`0` でブラー無効。 |
| `shadow_radius` | f32 | `8.0` | ドロップシャドウのブラー半径（ピクセル）。 |
| `shadow_opacity` | f32 | `0.3` | ドロップシャドウの不透明度（`0.0`〜`1.0`）。 |
| `border_width` | f32 | `1.0` | 角丸矩形アウトラインのボーダー幅（ピクセル）。 |

**例:**

```toml
[palette]
bg = "#14141EF0"
width_ratio = 0.5
corner_radius = 16.0
blur_radius = 24.0
max_visible_items = 12
```

---

### [status_bar]

ウィンドウ下部のカスタマイズ可能なステータスバー。左揃えと右揃えのセグメントで構成される。各セグメントはテンプレート変数を含められる。

#### トップレベルフィールド

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `enabled` | bool | `true` | ステータスバーの表示/非表示。 |
| `height` | f32 | `28.0` | ステータスバーの高さ（ピクセル）。 |
| `background` | String | `"#141420"` | ステータスバーの背景色。 |
| `padding_x` | f32 | `8.0` | 水平パディング（ピクセル）。 |
| `top_border` | bool | `true` | ステータスバー上部にボーダーを表示。 |
| `top_border_color` | String | `"#2A2A34"` | 上部ボーダーの色。 |

#### セグメント配列

セグメントは TOML の配列テーブルとして `[[status_bar.left]]` と `[[status_bar.right]]` で定義する:

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `content` | String | *（必須）* | `{variable}` プレースホルダーを含むテンプレート文字列（以下参照）。 |
| `fg` | String | `"#CCCCCC"` | セグメントの前景（テキスト）色。 |
| `bg` | String | `"#1A1A24"` | セグメントの背景色。 |

#### テンプレート変数

セグメントの `content` 文字列で以下の変数が使える:

| 変数 | 説明 |
|---|---|
| `{user}` | 現在のユーザー名（`$USER`）。 |
| `{host}` | システムのホスト名。 |
| `{cwd}` | フルパスのカレントディレクトリ。 |
| `{cwd_short}` | ホームを `~` に置換した短縮カレントディレクトリ。 |
| `{git_branch}` | 現在の git ブランチ名。 |
| `{git_status}` | git ステータスの要約。 |
| `{git_remote}` | git リモート名。 |
| `{git_worktree}` | git worktree パス。 |
| `{git_stash}` | git stash の数。 |
| `{git_ahead}` | リモートより先行しているコミット数。 |
| `{git_behind}` | リモートより遅れているコミット数。 |
| `{git_dirty}` | 変更された（modified）ファイル数。 |
| `{git_untracked}` | 未追跡ファイル数。 |
| `{ports}` | セッション内で検出されたリッスンポート。 |
| `{shell}` | シェル名（例: `zsh`, `bash`）。 |
| `{pid}` | シェルのプロセス ID。 |
| `{pane_size}` | 現在のペインサイズ（列 x 行）。 |
| `{font_size}` | 現在のフォントサイズ。 |
| `{workspace}` | 現在のワークスペース名。 |
| `{workspace_index}` | 現在のワークスペースインデックス（1始まり）。 |
| `{tab}` | 現在のタブ名。 |
| `{tab_index}` | 現在のタブインデックス（1始まり）。 |
| `{time}` | 現在のローカル時刻（HH:MM）。 |
| `{date}` | 現在のローカル日付（YYYY-MM-DD）。 |

**例:**

```toml
[status_bar]
enabled = true
height = 30.0
background = "#0A0A10"
top_border = true
top_border_color = "#1A1A28"

[[status_bar.left]]
content = "{user}@{host}"
fg = "#1A1A28"
bg = "#7AA2F7"

[[status_bar.left]]
content = "{cwd_short}"
fg = "#C0C0CC"
bg = "#1A1A28"

[[status_bar.left]]
content = "{git_branch} +{git_ahead} -{git_behind} *{git_dirty}"
fg = "#A6E3A1"
bg = "#0F0F18"

[[status_bar.right]]
content = "{ports}"
fg = "#94E2D5"
bg = "#0F0F18"

[[status_bar.right]]
content = "{shell}"
fg = "#606070"
bg = "#1A1A28"

[[status_bar.right]]
content = "{pane_size}"
fg = "#606070"
bg = "#0F0F18"

[[status_bar.right]]
content = "{time}"
fg = "#1A1A28"
bg = "#7AA2F7"
```

---

### [notifications]

デスクトップ通知の動作（NSNotificationCenter 経由で配信）。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `enabled` | bool | `true` | デスクトップ通知の有効化。 |
| `sound` | bool | `true` | 通知時にサウンドを再生。 |

**例:**

```toml
[notifications]
enabled = true
sound = true
```

---

### [allow_flow]

Allow Flow は AI エージェント連携システム。Claude Code などのツールが権限をリクエストした際に検出し、サイドバーやオーバーレイから承認・拒否できるようにする。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `overlay_enabled` | bool | `true` | 権限リクエスト保留中にターミナルペインにオーバーレイを表示。 |
| `side_panel_enabled` | bool | `true` | サイドバーパネルに保留中リクエストを表示。 |
| `auto_focus` | bool | `false` | 権限リクエスト検出時にペインを自動フォーカス。 |
| `sound` | bool | `false` | 権限リクエスト検出時にサウンドを再生。 |

#### カスタム検出パターン

`[[allow_flow.patterns]]` を使って任意のツールの権限プロンプトを検出するカスタム正規表現パターンを追加できる:

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `tool` | String | *（必須）* | このパターンがマッチするツール名（例: `"My CLI Tool"`）。 |
| `action` | String | *（必須）* | アクションの人間が読める説明（例: `"file write"`）。 |
| `pattern` | String | *（必須）* | ターミナル出力に対してマッチさせる正規表現パターン。 |
| `yes_response` | String | `"y\n"` | リクエスト承認時に PTY に書き込む文字列。 |
| `no_response` | String | `"n\n"` | リクエスト拒否時に PTY に書き込む文字列。 |

**例:**

```toml
[allow_flow]
overlay_enabled = true
side_panel_enabled = true
auto_focus = true
sound = true

[[allow_flow.patterns]]
tool = "My Deploy Tool"
action = "production deploy"
pattern = "Deploy to production\\? \\[y/N\\]"
yes_response = "y\n"
no_response = "n\n"

[[allow_flow.patterns]]
tool = "Database CLI"
action = "destructive query"
pattern = "This will delete \\d+ rows\\. Continue\\?"
yes_response = "yes\n"
no_response = "no\n"
```

---

### [quick_terminal]

画面端からグローバルホットキーでスライドするドロップダウン（Quake風）ターミナル。

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `enabled` | bool | `true` | Quick Terminal 機能の有効化。 |
| `hotkey` | String | `"ctrl+\`"` | Quick Terminal を切り替えるグローバルホットキー（Termojinal がフォーカスされていなくても有効）。 |
| `animation` | String | `"slide_down"` | アニメーションスタイル。選択肢: `"slide_down"`, `"slide_up"`, `"fade"`, `"none"`。 |
| `animation_duration_ms` | u32 | `200` | アニメーション時間（ミリ秒）。 |
| `height_ratio` | f32 | `0.4` | 画面の高さに対する Quick Terminal の高さの割合（`0.0`〜`1.0`）。 |
| `width_ratio` | f32 | `1.0` | 画面の幅に対する Quick Terminal の幅の割合（`0.0`〜`1.0`）。 |
| `position` | String | `"center"` | 画面上の水平位置。選択肢: `"left"`, `"center"`, `"right"`。 |
| `screen_edge` | String | `"top"` | ターミナルがスライドする画面端。選択肢: `"top"`, `"bottom"`。 |
| `hide_on_focus_loss` | bool | `false` | フォーカスを失った時に Quick Terminal を非表示。 |
| `dismiss_on_esc` | bool | `true` | Escape を押した時に Quick Terminal を非表示。 |
| `show_sidebar` | bool | `false` | Quick Terminal ウィンドウでサイドバーを表示。 |
| `show_tab_bar` | bool | `false` | Quick Terminal ウィンドウでタブバーを表示。 |
| `show_status_bar` | bool | `true` | Quick Terminal ウィンドウでステータスバーを表示。 |
| `window_level` | String | `"floating"` | ウィンドウスタッキングレベル。選択肢: `"normal"`, `"floating"`, `"above_all"`。 |
| `corner_radius` | f32 | `12.0` | Quick Terminal ウィンドウの角丸半径（ピクセル）。 |
| `own_workspace` | bool | `true` | Quick Terminal に専用ワークスペースを割り当てる。 |

**例:**

```toml
[quick_terminal]
enabled = true
hotkey = "ctrl+`"
animation = "slide_down"
animation_duration_ms = 150
height_ratio = 0.5
width_ratio = 0.8
position = "center"
screen_edge = "top"
hide_on_focus_loss = true
show_status_bar = true
window_level = "floating"
corner_radius = 16.0
```

---

## キーバインド

キーバインドは別ファイルで設定する:

- **プライマリ:** `~/.config/termojinal/keybindings.toml`
- **フォールバック:** `~/Library/Application Support/termojinal/keybindings.toml`

### 3レイヤー

Termojinal は3レイヤーのキーバインドシステムを使う。各レイヤーはキーの組み合わせをアクション名にマッピングする TOML テーブル:

| レイヤー | テーブル | アクティブになる条件 |
|---|---|---|
| **normal** | `[normal]` | Termojinal がフォーカスされていて通常のシェルが実行中。 |
| **global** | `[global]` | Termojinal がフォーカスされていなくても有効（macOS の CGEventTap 経由）。 |
| **alternate_screen** | `[alternate_screen]` | TUI アプリケーション（nvim, htop など）が代替スクリーンモードで実行中。 |

ユーザー指定のバインドはデフォルトにマージされる: オーバーライドしたキーのデフォルトが置き換えられるが、他のデフォルトバインドはすべて維持される。

### デフォルトキーバインド

#### Normal レイヤー

| キー | アクション | 説明 |
|---|---|---|
| `cmd+d` | `split_right` | 現在のペインを右に分割。 |
| `cmd+shift+d` | `split_down` | 現在のペインを下に分割。 |
| `cmd+shift+enter` | `zoom_pane` | 現在のペインのズームを切替。 |
| `cmd+]` | `next_pane` | 次のペインにフォーカス。 |
| `cmd+[` | `prev_pane` | 前のペインにフォーカス。 |
| `cmd+t` | `new_tab` | 新規タブを作成。 |
| `cmd+w` | `close_tab` | 現在のペインを閉じる（タブ/ワークスペース/アプリに連鎖）。 |
| `cmd+n` | `new_workspace` | 新規ワークスペースを作成。 |
| `cmd+shift+}` | `next_tab` | 次のタブに切替。 |
| `cmd+shift+{` | `prev_tab` | 前のタブに切替。 |
| `cmd+shift+]` | `next_workspace` | 次のワークスペースに切替。 |
| `cmd+shift+[` | `prev_workspace` | 前のワークスペースに切替。 |
| `cmd+1` .. `cmd+9` | `workspace(N)` | ワークスペース N に切替。 |
| `cmd+shift+p` | `command_palette` | コマンドパレットを開く。 |
| `cmd+,` | `open_settings` | 設定を開く。 |
| `cmd+c` | `copy` | 選択範囲をクリップボードにコピー。 |
| `cmd+v` | `paste` | クリップボードから貼り付け。 |
| `cmd+f` | `search` | 検索バーを開く。 |
| `cmd+k` | `clear_scrollback` | スクロールバックバッファと画面をクリア。 |
| `cmd+l` | `clear_screen` | 画面をクリア（PTY に ESC[2J ESC[H を送信）。 |
| `cmd+=` | `font_increase` | フォントサイズを拡大。 |
| `cmd+-` | `font_decrease` | フォントサイズを縮小。 |
| `cmd+b` | `toggle_sidebar` | サイドバーを切替。 |
| `cmd+q` | `quit` | アプリケーションを終了。 |

#### Global レイヤー

| キー | アクション | 説明 |
|---|---|---|
| `ctrl+\`` | `toggle_quick_terminal` | Quick Terminal バイザーウィンドウを切替。 |

#### Alternate Screen レイヤー

デフォルトバインドなし。TUI（nvim, htop など）実行中にキーをオーバーライドするために使う。

### 全アクション一覧

| アクション名 | 説明 |
|---|---|
| `split_right` | 現在のペインを右に分割。 |
| `split_down` | 現在のペインを下に分割。 |
| `zoom_pane` | 現在のペインのズームを切替。 |
| `next_pane` | 次のペインにフォーカス。 |
| `prev_pane` | 前のペインにフォーカス。 |
| `new_tab` | 現在のワークスペースに新規タブを作成。 |
| `close_tab` | 現在のペインを閉じる（タブ/ワークスペース/アプリに連鎖）。 |
| `new_workspace` | 新規ワークスペースを作成。 |
| `next_tab` | 現在のワークスペース内の次のタブに切替。 |
| `prev_tab` | 現在のワークスペース内の前のタブに切替。 |
| `next_workspace` | 次のワークスペースに切替。 |
| `prev_workspace` | 前のワークスペースに切替。 |
| `workspace(N)` | ワークスペース N（1〜9）に切替。TOML では: `{ "workspace" = 3 }`。 |
| `command_palette` | コマンドパレットを開く。 |
| `allow_flow_panel` | Allow Flow AI パネルを開く。 |
| `unread_jump` | 次の未読通知にジャンプ。 |
| `font_increase` | フォントサイズを拡大。 |
| `font_decrease` | フォントサイズを縮小。 |
| `copy` | 選択範囲をクリップボードにコピー。 |
| `paste` | クリップボードから貼り付け。 |
| `search` | 検索バーを開く。 |
| `open_settings` | 設定を開く。 |
| `clear_screen` | 画面をクリア。 |
| `clear_scrollback` | スクロールバックバッファと画面をクリア。 |
| `toggle_sidebar` | サイドバーを切替。 |
| `toggle_quick_terminal` | Quick Terminal バイザーウィンドウを切替。 |
| `passthrough` | キーを PTY に直接転送（Termojinal をバイパス）。 |
| `quit` | アプリケーションを終了。 |
| `about` | About 画面を表示（ライセンス、クレジット、バージョン）。 |
| `none` | キーを完全に無視（デフォルトバインドを無効化）。 |
| `{ "command" = "name" }` | 名前付きコマンドまたはプラグインを実行。 |

### オーバーライドの例

```toml
# ~/.config/termojinal/keybindings.toml

[normal]
# Cmd+D を分割ではなく新規タブにリマップ
"cmd+d" = "new_tab"

# Cmd+Q を無効化（誤終了を防止）
"cmd+q" = "none"

# Allow Flow パネルを開くカスタムキーバインドを追加
"cmd+shift+a" = "allow_flow_panel"

# カスタムコマンドを実行
"cmd+shift+r" = { "command" = "my_plugin" }

[global]
# Quick Terminal に別のホットキーを使用
"cmd+shift+space" = "toggle_quick_terminal"

# コマンドパレットをグローバルに開く
"ctrl+shift+p" = "command_palette"

[alternate_screen]
# Cmd+C を TUI アプリにパススルー（例: nvim のコピー用）
"cmd+c" = "passthrough"
"cmd+v" = "passthrough"
```

---

## カラーフォーマット

すべてのカラーフィールドは CSS スタイルの16進数カラー文字列を3つの形式で受け付ける:

| 形式 | 例 | 説明 |
|---|---|---|
| `#RGB` | `#F00` | チャンネルあたり4ビット、8ビットに拡張される（例: `#F00` は `#FF0000` になる）。 |
| `#RRGGBB` | `#FF0000` | チャンネルあたり8ビット、完全不透明。 |
| `#RRGGBBAA` | `#FF000080` | チャンネルあたり8ビット + アルファ。`00` = 透明、`FF` = 不透明。 |

アルファチャンネルは検索バー（`bar_bg`）、コマンドパレット（`bg`, `overlay_color`）、ペインフォーカスボーダー（`focus_border_color`）などの半透明 UI 要素に特に便利。

**例:**

```toml
# 完全不透明な赤
cursor = "#FF0000"

# 半透明の黒オーバーレイ
overlay_color = "#00000080"

# 検索バーの半透明背景
bar_bg = "#262633F2"

# 短縮形の青
cursor = "#00F"
```

---

## テーマファイル

テーマファイルは名前でロードできる再利用可能なカラースキームを定義する。

### 配置場所

```
~/.config/termojinal/themes/<name>.toml
```

例: `~/.config/termojinal/themes/nord.toml`

### 構造

テーマファイルは `config.toml` の `[theme]` セクションと同じ構造だが、`[theme]` ヘッダーは不要。すべてのフィールドはオプションで、未指定のフィールドはビルトインのデフォルトにフォールバックする。

**例**（`~/.config/termojinal/themes/nord.toml`）:

```toml
background = "#2E3440"
foreground = "#D8DEE9"
cursor = "#D8DEE9"
selection_bg = "#434C5E"

black = "#3B4252"
bright_black = "#4C566A"
red = "#BF616A"
bright_red = "#BF616A"
green = "#A3BE8C"
bright_green = "#A3BE8C"
yellow = "#EBCB8B"
bright_yellow = "#EBCB8B"
blue = "#81A1C1"
bright_blue = "#81A1C1"
magenta = "#B48EAD"
bright_magenta = "#B48EAD"
cyan = "#88C0D0"
bright_cyan = "#8FBCBB"
white = "#E5E9F0"
bright_white = "#ECEFF4"
```

### ダーク/ライトモードの自動切替

macOS のダーク/ライト外観切り替え時にテーマを自動切替するには、`[theme]` セクションで `auto_switch = true` を設定してテーマファイル名を指定する:

```toml
[theme]
auto_switch = true
dark = "catppuccin-mocha"    # ~/.config/termojinal/themes/catppuccin-mocha.toml をロード
light = "catppuccin-latte"   # ~/.config/termojinal/themes/catppuccin-latte.toml をロード
```

`auto_switch` が有効な場合、Termojinal はシステムの外観を監視して対応するテーマファイルを自動的にロードする。`dark` と `light` の値は `.toml` 拡張子なしのテーマファイル名。
