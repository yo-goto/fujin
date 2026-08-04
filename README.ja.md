<div align="center">
  <img src="assets/logo.svg" alt="fujin logo" width="180" />
</div>

# fujin

zellij用サイドバープラグインです。タブ > ペインを縦並びで一覧し、各ペインで動くAIエージェント（Claude Code等）の状態を可視化して、グローバルキーでジャンプできます。

名前は「布陣」＝陣を敷く、配置すること。複数のペインを部隊に見立てて配置し、俯瞰します。

```text
▸ 1 scheme
    nu
    koka
▾ 2 zeli-c                ← アクティブタブ
    nvim
  » claude +2             ← working、サブエージェント2つ稼働中
▸ 3 review
  ◆ claude                ← 入力待ち（要対応）
```

- **一覧** — セッション内のタブとペインを常時サイドバーに表示します
- **状態の可視化** — 各エージェントが処理中か、入力待ちか、終わっているかがアイコンで分かります
- **ジャンプ** — 作業ペインにフォーカスを置いたまま、キー操作だけで目的のペインへ移動できます（名前・タブ名・cwd のファジー検索つき）

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

`done` / `blocked` / `error` は**そのペインにフォーカスすると自動的にクリア**されます（既読モデル）。未対応のものだけが光ります。

## 必要なもの

- zellij 0.44 以上
- Rust + `rustup target add wasm32-wasip1`（ビルド時のみ）
- `jq`（Claude Code フック用）

## インストール

### 1. ビルドして配置する

wasm はどこに置いてもかまいませんが、OS を問わず `~/.config/zellij` が config
ディレクトリになるので、その配下にまとめておくと手順が環境に依存しません:

```bash
make install   # release ビルドして ~/.config/zellij/plugins/fujin.wasm へ配置
```

置き場所を変えたい場合は `make install PLUGIN_DIR=/path/to/plugins` としてください。

### 2. エイリアスを定義する

`~/.config/zellij/config.kdl` に以下を書きます:

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm"
}
```

- **`file:~/…` の `~`（と `$HOME` などの環境変数）は zellij が展開する**ので、
  ホームディレクトリ名を書く必要はありません
- 以降、レイアウトにもキーバインドにも `"fujin"` とだけ書けば済みます。
  パスを1箇所にまとめられるほか、**設定の食い違いによる事故を構造的に防げます**
  （[設定](#設定)を参照）
- エイリアス定義の `location` に**相対パスは書けません**。cwd 補完が効かないため、
  絶対パスか `~` 付きか `https://…` にしてください

## セットアップ

### 1. サイドバーの常駐（レイアウト）

デフォルトレイアウトのタブテンプレートにサイドバーを埋め込みます。例
（`~/.config/zellij/layouts/fujin.kdl`）:

