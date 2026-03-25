# コマンドシステム

## 概要

カスタムコマンドはインタラクティブなスクリプトで Termojinal を拡張する。コマンドは任意の実行可能ファイル（シェルスクリプト、Python、Deno、コンパイル済みバイナリ）で、**stdin/stdout 上の行区切り JSON** で Termojinal と通信する。各 JSON オブジェクトは正確に1行を占め、改行文字で終端される。

コマンドはコマンドパレット（Cmd+Shift+P）にビルトインアクションと並んで表示される。ユーザーがコマンドを選択すると、Termojinal はそれを子プロセスとして起動し、stdout をコマンドパレット UI にパイプし、ユーザーの応答を stdin に書き戻す。コマンドの stderr は診断用に Termojinal のログに転送される。

環境変数 `TERMOJINAL_SOCKET` には Termojinal の IPC Unix ソケットのパスが設定され、必要に応じてコマンドがアプリケーションと直接やり取りできる。

コマンドは `command.toml` があるディレクトリ（コマンド自身のディレクトリ）を作業ディレクトリとして実行される。

## ディレクトリ構造

コマンドは `~/.config/termojinal/commands/` から検出される。各コマンドは `command.toml` マニフェストと実行可能なエントリポイントを含む独自のサブディレクトリに配置する:

```
~/.config/termojinal/commands/
├── my-command/
│   ├── command.toml    # メタデータマニフェスト
│   └── run.sh          # 実行可能エントリポイント
├── another-command/
│   ├── command.toml
│   └── main.py
```

Termojinal は起動時にコマンドディレクトリの直下のサブディレクトリをすべてスキャンする。サブディレクトリは有効な `command.toml` を含み、参照されるスクリプトが存在する場合にのみコマンドとしてロードされる。`command.toml` のないディレクトリは黙って無視され、不正なマニフェストは警告としてログに記録されてスキップされる。

ロードされたコマンドはパレットでの順序を確定的にするため、名前のアルファベット順にソートされる。

## command.toml

マニフェストファイルはコマンドのメタデータとエントリポイントを記述する。以下のフィールドを持つ `[command]` テーブルを含む必要がある。

### 例

```toml
[command]
name = "My Command"
description = "What this command does"
icon = "star"           # SF Symbol 名
version = "1.0.0"
author = "you"
run = "./run.sh"        # 実行可能ファイルへの相対パス
tags = ["git", "tools"]
```

### フィールドリファレンス

