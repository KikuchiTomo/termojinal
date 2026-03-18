# jterm 現在の進捗 — 2026-03-18

## プロジェクト概要

macOS ネイティブ GPU ターミナルエミュレータ。Rust (winit + wgpu) で実装。
設計書: `docs/claude/jterm_design_v04.md`

---

## クレート構成

```
jterm/
├── Cargo.toml                     # ワークスペース + GUI バイナリ
├── Makefile                       # install / build / run-dev
├── src/
│   ├── main.rs                    # GUI アプリ (~3300行)
│   └── config.rs                  # TOML 設定ローダー
├── crates/
│   ├── jterm-pty/                 # PTY fork/exec (3テスト)
│   ├── jterm-vt/                  # VT パーサー + セルグリッド + スクロールバック + 画像
│   │   ├── src/term.rs            # Terminal 状態マシン
│   │   ├── src/grid.rs            # Grid + ダーティ行追跡
│   │   ├── src/cell.rs            # Cell (hyperlink フィールド含む)
│   │   ├── src/scrollback.rs      # Hot (VecDeque) + Warm (mmap) 2層バッファ
│   │   └── src/image.rs           # Kitty/Sixel/iTerm2 画像プロトコル
│   ├── jterm-render/              # wgpu GPU レンダラー
│   │   ├── src/renderer.rs        # セルインスタンスレンダリング + ビューポート + ダーティキャッシュ
│   │   ├── src/atlas.rs           # フォントアトラス (等幅+Nerd+CJK+プロシージャルブロック)
│   │   ├── src/emoji_atlas.rs     # Emoji RGBA アトラス (Core Text)
│   │   ├── src/image_render.rs    # 画像テクスチャレンダリング
│   │   ├── src/shader.wgsl        # メインシェーダー (cell_width_scale対応)
│   │   ├── src/image_shader.wgsl  # 画像シェーダー
│   │   └── src/color_convert.rs   # 256色パレット変換
│   ├── jterm-session/             # セッション管理デーモン
│   │   ├── src/daemon.rs          # Unix socket IPC + セッション復元
│   │   ├── src/hotkey.rs          # CGEventTap グローバルホットキー
│   │   ├── src/persistence.rs     # JSON 永続化
│   │   └── src/bin/jtermd.rs      # デーモンバイナリ
│   ├── jterm-layout/              # 不変 SplitTree (40テスト)
│   └── jterm-ipc/                 # IPC + keybinding + jt CLI (27テスト)
├── resources/
│   └── Assets.xcassets/AppIcon.appiconset/  # アプリアイコン PNG
├── tests/integration.rs           # PTY 統合テスト (2テスト)
└── docs/
    ├── claude/jterm_design_v04.md  # 設計書
    ├── progress_diff.md            # 設計書との差分
    └── progress_current.md         # この文書
```

---

## Phase 別進捗

### Phase 1 — 動く土台: ✅ 100% 完了

- PTY fork/exec (fish/zsh/bash)
- VT パーサー: Alternate Screen, Bracketed Paste, DECSCUSR, SGR全属性, 256色, Truecolor
- OSC 0/2/7/9/99/133/777
- マウストラッキング (1000/1002/1003/1006 SGR形式)
- フォーカスイベント (1004)
- Kitty Keyboard Protocol (モード追跡)
- OSC 8 ハイパーリンク, OSC 52 クリップボード
- jtermd デーモン + JSON 永続化 + 再起動復元
- CGEventTap グローバルホットキー (Cmd+Shift+P/A)

### Phase 2 — 描画 + UI: ✅ 100% 完了

- wgpu + Metal GPU レンダリング
- フォントアトラス: SFNSMono + MesloLGS NF (Nerd) + Hiragino (CJK) + Apple Color Emoji (Core Text RGBA)
- プロシージャルブロック要素 (█▀▄▌▐ 等14種 + シェード3種)
- CJK 全幅文字対応 (cell_width_scale シェーダー)
- ダーティレンダリング (ペイン別キャッシュ)
- 120fps ProMotion (PresentMode 自動選択)
- 画像プロトコル: Kitty Graphics + Sixel + iTerm2 インライン画像
- スクロールバック: Hot tier (10K行 VecDeque) + Warm tier (mmap)
- スクロールバー + マウスホイールスクロール
- Cmd+F 検索 (ハイライト + ナビゲーション)
- 3階層モデル: Workspace > Tab > Pane
- 縦サイドバー (cmux+Arc 風デザイン, git branch/status/ports表示)
- 横タブバー (iTerm2 風, TOML カスタマイズ, 自動タイトル)
- Command Palette (Cmd+Shift+P, fuzzy検索)
- マルチペイン: Split/Close/Zoom/Navigate + ドラッグリサイズ
- ペイン DnD (Cmd+click で抽出, タブドラッグ)
- テキスト選択 + Cmd+C/V (Bracketed Paste 対応)
- IME サポート (Preedit オーバーレイ + 候補ウィンドウ位置)
- 3レイヤー keybinding (全て TOML 設定可能)
- ステータスバー (左右セグメント, 18変数, 全 TOML カスタマイズ)
- Dock アイコン (objc2 経由 NSApplication)
- 背景透過 (PostMultiplied alpha, TOML opacity 設定)

