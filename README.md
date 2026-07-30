# agent-spaces

zellij用サイドバープラグイン。タブ > ペインを縦並びで一覧し、各ペインで動くAIエージェント（Claude Code等）の状態を可視化して、グローバルキーでジャンプする。

```
tenacious-cymbal          ← セッション名
▸ 1 scheme
    nu
    koka
▾ 2 zeli-c                ← アクティブタブ
    nvim
  » claude +2             ← working、サブエージェント2つ稼働中
▸ 3 review
  ◆ claude                ← 入力待ち（要対応）
```

## 状態アイコン

| アイコン | 状態 | 意味 |
|---|---|---|
| `»` | working | プロンプト処理中 |
| `◆` | blocked | 許可待ち・入力待ち（要対応） |
| `●` | done | ターン完了（未読） |
| `✕` | error | APIエラー・ツール失敗 |
| `○` | idle | エージェント起動中・待機 |
| `+N` | — | 稼働中のサブエージェント数 |
| `[N]` | — | 未完了タスク数 |

`done` / `blocked` / `error` は**そのペインにフォーカスすると自動的にクリア**される（既読モデル）。未対応のものだけが光る。

## 必要なもの

- zellij 0.44 以上
- Rust + `rustup target add wasm32-wasip1`（ビルド時のみ）
- `jq`（Claude Code フック用）

## ビルド

```bash
cargo build            # 開発: target/wasm32-wasip1/debug/agent-spaces.wasm
cargo build --release  # 配布: target/wasm32-wasip1/release/agent-spaces.wasm
```

## セットアップ

### 1. サイドバーの常駐（レイアウト）

デフォルトレイアウトのタブテンプレートにサイドバーを埋め込む。例
（`~/.config/zellij/layouts/agent-spaces.kdl`）:

```kdl
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="zellij:tab-bar"
        }
        pane split_direction="vertical" {
            pane size=32 borderless=true {
                plugin location="file:/path/to/agent-spaces.wasm"
            }
            children
        }
        pane size=1 borderless=true {
            plugin location="zellij:status-bar"
        }
    }
}
```

`~/.config/zellij/config.kdl` に:

```kdl
default_layout "agent-spaces"
```

お試しなら常駐させずフローティングでも動く:

```bash
zellij action new-pane --floating --width 40 --height 20 \
  -p "file:/path/to/agent-spaces.wasm"
```

初回ロード時に権限承認プロンプトが出るので、ペインにフォーカスして `y` で承認する
（要求権限: `ReadApplicationState` / `ChangeApplicationState` / `ReadCliPipes`）。

サイドバーの幅は zellij 標準の resize（`Ctrl+n` 等）でそのまま変更できる。

### 2. グローバルキーバインド（ジャンプ機能）

`~/.config/zellij/config.kdl` の keybinds ブロック（`shared_except "locked"` など）に追加:

```kdl
bind "Alt Up" {
    MessagePlugin "file:/path/to/agent-spaces.wasm" { name "agent_spaces_up"; }
}
bind "Alt Down" {
    MessagePlugin "file:/path/to/agent-spaces.wasm" { name "agent_spaces_down"; }
}
bind "Alt Enter" {
    MessagePlugin "file:/path/to/agent-spaces.wasm" { name "agent_spaces_go"; }
}
```

作業ペインにフォーカスを置いたまま、選択の上下移動と選択先へのジャンプができる
（サイドバーにフォーカスを移す必要はない。そもそもサイドバーはフォーカス巡回から
除外されている）。

注意: `MessagePluginId` は使わないこと。サイドバーはタブごとに1インスタンス
起動するため、ID指定ではキーが衝突する。URL指定の `MessagePlugin` なら全
インスタンスに届く。

### 3. Claude Code フック（状態通知）

`extras/claude-hooks/agent-spaces-hook.sh` をフックとして登録する。
`~/.claude/settings.json` の `hooks` に追記:

