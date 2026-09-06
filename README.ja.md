<div align="center">
  <img src="assets/logo.svg" alt="fujin logo" width="180" />
</div>

# fujin

zellij用サイドバープラグインです。タブ > ペインを縦並びで一覧し、各ペインで動くAIエージェント（Claude Code等）の状態を可視化して、グローバルキーでジャンプできます。

- **一覧** — セッション内のタブとペインを常時サイドバーに表示します
- **状態の可視化** — 各エージェントが処理中か、入力待ちか、終わっているかがアイコンで分かります
- **ジャンプ** — 作業ペインにフォーカスを置いたまま、キー操作または行のクリックで目的のペインへ移動できます（名前・タブ名・cwd のファジー検索つき）

> [!NOTE]
> 名前は「布陣」＝陣を敷き、配置すること。複数のペインを部隊に見立てて俯瞰します。
> 英語話者には「風神」とも読めるので、構える「布陣」（静）と駆け巡る「風神」（動）の
> 両方をこの名前に重ねています。

## 設計方針

> Local-first. Free. Zellij only. Just simple.

ネットワーク・クラウド接続には依存せず、設定はローカルのファイルだけで完結します。
恒久的に無料で、収益化のためにクラウド機能を後付けする予定もありません。汎用の
マルチプレクサ代替ではなく zellij に特化し、複雑さを増やす機能は入れません。

## インストール

zellij 0.44 以上と `curl`（Claude Code のフックを使うなら `jq` も）が要ります。
**Rust は要りません。**

```bash
curl -fsSLO https://github.com/yo-goto/fujin/releases/latest/download/setup.sh
bash setup.sh --download
```

`setup.sh` は wasm とフック本体を `~/.config/zellij/plugins/` へ置き、
`~/.config/zellij/layouts/fujin.kdl` を生成し、フックを `~/.claude/settings.json` の
10イベントへ登録します。**更新も同じコマンドで済みます**（冪等で、`settings.json` は
バックアップを取り、レイアウトの上書きだけ確認を挟みます）。

`config.kdl` だけは書き換えず、貼るべき内容を表示します（既存ブロックへのマージは、
壊したときの復旧が重いためです）:

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm"
}

// keybinds ブロックが既にあるなら、中の bind だけを足してください
keybinds {
    shared_except "locked" {
        bind "Ctrl y" {
            MessagePlugin "fujin" {
                name "fujin_mode"
                floating true
            }
        }
    }
}

