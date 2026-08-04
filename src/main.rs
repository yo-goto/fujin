// fujin — zellij用サイドバープラグイン
//
// タブ > ペインの縦並び表示、エージェント状態の可視化、グローバルキーでのジャンプ。
// 設計決定は docs/concept/design-decisions.md を参照。
//
// アーキテクチャ上の前提（すべて実測で確認済み。docs/dev/api-reference.md 参照）:
// - タブ数ぶんのインスタンスが同時稼働する（zellijの構造上回避不能）
// - pipe は全インスタンスに配送される
// - **PaneUpdate / TabUpdate は可視インスタンスにしか届かない。**
//   バックグラウンドのインスタンスはタブ・ペイン一覧が古いままになり、
//   「自分のタブがアクティブ」と思い込む
// - Event::Visible は全インスタンスに届き、true になるのは常に1つだけ
//
// モジュール構成:
// - agent  — フックイベントの解釈とエージェント状態の遷移
// - nav    — navモード（決定12）と検索サブモードのキー操作
// - search — ファジーマッチの純粋ロジック
// - render — サイドバーの描画
// - sync   — インスタンス間の状態同期（決定13）
// - summon — フローティングでの臨時召喚（決定16）

mod agent;
mod nav;
mod render;
mod search;
mod summon;
mod sync;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use zellij_tile::prelude::*;

use agent::{AgentInfo, StatusPayload};
use nav::SearchState;

// --- ワイヤプロトコル（pipe名） ---

// フックからの状態通知
const STATUS_PIPE: &str = "fujin_status";
// キーバインドからのナビゲーション
const NAV_UP_PIPE: &str = "fujin_up";
const NAV_DOWN_PIPE: &str = "fujin_down";
const NAV_GO_PIPE: &str = "fujin_go";
// navモードへの入場（zellijのモードキーと同じ使い勝手）
const NAV_MODE_PIPE: &str = "fujin_mode";
// インスタンス間の状態同期（決定13）
const SYNC_STATE_PIPE: &str = "fujin_sync_state";
// 既読クリアの他インスタンスへの伝播（決定13）
const READ_CLEAR_PIPE: &str = "fujin_read";
// 選択位置の他インスタンスへの伝播（決定13）
const SELECTION_PIPE: &str = "fujin_selection";
// 取り残された臨時召喚の強制掃除（決定16）。navモードへ入れないまま
// 取り残された召喚はキーを横取りしておらず Esc が届かない。zellij 側にも
// ペインIDを指定して閉じる手段が無い（`close-pane` はフォーカス中のみ、
// fujin は unselectable でフォーカスできない）ため、逃げ道を用意しておく
const DISMISS_PIPE: &str = "fujin_dismiss";

// 臨時召喚されたインスタンスに渡す configuration キー（決定16）。
// "true" で起動したインスタンスは、準備でき次第 navモードへ入る
const SUMMONED_CONFIG_KEY: &str = "summoned";

// サイドバーに並べる選択対象（ターミナルペイン1つぶん）
#[derive(Debug, Clone)]
struct Selectable {
    tab_position: usize,
    pane_id: u32,
    title: String,
    // フォーカス時の should_float_if_hidden に使う（決定14）
    is_floating: bool,
}