```jsonc
{
  "hooks": {
    // 以下のイベントすべてに同じエントリを追加する:
    // SessionStart, UserPromptSubmit, Stop, StopFailure, PostToolUseFailure,
    // SessionEnd, SubagentStart, SubagentStop, TaskCreated, TaskCompleted
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "/path/to/extras/claude-hooks/agent-spaces-hook.sh" }
        ]
      }
    ],
    // Notification だけは matcher で種別を絞る
    "Notification": [
      {
        "matcher": "permission_prompt|agent_needs_input|idle_prompt|elicitation_dialog",
        "hooks": [
          { "type": "command", "command": "/path/to/extras/claude-hooks/agent-spaces-hook.sh" }
        ]
      }
    ]
  }
}
```

- フックは**新しく起動した Claude Code セッションから**有効になる
- zellij 外（素のターミナル）で動く Claude Code では何もしない（no-op）
- サイドバーが起動していないときも完全な no-op（副作用なし）

## イベント → 状態のマッピング

| フックイベント | 状態遷移 |
|---|---|
| `SessionStart` | idle（登録・カウンタリセット） |
| `UserPromptSubmit` | working |
| `Notification` | blocked（メッセージ保持） |
| `Stop` | done（サブエージェント数を0に） |
| `StopFailure` / `PostToolUseFailure` | error |
| `SessionEnd` | 登録解除 |
| `SubagentStart` / `SubagentStop` | サブエージェント数 ±1 |
| `TaskCreated` / `TaskCompleted` | 未完了タスク数 ±1 |

## 設定

レイアウトのplugin ブロックで指定:

```kdl
plugin location="file:/path/to/agent-spaces.wasm" {
    show_cwd "true"   // ペイン行に cwd を表示（デフォルト false）
}
```

cwd はフックのペイロード由来なので、フック設定済みのエージェントペインにのみ表示される。

## ワイヤプロトコル（他エージェントの対応）

プラグインは pipe 名 `agent_spaces_status` で以下のJSONを受け取る。
Claude Code 以外のエージェント（codex 等）も、この形式で送れば同じように表示される:

```bash
zellij pipe --name agent_spaces_status -- '{
  "pane_id": '$ZELLIJ_PANE_ID',
  "agent": "codex",
  "event": "UserPromptSubmit",
  "cwd": "/path/to/project"
}'
```

- `pane_id`: zellij が各ペインに与える `$ZELLIJ_PANE_ID`
- `event`: 上記マッピング表のイベント名
- `cwd` / `detail`: 任意

**重要: `--plugin` オプションは付けないこと。** 付けると未起動のプラグインを
zellij が勝手に起動してしまう。付けなければ、起動中のプラグインにのみ配送され、
未起動時は無害な no-op になる。

## 開発

```bash
# 開発用レイアウトで起動（サイドバー + 作業ペイン）
zellij -l zellij.kdl

# コード変更後のリロード（zellij セッション内から）
cargo build && zellij action start-or-reload-plugin \
  "file:$PWD/target/wasm32-wasip1/debug/agent-spaces.wasm"

# 動作テスト（pipe を手で叩く）
zellij pipe --name agent_spaces_status -- \
  '{"pane_id":'$ZELLIJ_PANE_ID',"agent":"claude","event":"UserPromptSubmit"}'
zellij pipe --name agent_spaces_down -- x
```

`eprintln!` のログ出力先（macOS）:

```
$TMPDIR/zellij-<uid>/zellij-log/zellij.log
（実体は /var/folders/**/T/zellij-<uid>/zellij-log/zellij.log）
```

### 実装上の注意（ハマりどころ）

- **CLIパイプは受信したら即 `unblock_cli_pipe_input()`** を呼ぶ。忘れると送信側が
  1秒タイムアウトまでブロックされ、フックのレイテンシに直結する
- payload 付きで送っても直後に `payload=None` の2通目（EOFマーカー）が届く。
  無視すること
- `get_pane_running_command()` / `get_pane_cwd()` は環境によって全ペインで
  タイムアウトする（実測 ~100ms/ペイン）。ホットパスで呼んではいけない
- `set_selectable(false)` は権限承認**後**に呼ぶ。load() で呼ぶと承認プロンプトに
  フォーカスできなくなる
