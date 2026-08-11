// fujin — zellij用サイドバープラグイン
//
// タブ > ペインの縦並び表示、エージェント状態の可視化、グローバルキーでのジャンプ。
// 設計決定は docs/concept/design-decisions.md、モジュール構成の詳細は
// docs/dev/architecture.md を参照。
//
// アーキテクチャ上の前提（実測確認済み。docs/dev/api-reference.md）:
// - タブ数ぶんのインスタンスが同時稼働する（zellijの構造上回避不能）
// - pipe は全インスタンスに配送される
// - **PaneUpdate / TabUpdate は可視インスタンスにしか届かない**ため、
//   バックグラウンドのインスタンスの一覧は古いまま凍る
// - Event::Visible は全インスタンスに届き、true になるのは常に1つだけ
//
// モジュール構成:
// - agent  — フックイベントの解釈とエージェント状態の遷移
// - config — configuration の取り込みと設定仕様の正本（決定40）
// - command — コマンドペインのライフサイクルからのコマンド状態の導出（決定32）
// - nav    — navモード（決定12）と検索サブモードのキー操作、行クリック
// - search — ファジーマッチの純粋ロジック
// - triage — トリアージモード（navモードの内側の優先度順一覧）
// - mark   — 複数選択（マーク。決定39）の集合と、その配布
// - preview — プレビュー（決定42。選択行のペインの内容を覗き見る）
// - termination — 終了操作サブモード（対象ペインの close / kill / kill→close）
// - deploy — 配置演出（新規エージェント検出時のヘッダーアニメーション）
// - render — サイドバーの描画
// - width  — 表示セル幅の計算・切り詰めの純粋関数
// - sync   — インスタンス間の状態同期（決定13）
// - summon — フローティングでの臨時召喚（決定16）
// - entry  — wasm のエクスポート関数（`register_plugin!` の自前版。決定47）

mod agent;
mod command;
mod config;
mod deploy;
mod entry;
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

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use zellij_tile::prelude::*;

use agent::AgentInfo;
use command::CommandInfo;
use config::{Config, ShowDeployAnimation};
use deploy::Deployment;
use nav::{JumpState, SearchState};
use preview::{PreviewContent, PreviewState};
use termination::TerminationState;
use triage::TriageState;

// --- ワイヤプロトコル（pipe名） ---

// フックからの状態通知
const STATUS_PIPE: &str = "fujin_status";
// キーバインドからのナビゲーション（直接キー方式・決定6）
const NAV_UP_PIPE: &str = "fujin_up";
const NAV_DOWN_PIPE: &str = "fujin_down";
const NAV_GO_PIPE: &str = "fujin_go";
// navモードへの入場（決定12）
const NAV_MODE_PIPE: &str = "fujin_mode";
// cwd表示のトグル（docs/issues/toggle-cwd-key.md）。全インスタンスが独立に
// 反転するので権威判定（決定14）は要らない。**反転した値を兄弟へbroadcastして
// 補強してはいけない** — 未処理の兄弟へ先に届くと、そこからさらに反転して逆を向く
const TOGGLE_CWD_PIPE: &str = "fujin_toggle_cwd";
// インスタンス間の状態同期（決定13）
const SYNC_STATE_PIPE: &str = "fujin_sync_state";
// 既読クリアの兄弟インスタンスへの配布（決定13）
const READ_CLEAR_PIPE: &str = "fujin_read";
// コマンド状態の配布（決定32）。導出できるのは PaneUpdate が届く可視インスタンス
// だけなので、エージェント状態と違って自前では揃わない
const COMMAND_STATE_PIPE: &str = "fujin_command";
// 選択ペインIDの配布（決定13）
const SELECTION_PIPE: &str = "fujin_selection";
// マーク（決定39）の配布。集合をまるごと運ぶ — 差分で運ぶと、取りこぼした
// 1通ぶんだけ集合が食い違ったまま直らない
const MARK_PIPE: &str = "fujin_mark";
// プレビューのスナップショット送付（決定42）。1行目が対象ペイン名、2行目以降が内容
const PREVIEW_PIPE: &str = "fujin_preview";
// 取り残された召喚インスタンスの強制掃除（決定16）。取り残された召喚はキーを
// 横取りしておらず Esc が届かず、unselectable なので普段のペイン操作でも消せない。
// pipe 経由の逃げ道を用意しておく
const DISMISS_PIPE: &str = "fujin_dismiss";

