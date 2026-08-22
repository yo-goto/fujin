// fujin — zellij用サイドバープラグイン
//
// タブ > ペインの縦並び表示、エージェント状態の可視化、グローバルキーでのジャンプ。
// 設計決定は .docs/concept/design-decisions.md、モジュール構成の詳細は
// .docs/dev/module-map.md を参照。
//
// アーキテクチャ上の前提（実測確認済み。.docs/dev/api-reference.md）:
// - タブ数ぶんのインスタンスが同時稼働する（zellijの構造上回避不能）
// - pipe は全インスタンスに配送される
// - **PaneUpdate / TabUpdate は可視インスタンスにしか届かない**ため、
//   バックグラウンドのインスタンスの一覧は古いまま凍る
// - Event::Visible は全インスタンスに届き、true になるのは常に1つだけ
//
// モジュール構成:
// - agent  — フックイベントの解釈とエージェント状態の遷移
// - config — 設定仕様の正本と configuration の取り込み・設定警告の表示期限（決定202608080346）
// - command — コマンドペインのライフサイクルからのコマンド状態の導出（決定202608072218）
// - focus  — フォーカス同期と、navモード中のフォーカスの預かり（決定202608072359）
// - nav    — navモード（決定202607310311）の入退場・選択移動・行クリック
// - search — 検索サブモードのキー操作と、ファジーマッチの純粋ロジック（`search::matcher`）
// - jump   — 番号ジャンプサブモード（決定202608070342）の通し番号とキー操作
// - triage — トリアージモード（navモードの内側の優先度順一覧）
// - mark   — 複数選択（マーク。決定202608080250）の集合と、その配布
// - preview — プレビュー（決定202608082045。選択行のペインの内容を覗き見る）
// - termination — 終了操作サブモード（対象ペインの close / kill / kill→close）
// - deploy — 配置演出（新規エージェント検出時のヘッダーアニメーション）
// - render — サイドバーの描画（共通語彙。枠の要素ごとの組み立ては render::* の子モジュール）
// - width  — 表示セル幅の計算・切り詰めの純粋関数
// - sync   — インスタンス間の状態同期（決定202608012141）
// - width_sync — サイドバー幅のタブ間追従（用語: width-sync）
// - summon — フローティングでの臨時召喚（決定202608011644）
// - entry  — wasm のエクスポート関数（`register_plugin!` の自前版。決定202608111836）
// - host   — ホストコマンドの間接層（テストビルドでは発行の記録に差し替わる）

mod agent;
mod command;
mod config;
mod deploy;
mod entry;
mod focus;
mod host;
mod jump;
mod mark;
mod nav;
mod preview;
mod render;
mod search;
mod summon;
mod sync;
mod termination;
mod triage;
mod width;
mod width_sync;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use zellij_tile::prelude::*;

use agent::AgentInfo;
use command::CommandInfo;
use config::ShowDeployAnimation;
use deploy::Deployment;
use focus::ParkedFocus;
use jump::JumpState;
use preview::{PreviewContent, PreviewState};
use search::SearchState;
use termination::TerminationState;
use triage::TriageState;

// --- ワイヤプロトコル（pipe名） ---

