// エージェント状態の管理。
//
// 各ペインで動くエージェントはフックから STATUS_PIPE 経由でイベントを
// 送ってくる。ここではペイロードの解釈と、状態遷移・既読化・破棄を扱う。

use std::collections::BTreeSet;

use zellij_tile::prelude::*;

use crate::{State, READ_DELAY};

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
    // 状態アイコン凡例（決定25）に並べる順。緊急度（`triage_rank`）ではなく
    // 起動→実行→待ち→完了という素直な遷移の順にする。凡例は「どの記号が何か」を
    // 引くための表なので、優先度の主張はしない
    pub(crate) const ALL: [AgentState; 5] = [
        AgentState::Idle,
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Done,
        AgentState::Error,
    ];

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            AgentState::Idle => "○",
            AgentState::Working => "»",
            AgentState::Blocked => "◆",
            AgentState::Done => "●",
            // `✕` は東アジア文字幅が曖昧で右へずれるため `×` を使う
            AgentState::Error => "×",
        }
    }

    // 状態アイコン凡例に出す説明。状態名そのものを見せる（ユビキタス言語の
    // 決まりどおり、エージェント状態は英字のまま訳さない）。ワイヤ表現
    // （`as_str`）と字面は同じだが、片方を変えても他方は追従しない
    pub(crate) fn label(&self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Done => "done",
            AgentState::Error => "error",
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

    // トリアージモードの優先度階層（要件: docs/requirements/triage-mode/）。
    // 小さいほど緊急。`idle`（既読）はトリアージ一覧に出さないので None を返す
    pub(crate) fn triage_rank(&self) -> Option<u8> {
        match self {
            // 放置すると誰にも気づかれないまま止まり続けるので最上位
            AgentState::Error => Some(0),
            AgentState::Blocked => Some(1),
            AgentState::Working => Some(2),
            AgentState::Done => Some(3),
            AgentState::Idle => None,
        }
    }

    // 注意を引く状態か（要件: sidebar-header の待ち件数）。既読化（決定10）が
    // `idle` に戻す対象と同じ集合で、`working` は数えない — 走っている最中の
    // ペインは人の対応を待っていない
    pub(crate) fn is_waiting(&self) -> bool {
        matches!(
            self,
            AgentState::Done | AgentState::Blocked | AgentState::Error
        )
    }

    // Textのcolor_rangeレベル（テーマの強調色 0-3 とレベル6の error_color）。
    // 5状態が重複しない色を持ち、色だけで判別できるようにしてある（決定25）
    pub(crate) fn color(&self) -> usize {
        match self {
            AgentState::Idle => 0,
            AgentState::Working => 2,
            AgentState::Blocked => 3,
            AgentState::Done => 1,
            // `blocked` とレベル3で重複していたのを、テーマのエラー色へ移した
            AgentState::Error => 6,
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
    // 直近にエージェント状態が変わったときのシーケンス番号（要件: triage-mode）。
    // トリアージ一覧の同一階層内で「どちらが後に変わったか」だけを比べるための値で、
    // 壁時計は使わない — pipe が受信順に処理されるという既存の前提だけで足りる
    pub(crate) state_change_seq: u64,
}

impl AgentInfo {
    // 既読化（決定10）: 注意を引く状態（done/blocked/error）を idle に戻す。
    // 戻したら true
    pub(crate) fn mark_read(&mut self) -> bool {
        if !self.state.is_waiting() {
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
    // `SessionStart` にだけ付く起動理由（`startup` / `resume` / `clear` /
    // `compact` / `fork`）。配置演出のトリガー判定に使う（`deploy::detect_new_agent`）。
    // 古いフックスクリプトは送ってこないので None を許す
    pub(crate) source: Option<String>,
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
            source: get_str("source"),
            cwd: get_str("cwd"),
            detail: get_str("detail"),
        })
    }
}

impl State {
    // フックからの状態通知を適用。**新規エージェント検出になったら true**
    //（要件: header-animation）。
    //
    // 配置演出を出すかどうかまではここで決めない — 通知は全インスタンスへ配送されるので
    // 可視インスタンス判定を通す必要があるが、その問い合わせはホスト関数でテストから
    // 呼べない（docs/dev/build-and-test.md）。判定は呼び出し元（`main.rs` の
    // 状態通知ハンドラ）に置き、ここは通知の解釈だけに徹する
    pub(crate) fn apply_status(&mut self, payload: StatusPayload) -> bool {
        if let Some(cwd) = &payload.cwd {
            self.pane_cwds.insert(payload.pane_id, cwd.clone());
        }
        // 新規エージェント検出（`deploy::detect_new_agent`）。`entry` の可変借用が
        // 生きている間は self のメソッドを呼べないので、判定だけ先に済ませておく
        let mut detected = false;
        // シーケンス番号を振るのは**エージェント状態が実際に変わったとき**だけ。
        // カウンタだけが動くイベント（TaskCreated 等）で番号を進めると、
        // トリアージ一覧の同一階層内が「直近の状態変化順」でなくなる
        let before = self.agents.get(&payload.pane_id).map(|a| a.state);
        let entry = self.agents.entry(payload.pane_id).or_default();
        entry.agent = payload.agent;
        match payload.event.as_str() {
            "SessionStart" => {
                entry.state = AgentState::Idle;
                entry.subagents = 0;
                entry.open_tasks = 0;
                entry.turn_ended = false;
                detected = crate::deploy::detect_new_agent(payload.source.as_deref());
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
                // 走っている間は working のままにする。
                //
                // `error` は上書きしない。StopFailure に続けて Stop が届く可能性が
                // あり、上書きするとエラーで落ちたターンが done に見えてしまう。
                //
                // `blocked` を守らないのは意図的（`is_waiting()` でまとめない）。
                // `error` はターンが失敗して終わった**終端**の状態なのに対し、
                // `blocked` はターン中の応答待ちという**過渡**の状態で、Stop は
                // 「もう待っていない」を意味する。permission_prompt に承認した後は
                // ツールが再開するだけで UserPromptSubmit は発火しないので、ここで
                // 倒さないと正常に終わったターンが blocked のまま固着する
                if entry.subagents == 0 && entry.state != AgentState::Error {
                    entry.state = AgentState::Done;
                }
            }
            "StopFailure" => entry.state = AgentState::Error,
            // ツール単体の失敗では状態を変えない。PostToolUseFailure は存在しない
            // ファイルへの Read 程度でも発火し、その後リカバリしてターンが正常に
            // 終わることのほうが多い。error にすると誤警報が常態化する。
            // 既存のフック設定から届きうるので unknown 扱いにはしない
            "PostToolUseFailure" => {}
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
        if self.agents.get(&payload.pane_id).map(|a| a.state) != before {
            self.bump_state_seq(payload.pane_id);
        }
        detected
    }

    // 状態が変わったペインに新しいシーケンス番号を振る。
    // 既読化（mark_read）では振らない — 戻る先は `idle` で、トリアージ一覧から
    // 消える状態なので、順序の比較材料としては使われない
    fn bump_state_seq(&mut self, pane_id: u32) {
        self.state_seq += 1;
        let seq = self.state_seq;
        if let Some(entry) = self.agents.get_mut(&pane_id) {
            entry.state_change_seq = seq;
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
        // いまフォーカスされている作業ペイン。既読の対象であると同時に、
        // ここに居ないコマンドペインは「ユーザーが離れた」ことの観測になる
        let focused: BTreeSet<u32> = panes
            .iter()
            .filter(|pane| {
                !pane.is_plugin
                    && !pane.is_suppressed
                    && pane.is_focused
                    // フローティング層を表示中は、その層のフォーカスのみ有効
                    && pane.is_floating == active_tab.are_floating_panes_visible
            })
            .map(|pane| pane.id)
            .collect();
        // 既読の猶予（決定32）を解くのはフォーカスから外れているコマンドペイン
        // だけなので、下の保留ループとは対象が重ならない（同じフレームで解いて
        // 既読にする、という取りこぼしは起きない）
        self.release_read_grace(&focused);
        // フォーカスを外れたペインの保留は捨てる。**通過しただけのペインは
        // 滞在猶予が満ちる前に必ずここへ来る**ので、注意を引く状態はそのまま残る
        //（決定37。docs/issues/transit-focus-clears-read-state.md）
        self.pending_reads
            .retain(|pane_id, _| focused.contains(pane_id));
        for pane_id in focused {
            // 倒せる状態を持たないペインに保留を作らない。何も起きないのに
            // タイマーだけが回り続けるのを避ける
            if !self.has_unread(pane_id) {
                continue;
            }
            // 既に保留があるなら期限は延ばさない。フォーカスしたまま
            // PaneUpdate が届くたびに期限が先送りされると、留まっていても
            // いつまでも既読にならない
            self.pending_reads
                .entry(pane_id)
                .or_insert(self.elapsed + READ_DELAY);
        }
        if !self.pending_reads.is_empty() {
            self.arm_timer();
        }
    }

    // 既読にできる状態を持っているか（`mark_read` が何かを倒せるか）。
    // コマンド状態の再フォーカス待ち（決定32の猶予）は、解けるまで倒せないので
    // 持っていない扱いにする
    fn has_unread(&self, pane_id: u32) -> bool {
        let agent = self
            .agents
            .get(&pane_id)
            .is_some_and(|agent| agent.state.is_waiting());
        let command = self
            .commands
            .get(&pane_id)
            .is_some_and(|info| info.is_unread() && !info.awaiting_refocus);
        agent || command
    }

    // 滞在猶予が満ちた保留を既読にする（決定10・決定37）。再描画が要るかを返す。
    //
    // ここまで来たペインは、滞在猶予のあいだフォーカスされ続けていた
    // ＝通過点ではなく目的地だったとみなす
    pub(crate) fn apply_pending_reads(&mut self) -> bool {
        let due: Vec<u32> = self
            .pending_reads
            .iter()
            .filter(|(_, deadline)| **deadline <= self.elapsed)
            .map(|(pane_id, _)| *pane_id)
            .collect();
        if due.is_empty() {
            return false;
        }
        let mut cleared = Vec::new();
        for pane_id in due {
            self.pending_reads.remove(&pane_id);
            // コマンド状態も同じ既読モデルに乗る（決定32）。エージェント登録が
            // あるペインはそちらが優先されて表示に出ないが、両方を既読にしても
            // 実害は無いので、ソースを気にせず倒す
            let mut was_cleared = false;
            if let Some(agent) = self.agents.get_mut(&pane_id) {
                was_cleared |= agent.mark_read();
            }
            if let Some(info) = self.commands.get_mut(&pane_id) {
                was_cleared |= info.mark_read();
            }
            if was_cleared {
                cleared.push(pane_id);
            }
        }
        let cleared_any = !cleared.is_empty();
        // 非可視インスタンスには PaneUpdate が届かず、状態はイベント駆動なので
        // 見逃した変化は永久にずれたままになる（実測: サイドバー3つのセッションで
        // クリアを実行したのは1つだけ）。観測できた可視インスタンスから
        // 兄弟インスタンスへ配る
        self.broadcast_read_clears(&cleared);
        cleared_any
    }

    // 対応を待っているペインの数。エージェント状態とコマンド状態の両方を数える
    //（決定32）。
    //
    // 一覧に出るペイン（`selectable`）だけを数える。状態の入れ物を直接数えないのは、
    // 閉じたペインの状態が prune されるまでの一瞬、画面に無いものを数えてしまうため。
    //
    // **いまは表示の受け皿が無い。** 決定27でヘッダーの待ち件数表示を廃止したが、
    // 概念としては残すと決めた（決定32でコマンド状態も含む形に広げた）
    #[allow(dead_code)] // 上記の理由で、呼び出し元が無くても残す
    pub(crate) fn waiting_count(&self) -> usize {
        self.selectable
            .iter()
            .filter(|entry| {
                self.pane_status(entry.pane_id)
                    .is_some_and(|status| status.is_waiting())
            })
            .count()
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
        // コマンド状態は「コマンドペインでなくなったら捨てる」まで見る。
        // ペインは残っていてもコマンドペインでなくなることがある（実際には
        // 起こらないが、判定条件を導出側と揃えておく）
        let live_commands = crate::command::live_command_ids(manifest);
        self.prune_stale_commands(&live_commands);
    }
}