// 滞在猶予（決定37。docs/issues/transit-focus-clears-read-state.md）。フォーカス
// されてからこの秒数だけ留まって初めて既読にする。**通過と到着はフォーカスの
// 有無だけでは原理的に区別できない**ので、滞在時間で分ける。
// 実機での調整が残っている暫定値
const READ_DELAY: f64 = 0.6;

// 設定の警告をフッターへ優先表示する時間（秒）。決定40の「起動直後の一定時間
// だけ優先表示」。過ぎればフッターは通常の表示へ戻る。警告は stderr にも残る
const CONFIG_WARNING_SECS: f64 = 8.0;

// サイドバーに並べる選択対象（ターミナルペイン1つぶん）
#[derive(Debug, Clone)]
struct Selectable {
    tab_position: usize,
    pane_id: u32,
    title: String,
    // フォーカス時の should_float_if_hidden に使う（決定14）。
    // ペイン名を丸括弧で囲むかの判定も兼ねる
    //（要件: docs/requirements/floating-pane-indicator/）
    is_floating: bool,
}

// navモード中、サイドバーへ預けたフォーカスの戻し先（決定34）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ParkedFocus {
    pane_id: u32,
    // 戻すときの should_float_if_hidden に使う（`focus_selected` と同じ理由）
    is_floating: bool,
    // 預かったフォーカスが実際に自分へ来たのを一度でも観測したか。
    // フォーカス移動は非同期なので、待たずに判定すると入場直後に退場してしまう
    //（`park_taken_over`）
    confirmed: bool,
}

