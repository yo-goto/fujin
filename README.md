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

ビルドターゲットは `.cargo/config.toml` で `wasm32-wasip1` に固定してあるので
`--target` の指定は要らない。テストだけは事情が違う（[開発](#開発)を参照）。

## セットアップ

### 1. wasm の配置とエイリアス定義

wasm はどこに置いてもよいが、OS を問わず `~/.config/zellij` が config
ディレクトリになるので、その配下にまとめておくと手順が環境に依存しない:

```bash
make install   # target/wasm32-wasip1/release/fujin.wasm を ~/.config/zellij/plugins/ へ
```

`~/.config/zellij/config.kdl` にエイリアスを定義する:

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm"
}
```

- **`file:~/…` の `~`（と `$HOME` などの環境変数）は zellij が展開する**ので、
  ホームディレクトリ名を書かなくてよい
- 以降、レイアウトにもキーバインドにも `"fujin"` とだけ書けば済む。
  パスを1箇所にまとめられるほか、**設定の食い違いによる事故を構造的に防げる**
  （[設定](#設定)を参照）
- エイリアス定義の `location` に**相対パスは書けない**。cwd 補完が効かないため、
  絶対パスか `~` 付きか `https://…` にすること

### 2. サイドバーの常駐（レイアウト）

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
zellij action new-pane --floating --width 40 --height 20 -p "fujin"
```

初回ロード時に権限承認プロンプトが出るので、ペインにフォーカスして `y` で承認する
（要求権限: `ReadApplicationState` / `ChangeApplicationState` / `ReadCliPipes` /
`InterceptInput` / `MessageAndLaunchOtherPlugins` / `OpenTerminalsOrPlugins`）。
承認結果は展開後の wasm の絶対パスごとに記録されるので、**wasm を置き直すと
再承認になる**。

サイドバーの幅は zellij 標準の resize（`Ctrl+n` 等）でそのまま変更できる。

### 3. グローバルキーバインド（ジャンプ機能）

`~/.config/zellij/config.kdl` の keybinds ブロック（`shared_except "locked"` など）に
追加する。navモード方式と直接キー方式があり、併用もできる。

#### navモード（推奨）

zellij本体の `Ctrl+p` → pane モードと同じ操作感。`config.kdl` に書くのは**入場キー
1つだけ**で、モード内のキーはプラグインが自前で解釈する:

```kdl
bind "Ctrl y" {
    MessagePlugin "fujin" { name "fujin_mode"; }
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
    MessagePlugin "fujin" { name "fujin_up"; }
}
bind "Alt Down" {
    MessagePlugin "fujin" { name "fujin_down"; }
}
bind "Alt g" {
    MessagePlugin "fujin" { name "fujin_go"; }
}
```

**`Alt Enter` にはバインドしないこと。** Claude Code の Shift+Enter は端末側の設定
（`/terminal-setup` が入れる `Shift+Return -> ESC CR`）に依存していて、zellij はその
`ESC CR` を `Alt Enter` として解釈する。奪うと Shift+Enter の改行がペインに届かなくなる。

いずれの方式でも、作業ペインにフォーカスを置いたまま操作できる（サイドバーに
フォーカスを移す必要はない。そもそもサイドバーはフォーカス巡回から除外されている）。

注意: `MessagePluginId` は使わないこと。サイドバーはタブごとに1インスタンス
起動するため、ID指定ではキーが衝突する。エイリアス（またはURL）指定の
`MessagePlugin` なら全インスタンスに届く。

### 4. Claude Code フック（状態通知）

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

**エイリアス定義（`config.kdl` の `plugins` ブロック）に書くこと。**

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm" {
        show_cwd "true"   // ペイン行に cwd を表示（デフォルト false）
    }
}
```

cwd はフックのペイロード由来なので、フック設定済みのエージェントペインにのみ表示される。

### レイアウト側にだけ設定を書いてはいけない

`MessagePlugin` の宛先照合は**wasm のパスだけでなく設定（configuration）込み**で行われる。
レイアウトの plugin ブロックにだけ `show_cwd "true"` を書き、キーバインド側に書かないと、
両者は別物と見なされて**キーが常駐サイドバーに届かない**。しかも届かないだけで済まず、
zellij は**設定の一致する新しいインスタンスをその場に開いてしまう**（実測: 打鍵1回で
プラグインペインが1枚増える）。fujin のサイドバーは `set_selectable(false)` で
フォーカス巡回から外れているため、**そうしてできたペインはユーザーにも CLI にも閉じられない**。

エイリアスに寄せておけば、レイアウトもキーバインドも同じ定義を参照するので、この
食い違いは起きない。エイリアスを使わない場合は、レイアウトと**すべての**
`MessagePlugin` に同じ configuration を書き写す必要がある。

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

### タスクランナー

ターゲットの切り替えが絡むので、Makefile 越しに叩く。

| コマンド | 内容 |
| --- | --- |
| `make build` / `make release` | wasm のビルド（`cargo build [--release]` と同じ） |
| `make install` | リリースビルドを `~/.config/zellij/plugins/` へコピー（`PLUGIN_DIR` で変更可） |
| `make test` | ユニットテスト（ホストターゲット） |
| `make lint` | clippy。wasm 向けとホスト向け（テストコード込み）の両方、警告はエラー扱い |
| `make fmt` | rustfmt をかける |
| `make fmt-check` | 整形差分があれば失敗する（CI 向け） |
| `make check` | `fmt-check` → `lint` → `test` をまとめて |

コミット前は `make check` を通す。

### テスト

テストは `src/tests.rs`（`src/main.rs` の `#[cfg(test)] mod tests`）に置く。
外部クレートは足さず、標準の `#[test]` のみ。

**テストは wasm ではなくホストターゲットで走る。** `.cargo/config.toml` が既定
ターゲットを `wasm32-wasip1` にしているため、素の `cargo test` はテストバイナリを
wasm 向けにビルドしてしまい実行できない（wasm ランタイムが要る）。`make test` が
`--target <ホストのトリプル>` を補っている。

ホスト向けにリンクするには、wasm ホストが提供する関数 `host_run_plugin_command` の
スタブが要る（`src/tests.rs` の先頭）。この都合で、テストから触れる範囲に制約がある:

- **副作用だけのホストコマンド**（`focus_pane_with_id`, `pipe_message_to_plugin`,
  `intercept_key_presses` 等）は no-op になる。呼ばれても安全
- **戻り値を stdin から読み返す問い合わせ系**（`get_plugin_ids`,
  `get_focused_pane_info`）は**テストから呼べない**。呼ぶと stdin の読み取りに
  失敗して panic する。したがって `is_authoritative()` と、それを経由する
  `fujin_up` / `fujin_down` / `fujin_go` / `fujin_mode` の pipe ハンドラは
  ユニットテストの対象外で、実セッションでの手動確認に頼っている

### リント・フォーマット

設定の置き場所と方針:

- **`Cargo.toml` の `[lints]`** — `unsafe_code` は deny。`clippy::all` に加えて
  `unwrap_used` / `expect_used` も warn にしている。**プラグイン内の panic は
  サイドバーごと落として復旧手段がなくなる**ため、`Option`/`Result` は握り潰さず
  明示的に畳む
- **`clippy.toml`** — テストコード内の `unwrap`/`expect` だけは許可
  （失敗してもテストが落ちるだけなので）
- **`rustfmt.toml`** — 既定値のまま。`edition` と `newline_style` だけ明示している

### 手動での動作確認

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

## ライセンス

MIT License（`LICENSE` を参照）。依存している `zellij-tile` も MIT。