```kdl
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="zellij:tab-bar"
        }
        pane split_direction="vertical" {
            pane size=32 borderless=true {
                plugin location="fujin"
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

**`children` ではなく `pane` と書いてください。** `children` は「このレイアウトが定義する
タブのペインをここに差し込む」というマーカーなので、`tab` ノードを持たない
テンプレートだけのレイアウトでは**何も入らず、ターミナルが0個のタブができます**。
セッション作成時にこれが起きると zellij がそのまま終了します。

**`new_tab_template` にも同じ内容を書いてください。** `default_tab_template` は
「新規タブ用テンプレート」にフォールバックする実装ですが、**セッションマネージャ
（`Ctrl+o` → `w`）でレイアウトを選んで作ったセッションではフォールバックが効かず**、
zellij 組み込みのデフォルトが残ります。両方書けば起動経路に依存しません。

起動経路を問わず確実にしたい場合は、`NewTab` のキーバインド側でレイアウトを指定する
手もあります:

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

お試しなら、常駐させずフローティングでも動きます:

```bash
zellij action new-pane --floating --width 40 --height 20 -p "fujin"
```

初回ロード時に権限承認プロンプトが出るので、ペインにフォーカスして `y` で承認してください
（要求権限: `ReadApplicationState` / `ChangeApplicationState` / `ReadCliPipes` /
`InterceptInput` / `MessageAndLaunchOtherPlugins` / `OpenTerminalsOrPlugins`）。
承認結果は展開後の wasm の絶対パスごとに記録されるので、**wasm を置き直すと
再承認になります**。

サイドバーの幅は zellij 標準の resize（`Ctrl+n` 等）でそのまま変更できます。

### 2. グローバルキーバインド（ジャンプ機能）

`~/.config/zellij/config.kdl` の keybinds ブロック（`shared_except "locked"` など）に
追加します。navモード方式と直接キー方式があり、併用もできます。

#### navモード（推奨）

zellij本体の `Ctrl+p` → pane モードと同じ操作感です。`config.kdl` に書くのは**入場キー
1つだけ**で、モード内のキーはプラグインが自前で解釈します:

```kdl
bind "Ctrl y" {
    MessagePlugin "fujin" { name "fujin_mode"; }
}
```

`Ctrl+y` は zellij デフォルトと衝突しない空きキーです。埋まっていれば任意に変更してかまいません
（既定で使用済みなのは `p`/`t`/`n`/`h`/`s`/`o`/`q`/`g`）。
モードのキーマップはプラグイン側にあるので、**ビルトインモードを1つ潰す必要がなく**、
キーを増やしても `config.kdl` を触らずに済みます。操作方法は[使い方](#使い方)を参照してください。

#### 直接キー（モードなし）

1打鍵で動かしたい場合はこちらです:

```kdl
bind "Alt Up" {
    MessagePlugin "fujin" { name "fujin_up"; }
}
bind "Alt Down" {
    MessagePlugin "fujin" { name "fujin_down"; }
}
bind "Alt g" {
    MessagePlugin "fujin" { name "fujin_go"; }
}
```

**`Alt Enter` にはバインドしないでください。** Claude Code の Shift+Enter は端末側の設定
（`/terminal-setup` が入れる `Shift+Return -> ESC CR`）に依存していて、zellij はその
`ESC CR` を `Alt Enter` として解釈します。奪うと Shift+Enter の改行がペインに届かなくなります。

注意: `MessagePluginId` は使わないでください。サイドバーはタブごとに1インスタンス
起動するため、ID指定ではキーが衝突します。エイリアス（またはURL）指定の
`MessagePlugin` なら全インスタンスに届きます。

### 3. Claude Code フック（状態通知）

`extras/claude-hooks/fujin-hook.sh` をフックとして登録します。
`~/.claude/settings.json` の `hooks` に追記してください:

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

- フックは**新しく起動した Claude Code セッションから**有効になります
- zellij 外（素のターミナル）で動く Claude Code では何もしません（no-op）
- サイドバーが起動していないときも完全な no-op です（副作用なし）

## 使い方

作業ペインにフォーカスを置いたまま操作できます（サイドバーにフォーカスを移す必要は
ありません。そもそもサイドバーはフォーカス巡回から除外されています）。

### navモード

`Ctrl+y`（上で設定したキー）を押すとサイドバーのヘッダが `-- NAV --` に変わり、
以下のキーが効きます:

| キー | 動作 |
|---|---|
| `j` / `↓` / `Tab` | 次のペイン |
| `k` / `↑` | 前のペイン |
| `g` / `G` | 先頭 / 末尾 |
| `1`〜`9` | n番目へ直行してモードを抜ける |
| `Enter` / `l` / `Space` | 選択中のペインへジャンプしてモードを抜ける |
| `/` | 検索サブモードに入る（下記） |
| `Esc` / `q` | モードを抜ける |

上記以外のキーでもモードを抜けます（キー入力が取り残されないための安全弁です）。

ハイライトされている行は常にフォーカス中のペインに追従するので、navモードは
作業中のペインから始まります。例外は `Esc` で抜けたあとフォーカスを動かさずに
入り直した場合で、このときだけ前回見ていた位置から再開します。

### 検索（navモード内の `/`）

navモード中に `/` を押すとヘッダがクエリ入力行（`/…▏`）に変わり、ペイン名・
所属タブ名・cwd に対するファジー検索でツリーを絞り込めます。一致した文字は
ハイライトされ、その場所がどのフィールドに当たったかの提示を兼ねます
（cwd ヒット時は `show_cwd` が無効でもその行に cwd が出ます）。

| キー | 動作 |
|---|---|
| 印字可能文字 | クエリ末尾に追加（navモードの1文字ショートカットは全て無効になる） |
| `Backspace` | クエリ末尾を1文字削除 |
| `↓` / `Tab`、`↑` / `Shift+Tab` | 絞り込み結果内でカーソル移動 |
| `Enter` | 選択行へジャンプしてnavモードごと抜ける（0件時は何もしない） |
| `Esc` | クエリを破棄してnavモードへ戻る（もう一度 `Esc` でモードを抜ける） |

クエリは検索を抜けるたびに破棄され、次回は常に空から始まります。cwd はフック
から通知を受けたペインだけが持つので、一般のシェルペインは cwd では一致しません。

### イベント → 状態のマッピング

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

**エイリアス定義（`config.kdl` の `plugins` ブロック）に書いてください。**

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm" {
        show_cwd "true"   // ペイン行に cwd を表示（デフォルト false）
    }
}
```

cwd はフックのペイロード由来なので、フック設定済みのエージェントペインにのみ表示されます。

### レイアウト側にだけ設定を書いてはいけません

`MessagePlugin` の宛先照合は**wasm のパスだけでなく設定（configuration）込み**で行われます。
レイアウトの plugin ブロックにだけ `show_cwd "true"` を書き、キーバインド側に書かないと、
両者は別物と見なされて**キーが常駐サイドバーに届きません**。しかも届かないだけで済まず、
zellij は**設定の一致する新しいインスタンスをその場に開いてしまいます**（実測: 打鍵1回で
プラグインペインが1枚増えます）。fujin のサイドバーは `set_selectable(false)` で
フォーカス巡回から外れているため、**そうしてできたペインはユーザーにも CLI にも閉じられません**。

エイリアスに寄せておけば、レイアウトもキーバインドも同じ定義を参照するので、この
食い違いは起きません。エイリアスを使わない場合は、レイアウトと**すべての**
`MessagePlugin` に同じ configuration を書き写す必要があります。

## ワイヤプロトコル（他エージェントの対応）

プラグインは pipe 名 `fujin_status` で以下のJSONを受け取ります。
Claude Code 以外のエージェント（codex 等）も、この形式で送れば同じように表示されます:

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
`_down` / `_go` / `_mode` だけです。`fujin_sync_state` / `_read` / `_selection` は
インスタンス間の同期用の内部プロトコルなので、外から叩かないでください。

**重要: `--plugin` オプションは付けないでください。** 付けると未起動のプラグインを
zellij が勝手に起動してしまいます。付けなければ、起動中のプラグインにのみ配送され、
未起動時は無害な no-op になります。

## 開発

開発タスクは Makefile にまとめてあります。コミット前に `make check`
（`fmt-check` → `lint` → `test`）を通してください。

ビルド・テスト・手動確認の詳しい手順と、実装上のハマりどころは
別途 `docs/dev/` にまとめています。

## ライセンス

MIT License（`LICENSE` を参照）。依存している `zellij-tile` も MIT です。