// フックからの状態通知
const STATUS_PIPE: &str = "fujin_status";
// キーバインドからのナビゲーション（直接キー方式・決定202607302258）
const NAV_UP_PIPE: &str = "fujin_up";
const NAV_DOWN_PIPE: &str = "fujin_down";
const NAV_GO_PIPE: &str = "fujin_go";
// navモードへの入場（決定202607310311）
const NAV_MODE_PIPE: &str = "fujin_mode";
// cwd表示のトグル（.docs/issues/issue-toggle-cwd-key.md）。全インスタンスが独立に
// 反転するので権威判定（決定202608012142）は要らない。**反転した値を兄弟へbroadcastして
// 補強してはいけない** — 未処理の兄弟へ先に届くと、そこからさらに反転して逆を向く
const TOGGLE_CWD_PIPE: &str = "fujin_toggle_cwd";
// インスタンス間の状態同期（決定202608012141）
const SYNC_STATE_PIPE: &str = "fujin_sync_state";
// 既読クリアの兄弟インスタンスへの配布（決定202608012141）
const READ_CLEAR_PIPE: &str = "fujin_read";
// コマンド状態の配布（決定202608072218）。導出できるのは PaneUpdate が届く可視インスタンス
// だけなので、エージェント状態と違って自前では揃わない
const COMMAND_STATE_PIPE: &str = "fujin_command";
// 選択ペインIDの配布（決定202608012141）
const SELECTION_PIPE: &str = "fujin_selection";
// マーク（決定202608080250）の配布。集合をまるごと運ぶ — 差分で運ぶと、取りこぼした
// 1通ぶんだけ集合が食い違ったまま直らない
const MARK_PIPE: &str = "fujin_mark";
// プレビューのスナップショット送付（決定202608082045）。1行目が対象ペイン名、2行目以降が内容
const PREVIEW_PIPE: &str = "fujin_preview";
// サイドバー幅のタブ間追従（.docs/issues/issue-sidebar-width-persist-across-tabs.md）。
// zellij の `new_tab_template` はタブ生成時に複製されるだけの静的な雛形なので、
// あるタブでリサイズしても他タブには伝播しない。観測した幅を配って各自に
// 寄せさせる
const WIDTH_PIPE: &str = "fujin_width";
// 取り残された召喚インスタンスの強制掃除（決定202608011644）。取り残された召喚はキーを
// 横取りしておらず Esc が届かず、unselectable なので普段のペイン操作でも消せない。
// pipe 経由の逃げ道を用意しておく
const DISMISS_PIPE: &str = "fujin_dismiss";

// 滞在猶予（決定202608080109。.docs/issues/issue-transit-focus-clears-read-state.md）。フォーカス
// されてからこの秒数だけ留まって初めて既読にする。**通過と到着はフォーカスの
// 有無だけでは原理的に区別できない**ので、滞在時間で分ける。
// 実機での調整が残っている暫定値
const READ_DELAY: f64 = 0.6;

// サイドバーに並べる選択対象（ターミナルペイン1つぶん）
#[derive(Debug, Clone)]
struct Selectable {
    tab_position: usize,
    pane_id: u32,
    title: String,
    // フォーカス時の should_float_if_hidden に使う（決定202608012142）。
    // ペイン名を丸括弧で囲むかの判定も兼ねる
    //（要件: .docs/requirements/req-floating-pane-indicator.md）
    is_floating: bool,
}