#[derive(Default)]
struct State {
    tabs: Vec<TabInfo>,
    panes: Option<PaneManifest>,
    // key: ターミナルペインID
    agents: BTreeMap<u32, AgentInfo>,
    // コマンドペインの状態（決定32）。key は同じくターミナルペインID。
    // エージェント状態が優先される（`State::pane_status`）ため別の入れ物に持つ
    commands: BTreeMap<u32, CommandInfo>,
    // フラット化した選択対象
    selectable: Vec<Selectable>,
    selected: usize,
    // マーク（決定39）: 一括操作の対象として選んだペインIDの集合。タブを
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
    // 直近の実フォーカスがターミナルペインそのものだったか（決定34）。false なら
    // 作業ペインにフォーカス枠が点いていない ＝ フォーカスを預かる理由が無い
    focus_on_terminal: bool,
    // navモード中にフォーカスを預かっている作業ペイン（決定34）。退場でここへ
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
    // 召喚インスタンス（フローティング）か（決定16）
    summoned: bool,
    // 準備が整い次第 navモードへ入る予約。召喚直後は権限も一覧も未取得で、
    // 揃うまで待ってから入る
    pending_nav_entry: bool,
    // 自分が召喚したフローティングの、タブindex -> プラグインID（決定16）。
    // 重ねて召喚しないための記録。一覧では代用できない（summon.rs 参照）
    summoned_panes: BTreeMap<usize, u32>,
    // 以下のサブモードは中か否かを is_some() で表す（フラグと中身が食い違う
    // 状態を作らせない）
    search: Option<SearchState>,
    triage: Option<TriageState>,
    jump: Option<JumpState>,
    // 中身は入場時に捕まえた対象ペイン（決定35）
    termination: Option<TerminationState>,
    // プレビュー（決定42）。モードではなく navモード内のトグル可能な横断的
    // 表示状態なので、キー解釈は変わらない
    preview: Option<PreviewState>,
    // 自分がプレビュー用フローティングペインとして起動されたインスタンスか
    //（決定42）。真なら配られたスナップショットを描くだけに徹する
    is_preview: bool,
    // 描くスナップショット（プレビュー用フローティングペインでのみ埋まる）
    preview_content: PreviewContent,
    // 状態変化のたびに進む単調増加のカウンタ。トリアージ一覧の tie-break に使う
    state_seq: u64,
    // 縦スクロールで一覧が上に隠れている行数。毎フレーム導出されるローカルな
    // 表示状態で、兄弟インスタンスへは配らない（決定13の範囲外）
    scroll: usize,
    // 直近に描画した画面高。行クリックの逆引き（pane_at_row）が描画と同じ
    // 表示範囲を再現するために要る。0 は「まだ一度も描いていない」
    viewport_rows: usize,
    // 直近に描画した画面幅。配置演出の着地列は幅から決まるが、タイマーは
    // 描画の外で進むのでここに控えておく
    viewport_cols: usize,
    // 再生中の配置演出。再生中だけ Some で、終われば None に戻る
    deployment: Option<Deployment>,
    // 届いた `Event::Timer` の経過時間を積んだ値。壁時計を引けないので、
    // 期限判定はこれを時刻の代わりに使う（単調増加しかしない）
    elapsed: f64,
    // タイマーの鎖が繋がっているか。`set_timeout()` はキャンセルできず、二重に
    // 張ると Timer が二重に届くので、繋がっていないときだけ張る
    timer_armed: bool,
    // 既読を保留しているペイン -> 既読にしてよくなる時刻（`elapsed` 基準）。
    // 滞在猶予（決定37）の入れ物
    pending_reads: BTreeMap<u32, f64>,
    // direct-keys方式の configuration キー -> 画面に出すキー表記（決定27・28）。
    // 書かれていない項目は持たない＝ヒントからその項目だけが省かれる
    direct_keys: BTreeMap<String, String>,
    // 解釈できなかった設定のキー（決定40）。黙って既定値へ倒さない
    config_warnings: Vec<&'static str>,
    // 設定の警告をフッターに出しておく期限（`elapsed` 基準）。None は
    // 「出していない・もう出さない」
    config_warning_until: Option<f64>,
    // 直近にホストへ伝えた実カーソル位置（`show_cursor`）。同じ値を送り直さない
    //（`sync_input_cursor`）
    cursor_shown: Option<(usize, usize)>,
}