default_layout "fujin"
```

zellij を起動し直すと左端にサイドバーが出ます。初回は権限承認プロンプトが出るので、
そのペインにフォーカスして `y` を押してください。承認は **wasm の絶対パスごと**に
記録されるので、置き場所を変えると再承認になります。

### サイドバーの幅

既定の `pane size=32`（桁数指定）は zellij 標準の resize が効きません。**割合指定**に
書き直すと伸縮できるようになります。

```sh
make setup SETUP_ARGS="--width 20% --layout-only"
```

`Ctrl+n` で resize モードに入り `h`（縮む）/ `H`（広がる）、`Esc` で抜けるのが確実です
（`Alt+-` も使えますが、`Alt+=` / `Alt++` は `Shift` が要る文字なので端末によっては
届きません）。幅を変えると**他のタブのサイドバーも追従します**（割合指定のときだけ）——
ただしマウスドラッグで zellij の刻み（端末幅の5%）に乗らない幅にした場合は、最も近い
到達できる幅までです。

代償として幅が端末幅に比例し、狭い端末ではサイドバーも狭くなります（画面の文言は幅32を
前提に組んであります）。書き換えたレイアウトが効くのは新しいセッションからです。

<details>
<summary><code>setup.sh</code> のオプション</summary>

| オプション | 内容 |
|---|---|
| `--download` | wasm とフック本体をリリースから取得する |
| `--version TAG` | 取得するリリース（既定 `latest`） |
| `--dry-run` | 何も書かずに、書き込む内容を表示する |
| `--no-hooks` | Claude Code フックの登録をスキップする |
| `--layout-only` / `--hooks-only` / `--config-only` | その処理だけ実行する |
| `--width N` | サイドバーの幅（既定 32）。`20%` のように割合で書くと `--resizable` を指定したのと同じ扱いになる |
| `--resizable` | `--width`（桁数）を実行時の端末幅から割合へ換算する。zellij のペインの中で実行すると端末全体ではなくそのペインの幅を測るので、割合を直接指定するほうが確実 |
| `--force` | 既存のレイアウトファイルを確認なしで上書きする |

</details>

<details>
<summary>ソースからビルドする</summary>

Rust と `rustup target add wasm32-wasip1` が要ります。

```bash
git clone https://github.com/yo-goto/fujin.git && cd fujin
make setup   # release ビルド → 配置 → レイアウト生成 → Claude Code フック登録
```

`make setup` は `make install`（release ビルドと `~/.config/zellij/plugins/` への配置）まで
含みます。wasm の入れ替えだけなら `make install`、置き場所はどちらも
`PLUGIN_DIR=/path/to/plugins` で変えられます。`setup.sh` への引数は
`make setup SETUP_ARGS="--no-hooks"` のように渡します。検証は `make check` です。

</details>

## キーバインド

navモード方式と直接キー方式があり、併用もできます。どちらも `config.kdl` の keybinds
ブロック（`shared_except "locked"` など）に書きます。

### navモード（推奨）

zellij本体の `Ctrl+p` → pane モードと同じ操作感です。書くのは**入場キー1つだけ**で
（上のインストール手順のスニペットがそれです）、モード内のキーはプラグインが自前で
解釈するので、ビルトインモードを潰す必要も、キーが増えるたびに `config.kdl` を触る
必要もありません。`Ctrl+y` が埋まっていれば変更してかまいません（zellij の既定で
使用済みなのは `p`/`t`/`n`/`h`/`s`/`o`/`q`/`g`）。

> [!IMPORTANT]
> `floating true` は必ず書いてください。セッションに fujin が1つも居ないとき、この pipe には
> 宛先が無いので zellij 自身が fujin を新規起動します。既定ではタイルで開くため作業ペインを
> 分割して右側に出てしまい、プラグイン側からは直せません。

### 直接キー（モードなし）

1打鍵で動かしたい場合はこちらです:

```kdl
bind "Alt u" {
    MessagePlugin "fujin" { name "fujin_up"; }
}
bind "Alt d" {
    MessagePlugin "fujin" { name "fujin_down"; }
}
bind "Alt g" {
    MessagePlugin "fujin" { name "fujin_go"; }
}
bind "Alt c" {
    MessagePlugin "fujin" { name "fujin_toggle_cwd"; }
}
```

`fujin_toggle_cwd` だけは移動ではなく表示の切り替えで、cwd行（`show_cwd`）を実行中に
反転します。全タブが同時に切り替わり、あとから作ったタブにも現在の状態が引き継がれます
（`show_cwd` の設定は起動時の初期値という意味になります）。

> [!CAUTION]
> `Alt Enter` にはバインドしないでください。Claude Code の Shift+Enter は端末側の設定
> （`/terminal-setup` が入れる `Shift+Return -> ESC CR`）に依存していて、zellij はその
> `ESC CR` を `Alt Enter` として解釈します。奪うと Shift+Enter の改行がペインに届きません。

`MessagePluginId` は使わないでください。サイドバーはタブごとに1インスタンス起動するため、
ID指定ではキーが衝突します。エイリアス指定の `MessagePlugin` なら全インスタンスに届きます。

## 使い方

作業ペインにフォーカスを置いたまま操作できます。サイドバーにフォーカスを移す必要は
ありません（そもそもフォーカス巡回から除外されています）。

### 画面の見方

ペイン行の先頭には状態を表すアイコンが1文字出て、色も状態ごとに違います。**凡例は
プラグインの中にあります**——navモードで `?` を押すとキー一覧の下に出るので、ここへ
読みに戻る必要はありません。エージェントが乗っていないペインは同じ位置に `›` が出ます
（状態ではないので無色）。

- ペイン名の後ろのカウンタは、`+N` が稼働中のサブエージェント数、`[N]` が未完了タスク数
- フローティングペインは名前が `(name)` と丸括弧で囲まれます
- ペイン名が空なら、コマンドペインはコマンド文字列、それ以外は cwd を代わりに出します
- 一覧が画面に収まらないときは選択行が見えるところまでスクロールし、隠れた行は
  `▴ … N more` / `▾ … N more` が示します

**コマンドペイン**（`zellij run -- docker build .` やレイアウトの `command` ブロックで
開いたペイン）にも、フックの設定なしで同じアイコンが出ます。走行中が `working`、
終了コード 0 が `done`、それ以外（非0・シグナル・中断）が `error` です。`blocked` /
`idle` は持たず、同じペインにエージェントが登録されていればそちらが優先されます。

`done` / `blocked` / `error` は**そのペインにフォーカスすると自動的にクリア**されます
（既読モデル）。通り過ぎただけでは消えず、しばらく留まって初めて既読になります。

### クリックでジャンプ

ペイン行を左クリックすると、そのペインへジャンプします。モードに入る必要はありません。
別のタブのペインならタブごと移動し、フローティングペインならフローティング層が出てきます。
タブ見出しや一覧より下の余白をクリックしても何も起きません。反応しない場合は、zellij
本体の `mouse_mode` が無効になっていないか確認してください（既定は有効）。

### navモード

`Ctrl+y` を押すとヘッダが `▲ fujin  [nav]` に変わり、フッターに `?:help  esc:exit` が
出ます。以下のサブモードはすべてこのモードの内側です。

| キー | 動作 |
|---|---|
| `j` / `↓` / `Tab` | 次のペイン |
| `k` / `↑` | 前のペイン |
| `g` / `G` | 先頭 / 末尾 |
| `Enter` / `l` / `Space` | 選択中のペインへジャンプしてモードを抜ける |
| `/` | 検索 |
| `n` | 番号ジャンプ |
| `t` | トリアージ |
| `d` | ペインの終了 |
| `m` / `M` | マークを付け外し / すべて外す |
| `p` | プレビューの表示を切り替える |
| `r` | プレビュー中のペインを既読にする（プレビュー表示中のみ） |
| `?` | キーのヘルプを開く |
| `Esc` / `q` | モードを抜ける |

上記以外のキーでもモードを抜けます（キー入力が取り残されないための安全弁です）。

ハイライト行は常にフォーカス中のペインに追従するので、navモードは作業中のペインから
始まります（`Esc` で抜けたあとフォーカスを動かさずに入り直したときだけ、前回見ていた
位置から再開します）。モード中はサイドバーがフォーカスを借りるため、作業していたペインは
**枠は残ったまま強調だけが外れます**。抜けるとフォーカスは元のペイン（ジャンプしたなら
ジャンプ先）へ戻ります——モード中に自分でフォーカスを動かした場合は、奪い返さずに抜けます。

### 検索（`/`）

フッターがクエリ入力行（`/…`）に変わり、ペイン名・所属タブ名・cwd に対するファジー検索で
ツリーを絞り込みます。一致した文字がハイライトされ、その場所がどのフィールドに当たったかの
提示を兼ねます（cwd ヒット時は `show_cwd` が無効でもその行に cwd が出ます）。一致が0件なら
`no matches` と出ます。

vim のように**編集状態**と**操作状態**に分かれます。`/` で入った直後は編集状態で、`Esc` を
押すとクエリを持ったまま操作状態へ移ります。見分けはクエリの文字色（編集＝通常、操作＝dim）と
フッターのヒント（`esc:browse  enter:jump` / `j/k:move  ?:help  i:edit …`）です。

| キー | 編集状態 | 操作状態 |
|---|---|---|
| `Enter` | 選択行へジャンプしてnavモードごと抜ける（0件時は何もしない） | ← 同じ |
| `Esc` | 操作状態へ移る（クエリは残ります） | クエリを破棄してnavモードへ戻る |
| `↓` / `Tab`、`↑` / `Shift+Tab` | 絞り込み結果内でカーソル移動 | ← 同じ |
| `j` / `k` | クエリ末尾に追加 | 絞り込み結果内でカーソル移動 |
| `i` | クエリ末尾に追加 | 編集状態へ戻る（クエリはそのまま） |
| `?` | クエリ末尾に追加 | キーのヘルプを開く |
| `Backspace` | クエリ末尾を1文字削除 | 何も起きない |
| その他の印字可能文字 | クエリ末尾に追加 | 何も起きない |
| `alt+m` / `alt+p` | マークを付け外し / プレビューを切り替え | ← 同じ |

編集状態で上記のどれにも当たらないキー（`←` / `→` / ファンクションキーなど）を押すと
navモードごと抜けます（操作状態では何も起きません）。`alt+m` / `alt+p` 以外の修飾キー付きは、
どちらの状態でもnavモードごと抜けます。

クエリは検索を抜けるたびに破棄され、次回は常に空から始まります。cwd はフックから通知を
受けたペインだけが持つので、一般のシェルペインは cwd では一致しません。

### 番号ジャンプ（`n`）

選択対象のペインに全タブ貫通の通し番号が振られ、行の左に番号列が出ます。数字を打つたびに
前方一致で候補が絞られ、1件になった時点で即ジャンプします（`Enter` は要りません）。番号は
総数の桁数にゼロ埋めされるので（12ペインなら `01`〜`12`）、「`1` を打ったが `10` があって
確定できない」が起きません。存在しない番号を打つとバッファは空に戻ります。

| キー | 動作 |
|---|---|
| `0`〜`9` | 候補を絞り込み、1件になった時点でジャンプ |
| `Backspace` | 入力済みの数字を1つ削除 |
| `?` | キーのヘルプを開く |
| `Esc` | サブモードを抜けてツリー表示へ戻る（navモードは継続） |

フッターは `n` に続けて入力中の数字を表示し、まだ候補に残っている番号だけが点ります。
番号列はサブモード中だけ出るので、平常時の行の幅配分は変わりません。

### トリアージ（`t`）

ヘッダが `▲ fujin  [tri]` に変わり、一覧がタブの壁を無視したフラット表示になります。状態を
持つペインだけが緊急度順（`error` > `blocked` > `working` > `done`。同じ階層内は直近に
状態が変わった順）に並び、行の右端に所属タブ名が付きます。状態を一度も通知していない
ペインと既読済み（`idle`）は出ません。コマンドペインの状態も同じ階層に混ざります。
一覧が空なら `nothing to triage` と出ます。

| キー | 動作 |
|---|---|
| `j` / `↓` / `Tab`、`k` / `↑` / `Shift+Tab` | カーソル移動 |
| `g` / `G` | 先頭 / 末尾 |
| `Enter` | 選択中のペインへジャンプしてモードを抜ける（一覧が空なら何もしない） |
| `m` / `M` | マークを付け外し / すべて外す |
| `p` / `r` | プレビューの表示を切り替える / プレビュー中のペインを既読にする |
| `?` | キーのヘルプを開く |
| `Esc` | ツリー表示へ戻る（navモードは継続） |

`Esc` で戻ると、トリアージに入る前に選んでいた行へ選択が戻ります（フッターのヒントも
`esc:exit` ではなく `esc:back` です）。

### マーク（`m` / `M`）

`m` でいま光っている行のペインにマークが付き、もう一度押すと外れ、`M` ですべて外れます。
マークされた行には状態アイコンの左に `✓` が出ます（1つでもマークがあるフレームだけ列が
出るので、平常時の行の幅配分は変わりません）。

- ツリー表示・トリアージ一覧では `m` / `M`、検索の絞り込み結果では `alt+m` です
- **タブをまたいでマークでき、navモードを抜けても保持されます**
- 対象のペインが閉じられると、そのマークだけ自動的に外れます
- 終了操作を実行するとすべて外れます（`Esc` で取り消した場合は残ります）

いまのところマークの使い道は、次の終了操作の一括指定です。

### ペインの終了（`d`）

フッターが確認プロンプト（`c:close k:kill x:kill+close`）に変わり、ヘッダの `▲` ごと
テーマのエラー色になります。3つのうちどれかを選ぶまで何も起きず、`Esc` で取り消せます。

| キー | 動作 |
|---|---|
| `c` | プロセスには触れず、ペインを閉じる |
| `k` | ペインのプロセスに `SIGKILL` を送る |
| `x` | kill してから、ペインも閉じる |
| `?` | キーのヘルプを開く |
| `Esc` | 取り消してツリー表示へ戻る（navモードは継続） |

対象は[マーク](#マークm--m)したペインがあればその全部、無ければ選択行の1ペインです。
マークがあるときは `3 panes  c:close k:kill x:kill+close` のように件数が前置され、実行すると
マークはすべて外れます。対象が1つも無ければプロンプトは開きません。

エージェント・コマンドペイン・無関係なシェルのいずれでも3操作とも選べます。シェルを kill
すると子プロセスも巻き添えで終了してペインは zellij が自動的に閉じますが、コマンドペイン
（`zellij run -- …`）はコマンドが死んでもペインが残るため、そこで `x` が効きます。

### プレビュー（`p`）

サイドバーの右隣にプレビューが開き、いま光っている行のペインの内容が出ます。フォーカスは
移らないので、`j` / `k` で選択を動かしながら「そのペインで何が起きているか」を見て回れます。
もう一度 `p` を押すか、ジャンプ・navモードの退場で閉じます（検索サブモード中は `alt+p`）。

出るのは**選択が動いた時点のスナップショット**で、対象ペインが裏で動き続けても選択を
動かさない限り変わりません。zellij がプラグインに渡せるのは装飾のないプレーンテキストだけ
なので色や太字も再現されず、「大体の様子を掴む」ためのものと割り切っています。

見て回るだけでは既読になりません（フォーカスを移していないため）。確認したことを伝えるには
`r` を押してください（`done` / `blocked` / `error` のみ）。

### ヘルプ（`?`）

サイドバーの幅（32文字）には全キーの説明が収まらないため、常時出るヒントはフッターの
`?:help` / `esc:exit` だけに絞ってあります。`?` を押すとヘッダはそのままに、ツリーの部分が
キー一覧と状態アイコン凡例に切り替わり、フッターは `press any key to close` に変わります。
何かキーを押せば元の表示に戻り、そのキー自体は操作として解釈されないので、必要なら押し
直してください。navモードは開いている間も継続します。

キー一覧の中身はいま居るモードごとに変わり、状態アイコンの凡例はどこから開いても同じです。
画面の高さに収まらないぶんは、一覧と同じあふれマーカーで示します。

## 設定

**エイリアス定義（`config.kdl` の `plugins` ブロック）に書いてください。** 設定を書く場所
として fujin がサポートするのはここだけです（→
[エイリアス経由だけをサポートします](#エイリアス経由だけをサポートします)）。設定は下の
一覧がすべてです。

<!-- settings:begin -->
<!-- ここは repos/main/src/config.rs の SETTINGS から生成しています。手で直さず `make readme` を実行してください -->

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm" {
        show_cwd              "true"
        show_cwd_tilde        "true"
        show_deploy_animation "false"
        up_key                "Alt u"
        down_key              "Alt d"
        go_key                "Alt g"
        toggle_cwd_key        "Alt c"
    }
}
```