#[derive(Default)]
struct State {
    tabs: Vec<TabInfo>,
    panes: Option<PaneManifest>,
    // key: ターミナルペインID
    agents: BTreeMap<u32, AgentInfo>,
    // コマンドペインの状態（決定202608072218）。key は同じくターミナルペインID。
    // エージェント状態が優先される（`State::pane_status`）ため別の入れ物に持つ
    commands: BTreeMap<u32, CommandInfo>,
    // フラット化した選択対象
    selectable: Vec<Selectable>,
    selected: usize,
    // マーク（決定202608080250）: 一括操作の対象として選んだペインIDの集合。タブを
    // またいでよく、navモードを退場しても保持し、兄弟インスタンスへも配る
    marked: BTreeSet<u32>,
    visible: bool,
    own_plugin_id: Option<u32>,
    // 自分のwasm URL。実行時に判明する（同期の宛先・召喚の起動元に使う）
    own_plugin_url: Option<String>,
    permissions_granted: bool,
    show_cwd: bool,
    // 配置演出を出すか。既定値を型に持たせてある理由は config.rs 参照
    show_deploy_animation: ShowDeployAnimation,
    // ペインID -> cwd（フックのペイロード由来）
    pane_cwds: BTreeMap<u32, String>,
    // navモード中か。全キーを横取りしているインスタンスだけが true になる
    nav_mode: bool,
    // ヘルプオーバーレイを表示中か。navモードの内側の表示状態なので、
    // 退場時には必ず倒れる
    help_overlay: bool,
    // 直近に観測した実フォーカス（要件: focus-sync）。問い合わせ系のホスト関数は
    // テストから呼べないため、結果をここへ畳んで判定ロジックを切り離しておく
    focused_pane: Option<u32>,
    // 直近の実フォーカスがターミナルペインそのものだったか（決定202608072359）。false なら
    // 作業ペインにフォーカス枠が点いていない ＝ フォーカスを預かる理由が無い
    focus_on_terminal: bool,
    // navモード中にフォーカスを預かっている作業ペイン（決定202608072359）。退場でここへ
    // 戻す。預けている間の実フォーカスはサイドバー自身なので `focused_pane` とは別物
    focus_parked: Option<ParkedFocus>,
    // 前面に出たあと、まだ実フォーカスを取り直せていない（要件: focus-sync）。
    // 非可視の間はキャッシュが凍るため、可視化を合図に同値でも引き直させる
    pending_focus_resync: bool,
    // navモードを抜けたときのフォーカスと選択（要件: focus-sync）。
    // 次の入場で選択の初期値を決めるのに使う
    focus_at_nav_exit: Option<u32>,
    selection_at_nav_exit: Option<u32>,
    // 既に把握している兄弟インスタンスのプラグインID（同期の押し付け先判定）
    known_siblings: BTreeSet<u32>,
    // 兄弟へ状態ダンプを要求済みか（.docs/issues/issue-tab-switch-agent-status-desync.md）。
    // 押し付けだけでは新しいタブが取りこぼすので、兄弟を初めて見つけた1回だけ
    // こちらから取りに行く
    state_requested: bool,
    // 召喚インスタンス（フローティング）か（決定202608011644）
    summoned: bool,
    // 準備が整い次第 navモードへ入る予約。召喚直後は権限も一覧も未取得で、
    // 揃うまで待ってから入る
    pending_nav_entry: bool,
    // 自分が召喚したフローティングの、タブindex -> プラグインID（決定202608011644）。
    // 重ねて召喚しないための記録。一覧では代用できない（summon.rs 参照）
    summoned_panes: BTreeMap<usize, u32>,
    // 以下のサブモードは中か否かを is_some() で表す（フラグと中身が食い違う
    // 状態を作らせない）
    search: Option<SearchState>,
    triage: Option<TriageState>,
    jump: Option<JumpState>,
    // 中身は入場時に捕まえた対象ペイン（決定202608080140）
    termination: Option<TerminationState>,
    // プレビュー（決定202608082045）。モードではなく navモード内のトグル可能な横断的
    // 表示状態なので、キー解釈は変わらない
    preview: Option<PreviewState>,
    // 自分がプレビュー用フローティングペインとして起動されたインスタンスか
    //（決定202608082045）。真なら配られたスナップショットを描くだけに徹する
    is_preview: bool,
    // 描くスナップショット（プレビュー用フローティングペインでのみ埋まる）
    preview_content: PreviewContent,
    // 状態変化のたびに進む単調増加のカウンタ。トリアージ一覧の tie-break に使う
    state_seq: u64,
    // 縦スクロールで一覧が上に隠れている行数。毎フレーム導出されるローカルな
    // 表示状態で、兄弟インスタンスへは配らない（決定202608012141の範囲外）
    scroll: usize,
    // 直近に描画した画面高。行クリックの逆引き（pane_at_row）が描画と同じ
    // 表示範囲を再現するために要る。0 は「まだ一度も描いていない」
    viewport_rows: usize,
    // 直近に描画した画面幅。配置演出の着地列は幅から決まるが、タイマーは
    // 描画の外で進むのでここに控えておく
    viewport_cols: usize,
    // タブ間で揃えたいサイドバー幅（.docs/issues/issue-sidebar-width-persist-across-tabs.md）。
    // 誰かがリサイズしたらその桁数が権威になり、兄弟インスタンスへ配られる
    width_target: Option<usize>,
    // 目標へ寄せるために撃ったリサイズの回数。相対リサイズは端末幅の一定割合
    // ずつ動く量子化された操作なので、目標にぴったり乗るとは限らない。
    // 撃ち続けて振動しないよう回数で打ち切る
    width_attempts: usize,
    // 自分が撃ったリサイズの着地待ち。中身は撃った向きで、幅が実際に動くまで
    // 保持する。待っている間は次の一手を撃ち足さない — 多重に飛ばすと、あとから
    // 届く着地を利用者の操作と取り違えて配り直してしまう（実測でこの経路を踏んだ）。
    // 撃った向きと逆に幅が動いたときだけは自分の着地ではありえないので、
    // 利用者の操作として扱う
    width_adjusting: Option<Resize>,
    // これ以上寄せられないと分かったか（目標を跨いでしまった・回数を使い切った）。
    // 新しい目標が届いたら倒す
    width_settled: bool,
    // 再生中の配置演出。再生中だけ Some で、終われば None に戻る
    deployment: Option<Deployment>,
    // 届いた `Event::Timer` の経過時間を積んだ値。壁時計を引けないので、
    // 期限判定はこれを時刻の代わりに使う（単調増加しかしない）
    elapsed: f64,
    // タイマーの鎖が繋がっているか。`set_timeout()` はキャンセルできず、二重に
    // 張ると Timer が二重に届くので、繋がっていないときだけ張る
    timer_armed: bool,
    // 既読を保留しているペイン -> 既読にしてよくなる時刻（`elapsed` 基準）。
    // 滞在猶予（決定202608080109）の入れ物
    pending_reads: BTreeMap<u32, f64>,
    // direct-keys方式の configuration キー -> 画面に出すキー表記（決定202608070119・28）。
    // 書かれていない項目は持たない＝ヒントからその項目だけが省かれる
    direct_keys: BTreeMap<String, String>,
    // 解釈できなかった設定のキー（決定202608080346）。黙って既定値へ倒さない
    config_warnings: Vec<&'static str>,
    // 設定の警告をフッターに出しておく期限（`elapsed` 基準）。None は
    // 「出していない・もう出さない」
    config_warning_until: Option<f64>,
    // 直近にホストへ伝えたテキストカーソル位置（`show_cursor`）。同じ値を送り直さない
    //（`sync_input_cursor`）
    cursor_shown: Option<(usize, usize)>,
}

