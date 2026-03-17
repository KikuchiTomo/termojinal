# jterm 実装進捗 — 設計書 v0.4 との差分

**作成日**: 2026-03-17

---

## 設計書との構造的な差異

### ウィンドウ・UI レイヤー

| 設計書 | 実装 | 理由 |
|---|---|---|
| Swift / NSView / NSTextInputClient | **winit 0.30** (Rust) | winit が macOS IME をネイティブサポートしておりSwift FFI 不要。開発速度を優先 |
| SwiftUI 縦型サイドバー | **wgpu で直接描画** | Swift 統合なしでも同等の見た目を実現可能 |
| NSPanel Command Palette | **wgpu オーバーレイ** | winit ウィンドウ内にフローティングUI として描画 |
| UNUserNotificationCenter | **未実装** | Phase 3 の Allow Flow と同時に実装予定 |

### データ階層

| 設計書 | 実装 |
|---|---|
| ワークスペース + タブ（暗黙的） | **Workspace > Tab > Pane の明示的3階層** |
| サイドバーにワークスペース一覧 | **縦サイドバー = ワークスペース切替** |
| タブバーはなし（サイドバーで兼用） | **ワークスペース内に複数タブがある時のみタブバー表示（24px）** |

---

## Phase 1 — 動く土台

| 設計書の項目 | 状態 | 差分・備考 |
|---|---|---|
| jterm-pty: posix_openpt / fork / exec | **✅ 完了** | 設計書通り |
| fish/zsh/bash 全対応 | **✅ 完了** | $SHELL 自動検出 |
| jterm-vt: Alternate Screen | **✅ 完了** | ESC[?1049h/l |
| jterm-vt: Bracketed Paste | **✅ 完了** | ESC[?2004h/l |
| jterm-vt: DECSCUSR カーソル形状 | **✅ 完了** | |
| jterm-vt: DECSC/DECRC | **✅ 完了** | |
| jterm-vt: DECSTBM スクロール領域 | **✅ 完了** | |
| jterm-vt: SGR 全属性 | **✅ 完了** | bold/italic/underline/blink/reverse/strikethrough/dim/hidden + underline variants |
| jterm-vt: 256色 + Truecolor | **✅ 完了** | ESC[38;5;Nm / ESC[38;2;R;G;Bm / ESC[58;2;R;G;Bm (underline色) |
| jterm-vt: OSC 0/2 タイトル | **✅ 完了** | |
| jterm-vt: OSC 7 CWD | **✅ 完了** | |
| jterm-vt: OSC 9/99/777 通知 | **✅ 完了** | パース済、デスクトップ通知未連携 |
| jterm-vt: OSC 133 Shell Integration | **✅ 完了** | prompt_start/command_start/executed/finished |
| jterm-session: デーモン化 | **✅ 完了** | jtermd バイナリ |
| jterm-session: JSON 永続化 | **✅ 完了** | ~/.local/share/jterm/sessions/ |
| jterm-session: 再起動復元 | **✅ 完了** | ★設計書では「復元」のみ。実装は stale PID 検出 + CWD で再 spawn |
| jtermd: CGEventTap グローバルホットキー | **✅ 完了** | ★Accessibility 権限がない場合は graceful degradation |

### Phase 1 設計書にない追加実装

| 追加項目 | 内容 |
|---|---|
| マウストラッキング (1000/1002/1003) | SGR 1006 形式でエンコード。nvim 対応 |
| フォーカスイベント (1004) | ESC[I / ESC[O |
| Kitty Keyboard Protocol | モード追跡 (CSI > flags u / CSI < u) |
| OSC 8 ハイパーリンク | URI パース、セルにフラグ |
| OSC 52 クリップボード | Set/Query、base64 デコード |

---

## Phase 2 — 描画 + UI

| 設計書の項目 | 状態 | 差分・備考 |
|---|---|---|
| wgpu + Metal GPU 描画 | **✅ 完了** | wgpu 24, Metal backend |
| フォントアトラス（等幅） | **✅ 完了** | fontdue + SFNSMono |
| フォントアトラス（Nerd Font） | **✅ 完了** | MesloLGS NF 自動検出フォールバック |
| フォントアトラス（Emoji） | **✅ 完了** | ★設計書は「Apple Color Emoji」。実装は Core Text で RGBA ラスタライズ + 専用テクスチャアトラス |
| フォントアトラス（CJK） | **✅ 完了** | ★設計書にない追加。Hiragino Sans フォールバックで日本語グリフ対応 |
| ダーティ描画 | **✅ 完了** | ペイン別キャッシュ、変更行のみ再構築 |
| 120fps | **△ 部分的** | 構造は対応。ProMotion 検出・PresentMode 切替は未実装 |
| 画像テクスチャキャッシュ（Kitty） | **❌ 未着手** | |
| 画像テクスチャキャッシュ（Sixel） | **❌ 未着手** | |
| 画像テクスチャキャッシュ（iTerm2） | **❌ 未着手** | |
| スクロールバック Hot tier | **✅ 完了** | 10,000行 VecDeque |
| スクロールバック Warm tier (mmap) | **✅ 完了** | ★memmap2 で ~/.local/share/jterm/scrollback/{session-id}.bin |
| Cmd+F 検索 | **❌ 未着手** | Hot tier 検索は構造的に可能、UI 未実装 |
| jterm-layout: 不変 SplitTree | **✅ 完了** | 40テスト、split/close/resize/navigate/zoom |
| ドラッグリサイズ（最小 50px） | **✅ 完了** | マウスドラッグ + リサイズカーソル |
| Swift: NSWindow + NSView (IME) | **winit 代替** | ★winit 0.30 の IME サポートで代替。Preedit オーバーレイ + 候補ウィンドウ位置制御 |
| Swift: 縦型サイドバー | **wgpu 代替** | ★wgpu で直接描画。ワークスペース一覧 + アクティブインジケータ |
| Swift: NSPanel Command Palette | **wgpu 代替** | ★wgpu オーバーレイ。fuzzy 検索、14 コマンド |
| 3レイヤー keybinding | **✅ 完了** | normal/global/alternate_screen、TOML 設定 |
| jterm-ipc: Unix socket | **✅ 完了** | JSON プロトコル、27テスト |
| jterm-ipc: jt CLI | **✅ 完了** | list/new/kill/resize/ping |

### Phase 2 設計書にない追加実装

| 追加項目 | 内容 |
|---|---|
| Workspace > Tab > Pane 3階層 | 設計書は Workspace + Pane の2階層。Tab レイヤーを追加 |
| タブバー（条件表示） | ワークスペース内に複数タブがある時のみ 24px タブバー |
| テキスト選択 + Cmd+C/V | ドラッグ選択、Bracketed Paste 対応ペースト |
| スクロールバー | スクロールバック量に応じた位置インジケータ |
| パディング | 左右1文字、上下0.5文字（シングルペインのみ） |
| ペイン間セパレータ + フォーカスボーダー | 2px グレー線 + 青フォーカスインジケータ |
| マウスホイールスクロール | スクロールバック閲覧 + マウスモード時は PTY 転送 |
| Cmd+K / Cmd+L | 画面+スクロールバッククリア / 画面クリア |
| Cmd+B | サイドバー表示切替 |
| Cmd+N | 新規ワークスペース |
| macOS ActivationPolicy::Regular | cargo run でもキーボードフォーカスを受け取れるように |

---

## Phase 3 — AI 連携 + 完成（未着手）

| 設計書の項目 | 状態 | 備考 |
|---|---|---|
| jterm-claude: Allow Flow エンジン | **❌** | |
| オーバーレイ UI (Y/N 即答) | **❌** | |
| サイドパネル (Cmd+Shift+A) | **❌** | CGEventTap でホットキー検出済、UI 未実装 |
| 一括承認 | **❌** | |
| ルール記憶 | **❌** | |
| stdio JSON プロトコル完全実装 | **❌** | 7 type 定義済だが実行エンジン未実装 |
| コマンド署名システム | **❌** | |
| start-review コマンド | **❌** | |
| switch-worktree コマンド | **❌** | |
| run-agent コマンド | **❌** | |
| @jterm/sdk | **❌** | |
| テーマシステム（ダーク/ライト自動切替） | **❌** | カラー定数はハードコード |

---

## 残タスク優先順位

### 高（Phase 2 完了に必要）

1. **画像プロトコル** — Kitty Graphics + Sixel + iTerm2 inline image
2. **Cmd+F 検索** — Hot/Warm tier をストリーミング検索
3. **120fps ProMotion** — ディスプレイ検出 + PresentMode::Mailbox

### 中（Phase 3 準備）

4. **テーマシステム** — TOML 定義、ダーク/ライト自動切替
5. **Allow Flow エンジン** — PTY 出力の正規表現監視 + オーバーレイ UI
6. **stdio JSON プロトコル実行エンジン** — fuzzy/multi/confirm/text/info/done/error
7. **コマンド定義 (TOML + 実行ファイル)** — start-review, run-agent 等

### 低（品質向上）

8. **デスクトップ通知** — UNUserNotificationCenter 連携
9. **OSC 8 クリック** — Cmd+クリックで URL を open
10. **Kitty Keyboard エンコーディング** — key_to_bytes での CSI u エンコード
11. **Swift 移行** — winit → NSView（IME 品質向上が必要になった場合のみ）

---

## クレート構成（実装済み）

```
jterm/
├── Cargo.toml
├── Makefile
├── src/main.rs                    # GUI アプリ（winit + wgpu）
├── crates/
│   ├── jterm-pty/                 # PTY fork/exec（3テスト）
│   ├── jterm-vt/                  # VT パーサー + セルグリッド + スクロールバック（42テスト）
│   │   └── src/scrollback.rs      # Hot + Warm (mmap) 2層バッファ
│   ├── jterm-render/              # wgpu GPU レンダラー（21テスト）
│   │   ├── src/atlas.rs           # フォントアトラス（等幅 + Nerd + CJK フォールバック）
│   │   ├── src/emoji_atlas.rs     # Emoji RGBA アトラス（Core Text）
│   │   ├── src/renderer.rs        # セルインスタンスレンダリング + ビューポート + ダーティキャッシュ
│   │   ├── src/shader.wgsl        # WGSL 頂点 + フラグメントシェーダー
│   │   └── src/color_convert.rs   # 256色パレット変換
│   ├── jterm-session/             # セッション管理デーモン
│   │   ├── src/daemon.rs          # Unix socket IPC + セッション復元
│   │   ├── src/hotkey.rs          # CGEventTap グローバルホットキー
│   │   ├── src/persistence.rs     # JSON 永続化
│   │   └── src/bin/jtermd.rs      # デーモンバイナリ
│   ├── jterm-layout/              # 不変 SplitTree（40テスト）
│   └── jterm-ipc/                 # IPC プロトコル + keybinding + jt CLI（27テスト）
├── tests/integration.rs           # PTY 統合テスト（2テスト）
└── docs/
    ├── claude/jterm_design_v04.md  # 設計書
    └── progress_diff.md            # この文書
```

**合計テスト数: 135+**
