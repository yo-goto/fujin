// エージェント状態の管理。
//
// 各ペインで動くエージェントはフックから STATUS_PIPE 経由でイベントを
// 送ってくる。ここではペイロードの解釈と、状態遷移・既読化・破棄を扱う。

use std::collections::BTreeSet;

use zellij_tile::prelude::*;

use crate::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AgentState {
    #[default]
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

impl AgentState {
    pub(crate) fn icon(&self) -> &'static str {
        match self {
            AgentState::Idle => "○",
            AgentState::Working => "»",
            AgentState::Blocked => "◆",
            AgentState::Done => "●",
            AgentState::Error => "✕",
        }
    }

    // インスタンス間同期のワイヤ表現（決定13）
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Done => "done",
            AgentState::Error => "error",
        }
    }

    pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "done" => AgentState::Done,
            "error" => AgentState::Error,
            _ => AgentState::Idle,
        }
    }

    // Textのcolor_rangeレベル（テーマの強調色 0-3）
    pub(crate) fn color(&self) -> usize {
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
pub(crate) struct AgentInfo {
    pub(crate) state: AgentState,
    pub(crate) agent: String,
    // 稼働中サブエージェント数（SubagentStart/Stopで増減）
    pub(crate) subagents: usize,
    // 未完了タスク数（TaskCreated/Completedで増減）
    pub(crate) open_tasks: usize,
    // ターン終了済みか。バックグラウンドのサブエージェントは Stop より後まで
    // 走るので、最後の SubagentStop で done にしてよいかの判定に要る
    pub(crate) turn_ended: bool,
    // blocked時の通知メッセージ等
    pub(crate) detail: Option<String>,
}

impl AgentInfo {
    // 既読化（決定10）: 注意を引く状態（done/blocked/error）を idle に戻す。
    // 戻したら true
    pub(crate) fn mark_read(&mut self) -> bool {
        if !matches!(
            self.state,
            AgentState::Done | AgentState::Blocked | AgentState::Error
        ) {
            return false;
        }
        self.state = AgentState::Idle;
        self.detail = None;
        true
    }
}

// フックから送られてくるJSONペイロード
#[derive(Debug)]
pub(crate) struct StatusPayload {
    pub(crate) pane_id: u32,
    pub(crate) event: String,
    pub(crate) agent: String,
    pub(crate) cwd: Option<String>,
    pub(crate) detail: Option<String>,
}

impl StatusPayload {
    // 依存を増やさないため手書きの最小JSONパース。
    // フック側スクリプトが生成する平坦なJSONのみ想定する。
    pub(crate) fn parse(raw: &str) -> Option<Self> {
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

impl State {
    // フックからの状態通知を適用
    pub(crate) fn apply_status(&mut self, payload: StatusPayload) {
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
                entry.turn_ended = false;
            }
            "UserPromptSubmit" => {
                entry.state = AgentState::Working;
                entry.turn_ended = false;
            }
            "Notification" => {
                entry.state = AgentState::Blocked;
                entry.detail = payload.detail;
            }
            "Stop" => {
                entry.turn_ended = true;
                // バックグラウンドで起動したサブエージェントはターンを待たせないので、
                // Stop の時点でまだ走っていることがある（実測トレース:
                // SubagentStart → Stop → …数十秒後… → SubagentStop）。
                // 走っている間は working のままにする
                if entry.subagents == 0 {
                    entry.state = AgentState::Done;
                }
            }
            "StopFailure" | "PostToolUseFailure" => entry.state = AgentState::Error,
            "SessionEnd" => {
                self.agents.remove(&payload.pane_id);
            }
            "SubagentStart" => entry.subagents += 1,
            "SubagentStop" => {
                entry.subagents = entry.subagents.saturating_sub(1);
                // ターンが終わった後に残っていた最後の1つが終わったら done
                if entry.subagents == 0 && entry.turn_ended && entry.state == AgentState::Working {
                    entry.state = AgentState::Done;
                }
            }
            "TaskCreated" => entry.open_tasks += 1,
            "TaskCompleted" => entry.open_tasks = entry.open_tasks.saturating_sub(1),
            other => {
                eprintln!("fujin: unknown event: {}", other);
            }
        }
    }

    // 既読モデル（決定10）: フォーカスされたペインの done/blocked/error を idle に戻す
    pub(crate) fn apply_read_model(&mut self, manifest: &PaneManifest) {
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
            // フローティング層を表示中は、その層のフォーカスのみ有効
            if pane.is_floating != active_tab.are_floating_panes_visible {
                continue;
            }
            if let Some(agent) = self.agents.get_mut(&pane.id) {
                if agent.mark_read() {
                    cleared.push(pane.id);
                }
            }
        }
        // 非可視インスタンスには PaneUpdate が届かず、状態はイベント駆動なので
        // 見逃した変化は永久にずれたままになる（実測: サイドバー3つのセッションで
        // クリアを実行したのは1つだけ）。観測できた可視インスタンスから
        // 兄弟インスタンスへ配る
        self.broadcast_read_clears(&cleared);
    }

    // 閉じられたペインの状態を破棄
    pub(crate) fn prune_stale_agents(&mut self) {
        let Some(manifest) = &self.panes else {
            return;
        };
        let live: BTreeSet<u32> = manifest
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