| フィールド | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|-----------|------|
| `name` | string | はい | -- | コマンドパレットに表示される人間が読める名前。 |
| `description` | string | はい | -- | パレットで名前の下に表示される短い説明。 |
| `run` | string | はい | -- | コマンドディレクトリから実行可能スクリプトへの相対パス。 |
| `icon` | string | いいえ | `""` | コマンドアイコンの SF Symbol 名（例: `"star"`, `"trash"`）。 |
| `version` | string | いいえ | `""` | セマンティックバージョン文字列。 |
| `author` | string | いいえ | `""` | 作者名または識別子。 |
| `tags` | string[] | いいえ | `[]` | パレットでのフィルタリング用検索タグ。 |
| `signature` | string | いいえ | `null` | 16進数エンコードされた Ed25519 署名。[コマンド署名](#コマンド署名)を参照。 |

## JSON プロトコル

プロトコルは**メッセージ**（コマンドから Termojinal へ、stdout に書き込み）と**レスポンス**（Termojinal からコマンドへ、stdin に書き込み）で構成される。すべてのメッセージとレスポンスは1行に1つの JSON オブジェクトで、`"type"` フィールドで識別される。

### ライフサイクル

1. Termojinal がコマンドプロセスを起動する。
2. コマンドが JSON メッセージを stdout に書き込む。
3. メッセージがインタラクティブ（`fuzzy`, `multi`, `confirm`, `text`）な場合、Termojinal が UI を表示し、ユーザーの応答をコマンドの stdin に書き込む。
4. コマンドが stdin からレスポンスを読み、処理を行い、次のメッセージを送信する。
5. コマンドが `done` または `error` を送信するか、プロセスが終了するまでループが続く。

ファイアアンドフォーゲットメッセージ（`info`, `done`, `error`）はレスポンスを生成しない。送信後に stdin から読み込もうとしないこと。

### コマンドから Termojinal へのメッセージ（stdout）

#### 1. `fuzzy` -- ファジー選択リスト

フィルタリング可能なアイテムリストを表示する。ユーザーは1つだけ選択する。

```json
{
  "type": "fuzzy",
  "prompt": "Select a branch",
  "items": [
    {
      "value": "main",
      "label": "main",
      "description": "Default branch",
      "preview": "Last commit: fix typo in README",
      "icon": "arrow.triangle.branch"
    },
    {
      "value": "feature/login",
      "label": "feature/login",
      "description": "WIP login page"
    }
  ],
  "preview": true
}
```

| フィールド | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|-----------|------|
| `type` | `"fuzzy"` | はい | -- | メッセージタイプ識別子。 |
| `prompt` | string | はい | -- | リスト上部に表示されるプロンプトテキスト。 |
| `items` | FuzzyItem[] | はい | -- | 選択可能なアイテムの配列（以下参照）。 |
| `preview` | boolean | いいえ | `false` | アイテムプレビューのプレビューペインを表示するか。 |

**FuzzyItem のフィールド:**

| フィールド | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|-----------|------|
| `value` | string | はい | -- | このアイテムが選択された時に返される値。 |
| `label` | string | いいえ | value と同じ | リストに表示されるテキスト。 |
| `description` | string | いいえ | -- | ラベルの下に表示される補足説明テキスト。 |
| `preview` | string | いいえ | -- | プレビューペインに表示されるコンテンツ（プレーンテキスト）。 |
| `icon` | string | いいえ | -- | アイテムアイコンの SF Symbol 名。 |

**レスポンス:**

```json
{"type": "selected", "value": "main"}
```

#### 2. `multi` -- 複数選択リスト

チェックボックス付きのフィルタリング可能なリストを表示する。ユーザーは1つ以上のアイテムを選択できる。

```json
{
  "type": "multi",
  "prompt": "Select branches to delete",
  "items": [
    {"value": "feature/old", "label": "feature/old", "description": "merged 3 days ago"},
    {"value": "fix/typo", "label": "fix/typo", "description": "merged 1 week ago"}
  ]
}
```

| フィールド | 型 | 必須 | 説明 |
|-----------|------|------|------|
| `type` | `"multi"` | はい | メッセージタイプ識別子。 |
| `prompt` | string | はい | リスト上部に表示されるプロンプトテキスト。 |
| `items` | FuzzyItem[] | はい | 選択可能なアイテムの配列。 |

**レスポンス:**

```json
{"type": "multi_selected", "values": ["feature/old", "fix/typo"]}
```

#### 3. `confirm` -- はい/いいえダイアログ

はい/いいえの確認ダイアログを表示する。

```json
{
  "type": "confirm",
  "message": "Delete selected branches and their worktrees?",
  "default": false
}
```

| フィールド | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|-----------|------|
| `type` | `"confirm"` | はい | -- | メッセージタイプ識別子。 |
| `message` | string | はい | -- | 表示する質問。 |
| `default` | boolean | いいえ | `false` | 事前に選択されるオプション（true = はい）。 |

**レスポンス:**

```json
{"type": "confirmed", "yes": true}
```

#### 4. `text` -- テキスト入力

1行のテキスト入力フィールドを表示する。

```json
{
  "type": "text",
  "label": "Repository URL",
  "placeholder": "https://github.com/user/repo.git",
  "default": "",
  "completions": ["https://github.com/", "git@github.com:"]
}
```

| フィールド | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|-----------|------|
| `type` | `"text"` | はい | -- | メッセージタイプ識別子。 |
| `label` | string | はい | -- | 入力フィールドの上に表示されるラベルテキスト。 |
| `placeholder` | string | いいえ | `""` | 入力が空の時に表示されるプレースホルダーテキスト。 |
| `default` | string | いいえ | `""` | 入力フィールドに事前入力される初期値。 |
| `completions` | string[] | いいえ | `[]` | 入力の補完候補。 |

**レスポンス:**

```json
{"type": "text_input", "value": "https://github.com/user/repo.git"}
```

#### 5. `info` -- 進捗/情報メッセージ

一時的な情報メッセージを表示する。レスポンスは**生成されない**。コマンドは即座に次のメッセージの送信を続行すべき。

```json
{
  "type": "info",
  "message": "Cloning repository..."
}
```

| フィールド | 型 | 必須 | 説明 |
|-----------|------|------|------|
| `type` | `"info"` | はい | メッセージタイプ識別子。 |
| `message` | string | はい | 表示するメッセージ。 |

#### 6. `done` -- コマンド完了

正常完了を通知する。レスポンスは**生成されない**。コマンドプロセスはこのメッセージ送信後に終了すべき。`notify` が指定されている場合、Termojinal がそのテキストで macOS 通知をトリガーする。

```json
{
  "type": "done",
  "notify": "Repository cloned successfully"
}
```

| フィールド | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|-----------|------|
| `type` | `"done"` | はい | -- | メッセージタイプ識別子。 |
| `notify` | string | いいえ | `null` | 設定した場合、このテキストで macOS 通知をトリガー。 |

#### 7. `error` -- エラーメッセージ

コマンドがエラーに遭遇したことを通知する。レスポンスは**生成されない**。コマンドプロセスはこのメッセージ送信後に終了すべき。

```json
{
  "type": "error",
  "message": "gh CLI is not installed"
}
```

| フィールド | 型 | 必須 | 説明 |
|-----------|------|------|------|
| `type` | `"error"` | はい | メッセージタイプ識別子。 |
| `message` | string | はい | 表示するエラーの説明。 |

### Termojinal からコマンドへのレスポンス（stdin）

レスポンスはユーザーがインタラクティブメッセージを完了した後、コマンドの stdin に単一の JSON 行として書き込まれる。

| レスポンスタイプ | 送信後 | フィールド | 説明 |
|----------------|--------|-----------|------|
| `selected` | `fuzzy` | `value: string` | 選択されたアイテムの値。 |
| `multi_selected` | `multi` | `values: string[]` | 選択されたアイテムの値の配列。 |
| `confirmed` | `confirm` | `yes: boolean` | ユーザーが確認（true）または拒否（false）したか。 |
| `text_input` | `text` | `value: string` | ユーザーが入力したテキスト。 |
| `cancelled` | 任意 | （なし） | ユーザーが Escape を押してキャンセルした。 |

`cancelled` レスポンスはどのインタラクティブメッセージへの応答としても発生し得る。コマンドは常にこれを処理すべきで、通常は `done` を送信して終了する。

```json
{"type": "cancelled"}
```

## コマンドの記述

### Bash の例

git ブランチを一覧表示し、ユーザーに1つ選択させ、切替を確認して結果を報告する完全なコマンド:

**command.toml:**

```toml
[command]
name = "Switch Branch"
description = "Interactively switch git branches"
icon = "arrow.triangle.branch"
run = "./run.sh"
tags = ["git"]
```

**run.sh:**

```bash
#!/usr/bin/env bash
set -euo pipefail

# Step 1: git ブランチからアイテムリストを構築
branches=$(git branch --format='%(refname:short)')
items="["
first=true
while IFS= read -r branch; do
    [ -z "$branch" ] && continue
    if [ "$first" = true ]; then first=false; else items+=","; fi
    items+="{\"value\":\"$branch\",\"label\":\"$branch\"}"
done <<< "$branches"
items+="]"

# Step 2: ファジー選択を表示
echo "{\"type\":\"fuzzy\",\"prompt\":\"Switch to branch\",\"items\":$items}"

# Step 3: レスポンスを読み込み
read -r response

# キャンセル処理
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

selected=$(echo "$response" | jq -r '.value // empty')

# Step 4: 確認
echo "{\"type\":\"confirm\",\"message\":\"Switch to $selected?\",\"default\":true}"
read -r confirm_response

confirmed=$(echo "$confirm_response" | jq -r '.yes')
if [ "$confirmed" != "true" ]; then
    echo '{"type":"done"}'
    exit 0
fi

# Step 5: アクションを実行
echo "{\"type\":\"info\",\"message\":\"Switching to $selected...\"}"
git checkout "$selected" 2>/dev/null

# Step 6: 完了（通知付き）
echo "{\"type\":\"done\",\"notify\":\"Switched to $selected\"}"
```

### Python の例

Python の `json` モジュールを使ったコマンド:

**command.toml:**

```toml
[command]
name = "Create File"
description = "Create a new file from a template"
icon = "doc.badge.plus"
run = "./run.py"
tags = ["file", "template"]
```

**run.py:**

```python
#!/usr/bin/env python3
import json
import sys
import os

def send(msg):
    """JSON メッセージを stdout に書き込む。"""
    print(json.dumps(msg), flush=True)

def receive():
    """JSON レスポンスを stdin から読み込む。"""
    line = sys.stdin.readline().strip()
    return json.loads(line)

# Step 1: テンプレートを選択
send({
    "type": "fuzzy",
    "prompt": "Select template",
    "items": [
        {"value": "py", "label": "Python", "description": "Python script with main()"},
        {"value": "sh", "label": "Shell", "description": "Bash script with set -euo pipefail"},
        {"value": "rs", "label": "Rust", "description": "Rust main.rs"},
    ]
})

response = receive()
if response["type"] == "cancelled":
    send({"type": "done"})
    sys.exit(0)

template = response["value"]

# Step 2: ファイル名を取得
send({
    "type": "text",
    "label": "Filename",
    "placeholder": f"example.{template}",
    "default": "",
})

response = receive()
if response["type"] == "cancelled":
    send({"type": "done"})
    sys.exit(0)

filename = response["value"]

# Step 3: ファイルを作成
send({"type": "info", "message": f"Creating {filename}..."})

templates = {
    "py": '#!/usr/bin/env python3\n\ndef main():\n    pass\n\nif __name__ == "__main__":\n    main()\n',
    "sh": '#!/usr/bin/env bash\nset -euo pipefail\n\n',
    "rs": 'fn main() {\n    println!("Hello, world!");\n}\n',
}

with open(filename, "w") as f:
    f.write(templates.get(template, ""))

send({"type": "done", "notify": f"Created {filename}"})
```

### Deno / TypeScript の例

Termojinal リポジトリの `sdk/` ディレクトリには、JSON プロトコルをラップする高レベルヘルパーを持つ型付き Deno SDK（`@termojinal/sdk`）が含まれている。`mod.ts` からインポートして `fuzzy`, `multi`, `confirm`, `text`, `info`, `done`, `error` 関数にアクセスできる。

**command.toml:**

```toml
[command]
name = "Run Task"
description = "Select and run a project task"
icon = "play.circle"
run = "./run.ts"
tags = ["tasks"]
```

**run.ts:**

```typescript
#!/usr/bin/env -S deno run --allow-read

import { fuzzy, confirm, info, done, CancelledError } from "@termojinal/sdk";

try {
  const selected = await fuzzy("Select a task", [
    { value: "build", label: "Build", description: "Run cargo build" },
    { value: "test", label: "Test", description: "Run cargo test" },
    { value: "lint", label: "Lint", description: "Run clippy" },
  ]);

  const ok = await confirm(`Run ${selected}?`);
  if (!ok) {
    done();
    Deno.exit(0);
  }

  info(`Running ${selected}...`);
  // ... タスクを実行 ...
  done(`${selected} completed!`);
} catch (e) {
  if (e instanceof CancelledError) {
    done();
  } else {
    throw e;
  }
}
```

SDK は以下をエクスポートする:

**高レベル関数**（プロトコルのシリアライゼーションとキャンセルを処理）:

- `fuzzy(prompt, items)` -- 選択された値の文字列を返す
- `multi(prompt, items)` -- 選択された値の文字列の配列を返す
- `confirm(message, default?)` -- boolean を返す
- `text(label, options?)` -- 入力された文字列を返す
- `info(message)` -- ファイアアンドフォーゲットの進捗表示
- `done(notify?)` -- 完了を通知、オプションで macOS 通知
- `error(message)` -- エラーを通知して終了

**低レベル I/O**（カスタムプロトコル処理用）:

- `send(message)` -- `CommandMessage` の JSON 行を stdout に書き込む
- `receive()` -- `CommandResponse` の JSON 行を stdin から読み込む

すべてのインタラクティブ関数はユーザーが Escape を押すと `CancelledError` をスローし、try/catch でのキャンセル処理のクリーンなパターンを提供する。

## コマンド署名

コマンドは Ed25519 で暗号署名して信頼性を確立できる。署名ステータスはコマンドパレットでの表示に影響する:

| ステータス | パレット表示 | 説明 |
|-----------|-------------|------|
| 未署名 | "Plugin" | 署名なし。ユーザー作成コマンドのデフォルト。 |
| 検証済み | チェックマーク付き "Verified" | 署名が Termojinal の公式公開鍵と一致。 |
| 不正 | 警告インジケーター | 署名はあるが検証に失敗。 |

### 署名ワークフロー

1. **鍵ペアの生成**（初回のみ）:

   ```sh
   termojinal-sign --generate-key
   ```

   秘密鍵（16進数エンコード、64文字）と対応する公開鍵が出力される。秘密鍵は安全に保管すること。

2. **コマンドの署名:**

   ```sh
   termojinal-sign path/to/command.toml <secret-key-hex>
   ```

   TOML コンテンツ（`signature` フィールド自体を除く）に対して Ed25519 署名を計算し、16進数エンコードされた署名を `command.toml` ファイルの `signature` フィールドに書き込む。

3. **検証**はロード時に自動的に行われる。Termojinal は `signature` フィールドを読み、TOML コンテンツからそれを除去して、残りのコンテンツを組み込みの公式公開鍵に対して検証する。

### 署名済み command.toml の例

```toml
[command]
name = "My Signed Command"
description = "A verified command"
icon = "checkmark.seal"
run = "./run.sh"
signature = "a1b2c3d4...64_bytes_hex_encoded..."
```

## バンドルコマンド

以下のコマンドがリポジトリの `commands/` ディレクトリに同梱されている。プロトコルのリファレンス実装として機能する。

### hello-world

**プロトコルのデモ。** 異なる言語の挨拶をファジーリストで表示し、選択内容を info メッセージで表示して、macOS 通知で完了する。コマンドライフサイクルを理解するための最小の出発点。

- Icon: `hand.wave`
- Tags: `demo`, `example`

### start-review

**GitHub PR レビューワークフロー。** `gh pr list` でレビュー待ちの PR を取得し、ファジーセレクターで表示し、選択されたブランチをフェッチして、独立したレビュー用の git worktree をセットアップする。`gh` CLI のインストールと認証が必要。

- Icon: `arrow.triangle.branch`
- Tags: `github`, `review`, `claude`

### switch-worktree

**Git worktree の切替。** `git worktree list` で既存の git worktree を一覧表示し、ディレクトリ名とブランチを示すファジーセレクターで表示し、選択されたパスをワークスペース切替用に通知する。

- Icon: `arrow.triangle.swap`
- Tags: `git`, `worktree`

### kill-merged

**マージ済みブランチのクリーンアップ。** `main` にマージ済みのブランチを検出し、複数選択リストで表示（関連する worktree パスも表示）、破壊的操作の前に確認を求めてから、worktree とブランチの両方を削除する。`multi` の後に `confirm`、そしてアクションという完全なインタラクティブループのデモ。

- Icon: `trash`
- Tags: `git`, `cleanup`, `worktree`

### clone-and-open

**リポジトリのクローンとオープン。** テキスト入力でリポジトリ URL を要求し、ターゲットディレクトリを尋ね（デフォルト `~/repos`）、リポジトリをクローン（またはディレクトリが既にある場合はオープンを提案）し、通知で完了する。複数の `text` 入力と `confirm` のフォールバックの連鎖のデモ。

- Icon: `square.and.arrow.down`
- Tags: `git`, `clone`

### run-agent

**AI エージェントの起動。** AI エージェント（Claude Code, Codex CLI, Aider）のファジーセレクターを表示し、テキスト入力で作業ディレクトリを要求し、選択されたエージェントがインストールされているか検証して、起動を通知する。依存関係が見つからない場合の `error` タイプのデモ。

- Icon: `brain`
- Tags: `ai`, `agent`, `claude`

## Tips

- **シェルでの JSON 構築:** 利用可能な場合は `jq` を使って JSON を構築する。単純なケースでは文字列連結も可能だが、エスケープに注意が必要。バンドルコマンドで両方のアプローチを示している。

- **作業ディレクトリ:** コマンドは `command.toml` があるディレクトリ（コマンド自身のディレクトリ）を CWD として実行される。ユーザーのプロジェクトディレクトリではない。プロジェクトディレクトリへのアクセスは環境変数か、シェルの元の `$PWD` からの相対パス解決で行う。

- **キャンセルは常に可能:** ユーザーはどのインタラクティブステップでも Escape を押せる。常に `{"type":"cancelled"}` レスポンスをチェックし、`{"type":"done"}` を送信してクリーンに終了すること。

- **プレビューコンテンツ:** ファジーアイテムの `preview` フィールドはプレビューペインにプレーンテキストとしてレンダリングされる。`fuzzy` メッセージに `"preview": true` を設定してプレビューペインを有効にする。

- **通知:** `done` メッセージの `notify` フィールドは NSNotificationCenter 経由でネイティブの macOS 通知をトリガーする。長時間実行コマンドの完了をユーザーに知らせるために使う。

- **デバッグ用 stderr:** コマンドの stderr は Termojinal のログに転送される。プロトコルに干渉せずに診断出力を行うには、シェルスクリプトで `echo "debug info" >&2` を、他の言語で `eprint!` / `print(..., file=sys.stderr)` を使う。

- **1行に1つの JSON オブジェクト:** プロトコルは厳密に行区切り。JSON メッセージを整形出力（pretty-print）しないこと。各メッセージは `\n` で終端される1行でなければならない。

- **スクリプトを実行可能に:** 実行スクリプトには実行権限が必要。Unix では作成後に `chmod +x run.sh` を実行する。