// `register_plugin!(State)` は使わない。エクスポート関数は entry.rs が持つ
//（IME経由の非ASCII入力を拾うため。決定202608111836 / .docs/issues/issue-ime-input-support.md）
fn main() {
    entry::install_panic_hook();
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.apply_config(&configuration);
        // プレビュー用フローティングペインとして開かれたか（決定202608082045）。
        // 一覧も navモードも持たない描き手専用のインスタンスになる
        self.is_preview = config::is_preview(&configuration);
        // 別インスタンスに召喚されたフローティングなら、入場pipeを取り逃している。
        // 押し直させずに済むよう、準備でき次第こちらから navモードへ入る（決定202608011644）
        self.summoned = config::summoned(&configuration);
        self.pending_nav_entry = self.summoned;
        self.own_plugin_id = Some(get_plugin_ids().plugin_id);
        // selectable はペイン側の属性でリロードしても前回の false が残り、承認
        // プロンプトにフォーカスできないデッドロックになるため、毎回戻す。
        // 承認済みなら PermissionRequestResult が即返り、すぐ false に戻る
        host::set_selectable(true);
        host::request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
            // navモードのキー横取り（決定202607310311）
            PermissionType::InterceptInput,
            // 兄弟インスタンスとの状態同期（決定202608012141）
            PermissionType::MessageAndLaunchOtherPlugins,
            // 臨時召喚（決定202608011644）。OpenPluginPaneFloating はこれを要求し
            //（MessageAndLaunchOtherPlugins では足りない）、拒否されると
            // shim 側の unwrap でプラグインごと落ちる
            PermissionType::OpenTerminalsOrPlugins,
            // プレビューのスナップショット取得（決定202608082045）。拒否されてもパニック
            // せず「取れなかった」扱い（preview::UNAVAILABLE）に落ちる
            PermissionType::ReadPaneContents,
        ]);
        host::subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::Visible,
            EventType::InterceptedKeyPress,
            // 一括で届くテキスト入力（貼り付けと**IMEの変換確定**）。横取りとは
            // 別の経路で来るので、これが無いと確定した文字列が消える
            //（.docs/issues/issue-ime-input-support.md）
            EventType::PastedText,
            // 行クリックでのフォーカス移動（要件: .docs/requirements/req-click-to-focus.md）
            EventType::Mouse,
            // 配置演出のフレーム送り（要件: .docs/requirements/req-header-animation.md）。
            // 発火するのは再生中に set_timeout() を繋いでいる間だけ
            EventType::Timer,
            // プラグイン終了・リロード時に横取りを解除する保険
            EventType::BeforeClose,
        ]);
        // 注意: set_selectable(false) はここでは呼ばない。呼ぶと権限承認
        // プロンプトにフォーカスできず承認不能になる。
        // PermissionRequestResult 受信後に呼ぶ（上の set_selectable(true) と対）。
    }

    fn update(&mut self, event: Event) -> bool {
        // プレビュー用フローティングペインは配られたスナップショットを描くだけ
        //（決定202608082045）。一覧の再構築も権威判定も要らないうえ、走らせると兄弟
        // インスタンスとして状態の配布に混ざってしまう
        if self.is_preview {
            return self.update_as_preview(event);
        }
        // 打っている本人の入力か（下の `defers_render_while_typing` の例外）
        let from_input = matches!(event, Event::InterceptedKeyPress(_) | Event::PastedText(_));
        let should_render = match event {
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                if self.permissions_granted {
                    // フローティングで起動されていたら召喚インスタンスとして自覚する
                    //（決定202608011644。下の set_selectable の分岐に効くので、ここより前に）
                    self.adopt_floating_as_summoned();
                    // フォーカス巡回にサイドバーを混ぜない（決定202607302258）。**臨時召喚は
                    // 例外**（決定202608011644）— unselectable だとロジックが壊れたとき普段の
                    // ペイン操作で消せなくなるので、「必ず自分で消せる」ほうを取る
                    if !self.summoned {
                        host::set_selectable(false);
                    }
                    // 既定のペイン名はwasmのフルURLで長すぎるので短くする
                    if let Some(id) = self.own_plugin_id {
                        host::rename_plugin_pane(id, "fujin");
                    }
                    // ここより前に来た PaneUpdate は未承認として捨てている。
                    // 承認を待たずに一覧が揃うと入場の機会がここしか無い
                    self.learn_own_plugin_url();
                    self.enter_nav_mode_if_pending();
                    // 警告の時計はここから回す（決定202608080346）。承認が済むまでフッターは
                    // 描かれないので、load() から数えると見られないまま期限が切れる
                    self.arm_config_warning();
                }
                true
            }
            Event::Visible(visible) => {
                self.visible = visible;
                if visible {
                    // タブ切り替えで前面に出た合図。非可視の間に凍ったキャッシュを
                    // 信用せず、実フォーカスから選択を引き直す（要件: focus-sync）
                    self.pending_focus_resync = true;
                    self.refresh_focus();
                }
                visible
            }
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                self.rebuild_selectable();
                // タブの切り替えでもフォーカスは動く（要件: focus-sync）
                self.refresh_focus();
                self.enter_nav_mode_if_pending();
                true
            }
            Event::PaneUpdate(manifest) => {
                // コマンド状態の導出は既読モデルより先（決定202608072218）。導出した状態が
                // その場で既読になってしまう問題は、既読の猶予
                //（`CommandInfo::awaiting_refocus`）が防ぐ
                self.apply_command_states(&manifest);
                self.apply_read_model(&manifest);
                self.panes = Some(manifest);
                self.rebuild_selectable();
                self.prune_stale_agents();
                // 自分のURLは PaneManifest で初めて分かる。新しい兄弟
                // インスタンスを見つけたら状態を配る（決定202608012141）
                self.learn_own_plugin_url();
                self.push_state_to_new_siblings();
                // ペインのフォーカス移動はここに届く（要件: focus-sync）
                self.refresh_focus();
                self.enter_nav_mode_if_pending();
                true
            }
            Event::BeforeClose => {
                // 横取りしたままプラグインが消えるとキー入力が戻らなくなる。
                // exit_nav_mode() は使わない — 閉じられている最中に自分を
                // close_plugin_pane() すると二重解放になる
                if self.nav_mode {
                    self.nav_mode = false;
                    host::clear_key_presses_intercepts();
                }
                // プレビューと預かったフォーカスの後始末。どちらも対象は自分では
                // なく別のペインなので、閉じられている最中でも投げてよい。
                // フォーカスを返さないと、リロード時にユーザーは unselectable に
                // 戻ったサイドバーにフォーカスを残して詰まる（決定202608072359・決定202608082045）
                self.close_preview();
                self.release_parked_focus(true);
                false
            }
            // 配置演出のフレーム送りと滞在猶予の期限判定。どちらも同じ鎖に
            // 相乗りする（Timer にはどのタイマーが発火したかの区別が無い）。
            // 仕事が無くなれば鎖が切れ、次のタイマーは来ない
            Event::Timer(elapsed) => self.on_timer(elapsed),
            // 左クリックだけを扱う（要件: click-to-focus）。
            // ダブルクリック・ドラッグ・右クリックはv1対象外
            Event::Mouse(Mouse::LeftClick(line, _column)) => self.handle_click(line),
            Event::InterceptedKeyPress(key) => {
                // 横取りを要求したインスタンスにしか届かないが、念のため。
                // `return` で抜けない — 下のテキストカーソルの追従はここでも通す
                self.nav_mode && self.handle_nav_key(key)
            }
            // 貼り付け・IMEの変換確定。フォーカスを預かっている（決定202608072359）間だけ
            // 自分に届く。入力欄の外なら中で捨てる
            Event::PastedText(text) => self.handle_pasted_text(&text),
            _ => false,
        };
        // 入力欄のテキストカーソル（IMEの候補窓が付いてくる）はここで伝える。
        // **`render()` の中からは呼べない**（`sync_input_cursor` 参照）
        self.sync_input_cursor();
        should_render && (from_input || !self.defers_render_while_typing())
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let is_ours = matches!(
            pipe_message.name.as_str(),
            STATUS_PIPE
                | NAV_UP_PIPE
                | NAV_DOWN_PIPE
                | NAV_GO_PIPE
                | NAV_MODE_PIPE
                | TOGGLE_CWD_PIPE
                | SYNC_STATE_PIPE
                | READ_CLEAR_PIPE
                | COMMAND_STATE_PIPE
                | SELECTION_PIPE
                | MARK_PIPE
                | PREVIEW_PIPE
                | WIDTH_PIPE
                | DISMISS_PIPE
        );
        // CLI pipe は即座にunblockしないと送信側が1秒タイムアウトまで待たされ、
        // フックのレイテンシに直結する（実測でroute.rsのタイムアウトを確認済み）
        if is_ours && matches!(pipe_message.source, PipeSource::Cli(_)) {
            host::unblock_cli_pipe_input(&pipe_message.name);
        }
        // プレビュー用フローティングペイン（決定202608082045）が扱うのはスナップショットだけ。
        // 状態通知や同期まで取り込むと、描くのに使わない状態を溜め込んだうえに
        // 兄弟インスタンスとして配布の輪に混ざる
        if self.is_preview && pipe_message.name != PREVIEW_PIPE {
            return false;
        }
        // 各アームの中身は担当モジュール側のハンドラにある。ここは配線だけ
        let payload = pipe_message.payload.as_deref();
        let should_render = match pipe_message.name.as_str() {
            STATUS_PIPE => self.handle_status_pipe(payload),
            NAV_UP_PIPE => self.handle_nav_step_pipe(false),
            NAV_DOWN_PIPE => self.handle_nav_step_pipe(true),
            NAV_GO_PIPE => self.handle_nav_go_pipe(),
            NAV_MODE_PIPE => self.handle_nav_mode_pipe(),
            TOGGLE_CWD_PIPE => self.handle_toggle_cwd_pipe(payload),
            DISMISS_PIPE => self.handle_dismiss_pipe(),
            SELECTION_PIPE => self.handle_selection_pipe(payload),
            PREVIEW_PIPE => self.handle_preview_pipe(payload),
            MARK_PIPE => self.handle_mark_pipe(payload),
            WIDTH_PIPE => self.handle_width_pipe(payload, &pipe_message.source),
            READ_CLEAR_PIPE => self.handle_read_clear_pipe(payload),
            COMMAND_STATE_PIPE => self.handle_command_state_pipe(payload),
            SYNC_STATE_PIPE => self.handle_sync_state_pipe(payload, &pipe_message.source),
            _ => false,
        };
        // navモードへの入場は pipe 経由でも起きる（fujin_mode）ので、
        // テキストカーソルの追従は update と同じくこちらでも行う
        self.sync_input_cursor();
        // pipe は全て「外から」来るので、入力中は描き直さない（update と同じ理由）
        should_render && !self.defers_render_while_typing()
    }

    fn render(&mut self, rows: usize, cols: usize) {
        // 表示範囲の寄せ直しは描画の直前に行う。画面高が分かるのがここだけで、
        // 行の増減も選択の移動もまとめて吸収できる
        self.reconcile_viewport(rows);
        // 自分の幅が分かるのも描画のときだけ。リサイズの検出と寄せ直しは
        // viewport_cols を更新する前に済ませる（前回との差が判定材料）
        self.reconcile_width(cols);
        // 配置演出のタイマーは描画の外で進むので、幅を控えておく
        self.viewport_cols = cols;
        self.draw(rows, cols);
    }
}