| キー | 値 | 既定 | 説明 |
| --- | --- | --- | --- |
| `show_cwd` | `"true"` / `"false"` | `false` | ペイン行の下に cwd を表示します（フック設定済みのペインのみ） |
| `show_cwd_tilde` | `"true"` / `"false"` | `false` | cwd行のホームディレクトリ配下を `~` で短縮表示します（実験的） |
| `show_deploy_animation` | `"true"` / `"false"` | `true` | 新規エージェントを検出したときヘッダーで配置演出を再生します |
| `up_key` | キー表記（`"Alt u"` / `"alt+u"`） | 未設定（そのヒントを出さない） | `fujin_up` に割り当てたキーの表記（フッターのヒント用・表示専用） |
| `down_key` | キー表記（`"Alt u"` / `"alt+u"`） | 未設定（そのヒントを出さない） | `fujin_down` に割り当てたキーの表記（フッターのヒント用・表示専用） |
| `go_key` | キー表記（`"Alt u"` / `"alt+u"`） | 未設定（そのヒントを出さない） | `fujin_go` に割り当てたキーの表記（フッターのヒント用・表示専用） |
| `toggle_cwd_key` | キー表記（`"Alt u"` / `"alt+u"`） | 未設定（そのヒントを出さない） | `fujin_toggle_cwd` に割り当てたキーの表記（フッターのヒント用・表示専用） |

