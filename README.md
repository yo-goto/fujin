# fujin（布陣）

zellij用サイドバープラグイン。タブ > ペインを縦並びで一覧し、各ペインで動くAIエージェント（Claude Code等）の状態を可視化して、グローバルキーでジャンプする。

名前は「布陣」＝陣を敷く、配置すること。複数のペインを部隊に見立てて配置し、俯瞰する。

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
cargo build            # 開発: target/wasm32-wasip1/debug/fujin.wasm
cargo build --release  # 配布: target/wasm32-wasip1/release/fujin.wasm
```

## セットアップ

### 1. サイドバーの常駐（レイアウト）

デフォルトレイアウトのタブテンプレートにサイドバーを埋め込む。例
（`~/.config/zellij/layouts/fujin.kdl`）:

```kdl
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="zellij:tab-bar"
        }
        pane split_direction="vertical" {
            pane size=32 borderless=true {
                plugin location="file:/path/to/fujin.wasm"
            }
            pane
        }
        pane size=1 borderless=true {
            plugin location="zellij:status-bar"
        }
    }
    // 同じ内容を new_tab_template にも書く（後述）
}
```

**`children` ではなく `pane` と書くこと。** `children` は「このレイアウトが定義する
タブのペインをここに差し込む」というマーカーなので、`tab` ノードを持たない
テンプレートだけのレイアウトでは**何も入らず、ターミナルが0個のタブができる**。
セッション作成時にこれが起きると zellij がそのまま終了する。

**`new_tab_template` にも同じ内容を書くこと。** `default_tab_template` は
「新規タブ用テンプレート」にフォールバックする実装だが、**セッションマネージャ
（`Ctrl+o` → `w`）でレイアウトを選んで作ったセッションではフォールバックが効かず**、
zellij 組み込みのデフォルトが残る。両方書けば起動経路に依存しない。

起動経路を問わず確実にしたいなら、`NewTab` のキーバインド側でレイアウトを指定する
手もある:

```kdl
bind "n" {
    NewTab { layout "fujin"; }
    SwitchToMode "normal"
}
```

`~/.config/zellij/config.kdl` に:

```kdl
default_layout "fujin"
```

お試しなら常駐させずフローティングでも動く:

```bash
zellij action new-pane --floating --width 40 --height 20 \
  -p "file:/path/to/fujin.wasm"
```

初回ロード時に権限承認プロンプトが出るので、ペインにフォーカスして `y` で承認する
（要求権限: `ReadApplicationState` / `ChangeApplicationState` / `ReadCliPipes` /
`InterceptInput` / `MessageAndLaunchOtherPlugins`）。

サイドバーの幅は zellij 標準の resize（`Ctrl+n` 等）でそのまま変更できる。

### 2. グローバルキーバインド（ジャンプ機能）

`~/.config/zellij/config.kdl` の keybinds ブロック（`shared_except "locked"` など）に
追加する。navモード方式と直接キー方式があり、併用もできる。

#### navモード（推奨）

zellij本体の `Ctrl+p` → pane モードと同じ操作感。`config.kdl` に書くのは**入場キー
1つだけ**で、モード内のキーはプラグインが自前で解釈する:

```kdl
bind "Ctrl y" {
    MessagePlugin "file:/path/to/fujin.wasm" { name "fujin_mode"; }
}
```

`Ctrl+y` を押すとサイドバーのヘッダが `-- NAV --` に変わり、以下のキーが効く:

| キー | 動作 |
|---|---|
| `j` / `↓` / `Tab` | 次のペイン |
| `k` / `↑` | 前のペイン |
| `g` / `G` | 先頭 / 末尾 |
| `1`〜`9` | n番目へ直行してモードを抜ける |
| `Enter` / `l` / `Space` | 選択中のペインへジャンプしてモードを抜ける |
| `Esc` / `q` | モードを抜ける |

上記以外のキーでもモードを抜ける（キー入力が取り残されないための安全弁）。
`Ctrl+y` は zellij デフォルトと衝突しない空きキー。埋まっていれば任意に変更してよい
（既定で使用済みなのは `p`/`t`/`n`/`h`/`s`/`o`/`q`/`g`）。

モードのキーマップはプラグイン側にあるので、**ビルトインモードを1つ潰す必要がなく**、
キーを増やしても `config.kdl` を触らずに済む。

#### 直接キー（モードなし）

1打鍵で動かしたい場合:

```kdl
bind "Alt Up" {
    MessagePlugin "file:/path/to/fujin.wasm" { name "fujin_up"; }
}
bind "Alt Down" {
    MessagePlugin "file:/path/to/fujin.wasm" { name "fujin_down"; }
}
bind "Alt Enter" {
    MessagePlugin "file:/path/to/fujin.wasm" { name "fujin_go"; }
}
```

いずれの方式でも、作業ペインにフォーカスを置いたまま操作できる（サイドバーに
フォーカスを移す必要はない。そもそもサイドバーはフォーカス巡回から除外されている）。

注意: `MessagePluginId` は使わないこと。サイドバーはタブごとに1インスタンス
起動するため、ID指定ではキーが衝突する。URL指定の `MessagePlugin` なら全
インスタンスに届く。

### 3. Claude Code フック（状態通知）

`extras/claude-hooks/fujin-hook.sh` をフックとして登録する。
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
          { "type": "command", "command": "/path/to/extras/claude-hooks/fujin-hook.sh" }
        ]
      }
    ],
    // Notification だけは matcher で種別を絞る
    "Notification": [
      {
        "matcher": "permission_prompt|agent_needs_input|idle_prompt|elicitation_dialog",
        "hooks": [
          { "type": "command", "command": "/path/to/extras/claude-hooks/fujin-hook.sh" }
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
plugin location="file:/path/to/fujin.wasm" {
    show_cwd "true"   // ペイン行に cwd を表示（デフォルト false）
}
```