#[derive(Default)]
struct State {
    tabs: Vec<TabInfo>,
    panes: Option<PaneManifest>,
    session_name: Option<String>,
    // key: ターミナルペインID
    agents: BTreeMap<u32, AgentInfo>,
    // フラット化した選択対象
    selectable: Vec<Selectable>,
    selected: usize,
    visible: bool,
    own_plugin_id: Option<u32>,
    // 自分のwasm URL。実行時に判明する（同期の宛先・召喚の起動元に使う）
    own_plugin_url: Option<String>,
    permissions_granted: bool,
    show_cwd: bool,
    // ペインID -> cwd（フックのペイロード由来）
    pane_cwds: BTreeMap<u32, String>,
    // navモード中か。全キーを横取りしているインスタンスだけが true になる
    nav_mode: bool,
    // 既に把握している兄弟インスタンスのプラグインID（同期の押し付け先判定）
    known_siblings: BTreeSet<u32>,
    // 臨時召喚された（フローティングの）インスタンスか（決定16）
    summoned: bool,
    // 準備が整い次第 navモードへ入る予約。召喚直後は権限も一覧も未取得で、
    // その時点で入場しても選択対象が空なので、揃うまで待ってから入る
    pending_nav_entry: bool,
    // 自分が召喚したフローティングの、タブindex -> プラグインID（決定16）。
    // 重ねて召喚しないための記録。一覧では代用できない（summon.rs 参照）
    summoned_panes: BTreeMap<usize, u32>,
    // 検索サブモード中か否かは is_some() で表す。
    // フラグとクエリが食い違う状態を作らせない
    search: Option<SearchState>,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.show_cwd = configuration
            .get("show_cwd")
            .map(|v| v == "true")
            .unwrap_or(false);
        // 別インスタンスに召喚されたフローティングなら、入場pipeを取り逃している。
        // 押し直させずに済むよう、準備でき次第こちらから navモードへ入る（決定16）
        self.summoned = configuration
            .get(SUMMONED_CONFIG_KEY)
            .map(|v| v == "true")
            .unwrap_or(false);
        self.pending_nav_entry = self.summoned;
        self.own_plugin_id = Some(get_plugin_ids().plugin_id);
        // selectable はペイン側の属性で、リロードしても前回の false が残る。
        // 権限を追加した新版をリロードすると「承認プロンプトは出ているのに
        // そのペインにフォーカスできない」デッドロックになるため、毎回戻す。
        // 承認済みなら PermissionRequestResult が即返り、すぐ false に戻る。
        set_selectable(true);
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
            // navモードでキーを横取りするため（決定12）
            PermissionType::InterceptInput,
            // 他インスタンスとの状態同期のため（決定13）
            PermissionType::MessageAndLaunchOtherPlugins,
            // フローティングでの臨時召喚のため（決定16）。
            // OpenPluginPaneFloating はこれを要求し（MessageAndLaunchOtherPlugins
            // では足りない）、拒否されると shim 側の unwrap でプラグインごと落ちる
            PermissionType::OpenTerminalsOrPlugins,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::ModeUpdate,
            EventType::PermissionRequestResult,
            EventType::Visible,
            EventType::InterceptedKeyPress,
            // プラグイン終了・リロード時に横取りを解除する保険
            EventType::BeforeClose,
        ]);
        // 注意: set_selectable(false) はここでは呼ばない。呼ぶと権限承認
        // プロンプトにフォーカスできず承認不能になる。
        // PermissionRequestResult 受信後に呼ぶ（上の set_selectable(true) と対）。
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                if self.permissions_granted {
                    // フォーカス巡回にサイドバーが混ざらないようにする（決定6）。
                    //
                    // **臨時召喚は例外**（決定16）。unselectable なペインは
                    // フォーカスできず、zellij にはペインIDを指定して閉じる
                    // 手段が無いので、プラグイン側のロジックが壊れると
                    // ユーザーには消す手段が一つも無くなる（実測でそうなった）。
                    // 一時的に出ているだけなので「必ず自分で消せる」ほうを取る
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
                }
                true
            }
            Event::Visible(visible) => {
                self.visible = visible;
                visible
            }
            Event::ModeUpdate(mode_info) => {
                let changed = self.session_name != mode_info.session_name;
                self.session_name = mode_info.session_name;
                changed
            }
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                self.rebuild_selectable();
                self.enter_nav_mode_if_pending();
                true
            }
            Event::PaneUpdate(manifest) => {
                self.apply_read_model(&manifest);
                self.panes = Some(manifest);
                self.rebuild_selectable();
                self.prune_stale_agents();
                // 自分のURLは PaneManifest で初めて分かる。新しい兄弟
                // インスタンスを見つけたら状態を配る（決定13）
                self.learn_own_plugin_url();
                self.push_state_to_new_siblings();
                self.enter_nav_mode_if_pending();
                true
            }
            Event::BeforeClose => {
                // 横取りしたままプラグインが消えるとキー入力が戻らなくなる。
                // exit_nav_mode() は使わない。閉じられている最中に自分を
                // close_plugin_pane() すると二重解放になるため
                if self.nav_mode {
                    self.nav_mode = false;
                    clear_key_presses_intercepts();
                }
                false
            }
            Event::InterceptedKeyPress(key) => {
                // 横取りを要求したインスタンスにしか届かないが、念のため
                if !self.nav_mode {
                    return false;
                }
                self.handle_nav_key(key)
            }
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let is_ours = matches!(
            pipe_message.name.as_str(),
            STATUS_PIPE
                | NAV_UP_PIPE
                | NAV_DOWN_PIPE
                | NAV_GO_PIPE
                | NAV_MODE_PIPE
                | SYNC_STATE_PIPE
                | READ_CLEAR_PIPE
                | SELECTION_PIPE
                | DISMISS_PIPE
        );
        // CLIパイプは即座にunblockしないと送信側が1秒タイムアウトまで待たされ、
        // フックのレイテンシに直結する（実測でroute.rsのタイムアウトを確認済み）
        if is_ours && matches!(pipe_message.source, PipeSource::Cli(_)) {
            unblock_cli_pipe_input(&pipe_message.name);
        }
        match pipe_message.name.as_str() {
            STATUS_PIPE => {
                if let Some(raw) = pipe_message.payload.as_deref() {
                    if let Some(payload) = StatusPayload::parse(raw) {
                        self.apply_status(payload);
                        return true;
                    }
                    eprintln!("fujin: unparsable status payload: {}", raw);
                }
                false
            }
            // 選択を動かすのは可視インスタンスだけ（決定14）。全員が自前で
            // 動かすと、一覧が古いインスタンスでは境界判定とクランプの結果が
            // 違って選択がずれる
            NAV_UP_PIPE => {
                if !self.is_authoritative() {
                    return false;
                }
                self.selected = self.selected.saturating_sub(1);
                self.broadcast_selection();
                true
            }
            NAV_DOWN_PIPE => {
                if !self.is_authoritative() {
                    return false;
                }
                if self.selected + 1 < self.selectable.len() {
                    self.selected += 1;
                }
                self.broadcast_selection();
                true
            }
            NAV_GO_PIPE => {
                // 副作用は可視インスタンスのみ実行（多重発行の防止・決定14）
                if self.is_authoritative() {
                    self.focus_selected();
                }
                false
            }
            DISMISS_PIPE => {
                // 召喚された本人は自分で退場し、常駐インスタンスは
                // 取り残された召喚を代わりに閉じる（決定16）
                if self.summoned {
                    self.exit_nav_mode();
                } else {
                    self.dismiss_stranded_summons();
                }
                false
            }
            SELECTION_PIPE => {
                // 選択はインデックスではなく**ペインIDで**運ぶ。インデックスは
                // 各インスタンスの selectable に依存し、一覧が古いインスタンス
                // では別の行を指してしまうため
                let target: Option<u32> = pipe_message
                    .payload
                    .as_deref()
                    .and_then(|p| p.trim().parse().ok());
                if let Some(target) = target {
                    if let Some(index) = self.selectable.iter().position(|e| e.pane_id == target) {
                        let changed = self.selected != index;
                        self.selected = index;
                        return changed;
                    }
                }
                false
            }
            READ_CLEAR_PIPE => {
                // 可視インスタンスが観測した既読クリアを取り込む
                let mut changed = false;
                if let Some(raw) = pipe_message.payload.as_deref() {
                    for pane_id in raw.split(',').filter_map(|s| s.trim().parse::<u32>().ok()) {
                        if let Some(agent) = self.agents.get_mut(&pane_id) {
                            changed |= agent.mark_read();
                        }
                    }
                }
                changed
            }
            SYNC_STATE_PIPE => {
                // 空のときだけ取り込む。既に自前の状態を持っているなら、
                // 古いダンプで上書きしてしまわないよう無視する
                if self.agents.is_empty() {
                    if let Some(raw) = pipe_message.payload.as_deref() {
                        self.apply_state_dump(raw);
                        return true;
                    }
                }
                false
            }
            NAV_MODE_PIPE => {
                // キーの横取りは権威インスタンス1つだけが行う。全員が
                // intercept_key_presses() を呼ぶと誰が受け取るか不定になる。
                //
                // なお臨時召喚されたインスタンスにはこの pipe が届かない。
                // キーバインドの `MessagePlugin` はURL一致で配送されるが、
                // 召喚は configuration に `summoned=true` を持つため一致しない
                //（実測: 受信ログが一切出ない）。トグルは本人ではなく
                // 召喚役が担う（決定16）
                if self.is_authoritative() {
                    if !self.nav_mode {
                        self.enter_nav_mode();
                        return true;
                    }
                    return false;
                }
                // ここへ来たインスタンスはフォーカス中のタブに居ない。そのタブに
                // fujin が1つも無ければ権威がどこにも立たず、pipe が届いても
                // 無反応になる。代表1つがフローティングで召喚して穴を埋める（決定16）
                self.summon_floating_if_absent();
                false
            }
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.draw(rows, cols);
    }
}