// `register_plugin!(State)` は使わない。エクスポート関数は entry.rs が持つ
//（IME経由の非ASCII入力を拾うため。決定47 / docs/issues/ime-input-support.md）
fn main() {
    entry::install_panic_hook();
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.apply_config(&configuration);
        // プレビュー用フローティングペインとして開かれたか（決定42）。
        // 一覧も navモードも持たない描き手専用のインスタンスになる
        self.is_preview = config::is_preview(&configuration);
        // 別インスタンスに召喚されたフローティングなら、入場pipeを取り逃している。
        // 押し直させずに済むよう、準備でき次第こちらから navモードへ入る（決定16）
        self.summoned = config::summoned(&configuration);
        self.pending_nav_entry = self.summoned;
        self.own_plugin_id = Some(get_plugin_ids().plugin_id);
        // selectable はペイン側の属性でリロードしても前回の false が残り、承認
        // プロンプトにフォーカスできないデッドロックになるため、毎回戻す。
        // 承認済みなら PermissionRequestResult が即返り、すぐ false に戻る
        set_selectable(true);
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
            // navモードのキー横取り（決定12）
            PermissionType::InterceptInput,
            // 兄弟インスタンスとの状態同期（決定13）
            PermissionType::MessageAndLaunchOtherPlugins,
            // 臨時召喚（決定16）。OpenPluginPaneFloating はこれを要求し
            //（MessageAndLaunchOtherPlugins では足りない）、拒否されると
            // shim 側の unwrap でプラグインごと落ちる
            PermissionType::OpenTerminalsOrPlugins,
            // プレビューのスナップショット取得（決定42）。拒否されてもパニック
            // せず「取れなかった」扱い（preview::UNAVAILABLE）に落ちる
            PermissionType::ReadPaneContents,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::Visible,
            EventType::InterceptedKeyPress,
            // 行クリックでのフォーカス移動（要件: docs/requirements/click-to-focus/）
            EventType::Mouse,
            // 配置演出のフレーム送り（要件: docs/requirements/header-animation/）。
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
        //（決定42）。一覧の再構築も権威判定も要らないうえ、走らせると兄弟
        // インスタンスとして状態の配布に混ざってしまう
        if self.is_preview {
            return self.update_as_preview(event);
        }
        let should_render = match event {
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                if self.permissions_granted {
                    // フローティングで起動されていたら召喚インスタンスとして自覚する
                    //（決定16。下の set_selectable の分岐に効くので、ここより前に）
                    self.adopt_floating_as_summoned();
                    // フォーカス巡回にサイドバーを混ぜない（決定6）。**臨時召喚は
                    // 例外**（決定16）— unselectable だとロジックが壊れたとき普段の
                    // ペイン操作で消せなくなるので、「必ず自分で消せる」ほうを取る
                    if !self.summoned {
                        set_selectable(false);
                    }
                    // 既定のペイン名はwasmのフルURLで長すぎるので短くする
                    if let Some(id) = self.own_plugin_id {
                        rename_plugin_pane(id, "fujin");
                    }
                    // ここより前に来た PaneUpdate は未承認として捨てている。
                    // 承認を待たずに一覧が揃うと入場の機会がここしか無い
                    self.learn_own_plugin_url();
                    self.enter_nav_mode_if_pending();
                    // 警告の時計はここから回す（決定40）。承認が済むまでフッターは
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
                // コマンド状態の導出は既読モデルより先（決定32）。導出した状態が
                // その場で既読になってしまう問題は、既読の猶予
                //（`CommandInfo::awaiting_refocus`）が防ぐ
                self.apply_command_states(&manifest);
                self.apply_read_model(&manifest);
                self.panes = Some(manifest);
                self.rebuild_selectable();
                self.prune_stale_agents();
                // 自分のURLは PaneManifest で初めて分かる。新しい兄弟
                // インスタンスを見つけたら状態を配る（決定13）
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
                    clear_key_presses_intercepts();
                }
                // プレビューと預かったフォーカスの後始末。どちらも対象は自分では
                // なく別のペインなので、閉じられている最中でも投げてよい。
                // フォーカスを返さないと、リロード時にユーザーは unselectable に
                // 戻ったサイドバーにフォーカスを残して詰まる（決定34・決定42）
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
                // 横取りを要求したインスタンスにしか届かないが、念のため
                if !self.nav_mode {
                    return false;
                }
                self.handle_nav_key(key)
            }
            _ => false,
        };
        // 入力欄の実カーソル（IMEの候補窓が付いてくる）はここで伝える。
        // **`render()` の中からは呼べない**（`sync_input_cursor` 参照）
        self.sync_input_cursor();
        should_render
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
                | DISMISS_PIPE
        );
        // CLI pipe は即座にunblockしないと送信側が1秒タイムアウトまで待たされ、
        // フックのレイテンシに直結する（実測でroute.rsのタイムアウトを確認済み）
        if is_ours && matches!(pipe_message.source, PipeSource::Cli(_)) {
            unblock_cli_pipe_input(&pipe_message.name);
        }
        // プレビュー用フローティングペイン（決定42）が扱うのはスナップショットだけ。
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
            READ_CLEAR_PIPE => self.handle_read_clear_pipe(payload),
            COMMAND_STATE_PIPE => self.handle_command_state_pipe(payload),
            SYNC_STATE_PIPE => self.handle_sync_state_pipe(payload),
            _ => false,
        };
        // navモードへの入場は pipe 経由でも起きる（fujin_mode）ので、
        // 実カーソルの追従は update と同じくこちらでも行う
        self.sync_input_cursor();
        should_render
    }

    fn render(&mut self, rows: usize, cols: usize) {
        // 表示範囲の寄せ直しは描画の直前に行う。画面高が分かるのがここだけで、
        // 行の増減も選択の移動もまとめて吸収できる
        self.reconcile_viewport(rows);
        // 配置演出のタイマーは描画の外で進むので、幅を控えておく
        self.viewport_cols = cols;
        self.draw(rows, cols);
    }
}

