// agent-spaces — zellij用サイドバープラグイン
//
// タブ > ペインの縦並び表示、エージェント状態の可視化、グローバルキーでのジャンプ。
// 設計決定は docs/04-design-decisions.md を参照。
//
// アーキテクチャ上の前提:
// - タブ数ぶんのインスタンスが同時稼働する（zellijの構造上回避不能）
// - pipe は全インスタンスに配送されるため、選択状態は自然に同期する
// - 副作用（フォーカス移動・OS問い合わせ）は可視インスタンスのみが実行する

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

// ワイヤプロトコル: フックからの状態通知
const STATUS_PIPE: &str = "agent_spaces_status";
// ワイヤプロトコル: キーバインドからのナビゲーション
const NAV_UP_PIPE: &str = "agent_spaces_up";
const NAV_DOWN_PIPE: &str = "agent_spaces_down";
const NAV_GO_PIPE: &str = "agent_spaces_go";

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
                let end = rest.find(|c: char| c == ',' || c == '}')?;
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

#[derive(Default)]
struct State {
    tabs: Vec<TabInfo>,
    panes: Option<PaneManifest>,
    session_name: Option<String>,
    // key: ターミナルペインID
    agents: BTreeMap<u32, AgentInfo>,
    // フラット化した選択対象（tab_position, pane_id, pane_title）
    selectable: Vec<(usize, u32, String)>,
    selected: usize,
    visible: bool,
    own_plugin_id: Option<u32>,
    permissions_granted: bool,
    show_cwd: bool,
    // ペインID -> cwd（フックのペイロード由来）
    pane_cwds: BTreeMap<u32, String>,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.show_cwd = configuration
            .get("show_cwd")
            .map(|v| v == "true")
            .unwrap_or(false);
        self.own_plugin_id = Some(get_plugin_ids().plugin_id);
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::ReadCliPipes,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::ModeUpdate,
            EventType::PermissionRequestResult,
            EventType::Visible,
        ]);
        // 注意: set_selectable(false) はここでは呼ばない。
        // 呼ぶと権限承認プロンプトにフォーカスできず承認不能になる。
        // PermissionRequestResult 受信後に呼ぶ。
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                if self.permissions_granted {
                    // フォーカス巡回にサイドバーが混ざらないようにする（決定6）
                    set_selectable(false);
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
                true
            }
            Event::PaneUpdate(manifest) => {
                self.apply_read_model(&manifest);
                self.panes = Some(manifest);
                self.rebuild_selectable();
                self.prune_stale_agents();
                true
            }
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let is_ours = matches!(
            pipe_message.name.as_str(),
            STATUS_PIPE | NAV_UP_PIPE | NAV_DOWN_PIPE | NAV_GO_PIPE
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
                    eprintln!("agent-spaces: unparsable status payload: {}", raw);
                }
                false
            }
            NAV_UP_PIPE => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                true
            }
            NAV_DOWN_PIPE => {
                if self.selected + 1 < self.selectable.len() {
                    self.selected += 1;
                }
                true
            }
            NAV_GO_PIPE => {
                // 副作用はアクティブタブのインスタンスのみ実行（多重発行の防止）
                if self.is_active_instance() {
                    if let Some((_, pane_id, _)) = self.selectable.get(self.selected) {
                        focus_pane_with_id(PaneId::Terminal(*pane_id), false, false);
                    }
                }
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
        // ヘッダ: セッション名
        if let Some(name) = &self.session_name {
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
            let title = truncate(&format!("{} {} {}", marker, tab.position + 1, tab.name), cols);
            let mut text = Text::new(&title);
            if tab.active {
                text = text.color_range(0, ..title.chars().count());
            }
            print_text_with_coordinates(text, 0, y, None, None);
            y += 1;

            // ペイン行
            for (tab_position, pane_id, pane_title) in &self.selectable {
                if *tab_position != tab.position {
                    continue;
                }
                if y >= rows {
                    break;
                }
                let agent = self.agents.get(pane_id);
                let icon = agent.map(|a| a.state.icon()).unwrap_or(" ");
                let mut label = format!("  {} {}", icon, pane_title);
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
                let label = truncate(&label, cols);
                let mut text = Text::new(&label);
                if let Some(a) = agent {
                    // アイコン部分（先頭2..3文字目）に状態色
                    text = text.color_range(a.state.color(), 2..3);
                }
                if flat_index == self.selected {
                    text = text.selected();
                }
                print_text_with_coordinates(text, 0, y, None, None);
                y += 1;
                flat_index += 1;
            }
        }
    }
}

impl State {
    // 自分のペインがアクティブタブにあるか。
    // タブ数ぶんのインスタンスのうち副作用（OS問い合わせ・フォーカス移動）を
    // 実行してよいのは1つだけ、の判定に使う。全インスタンスが同じデータを
    // 持つため判定結果は矛盾しない。Visible イベントは初回配送が保証されて
    // いる確証がないため、こちらを正とする。
    fn is_active_instance(&self) -> bool {
        let Some(own_id) = self.own_plugin_id else {
            return false;
        };
        let Some(manifest) = &self.panes else {
            return false;
        };
        let Some(active_tab) = self.tabs.iter().find(|t| t.active) else {
            return false;
        };
        manifest
            .panes
            .get(&active_tab.position)
            .map(|panes| panes.iter().any(|p| p.is_plugin && p.id == own_id))
            .unwrap_or(false)
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
                    self.selectable
                        .push((position, pane.id, pane.title.clone()));
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
                eprintln!("agent-spaces: unknown event: {}", other);
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
                }
            }
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
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