<!-- settings:end -->

- 値は引用符で囲んで書いてください（`show_cwd "true"`）。プロパティ形式
  （`show_cwd="true"`）でも同じ意味に解釈します
- 真偽値として受け付けるのは `"true"` と `"false"` だけです。`1` や `yes` は解釈しません
- 解釈できない値があると、**起動直後の数秒だけ**フッターに `!bad value: show_cwd` のような
  警告が出ます。その項目は既定値のまま動きます

### フッターのキーヒント

`up_key` / `down_key` / `go_key` / `toggle_cwd_key` は**表示専用**で、キーを割り当てるもの
ではありません。割り当ては[直接キー](#直接キーモードなし)で行い、同じキーをここにも書いて
ください（zellij がプラグインへ宛先の pipe 名を渡さないため、fujin 側から実際の割り当てを
読み取れません）。書かなかった項目は、そのヒントだけが出なくなります。

表記は `"Alt u"` でも `"alt+u"` でも受け付け、どちらも `alt+u` と表示します。解釈できない値は
書かれたまま出るので、書き間違いは黙って消えず見える形で残ります。すべてのヒントの修飾キーが
揃っているときは `alt + › u:up  d:down  g:jump` のように先頭へまとめます。

### エイリアス経由だけをサポートします

fujin が動作を保証するのは、**エイリアス定義に設定を書き、レイアウトとキーバインドの両方から
そのエイリアス名で参照する**構成だけです。レイアウトに wasm のパスを直接書いても動きはしますが、
サポート対象外です。

> [!WARNING]
> `MessagePlugin` の宛先照合は**wasm のパスだけでなく設定（configuration）込み**で行われます。
> レイアウトの plugin ブロックにだけ `show_cwd "true"` を書き、キーバインド側に書かないと、
> 両者は別物と見なされて**キーが常駐サイドバーに届きません**。しかも届かないだけで済まず、
> zellij は**設定の一致する新しいインスタンスをその場に開いてしまいます**。fujin のサイドバーは
> フォーカス巡回から外れているため、**そうしてできたペインは閉じられません**。

エイリアスに寄せれば起きません。使わない場合は、レイアウトと**すべての** `MessagePlugin` に
同じ configuration を書き写す必要があります。

## エージェントからの状態通知

### Claude Code のフック

`setup.sh` が `extras/claude-hooks/fujin-hook.sh` を `~/.config/zellij/plugins/` へコピーし、
そのパスを `~/.claude/settings.json` へ登録します。手で書く場合はこの形です:

```jsonc
{
  "hooks": {
    // 以下のイベントすべてに同じエントリを追加する:
    // SessionStart, UserPromptSubmit, Stop, StopFailure,
    // SessionEnd, SubagentStart, SubagentStop, TaskCreated, TaskCompleted
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "/path/to/fujin-hook.sh" }
        ]
      }
    ],
    // Notification だけは matcher で種別を絞る
    "Notification": [
      {
        "matcher": "permission_prompt|agent_needs_input|idle_prompt|elicitation_dialog",
        "hooks": [
          { "type": "command", "command": "/path/to/fujin-hook.sh" }
        ]
      }
    ]
  }
}
```

フックは**新しく起動した Claude Code セッションから**有効になります。zellij 外（素の
ターミナル）で動く Claude Code や、サイドバーが起動していないときは完全な no-op です。

| フックイベント | 状態遷移 |
|---|---|
| `SessionStart` | idle（登録・カウンタリセット） |
| `UserPromptSubmit` | working |
| `Notification` | blocked（メッセージ保持） |
| `Stop` | done（バックグラウンドのサブエージェントが残っている間は working のまま） |
| `StopFailure` | error（後続の `Stop` では上書きされない） |
| `SessionEnd` | 登録解除 |
| `SubagentStart` / `SubagentStop` | サブエージェント数 ±1（`Stop` 後に最後の1つが終わったら done） |
| `TaskCreated` / `TaskCompleted` | 未完了タスク数 ±1 |

### 他のエージェント

プラグインは pipe 名 `fujin_status` で以下のJSONを受け取ります。Claude Code 以外
（codex 等）も、この形式で送れば同じように表示されます（**未検証**——動作するはずですが、
実機での確認は済んでいません）。

```bash
zellij pipe --name fujin_status -- '{
  "pane_id": '$ZELLIJ_PANE_ID',
  "agent": "codex",
  "event": "UserPromptSubmit",
  "cwd": "/path/to/project"
}'
```

`pane_id` は zellij が各ペインに与える `$ZELLIJ_PANE_ID`、`event` は上の表のイベント名、
`cwd` と `detail` は任意です。

> [!IMPORTANT]
> `--plugin` オプションは付けないでください。付けると未起動のプラグインを zellij が勝手に
> 起動してしまいます。付けなければ起動中のプラグインにのみ配送され、未起動時は無害な
> no-op になります。

外部から使う pipe は `fujin_status`、キーバインド用の `fujin_up` / `_down` / `_go` /
`_mode` / `_toggle_cwd`、後始末用の `fujin_dismiss` だけです（`fujin_toggle_cwd` は
ペイロード無しなら反転、`true` / `false` を渡せばその値に揃えます）。`fujin_sync_state` /
`_read` / `_selection` / `_command` / `_mark` / `_preview` / `_width` はインスタンス間の
同期用の内部プロトコルなので、外から叩かないでください。

## トラブルシューティング

### サイドバーが出ない

レイアウトが効いていないケースがほとんどです。`config.kdl` に `default_layout "fujin"` が
あるか、レイアウトに `new_tab_template` が書かれているかを確認してください（`setup.sh` は
未設定の項目を判定して教えてくれます）。いま何が読み込まれているかは
`zellij action dump-layout` で確認できます。

ペインの場所は確保されているのに**中身が完全に空白**なら、権限の承認が途中で止まっています。
要求権限が1つでも未承認だと、承認プロンプトも描画も出ないまま空白になります（以前のバージョンを
承認済みで、その後に要求権限が増えた場合にも起こります）。zellij のキャッシュディレクトリにある
`permissions.kdl` から該当 wasm のエントリを削除し、セッションを作り直してください。

### 更新したのに古い挙動のまま

**wasm の入れ替えは稼働中のセッションには反映されません。** 既存のインスタンスは古い wasm の
まま動き続けるので、セッションを作り直してください。

### キー（`Ctrl+y` など）が効かない

エイリアスを使わずに設定していると、レイアウト側とキーバインド側で設定が食い違っている
可能性があります（→[エイリアス経由だけをサポートします](#エイリアス経由だけをサポートします)）。

### 権限プロンプトが繰り返し出る

承認結果は**展開後の絶対パスごと**に記録されます。置き場所を変えると新しいパスとして
扱われます。

### フローティングのサイドバーが残ってしまった

フォーカス中のタブにサイドバーが1つも居ないと、`Ctrl+y` で fujin が自分をフローティングで
一時的に呼び出します。通常は `Esc` やジャンプで自分から閉じますが、取り残されると
フォーカス巡回から外れているためユーザーからも CLI からも閉じられません。掃除はこれで:

```bash
zellij pipe --name fujin_dismiss
```

### 起動しない・挙動がおかしい

zellij のキャッシュを消してから試してください。承認結果（`permissions.kdl`）も消えるので、
次の起動で承認をやり直します。

```bash
rm -rf ~/Library/Caches/org.Zellij-Contributors.Zellij   # macOS
rm -rf ~/.cache/zellij                                   # Linux
```

### 設定を変えずに試したい

常駐させずフローティングで開けます。zellij 組み込みのプラグインマネージャ（`Ctrl+o` → `p`）
から wasm のパスを指定して読み込むこともできます。

```bash
zellij action new-pane --floating --width 40 --height 20 -p "fujin"
```

## 付録: 設定を手で書く場合

`setup.sh` を使わずに設定する場合や、既存の設定へ部分的に取り込みたい場合の内訳です。
キーバインドは[キーバインド](#キーバインド)、フックは[エージェントからの状態通知](#エージェントからの状態通知)を参照してください。

<details>
<summary>wasm の配置とエイリアス定義</summary>

wasm はどこに置いてもかまいませんが、OS を問わず `~/.config/zellij` が config ディレクトリに
なるので、その配下にまとめておくと手順が環境に依存しません。

```bash
mkdir -p ~/.config/zellij/plugins
curl -fsSL https://github.com/yo-goto/fujin/releases/latest/download/fujin.wasm \
  -o ~/.config/zellij/plugins/fujin.wasm
```

`config.kdl` でエイリアスを定義すると、以降はレイアウトにもキーバインドにも `"fujin"` と
だけ書けば済みます。

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm"
}
```

`file:~/…` の `~`（と `$HOME` などの環境変数）は zellij が展開します。ただし
**相対パスは書けません**（cwd 補完が効かないため、絶対パスか `~` 付きか `https://…` に
してください）。

</details>

<details>
<summary>サイドバーを常駐させるレイアウト</summary>

デフォルトレイアウトのタブテンプレートにサイドバーを埋め込みます
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
    // 同じ内容を new_tab_template にも書く
}
```

> [!CAUTION]
> `children` ではなく `pane` と書いてください。`children` は「このレイアウトが定義するタブの
> ペインをここに差し込む」というマーカーなので、`tab` ノードを持たないテンプレートだけの
> レイアウトでは**何も入らず、ターミナルが0個のタブができます**。セッション作成時にこれが
> 起きると zellij がそのまま終了します。

<!-- -->

> [!IMPORTANT]
> `new_tab_template` にも同じ内容を書いてください。`default_tab_template` は「新規タブ用
> テンプレート」にフォールバックしますが、**セッションマネージャ（`Ctrl+o` → `w`）で
> レイアウトを選んで作ったセッションではフォールバックが効きません**。両方書けば起動経路に
> 依存しなくなります。

あとは `config.kdl` に `default_layout "fujin"` を書きます。`NewTab` のキーバインド側で
レイアウトを指定する手（`bind "n" { NewTab { layout "fujin"; } SwitchToMode "normal" }`）も
あります。

初回ロード時の権限承認で要求するのは `ReadApplicationState` / `ChangeApplicationState` /
`ReadCliPipes` / `InterceptInput` / `MessageAndLaunchOtherPlugins` / `OpenTerminalsOrPlugins` /
`ReadPaneContents` の7つです。

</details>

## ライセンス

MIT License（`LICENSE` を参照）。依存している `zellij-tile` も MIT です。
