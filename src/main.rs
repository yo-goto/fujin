// fujin — zellij用サイドバープラグイン
//
// タブ > ペインの縦並び表示、エージェント状態の可視化、グローバルキーでのジャンプ。
// 設計決定は docs/04-design-decisions.md を参照。
//
// アーキテクチャ上の前提（すべて実測で確認済み。docs/02-api-reference.md 参照）:
// - タブ数ぶんのインスタンスが同時稼働する（zellijの構造上回避不能）
// - pipe は全インスタンスに配送される
// - **PaneUpdate / TabUpdate は可視インスタンスにしか届かない。**
//   バックグラウンドのインスタンスはタブ・ペイン一覧が古いままになり、
//   「自分のタブがアクティブ」と思い込む
// - Event::Visible は全インスタンスに届き、true になるのは常に1つだけ。
//   したがって**唯一の権威は self.visible**（決定14）

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

#[cfg(test)]
mod tests;

// ワイヤプロトコル: フックからの状態通知
const STATUS_PIPE: &str = "fujin_status";
// ワイヤプロトコル: キーバインドからのナビゲーション
const NAV_UP_PIPE: &str = "fujin_up";
const NAV_DOWN_PIPE: &str = "fujin_down";
const NAV_GO_PIPE: &str = "fujin_go";
// ワイヤプロトコル: navモードへの入場（zellijのモードキーと同じ使い勝手）
const NAV_MODE_PIPE: &str = "fujin_mode";
// ワイヤプロトコル: インスタンス間の状態同期（決定13）
const SYNC_STATE_PIPE: &str = "fujin_sync_state";
// ワイヤプロトコル: 既読クリアの他インスタンスへの伝播（決定13）
const READ_CLEAR_PIPE: &str = "fujin_read";
// ワイヤプロトコル: 選択位置の他インスタンスへの伝播（決定13）
const SELECTION_PIPE: &str = "fujin_selection";
// ワイヤプロトコル: 臨時召喚されたインスタンスの強制退場（決定16）。
// 通常は Esc で自分から閉じるが、navモードへ入れないまま取り残された
// 召喚は**キー入力の横取りをしていないので Esc が届かない**。
// zellij 側にペインIDを指定して閉じる手段が無い（`close-pane` は
// フォーカス中のみ、fujin は unselectable でフォーカスできない）ため、
// 掃除の逃げ道をプラグイン側に用意しておく
const DISMISS_PIPE: &str = "fujin_dismiss";
// 臨時召喚されたインスタンスに渡す configuration キー（決定16）。
// これが "true" で起動したインスタンスは、準備でき次第 navモードへ入る
const SUMMONED_CONFIG_KEY: &str = "summoned";
// 召喚するフローティングの幅。レイアウトの常駐サイドバー（size=32）に合わせる
const SIDEBAR_WIDTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AgentState {
    #[default]
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

impl AgentState {
    fn icon(&self) -> &'static str {
        match self {
            AgentState::Idle => "○",
            AgentState::Working => "»",
            AgentState::Blocked => "◆",
            AgentState::Done => "●",
            AgentState::Error => "✕",
        }
    }

    // インスタンス間同期のワイヤ表現（決定13）
    fn as_str(&self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Done => "done",
            AgentState::Error => "error",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "done" => AgentState::Done,
            "error" => AgentState::Error,
            _ => AgentState::Idle,
        }
    }

    // Textのcolor_rangeレベル（テーマの強調色 0-3）
    fn color(&self) -> usize {
        match self {
            AgentState::Idle => 0,
            AgentState::Working => 2,
            AgentState::Blocked => 3,
            AgentState::Done => 1,
            AgentState::Error => 3,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct AgentInfo {
    state: AgentState,
    agent: String,
    // 稼働中サブエージェント数（SubagentStart/Stopで増減、Stopで0リセット）
    subagents: usize,
    // 未完了タスク数（TaskCreated/Completedで増減）
    open_tasks: usize,
    // blocked時の通知メッセージ等
    detail: Option<String>,
}

// フックから送られてくるJSONペイロード
#[derive(Debug)]
struct StatusPayload {
    pane_id: u32,
    event: String,
    agent: String,
    cwd: Option<String>,
    detail: Option<String>,
}

impl StatusPayload {
    // 依存を増やさないため手書きの最小JSONパース。
    // フック側スクリプトが生成する平坦なJSONのみ想定する。
    fn parse(raw: &str) -> Option<Self> {
        let get_str = |key: &str| -> Option<String> {
            let pat = format!("\"{}\":", key);
            let start = raw.find(&pat)? + pat.len();
            let rest = raw[start..].trim_start();
            if let Some(stripped) = rest.strip_prefix('"') {
                let end = stripped.find('"')?;
                Some(stripped[..end].to_string())
            } else {
                let end = rest.find([',', '}'])?;
                Some(rest[..end].trim().to_string())
            }
        };
        Some(StatusPayload {
            pane_id: get_str("pane_id")?.parse().ok()?,
            event: get_str("event")?,
            agent: get_str("agent").unwrap_or_else(|| "unknown".to_string()),
            cwd: get_str("cwd"),
            detail: get_str("detail"),
        })
    }
}

// サイドバーに並べる選択対象（ターミナルペイン1つぶん）
#[derive(Debug, Clone)]
struct Selectable {
    tab_position: usize,
    pane_id: u32,
    title: String,
    // フォーカス時の should_float_if_hidden の値に使う（決定14）
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
    permissions_granted: bool,
    show_cwd: bool,
    // ペインID -> cwd（フックのペイロード由来）
    pane_cwds: BTreeMap<u32, String>,
    // navモード中か。全キーを横取りしているインスタンスだけが true になる
    nav_mode: bool,
    // 自分のwasm URL。実行時に判明する（同期の宛先・召喚の起動元に使う）
    own_plugin_url: Option<String>,
    // 既に把握している兄弟インスタンスのプラグインID（同期の押し付け先判定）
    known_siblings: std::collections::BTreeSet<u32>,
    // 臨時召喚された（フローティングの）インスタンスか（決定16）
    summoned: bool,
    // 準備が整い次第 navモードへ入る予約。召喚直後は権限も一覧も未取得で、
    // その時点で入場しても選択対象が空なので、一覧が揃うまで待ってから入る
    pending_nav_entry: bool,
    // 自分が召喚したフローティングの、タブindex -> プラグインID（決定16）。
    // 重ねて召喚しないための記録。一覧では代用できない（下記 summon 参照）
    summoned_panes: BTreeMap<usize, u32>,
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
            // MessageAndLaunchOtherPlugins では足りない。OpenPluginPaneFloating は
            // これを要求し、拒否されると shim 側の unwrap でプラグインごと落ちる
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
        // 注意: set_selectable(false) はここでは呼ばない。
        // 呼ぶと権限承認プロンプトにフォーカスできず承認不能になる。
        // PermissionRequestResult 受信後に呼ぶ（上の set_selectable(true) と対）。
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                if self.permissions_granted {
                    // フォーカス巡回にサイドバーが混ざらないようにする（決定6）
                    set_selectable(false);
                    // 既定のペイン名はwasmのフルURL（`(.) - file:/…/fujin.wasm`）
                    // で長すぎるので、プラグイン名だけにする
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
                // ここでは exit_nav_mode() を使わない。閉じられている最中に
                // 自分を close_plugin_pane() すると二重解放になるため
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
            NAV_UP_PIPE => {
                // 選択を動かすのは可視インスタンスだけ（決定14）。
                // 全員が自前で動かすと、一覧が古いインスタンスでは
                // 境界判定とクランプの結果が違って選択がずれる。
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
            DISMISS_PIPE => {
                // 召喚された本人は自分で退場する
                if self.summoned {
                    self.exit_nav_mode();
                    return false;
                }
                // 常駐インスタンスは取り残された召喚を代わりに閉じる。
                // navモードへ入れなかった召喚は横取りをしていないので Esc が
                // 届かず、zellij 側にもペインIDを指定して閉じる手段が無い
                //（`close-pane` はフォーカス中のみ、fujin は unselectable で
                // フォーカス巡回にも乗らない）。常駐サイドバーはタイル（決定5）
                // なので、同じURLのフローティング＝召喚と見なせる
                self.dismiss_stranded_summons();
                false
            }
            NAV_GO_PIPE => {
                // 副作用は可視インスタンスのみ実行（多重発行の防止・決定14）
                if self.is_authoritative() {
                    self.focus_selected();
                }
                false
            }
            SELECTION_PIPE => {
                // 選択はインデックスではなく**ペインIDで**運ぶ。
                // インデックスは各インスタンスの selectable に依存し、
                // 一覧が古いインスタンスでは別の行を指してしまうため。
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
                            if matches!(
                                agent.state,
                                AgentState::Done | AgentState::Blocked | AgentState::Error
                            ) {
                                agent.state = AgentState::Idle;
                                agent.detail = None;
                                changed = true;
                            }
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
                // キーの横取りは1インスタンスだけが行う。全インスタンスが
                // intercept_key_presses() を呼ぶと誰が受け取るか不定になるため。
                if self.is_authoritative() {
                    if !self.nav_mode {
                        self.enter_nav_mode();
                        return true;
                    }
                    return false;
                }
                // ここへ来たインスタンスはフォーカス中のタブに居ない。
                // そのタブに fujin が1つも無ければ権威を持つインスタンスが
                // どこにも存在せず、pipe は届いているのに無反応になる。
                // 代表1つがフローティングで召喚して穴を埋める（決定16）。
                self.summon_floating_if_absent();
                false
            }
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if !self.permissions_granted {
            print_text_with_coordinates(
                Text::new("permissions required (press y)"),
                0,
                0,
                None,
                None,
            );
            return;
        }
        let mut y = 0;
        // ヘッダ: セッション名（navモード中はモード名に置き換える）
        if self.nav_mode {
            let header = truncate("-- NAV --  j/k ↵ esc", cols);
            print_text_with_coordinates(
                Text::new(&header).color_range(3, ..header.chars().count()),
                0,
                y,
                None,
                None,
            );
            y += 1;
        } else if let Some(name) = &self.session_name {
            let header = truncate(name, cols);
            print_text_with_coordinates(
                Text::new(&header).color_range(2, ..header.chars().count()),
                0,
                y,
                None,
                None,
            );
            y += 1;
        }

        let mut flat_index = 0;
        let mut sorted_tabs: Vec<&TabInfo> = self.tabs.iter().collect();
        sorted_tabs.sort_by_key(|t| t.position);
        for tab in sorted_tabs {
            if y >= rows {
                break;
            }
            // タブ見出し
            let marker = if tab.active { "▾" } else { "▸" };
            let title = truncate(
                &format!("{} {} {}", marker, tab.position + 1, tab.name),
                cols,
            );
            let mut text = Text::new(&title);
            if tab.active {
                text = text.color_range(0, ..title.chars().count());
            }
            print_text_with_coordinates(text, 0, y, None, None);
            y += 1;

            // ペイン行
            for entry in &self.selectable {
                let (pane_id, pane_title) = (&entry.pane_id, &entry.title);
                if entry.tab_position != tab.position {
                    continue;
                }
                if y >= rows {
                    break;
                }
                let is_selected = flat_index == self.selected;
                let agent = self.agents.get(pane_id);
                let icon = agent.map(|a| a.state.icon()).unwrap_or(" ");
                // 選択行は左端にバーを立てる。テーマの選択色が沈む配色でも
                // どこが選択中か一目で分かるようにするため（幅は2文字で固定し、
                // アイコンの color_range 2..3 をずらさない）
                let prefix = if is_selected { "▌ " } else { "  " };
                let mut label = format!("{}{} {}", prefix, icon, pane_title);
                if let Some(a) = agent {
                    if a.subagents > 0 {
                        label.push_str(&format!(" +{}", a.subagents));
                    }
                    if a.open_tasks > 0 {
                        label.push_str(&format!(" [{}]", a.open_tasks));
                    }
                }
                if self.show_cwd {
                    if let Some(cwd) = self.pane_cwds.get(pane_id) {
                        label.push_str(&format!("  {}", cwd));
                    }
                }
                let mut label = truncate(&label, cols);
                if is_selected {
                    // 選択背景がサイドバー幅いっぱいに伸びるよう空白で埋める。
                    // 埋めないと文字列の長さぶんしか色が乗らず、帯に見えない
                    let pad = cols.saturating_sub(label.chars().count());
                    label.push_str(&" ".repeat(pad));
                }
                let mut text = Text::new(&label);
                if let Some(a) = agent {
                    // アイコン部分（先頭2..3文字目）に状態色
                    text = text.color_range(a.state.color(), 2..3);
                }
                if is_selected {
                    // opaque を付けないと背景が透けて選択色が沈む
                    text = text.selected().opaque().color_range(2, 0..1);
                }
                print_text_with_coordinates(text, 0, y, None, None);
                y += 1;
                flat_index += 1;
            }
        }
    }
}

impl State {
    // 操作の権威を持つインスタンスか（決定14）。
    //
    // イベントの配送は当てにできない:
    // - PaneUpdate / TabUpdate は「タブがアクティブなインスタンス」にしか
    //   届かないので、バックグラウンドのインスタンスは自分のタブがまだ
    //   アクティブだと思い込む（実測で複数が同時に真になった）
    // - Event::Visible は真が常に1つだけで正確だが、**プラグインを
    //   リロードすると再送されない**（zellij から見て可視状態は変化して
    //   いないが、プラグインの状態は初期化される）
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
        // manifest が古くても、自分のペインの所属タブは動かないので判定できる。
        manifest
            .panes
            .get(&focused_tab)
            .map(|panes| panes.iter().any(|p| p.is_plugin && p.id == own_id))
            .unwrap_or(false)
    }

    // --- 臨時召喚（決定16） ---
    //
    // フォーカス中のタブに fujin が1つも居ないと、`is_authoritative()` が
    // 真になるインスタンスがどこにも存在せず、入場pipeが届いているのに
    // 誰も横取りを始めない（無反応に見える）。
    //
    // レイアウト側だけでは穴を塞ぎきれない。既存セッションの復活、
    // セッションマネージャ経由でのタブ作成、別レイアウトの指定、
    // ユーザーがサイドバーを閉じた場合はいずれも fujin の居ないタブを作る。
    // そこで代表1つがフローティングで自分を召喚する。
    //
    // 常駐をフローティングに変えるわけではない（決定5は維持）。タイルは
    // 幅を返す代わりに下のペインを欠けさせないので長時間の常駐に向くが、
    // ここでは一時的に出すだけなので重なっても構わない。ピン留めもしない。
    fn summon_floating_if_absent(&mut self) {
        let (Some(own_id), Some(own_url)) = (self.own_plugin_id, self.own_plugin_url.clone())
        else {
            eprintln!("fujin: summon skipped (own id/url unknown)");
            return;
        };
        let Some(manifest) = self.panes.as_ref() else {
            eprintln!("fujin: summon skipped (no pane manifest)");
            return;
        };
        let Ok((focused_tab, _)) = get_focused_pane_info() else {
            eprintln!("fujin: summon skipped (focused pane query failed)");
            return;
        };
        // 自分が前に召喚したものがまだ生きていれば重ねない。
        //
        // 一覧では判定できない。召喚役は多くの場合フォーカス中のタブに
        // 居ない＝非可視で、**非可視のインスタンスには PaneUpdate が届かない**
        // ため、自分が召喚したペインすら一覧に載らない。実測では入場のたびに
        // 積み上がって7枚溜まった。`get_pane_info()` はサーバへの問い合わせな
        // ので、記録したIDの生存確認には使える。
        if let Some(previous) = self.summoned_panes.get(&focused_tab) {
            if get_pane_info(PaneId::Plugin(*previous)).is_some() {
                eprintln!("fujin: summon skipped (tab {focused_tab} already has a summon)");
                return;
            }
        }
        // そのタブに兄弟が居るなら、そいつが権威を持つので任せる。
        // manifest が古くて取りこぼしても、二重に出るだけで操作不能にはならない。
        let already_present = manifest
            .panes
            .get(&focused_tab)
            .map(|panes| {
                panes
                    .iter()
                    .any(|p| p.is_plugin && p.plugin_url.as_deref() == Some(own_url.as_str()))
            })
            .unwrap_or(false);
        // 居るのに権威が立たなかった＝ manifest のずれ。実装を疑う手がかりになる
        if already_present {
            eprintln!(
                "fujin: summon skipped (tab {} already has fujin)",
                focused_tab
            );
            return;
        }
        // 代表でなければ黙って降りる（インスタンス数ぶん出るとノイズになる）
        if !self.is_summon_delegate(own_id, &own_url) {
            return;
        }
        eprintln!("fujin: summoning floating instance into tab {focused_tab}");

        let mut config = BTreeMap::new();
        config.insert(SUMMONED_CONFIG_KEY.to_string(), "true".to_string());
        if self.show_cwd {
            config.insert("show_cwd".to_string(), "true".to_string());
        }
        let summoned = open_plugin_pane_floating(
            &own_url,
            config,
            Some(Self::summon_coordinates()),
            BTreeMap::new(),
        );
        // 表示への切り替えはここではやらない。召喚した側は非フォーカスの
        // タブに居るため `show_floating_panes()` が「アクティブなタブ」を
        // 特定できず、`None` でも tab index でも "Tab not found" になる（実測）。
        // ピン留めしてあるので表示状態に関係なく最前面に出る。

        let Some(PaneId::Plugin(new_id)) = summoned else {
            eprintln!("fujin: summon failed (no pane id returned)");
            return;
        };
        self.summoned_panes.insert(focused_tab, new_id);
        // **開くときに渡した座標は効かない。** 実測では pinned だけが通り、
        // x/y/width/height は既定のカスケード配置（118x30 を少しずつずらす）
        // のままだった。開いた後に指定し直すと効く。
        change_floating_panes_coordinates(vec![(
            PaneId::Plugin(new_id),
            Self::summon_coordinates(),
        )]);

        // 決定13の同期は「可視インスタンスが PaneUpdate で新入りに気づいて配る」
        // 方式だが、このタブには可視インスタンスが居ないため誰も気づけない。
        // 召喚した本人が明示的に配る。
        if !self.agents.is_empty() {
            pipe_message_to_plugin(
                MessageToPlugin::new(SYNC_STATE_PIPE)
                    .with_destination_plugin_id(new_id)
                    .with_payload(self.state_dump()),
            );
        }
    }

    // 召喚するフローティングの配置。常駐サイドバーと同じ見た目・同じ位置。
    //
    // ピン留めは必須。タブのフローティングは既定で非表示状態のため、
    // 普通に開くとペインは在るのに描画されない。`show_floating_panes()`
    // で表示に切り替える手も試したが、召喚した側でも召喚された側でも
    // "Tab not found" で失敗する（zellij 0.44.3、tab index も None も不可）。
    // ピン留めしたペインはその表示状態に関係なく最前面に出る。
    fn summon_coordinates() -> FloatingPaneCoordinates {
        let mut coordinates = FloatingPaneCoordinates::default()
            .with_x_fixed(0)
            .with_y_fixed(0)
            .with_width_fixed(SIDEBAR_WIDTH)
            .with_height_percent(100);
        coordinates.pinned = Some(true);
        coordinates
    }

    // 召喚の実行役を1つに絞る。兄弟IDの昇順で、実在する最初のIDが代表。
    //
    // 単純な最小IDだと、閉じられたペインが manifest に残っているかどうかで
    // インスタンスごとに結論が食い違う。`get_pane_info()` はサーバへの
    // 問い合わせなので、生存確認を挟めば鮮度の違いを吸収できる。
    fn is_summon_delegate(&self, own_id: u32, own_url: &str) -> bool {
        let Some(manifest) = self.panes.as_ref() else {
            return false;
        };
        let mut ids: Vec<u32> = manifest
            .panes
            .values()
            .flatten()
            .filter(|p| p.is_plugin && p.plugin_url.as_deref() == Some(own_url))
            .map(|p| p.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            if id == own_id {
                return true;
            }
            // 自分より若いインスタンスが生きていればそちらに譲る
            if get_pane_info(PaneId::Plugin(id)).is_some() {
                return false;
            }
        }
        false
    }

    // 召喚されたインスタンスの入場。入場pipeは自分の起動前に流れているので
    // 受け取れない。一覧が揃ってから入る（空のまま入ると j/k が効かない）。
    fn enter_nav_mode_if_pending(&mut self) {
        if !self.pending_nav_entry || !self.permissions_granted || self.selectable.is_empty() {
            return;
        }
        self.pending_nav_entry = false;
        if !self.nav_mode {
            self.enter_nav_mode();
        }
    }

    // --- インスタンス間の状態同期（決定13） ---
    //
    // pipe は起動中の全インスタンスに届くが、**後から起動したインスタンスは
    // それ以前のイベントを見ていない**。タブを後から作ると、そのサイドバーだけ
    // アイコンが出ない/古いという食い違いになる。
    // そこで**既存インスタンスが新入りを見つけて押し付ける**。
    //
    // 逆（新入りが要求を投げる）にしてはいけない。宛先をURLで指定する
    // `MessageToPlugin::with_plugin_url` は、起動中のインスタンスに配送されず
    // **新しいプラグインを起動しようとする**（cwd/config まで一致を要求する
    // ため、レイアウト由来のインスタンスにマッチしない）。実測でも
    // `wasm_bridge.rs:1897 Failed to load plugin` が出て、実セッションなら
    // タブを作るたびに迷子のサイドバーペインが増えるところだった。
    // 宛先をプラグインIDで直接指定すれば起動は起こらない。

    // 自分のwasm URLを知る（get_plugin_ids() には入っていない）
    fn learn_own_plugin_url(&mut self) {
        if self.own_plugin_url.is_some() {
            return;
        }
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        // 一覧から引ければそれでよいが、**非可視のインスタンスには
        // PaneUpdate が届かない**ので、それだけでは永久に埋まらない。
        // 埋まらないまま入場pipeを受けると召喚（決定16）が
        // 「own id/url unknown」で不発になる（実測）。
        // `get_pane_info()` はサーバへの問い合わせなので可視性に依らない。
        self.own_plugin_url = self
            .panes
            .as_ref()
            .and_then(|manifest| {
                manifest
                    .panes
                    .values()
                    .flatten()
                    .find(|p| p.is_plugin && p.id == own_id)
            })
            .and_then(|p| p.plugin_url.clone())
            .or_else(|| get_pane_info(PaneId::Plugin(own_id)).and_then(|p| p.plugin_url));
    }

    // 新しく現れた兄弟インスタンス（同じURLのプラグインペイン）に状態を配る。
    // 状態を持っているインスタンスは全員が送るが、受け手は空のときしか
    // 取り込まないので重複しても害はない。リーダー選出は不要。
    fn push_state_to_new_siblings(&mut self) {
        let (Some(own_id), Some(own_url), Some(manifest)) = (
            self.own_plugin_id,
            self.own_plugin_url.as_deref(),
            self.panes.as_ref(),
        ) else {
            return;
        };
        let siblings: Vec<u32> = manifest
            .panes
            .values()
            .flatten()
            .filter(|p| p.is_plugin && p.id != own_id && p.plugin_url.as_deref() == Some(own_url))
            .map(|p| p.id)
            .collect();
        let newcomers: Vec<u32> = siblings
            .iter()
            .copied()
            .filter(|id| !self.known_siblings.contains(id))
            .collect();
        self.known_siblings = siblings.into_iter().collect();
        if self.agents.is_empty() {
            return;
        }
        let dump = self.state_dump();
        for id in newcomers {
            pipe_message_to_plugin(
                MessageToPlugin::new(SYNC_STATE_PIPE)
                    .with_destination_plugin_id(id)
                    .with_payload(dump.clone()),
            );
        }
    }

    // 1ペイン1行のTSV。区切りにタブと改行を使うのは、パス（cwd）にも
    // エージェント名にも現れないため
    fn state_dump(&self) -> String {
        let mut out = String::new();
        for (pane_id, info) in &self.agents {
            let cwd = self
                .pane_cwds
                .get(pane_id)
                .map(|s| s.as_str())
                .unwrap_or("");
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                pane_id,
                info.state.as_str(),
                info.subagents,
                info.open_tasks,
                info.agent,
                cwd
            ));
        }
        out
    }

    fn apply_state_dump(&mut self, raw: &str) {
        for line in raw.lines() {
            let mut fields = line.split('\t');
            let (Some(pane_id), Some(state), Some(subagents), Some(open_tasks), Some(agent)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            let Ok(pane_id) = pane_id.parse::<u32>() else {
                continue;
            };
            self.agents.insert(
                pane_id,
                AgentInfo {
                    state: AgentState::from_str(state),
                    agent: agent.to_string(),
                    subagents: subagents.parse().unwrap_or(0),
                    open_tasks: open_tasks.parse().unwrap_or(0),
                    detail: None,
                },
            );
            if let Some(cwd) = fields.next().filter(|c| !c.is_empty()) {
                self.pane_cwds.insert(pane_id, cwd.to_string());
            }
        }
        // 既に閉じたペインの状態が混ざらないようにする
        self.prune_stale_agents();
        // 起動ごとに高々1回。食い違いを追うときの手がかりになるので残す
        eprintln!("fujin: synced {} agents from peer", self.agents.len());
    }

    // --- navモード（決定12） ---
    //
    // zellij のモード（Ctrl+p でpaneモード…）と同じ操作感を、ビルトインモードを
    // 潰さずに実現する。config.kdl には入場キー1つだけを書き、モード内のキーは
    // プラグイン側で解釈する。

    fn enter_nav_mode(&mut self) {
        self.nav_mode = true;
        // 選択位置は前回のまま引き継ぐ。以前は入場のたびに実フォーカスから
        // 引き直していたが、それはインスタンス間で選択がずれることへの
        // 対症療法で、決定13で選択位置そのものを配るようにしたので不要になった。
        // 引き直しは「作業中のペイン＝多くは自分がいる行」へ毎回選択を戻すため、
        // 前回どこまで見ていたかが失われる。
        self.broadcast_selection();
        intercept_key_presses();
    }

    fn exit_nav_mode(&mut self) {
        self.nav_mode = false;
        clear_key_presses_intercepts();
        // 臨時召喚されたインスタンスは用が済んだら自分で退場する（決定16）。
        // 残すと作業ペインに重なり続けるうえ、召喚時に表示へ切り替えた
        // フローティングの可視状態も元へ戻せない。次の入場でまた呼べばよい
        // （召喚から入場まで実測16ms）。
        if self.summoned {
            if let Some(own_id) = self.own_plugin_id {
                close_plugin_pane(own_id);
            }
        }
    }

    // 取り残された臨時召喚を閉じる（決定16）。掃除の逃げ道なので、
    // 判定材料は「同じURLのプラグイン」かつ「フローティング」だけに絞る。
    // 召喚側が発行したIDを覚えておく手もあるが、覚えている本人が
    // リロードや再起動で記憶を失うと届かなくなる
    fn dismiss_stranded_summons(&mut self) {
        let (Some(own_url), Some(manifest)) = (self.own_plugin_url.as_deref(), self.panes.as_ref())
        else {
            eprintln!("fujin: dismiss skipped (own url or pane list unknown)");
            return;
        };
        for pane in manifest.panes.values().flatten() {
            if pane.is_plugin
                && pane.is_floating
                && pane.plugin_url.as_deref() == Some(own_url)
                && Some(pane.id) != self.own_plugin_id
            {
                eprintln!("fujin: dismissing stranded summon {}", pane.id);
                close_plugin_pane(pane.id);
            }
        }
        self.summoned_panes.clear();
    }

    // モード中のキー解釈。戻り値は再描画するか。
    fn handle_nav_key(&mut self, key: KeyWithModifier) -> bool {
        // Shift は素通し（`G` が Shift付きで来る端末があるため）。
        // Ctrl/Alt/Super 付きは未定義なので抜けて安全側に倒す。
        if key.key_modifiers.iter().any(|m| *m != KeyModifier::Shift) {
            self.exit_nav_mode();
            return true;
        }
        match key.bare_key {
            BareKey::Down | BareKey::Tab | BareKey::Char('j') => {
                if self.selected + 1 < self.selectable.len() {
                    self.selected += 1;
                }
            }
            BareKey::Up | BareKey::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            BareKey::Char('g') => self.selected = 0,
            BareKey::Char('G') => {
                self.selected = self.selectable.len().saturating_sub(1);
            }
            // 1-9 で n 番目へ直行
            BareKey::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                if index < self.selectable.len() {
                    self.selected = index;
                    self.exit_nav_mode();
                    self.focus_selected();
                }
            }
            BareKey::Enter | BareKey::Char(' ') | BareKey::Char('l') => {
                // フォーカス移動でタブが変わりうるので、先に横取りを解除する
                self.exit_nav_mode();
                self.focus_selected();
            }
            // Esc / q は明示的な離脱。それ以外の未定義キーでも抜ける:
            // 万一プラグインが応答不能になってもキー入力が取り残されないため。
            _ => self.exit_nav_mode(),
        }
        // 横取り中の移動は自分にしか起きないので、都度配る
        self.broadcast_selection();
        true
    }

    fn focus_selected(&self) {
        if let Some(entry) = self.selectable.get(self.selected) {
            // 第2引数 should_float_if_hidden はターゲットに合わせて切り替える。
            // false のままだと**フローティング層が隠れているタブのフローティング
            // ペインにジャンプできない**（タブ切り替えすら起きず無反応）。
            // かといって常に true にすると、今度は**フローティング表示中に
            // タイルペインへ戻れなくなる**。どちらも実測で確認済み。
            focus_pane_with_id(PaneId::Terminal(entry.pane_id), entry.is_floating, false);
        }
    }

    // 選択対象（ターミナルペイン）のフラットリストをタブ順で再構築
    fn rebuild_selectable(&mut self) {
        self.selectable.clear();
        let Some(manifest) = &self.panes else {
            return;
        };
        let mut positions: Vec<usize> = manifest.panes.keys().copied().collect();
        positions.sort();
        for position in positions {
            if let Some(panes) = manifest.panes.get(&position) {
                for pane in panes {
                    if pane.is_plugin || pane.is_suppressed {
                        continue;
                    }
                    self.selectable.push(Selectable {
                        tab_position: position,
                        pane_id: pane.id,
                        title: pane.title.clone(),
                        is_floating: pane.is_floating,
                    });
                }
            }
        }
        if self.selected >= self.selectable.len() {
            self.selected = self.selectable.len().saturating_sub(1);
        }
    }

    // フックからの状態通知を適用
    fn apply_status(&mut self, payload: StatusPayload) {
        if let Some(cwd) = &payload.cwd {
            self.pane_cwds.insert(payload.pane_id, cwd.clone());
        }
        let entry = self.agents.entry(payload.pane_id).or_default();
        entry.agent = payload.agent;
        match payload.event.as_str() {
            "SessionStart" => {
                entry.state = AgentState::Idle;
                entry.subagents = 0;
                entry.open_tasks = 0;
            }
            "UserPromptSubmit" => entry.state = AgentState::Working,
            "Notification" => {
                entry.state = AgentState::Blocked;
                entry.detail = payload.detail;
            }
            "Stop" => {
                entry.state = AgentState::Done;
                // ターン終了時点でサブエージェントは全て終わっている
                entry.subagents = 0;
            }
            "StopFailure" | "PostToolUseFailure" => entry.state = AgentState::Error,
            "SessionEnd" => {
                self.agents.remove(&payload.pane_id);
            }
            "SubagentStart" => entry.subagents += 1,
            "SubagentStop" => entry.subagents = entry.subagents.saturating_sub(1),
            "TaskCreated" => entry.open_tasks += 1,
            "TaskCompleted" => entry.open_tasks = entry.open_tasks.saturating_sub(1),
            other => {
                eprintln!("fujin: unknown event: {}", other);
            }
        }
    }

    // 既読モデル（決定10）: フォーカスされたペインの done/blocked/error を idle に戻す
    fn apply_read_model(&mut self, manifest: &PaneManifest) {
        let Some(active_tab) = self.tabs.iter().find(|t| t.active) else {
            return;
        };
        let Some(panes) = manifest.panes.get(&active_tab.position) else {
            return;
        };
        let mut cleared = Vec::new();
        for pane in panes {
            if pane.is_plugin || pane.is_suppressed || !pane.is_focused {
                continue;
            }
            // フローティング表示中はフローティング層のフォーカスのみ有効
            if pane.is_floating != active_tab.are_floating_panes_visible {
                continue;
            }
            if let Some(agent) = self.agents.get_mut(&pane.id) {
                if matches!(
                    agent.state,
                    AgentState::Done | AgentState::Blocked | AgentState::Error
                ) {
                    agent.state = AgentState::Idle;
                    agent.detail = None;
                    cleared.push(pane.id);
                }
            }
        }
        // バックグラウンドのタブのインスタンスには PaneUpdate が届かない
        // （実測: サイドバー3つのセッションでクリアを実行したのは1つだけ）。
        // 状態はイベント駆動なので、見逃した変化は永久にずれたままになる。
        // 観測できた可視インスタンスから他へ伝える。
        self.broadcast_read_clears(&cleared);
    }

    // 選択位置を兄弟へ配る。navモード中の移動は横取り中の1インスタンスにしか
    // 起きないため、これがないとタブごとに違う行が光る。
    fn broadcast_selection(&self) {
        let Some(entry) = self.selectable.get(self.selected) else {
            return;
        };
        let pane_id = entry.pane_id;
        for sibling in &self.known_siblings {
            pipe_message_to_plugin(
                MessageToPlugin::new(SELECTION_PIPE)
                    .with_destination_plugin_id(*sibling)
                    .with_payload(pane_id.to_string()),
            );
        }
    }

    fn broadcast_read_clears(&self, pane_ids: &[u32]) {
        if pane_ids.is_empty() {
            return;
        }
        let payload = pane_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        for sibling in &self.known_siblings {
            pipe_message_to_plugin(
                MessageToPlugin::new(READ_CLEAR_PIPE)
                    .with_destination_plugin_id(*sibling)
                    .with_payload(payload.clone()),
            );
        }
    }

    // 閉じられたペインの状態を破棄
    fn prune_stale_agents(&mut self) {
        let Some(manifest) = &self.panes else {
            return;
        };
        let live: Vec<u32> = manifest
            .panes
            .values()
            .flatten()
            .filter(|p| !p.is_plugin)
            .map(|p| p.id)
            .collect();
        self.agents.retain(|id, _| live.contains(id));
        self.pane_cwds.retain(|id, _| live.contains(id));
    }
}

// 文字数ベースの単純切り詰め（v1: CJK幅は考慮しない）
fn truncate(s: &str, max: usize) -> String {
    // 幅0のときに省略記号だけがはみ出さないようにする
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