### Phase 3 — AI 連携 + 完成: ❌ 未着手

- Allow Flow エンジン
- stdio JSON プロトコル実行エンジン
- コマンド署名システム
- start-review / run-agent 等標準コマンド
- @jterm/sdk
- テーマシステム (ダーク/ライト自動切替)

---

## 設定ファイル

### ~/.config/jterm/config.toml

```toml
[font]
family = "monospace"     # フォントファミリー
size = 16.0              # フォントサイズ (px)
line_height = 1.2        # 行高さ倍率

[window]
width = 960              # 初期ウィンドウ幅
height = 640             # 初期ウィンドウ高さ
opacity = 1.0            # 背景透明度 (0.0-1.0)
padding_x = 1.0          # 水平パディング (セル幅単位) ※未統合
padding_y = 0.5          # 垂直パディング (セル高さ単位) ※未統合
sidebar_width = 200      # サイドバー初期幅 ※未統合

[theme]
background = "#11111A"   # 背景色 ※未統合 (color_convert のハードコード値を使用中)
foreground = "#D9D9D9"   # 前景色 ※未統合
cursor = "#D9D9D9"       # カーソル色 ※未統合
selection_bg = "#3A3A50"  # 選択背景 ※未統合

[tab_bar]
format = "{title|cwd_base|Tab {index}}"
always_show = false
max_width = 200

[status_bar]
enabled = true
height = 24
background = "#1A1A24"
left = [
    { content = " {user}@{host} ", fg = "#FFFFFF", bg = "#3A3AFF" },
    { content = " {cwd_short} ", fg = "#CCCCCC", bg = "#2A2A34" },
    { content = " {git_branch} {git_status} ", fg = "#A6E3A1", bg = "#1A1A24" },
]
right = [
    { content = " {ports} ", fg = "#94E2D5", bg = "#1A1A24" },
    { content = " {shell} ", fg = "#888888", bg = "#2A2A34" },
    { content = " {pane_size} ", fg = "#888888", bg = "#1A1A24" },
    { content = " {font_size}px ", fg = "#888888", bg = "#2A2A34" },
    { content = " {time} ", fg = "#FFFFFF", bg = "#3A3AFF" },
]
```

### ~/.config/jterm/keybindings.toml

```toml
[keybindings]
"cmd+d" = "split_right"
"cmd+shift+d" = "split_down"
"cmd+shift+enter" = "zoom_pane"
"cmd+t" = "new_tab"
"cmd+w" = "close_tab"
"cmd+n" = "new_workspace"
"cmd+k" = "clear_scrollback"
"cmd+l" = "clear_screen"
"cmd+f" = "search"
"cmd+c" = "copy"
"cmd+v" = "paste"
"cmd+q" = "quit"
"cmd+b" = "toggle_sidebar"
"cmd+shift+p" = "command_palette"
"cmd+=" = "font_increase"
"cmd+-" = "font_decrease"
# ... 全キーバインド TOML で変更可能
```

---

## 既知の未解決事項

1. **背景透過が効いていない可能性** — `PostMultiplied` alpha モードを設定し bg_opacity を適用するコードは入っているが、デフォルト opacity=1.0 のため透過しない。`config.toml` で `opacity = 0.85` 等に設定すれば透過するはず。動作未確認。

2. **theme セクション未統合** — config.toml の `[theme]` で背景色/前景色を設定できるが、`color_convert.rs` の `DEFAULT_BG` / `DEFAULT_FG` がハードコードのまま。設定値をレンダラーに渡す統合が必要。

3. **window.padding_x/y/sidebar_width 未統合** — config.toml から読めるが、実際のレンダリングでは使われていない。

4. **ASCII アートのずれ** — ブロック要素はプロシージャル描画で改善したが、一部の TUI アプリでまだずれる可能性。

5. **git_remote** — ステータスバーの `{git_remote}` 変数は常に空。`WorkspaceInfo` への `git remote` コマンド追加が必要。

---

## テスト

**合計: 162+ テスト**
- jterm-pty: 3
- jterm-vt: 42+ (scrollback 10, image 30, VT parser 32)
- jterm-render: 21+
- jterm-layout: 40
- jterm-ipc: 27+
- integration: 2

全テスト通過。

---

## ビルド・実行

```bash
make install    # 依存関係インストール
make            # release ビルド
make run-dev    # dev モード起動
make test       # テスト実行
```

## 次回作業予定

1. **Phase 3 開始**: Allow Flow エンジン, テーマシステム
2. **theme 統合**: config.toml の色設定をレンダラーに反映
3. **背景透過の動作確認と修正**
4. **padding/sidebar_width の config 統合**