impl State {
    // プレビュー用フローティングペイン（決定202608082045）のイベント処理。
    //
    // 描き手に徹するので、見るのは自分が描けるようになったか（権限）だけ。
    // 一覧・エージェント状態・navモードには一切関わらない
    fn update_as_preview(&mut self, event: Event) -> bool {
        let Event::PermissionRequestResult(status) = event else {
            return false;
        };
        self.permissions_granted = matches!(status, PermissionStatus::Granted);
        if self.permissions_granted {
            // 常駐サイドバーと違い `set_selectable(false)` は呼ばない。
            // 臨時召喚（決定202608011644）と同じ理由で、一時的に出ているだけのペインは
            // 「必ず自分で消せる」ほうを取る
            if let Some(id) = self.own_plugin_id {
                // 記号付きで通常ペインと見分けを付ける。枠色は zellij 側に
                // API が無く（決定202608072359）、内容領域の背景色は「色はテーマから
                // 借りる」原則（ui-design.md 原則1）と衝突するため、
                // ネイティブのタイトルバー文字列で代替している
                host::rename_plugin_pane(id, "▣ preview");
            }
        }
        true
    }

    // タイマーの鎖を繋ぐ。
    //
    // `set_timeout()` はキャンセルできないので、繋がっている間は張り直さない —
    // 二重に張ると Timer が二重に届き、配置演出のフレームが倍速になる。刻みは
    // 配置演出のフレーム間隔で、滞在猶予の期限判定はそこへ相乗りする（既読側は
    // 経過時間で期限を見るので、刻みが細かくても判定はずれない）
    pub(crate) fn arm_timer(&mut self) {
        if !self.timer_armed {
            host::set_timeout(deploy::FRAME_INTERVAL);
            self.timer_armed = true;
        }
    }

    // タイマー1回ぶん進める。再描画が要るかを返す
    fn on_timer(&mut self, elapsed: f64) -> bool {
        self.timer_armed = false;
        // 壁時計は引けないので、届いた経過時間を積んだ値を時刻の代わりに使う
        self.elapsed += elapsed;
        let mut render = self.advance_deployment();
        render |= self.apply_pending_reads();
        render |= self.expire_config_warning();
        // 仕事が残っているあいだだけ鎖を繋ぎ直す。静かなときは回し続けない
        if self.deployment.is_some()
            || !self.pending_reads.is_empty()
            || self.config_warning_until.is_some()
        {
            self.arm_timer();
        }
        render
    }

    // 自分のプラグインペインが指定タブに居るか。
    // manifest が古くても、自分のペインの所属タブは動かないので判定できる
    pub(crate) fn owns_tab(&self, tab_position: usize) -> bool {
        let (Some(own_id), Some(manifest)) = (self.own_plugin_id, self.panes.as_ref()) else {
            return false;
        };
        manifest
            .panes
            .get(&tab_position)
            .map(|panes| panes.iter().any(|p| p.is_plugin && p.id == own_id))
            .unwrap_or(false)
    }
}