impl State {
    // 操作の権威を持つインスタンスか（決定14）。
    //
    // イベントの配送は当てにできない:
    // - PaneUpdate / TabUpdate は非可視インスタンスに届かないため、
    //   「自分のタブがアクティブ」なインスタンスが複数現れる（実測）
    // - Event::Visible は真が常に1つだけで正確だが、**プラグインを
    //   リロードすると再送されない**（zellij から見て可視状態は不変でも、
    //   プラグインの状態は初期化される）
    //
    // そこでサーバへ直接問い合わせる。get_focused_pane_info() は
    // 「このプラグインのクライアントにとっての」フォーカス中のタブを返すので、
    // 常に最新かつ、真になるインスタンスは1つだけになる。
    fn is_authoritative(&self) -> bool {
        let Some(own_id) = self.own_plugin_id else {
            return false;
        };
        let Some(manifest) = &self.panes else {
            return false;
        };
        let Ok((focused_tab, _)) = get_focused_pane_info() else {
            // 問い合わせに失敗したときだけ Visible に落とす
            return self.visible;
        };
        // 自分のプラグインペインがフォーカス中のタブにいるか。
        // manifest が古くても、自分のペインの所属タブは動かないので判定できる
        manifest
            .panes
            .get(&focused_tab)
            .map(|panes| panes.iter().any(|p| p.is_plugin && p.id == own_id))
            .unwrap_or(false)
    }
}