cwd はフックのペイロード由来なので、フック設定済みのエージェントペインにのみ表示される。

## ワイヤプロトコル（他エージェントの対応）

プラグインは pipe 名 `fujin_status` で以下のJSONを受け取る。
Claude Code 以外のエージェント（codex 等）も、この形式で送れば同じように表示される:

```bash
zellij pipe --name fujin_status -- '{
  "pane_id": '$ZELLIJ_PANE_ID',
  "agent": "codex",
  "event": "UserPromptSubmit",
  "cwd": "/path/to/project"
}'
```

- `pane_id`: zellij が各ペインに与える `$ZELLIJ_PANE_ID`
- `event`: 上記マッピング表のイベント名
- `cwd` / `detail`: 任意

外部から使うのは `fujin_status` と、キーバインド用の `fujin_up` /
`_down` / `_go` / `_mode` だけ。`fujin_sync_state` / `_read` / `_selection` は
インスタンス間の同期用の内部プロトコルなので、外から叩かないこと。

**重要: `--plugin` オプションは付けないこと。** 付けると未起動のプラグインを
zellij が勝手に起動してしまう。付けなければ、起動中のプラグインにのみ配送され、
未起動時は無害な no-op になる。

## 開発

```bash
# 開発用レイアウトで起動（サイドバー + 作業ペイン）
zellij -l zellij.kdl

# コード変更後のリロード（zellij セッション内から）
cargo build && zellij action start-or-reload-plugin \
  "file:$PWD/target/wasm32-wasip1/debug/fujin.wasm"

# 動作テスト（pipe を手で叩く）
zellij pipe --name fujin_status -- \
  '{"pane_id":'$ZELLIJ_PANE_ID',"agent":"claude","event":"UserPromptSubmit"}'
zellij pipe --name fujin_down -- x

# navモードに入る（キーバインドなしで試す）
zellij pipe --name fujin_mode -- x
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
- フックスクリプトの jq で **`key: (.foo // empty)` と書いてはいけない**。jq は
  オブジェクト構築中の値が `empty` になると、そのキーだけでなく**オブジェクト全体を
  消す**。`.message` を持たないイベント（`Notification` 以外すべて）で payload が
  空になり、状態が一切届かなくなる。`null` を入れて `with_entries(select(.value != null))`
  で落とすこと
- `set_selectable(false)` は権限承認**後**に呼ぶ。load() で呼ぶと承認プロンプトに
  フォーカスできなくなる
- `selectable` は**ペイン側の属性でリロードしても引き継がれる**。要求権限を増やした
  新版をリロードすると、プラグインは未承認状態なのにペインは unselectable のままで、
  「承認プロンプトが出ているのにフォーカスできない」デッドロックになる。`load()` の
  先頭で `set_selectable(true)` に戻すこと
- **`PaneUpdate` はバックグラウンドのタブのインスタンスには届かない**（実測: サイドバー
  3つのセッションで既読クリアを実行したのは可視インスタンス1つだけ）。pipe は全
  インスタンスに届くので油断しやすいが、イベント駆動の状態は見逃した変化のぶんだけ
  永久にずれる。可視インスタンスが観測した変化（既読クリア・選択位置）は明示的に
  他へ配ること。**選択位置はインデックスではなくペインIDで送る** — インデックスは
  各インスタンスのペイン一覧に依存し、一覧が古い相手では別の行を指す
- 「どのインスタンスが操作を担当するか」は `get_focused_pane_info()` で毎回サーバに
  聞く。`TabInfo.active` との突き合わせは、`TabUpdate` が届かないバックグラウンドの
  インスタンスも真を返す（実測で複数同時に真）。`Event::Visible` は正確だが
  **リロード時に再送されない**ので、それだけに頼ると reload 後に全機能が死ぬ
- `focus_pane_with_id` の `should_float_if_hidden` は**ターゲットが
  フローティングかどうかで切り替える**。`false` 固定だとフローティングペインへ
  ジャンプできず、`true` 固定だとフローティング表示中にタイルペインへ戻れない
- `MessageToPlugin::with_plugin_url` は**起動中のインスタンスに配送されない**。
  cwd/config まで一致を要求するため、レイアウト由来のインスタンスにマッチせず
  「未起動 → 新規起動」に倒れ、タブを作るたびに迷子のペインが増える。
  プラグイン間の通信は `with_destination_plugin_id`（ID指定）を使うこと。
  宛先IDは `PaneManifest` の `PaneInfo.plugin_url` で同じURLのペインを探して得る
- 横取り中は全キーがプラグイン行きになるため、**抜けられなくなるとセッションが
  操作不能**になる。未定義キーもすべて離脱扱いにし、`BeforeClose` でも
  `clear_key_presses_intercepts()` を呼ぶこと
- `start-or-reload-plugin` は、**そのURLのインスタンスがセッションに無いと新しい
  ペインを開く**。レイアウトが読んでいるURL（例: `~/.config/zellij/plugins/` 配下）
  と別のパスを指定すると迷子のペインが増える。しかもサイドバーは
  `set_selectable(false)` なのでフォーカスできず、`Ctrl+p`→`x` でも
  `zellij action focus-pane-id` でも閉じられない（`close_self()` を一時的に
  仕込んでリロードするしかない）。**リロードは常にレイアウトと同じURLで**