impl State {
    // プレビュー用フローティングペイン（決定42）のイベント処理。
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
            // 臨時召喚（決定16）と同じ理由で、一時的に出ているだけのペインは
            // 「必ず自分で消せる」ほうを取る
            if let Some(id) = self.own_plugin_id {
                rename_plugin_pane(id, "preview");
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
            set_timeout(deploy::FRAME_INTERVAL);
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

    // 設定の警告の表示期限を切る（決定40）。フッターを通常表示へ戻すために
    // 1回だけ描き直しが要るので、切れた瞬間を返す
    fn expire_config_warning(&mut self) -> bool {
        match self.config_warning_until {
            Some(until) if self.elapsed >= until => {
                self.config_warning_until = None;
                true
            }
            _ => false,
        }
    }

    // 設定の警告を出し始める。警告が無ければ何もしない（タイマーも張らない）
    pub(crate) fn arm_config_warning(&mut self) {
        if self.config_warnings.is_empty() {
            return;
        }
        self.config_warning_until = Some(self.elapsed + CONFIG_WARNING_SECS);
        self.arm_timer();
    }

    // いまフッターに設定の警告を出しているか（決定40）。
    //
    // **ユーザーがいま操作している文脈は警告より優先する** — 入力欄
    //（検索クエリ・番号ジャンプ）と確認プロンプト（終了操作）、ヘルプの
    // 閉じ方は、そこに出ていないと操作が成立しない。警告が譲るのは静的な
    // ヒント（nav・トリアージの help/exit、direct-keys）に対してだけで、
    // 譲っているあいだも期限は進む — 見せ場を作るために操作を待たせない
    pub(crate) fn showing_config_warning(&self) -> bool {
        self.config_warning_until.is_some()
            && !self.help_overlay
            && self.termination.is_none()
            && self.search.is_none()
            && self.jump.is_none()
    }

    // 自分が可視インスタンスか（決定14の権威判定のうち、**副作用のない部分だけ**）。
    //
    // `refresh_focus()` を流用しないのは、あちらが選択の追従・navモード退場という
    // 副作用を持つため — 状態通知が届いただけで探索位置を動かすわけにはいかない。
    // `self.visible` を第一手にしないのも refresh_focus と同じ理由（リロードで
    // `Event::Visible` が再送されない）。問い合わせに失敗したときだけ旗に落ちる
    pub(crate) fn is_visible_instance(&self) -> bool {
        let Ok((focused_tab, _)) = get_focused_pane_info() else {
            return self.visible;
        };
        self.owns_tab(focused_tab)
    }

    // フォーカス情報をサーバへ1回だけ問い合わせて、
    //  - 観測したフォーカスを取り込み、navモード外なら選択行を追従させる
    //    （要件: docs/requirements/focus-sync/）
    //  - 自分が操作の権威を持つインスタンスか（決定14）を返す
    //
    // イベントの配送は権威判定に当てにできない — PaneUpdate / TabUpdate は
    // 非可視インスタンスに届かず「自分のタブがアクティブ」が複数現れ（実測）、
    // Event::Visible はリロードで再送されない。get_focused_pane_info() への
    // 直接問い合わせなら常に最新で、権威になるインスタンスは1つだけになる
    fn refresh_focus(&mut self) -> bool {
        let Ok((focused_tab, focused_pane)) = get_focused_pane_info() else {
            // 問い合わせに失敗したときだけ Visible に落とす
            return self.visible;
        };
        if !self.owns_tab(focused_tab) {
            // フォーカス中のタブに居ないインスタンスは選択を自分では動かさない。
            // 動かすのは権威1つだけで、兄弟へは決定13の同期で配られる
            return false;
        }
        self.focus_on_terminal = matches!(focused_pane, PaneId::Terminal(_));
        // プレビュー用フローティングペインは自分の一部として数える（決定42）。
        // 別ペイン扱いにすると、開いた直後の移動を「持って行かれた」と誤読して
        // navモードを抜けてしまう
        let own_pane_focused = match (focused_pane, self.own_plugin_id) {
            (PaneId::Plugin(id), Some(own_id)) => id == own_id || self.is_preview_pane(id),
            _ => false,
        };
        // 預かりが成立したのを観測しておく（決定34。`park_taken_over` が使う）
        if own_pane_focused {
            if let Some(parked) = self.focus_parked.as_mut() {
                parked.confirmed = true;
            }
        }
        // 預けたフォーカスをユーザーの操作で持って行かれたら、奪い返さずに手放す
        let park_lost = self.park_taken_over(own_pane_focused);
        if park_lost {
            self.release_parked_focus(false);
        }
        let focused = match focused_pane {
            PaneId::Terminal(id) => Some(id),
            // フォーカスを預かっている間（決定34）は預かった当のペインを指す。
            // 一覧から拾い直すと「タイル層でフォーカス中のターミナル」が居らず
            // None になり、探索位置も戻し先も失う
            PaneId::Plugin(_) if own_pane_focused && self.focus_parked.is_some() => self
                .focus_parked
                .map(|parked| parked.pane_id)
                .or_else(|| self.focused_terminal_in_tab(focused_tab)),
            // 他のプラグインペインは selectable に無いので、一覧から作業ペインを
            // 拾い直す（召喚インスタンス自身がフォーカスを持つ場合がこれ）
            PaneId::Plugin(_) => self.focused_terminal_in_tab(focused_tab),
        };
        // 可視化直後の1回は、キャッシュと同じフォーカスでも引き直す
        let force = std::mem::take(&mut self.pending_focus_resync);
        let follow = self.focus_to_follow(focused, force);
        // navモード中に実フォーカスが動いた＝探索をやめて作業に戻ったとみなして
        // 退場する（要件: nav-mode / focus-sync）。主な経路はマウスでのペイン選択で、
        // 横取りを解かないと移動先で j/k が食われ続ける。預けたフォーカスを
        // 持って行かれた場合（`park_lost`）も同じ扱い — 行き先が預かる前と同じ
        // ペインでも「作業に戻る」なので、ペインIDの比較だけでは取りこぼす
        let interrupted =
            self.nav_mode && (park_lost || (focused.is_some() && focused != self.focused_pane));
        // 記録は追従しない場合も続ける。navモード退場時の「動いたか」の比較材料
        self.focused_pane = focused;
        if interrupted {
            // 預かりの手放しは上（`park_lost`）で済んでいる。ここで返す形にすると
            // ユーザーが自分で選んだ先からフォーカスを奪い返してしまう
            self.leave_nav_mode();
        } else if let Some(pane_id) = follow {
            if self.select_pane_id(pane_id) {
                self.broadcast_selection();
            }
        }
        true
    }

    // 選択を引き直す先（要件: docs/requirements/focus-sync/）。
    // `force` は可視化直後など、キャッシュを信用できないときに立てる
    pub(crate) fn focus_to_follow(&self, focused: Option<u32>, force: bool) -> Option<u32> {
        // navモード中の選択はユーザーの探索位置なので追従させない
        if self.nav_mode {
            return None;
        }
        let pane_id = focused?;
        // **フォーカスが動いたときだけ**引き直す。PaneUpdate はペイン名の変化
        // でも飛んでくるので、毎回引き直すと fujin_up / fujin_down で動かした
        // 選択が勝手に戻ってしまう
        if !force && focused == self.focused_pane {
            return None;
        }
        Some(pane_id)
    }

    // 指定タブでフォーカス中のターミナルペイン（要件: focus-sync）。
    //
    // `get_focused_pane_info()` がプラグインペインを返したときの受け皿。
    // 召喚インスタンス（臨時召喚・コールドスタートの両経路。決定16）は**自分が
    // フローティング層のフォーカスを持つ**ため、問い合わせでは作業ペインが
    // 分からず、追従も入場時の初期選択も効かなくなる（実測）。
    //
    // `PaneInfo.is_focused` は**レイヤごと**の意味（`data.rs:2302`
    // "focused in its layer"）なので、フローティングが前面にあってもタイル層の
    // フォーカスは一覧に残っている。作業ペインは通常タイルなのでそちらを優先し、
    // 無ければフローティングのターミナルを拾う
    pub(crate) fn focused_terminal_in_tab(&self, tab_position: usize) -> Option<u32> {
        let panes = self.panes.as_ref()?.panes.get(&tab_position)?;
        let mut floating = None;
        for pane in panes {
            if pane.is_plugin || pane.is_suppressed || !pane.is_focused {
                continue;
            }
            if !pane.is_floating {
                return Some(pane.id);
            }
            floating = Some(pane.id);
        }
        floating
    }

    // 預けたフォーカスをユーザーの操作で持って行かれたか（決定34）。
    // `own_pane_focused` は「いま実フォーカスが自分のペインにあるか」の観測結果。
    //
    // **`confirmed` を待つのが要点。** フォーカスの移動は非同期なので、
    // 預けた命令が処理される前に届いた `PaneUpdate` では自分にフォーカスが無い。
    // 確認を待たずに判定すると、入場した直後に「持って行かれた」と誤読して退場する
    pub(crate) fn park_taken_over(&self, own_pane_focused: bool) -> bool {
        self.focus_parked.is_some_and(|parked| parked.confirmed) && !own_pane_focused
    }

    // 指定ターミナルペインがフローティングか（決定34）。フォーカスを預かるとき、
    // 戻すための `should_float_if_hidden` を控えておくのに使う。
    // 一覧に無ければ false ＝ タイル扱い（`focus_selected` の既定と同じ）
    pub(crate) fn pane_is_floating(&self, pane_id: u32) -> bool {
        self.selectable
            .iter()
            .find(|entry| entry.pane_id == pane_id)
            .map(|entry| entry.is_floating)
            .unwrap_or(false)
    }

    // configuration を取り込む（決定40）。**設定の入口はここ1本だけ**にして、
    // 値の正規化と解釈できない値の扱いを項目ごとにばらけさせない。
    // 解釈と仕様の正本は config.rs 側にある。
    //
    // direct-keys のヒント（決定28）が「キーの表記だけを設定で受け取る」形なのは、
    // zellij 0.44.3 のプラグインAPIが `Action::KeybindPipe` の `name`/`payload` を
    // 捨てて渡すため、実際の割り当てを `Event::InitialKeybinds` から解決できないから
    //（`zellij-utils/src/plugin_api/action.rs`）
    pub(crate) fn apply_config(&mut self, configuration: &BTreeMap<String, String>) {
        let config = Config::parse(configuration);
        self.show_cwd = config.show_cwd;
        self.show_deploy_animation = config.show_deploy_animation;
        self.direct_keys = config.direct_keys;
        self.config_warnings = config.warnings;
        // フッターは幅32でキー名しか出せない。何が悪かったのかを追える形は
        // ログ側に残す（開発時の出力先は docs/dev/dev-workflow.md 参照）
        for key in &self.config_warnings {
            eprintln!(
                "fujin: unusable value for `{}` in the plugin configuration",
                key
            );
        }
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
