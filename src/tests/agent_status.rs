// エージェント状態（要件: .docs/requirements/req-agent-status.md）: フック通知からの状態遷移・
// 既読モデル・滞在猶予・消えたペインの掃除

use crate::agent::AgentState;
use crate::command::CommandState;
use crate::command::PaneStatus;
use crate::test_support::*;
use crate::*;

// --- apply_status（イベント→状態の遷移） ---

#[test]
fn status_events_drive_agent_state() {
    let mut state = State::default();

    state.apply_status(status(1, "UserPromptSubmit"));
    assert_eq!(state.agents[&1].state, AgentState::Working);

    let mut notification = status(1, "Notification");
    notification.detail = Some("waiting".to_string());
    state.apply_status(notification);
    assert_eq!(state.agents[&1].state, AgentState::Blocked);
    assert_eq!(state.agents[&1].detail.as_deref(), Some("waiting"));

    state.apply_status(status(1, "StopFailure"));
    assert_eq!(state.agents[&1].state, AgentState::Error);
}

// ツール単体の失敗（PostToolUseFailure）は状態を変えない。存在しないファイルへの
// Read 程度でも発火し、その後リカバリしてターンが正常に終わることのほうが多いので、
// error にすると誤警報になる（issues/error-state-overwritten-by-stop.md）
#[test]
fn tool_failure_does_not_change_state() {
    let mut state = State::default();
    state.apply_status(status(1, "UserPromptSubmit"));
    state.apply_status(status(1, "PostToolUseFailure"));
    assert_eq!(state.agents[&1].state, AgentState::Working);

    state.apply_status(status(1, "Stop"));
    assert_eq!(state.agents[&1].state, AgentState::Done);
}

// StopFailure に続けて Stop が届いても error のまま。上書きするとエラーで落ちた
// ターンが done に見えてしまう（issues/error-state-overwritten-by-stop.md）
#[test]
fn stop_does_not_overwrite_error() {
    let mut state = State::default();
    state.apply_status(status(1, "UserPromptSubmit"));
    state.apply_status(status(1, "StopFailure"));
    state.apply_status(status(1, "Stop"));
    assert_eq!(state.agents[&1].state, AgentState::Error);

    // 次のターンが始まれば working に戻る（error から抜ける経路は既読化と
    // UserPromptSubmit の2つだけ）
    state.apply_status(status(1, "UserPromptSubmit"));
    assert_eq!(state.agents[&1].state, AgentState::Working);
}

// `blocked` は `error` と違って Stop で done に倒す。応答待ちはターン中の過渡的な
// 状態で、Stop が届いた時点でもう待っていない。ここで倒さないと、permission_prompt
// に承認して正常に終わったターンが blocked のまま固着する
// （issues/blocked-status-overwritten-by-stop.md）
#[test]
fn stop_resolves_blocked() {
    let mut state = State::default();
    state.apply_status(status(1, "UserPromptSubmit"));

    let mut notification = status(1, "Notification");
    notification.detail = Some("permission required".to_string());
    state.apply_status(notification);
    assert_eq!(state.agents[&1].state, AgentState::Blocked);

    state.apply_status(status(1, "Stop"));
    assert_eq!(state.agents[&1].state, AgentState::Done);
}

#[test]
fn subagents_are_counted_until_they_stop() {
    let mut state = State::default();
    state.apply_status(status(1, "UserPromptSubmit"));
    state.apply_status(status(1, "SubagentStart"));
    state.apply_status(status(1, "SubagentStart"));
    assert_eq!(state.agents[&1].subagents, 2);

    state.apply_status(status(1, "SubagentStop"));
    assert_eq!(state.agents[&1].subagents, 1);
    // ターンはまだ終わっていないので done にはしない
    assert_eq!(state.agents[&1].state, AgentState::Working);

    state.apply_status(status(1, "SubagentStop"));
    assert_eq!(state.agents[&1].subagents, 0);
    assert_eq!(state.agents[&1].state, AgentState::Working);

    state.apply_status(status(1, "Stop"));
    assert_eq!(state.agents[&1].state, AgentState::Done);
}

// バックグラウンドのサブエージェントを起動すると、そのターンは待たずに終わるので
// サブエージェントが走っている最中に Stop が届く（実測トレース: SessionStart →
// UserPromptSubmit → SubagentStart → Stop → …数十秒後… → SubagentStop）。
// この間ペインは working のままでなければならない
#[test]
fn background_subagent_keeps_the_pane_working() {
    let mut state = state_with_panes(1);
    state.apply_status(status(1, "SessionStart"));
    state.apply_status(status(1, "UserPromptSubmit"));
    state.apply_status(status(1, "SubagentStart"));
    state.apply_status(status(1, "Stop"));

    assert_eq!(state.agents[&1].subagents, 1);
    assert_eq!(state.agents[&1].state, AgentState::Working);

    // 走っている最中にフォーカスされても既読化で idle に落ちない
    let focused = PaneInfo {
        is_focused: true,
        ..terminal_pane(1, "pane1")
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused.clone()])]));
    assert_eq!(state.agents[&1].state, AgentState::Working);

    // サブエージェントが終わって初めて done になる
    state.apply_status(status(1, "SubagentStop"));
    assert_eq!(state.agents[&1].subagents, 0);
    assert_eq!(state.agents[&1].state, AgentState::Done);

    // done になった後はこれまで通り既読化できる
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);
    assert_eq!(state.agents[&1].state, AgentState::Idle);
}

#[test]
fn counters_do_not_underflow() {
    // 起動を見逃したインスタンスに Stop だけ届くことがある
    let mut state = State::default();
    state.apply_status(status(1, "SubagentStop"));
    state.apply_status(status(1, "TaskCompleted"));
    assert_eq!(state.agents[&1].subagents, 0);
    assert_eq!(state.agents[&1].open_tasks, 0);
}

#[test]
fn session_start_resets_counters() {
    let mut state = State::default();
    state.apply_status(status(1, "SubagentStart"));
    state.apply_status(status(1, "TaskCreated"));
    state.apply_status(status(1, "SessionStart"));
    let agent = &state.agents[&1];
    assert_eq!(agent.state, AgentState::Idle);
    assert_eq!(agent.subagents, 0);
    assert_eq!(agent.open_tasks, 0);
}

#[test]
fn session_end_drops_the_agent() {
    let mut state = State::default();
    state.apply_status(status(1, "UserPromptSubmit"));
    state.apply_status(status(1, "SessionEnd"));
    assert!(!state.agents.contains_key(&1));
}

#[test]
fn status_records_cwd() {
    let mut state = State::default();
    let mut payload = status(1, "SessionStart");
    payload.cwd = Some("/work/fujin".to_string());
    state.apply_status(payload);
    assert_eq!(
        state.pane_cwds.get(&1).map(String::as_str),
        Some("/work/fujin")
    );
}

#[test]
fn unknown_event_keeps_state() {
    let mut state = State::default();
    state.apply_status(status(1, "UserPromptSubmit"));
    state.apply_status(status(1, "SomeFutureHook"));
    assert_eq!(state.agents[&1].state, AgentState::Working);
}

// --- 既読モデル（決定202607302302） ---

#[test]
fn focusing_a_pane_marks_it_read() {
    let mut state = state_with_panes(2);
    state.apply_status(status(1, "Stop"));
    state.apply_status(status(2, "Stop"));

    let focused = PaneInfo {
        is_focused: true,
        ..terminal_pane(1, "pane1")
    };
    let manifest = manifest(vec![(0, vec![focused, terminal_pane(2, "pane2")])]);
    state.apply_read_model(&manifest);
    settle_read(&mut state);

    assert_eq!(state.agents[&1].state, AgentState::Idle);
    // フォーカスしていないペインは既読にしない
    assert_eq!(state.agents[&2].state, AgentState::Done);
}

// --- 滞在猶予（.docs/issues/issue-transit-focus-clears-read-state.md） ---
//
// zellijネイティブのペイン移動（`Alt+矢印` 等）はキー1打ごとに実フォーカスを
// 確定させるので、目的地までに経由したペインにも本物のフォーカスが一瞬当たる。
// 通過と到着はフォーカスの有無だけでは区別できないため、滞在時間で分ける

#[test]
fn passing_through_a_pane_keeps_its_state() {
    let mut state = state_with_panes(3);
    state.apply_status(status(2, "Stop"));

    // 経由: ペイン2に一瞬フォーカスが当たる
    state.apply_read_model(&manifest(vec![(
        0,
        vec![
            terminal_pane(1, "pane1"),
            focused(terminal_pane(2, "pane2")),
            terminal_pane(3, "pane3"),
        ],
    )]));
    assert_eq!(
        state.agents[&2].state,
        AgentState::Done,
        "フォーカスした瞬間には既読にしない"
    );

    // 猶予が満ちる前に目的地のペイン3へ抜ける
    state.apply_read_model(&manifest(vec![(
        0,
        vec![
            terminal_pane(1, "pane1"),
            terminal_pane(2, "pane2"),
            focused(terminal_pane(3, "pane3")),
        ],
    )]));
    settle_read(&mut state);
    assert_eq!(
        state.agents[&2].state,
        AgentState::Done,
        "通過しただけのペインの状態は残る"
    );
}

#[test]
fn passing_through_a_finished_command_pane_keeps_its_state() {
    // コマンド状態も同じ既読モデルに乗る（決定202608072218）ので、猶予も同じく効く
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);

    state.apply_read_model(&manifest(vec![(
        0,
        vec![focused(exited_command_pane(1, "make", Some(0)))],
    )]));
    // 猶予が満ちる前に離れる
    state.apply_read_model(&manifest(vec![(
        0,
        vec![
            exited_command_pane(1, "make", Some(0)),
            focused(terminal_pane(2, "zsh")),
        ],
    )]));
    settle_read(&mut state);

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Done)),
        "通過しただけのコマンドペインの状態も残る"
    );
}

#[test]
fn the_read_grace_does_not_slide_while_focused() {
    // フォーカスしたまま PaneUpdate が何度も届いても期限は先送りしない。
    // 延ばすと、留まっているのにいつまでも既読にならない
    let mut state = state_with_panes(1);
    state.apply_status(status(1, "Stop"));
    let there = manifest(vec![(0, vec![focused(terminal_pane(1, "pane1"))])]);

    state.apply_read_model(&there);
    let deadline = state.pending_reads[&1];
    state.elapsed += READ_DELAY / 2.0;
    state.apply_read_model(&there);

    assert_eq!(state.pending_reads[&1], deadline, "期限は据え置き");
}

#[test]
fn the_timer_chain_stops_once_nothing_is_pending() {
    // 猶予が済めば鎖は切れる（静かなときにタイマーを回し続けない）
    let mut state = state_with_panes(1);
    state.apply_status(status(1, "Stop"));
    state.apply_read_model(&manifest(vec![(
        0,
        vec![focused(terminal_pane(1, "pane1"))],
    )]));
    assert!(state.timer_armed, "保留があるあいだは鎖を繋ぐ");

    state.on_timer(READ_DELAY);

    assert_eq!(state.agents[&1].state, AgentState::Idle, "留まったので既読");
    assert!(state.pending_reads.is_empty());
    assert!(!state.timer_armed, "保留が無くなれば鎖は切れる");
}

#[test]
fn a_pane_without_anything_to_read_gets_no_grace() {
    // 倒せる状態が無いペインに保留を作ると、何も起きないのにタイマーだけが回る
    let mut state = state_with_panes(1);
    state.apply_status(status(1, "UserPromptSubmit"));
    state.apply_read_model(&manifest(vec![(
        0,
        vec![focused(terminal_pane(1, "pane1"))],
    )]));

    assert!(state.pending_reads.is_empty(), "working は既読の対象外");
    assert!(!state.timer_armed);
}

#[test]
fn read_model_ignores_the_hidden_layer() {
    // フローティング層が隠れているとき、その層のフォーカスは既読にならない
    let mut state = state_with_panes(1);
    state.apply_status(status(1, "Notification"));

    let floating = PaneInfo {
        is_focused: true,
        is_floating: true,
        ..terminal_pane(1, "pane1")
    };
    state.apply_read_model(&manifest(vec![(0, vec![floating])]));
    assert_eq!(state.agents[&1].state, AgentState::Blocked);
}

#[test]
fn read_model_does_nothing_without_an_active_tab() {
    let mut state = state_with_panes(1);
    state.tabs.clear();
    state.apply_status(status(1, "Stop"));

    let focused = PaneInfo {
        is_focused: true,
        ..terminal_pane(1, "pane1")
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    assert_eq!(state.agents[&1].state, AgentState::Done);
}

// --- prune_stale_agents ---

#[test]
fn closed_panes_lose_their_state() {
    let mut state = state_with_panes(2);
    state.apply_status(status(1, "Stop"));
    state.apply_status(status(2, "Stop"));
    state.pane_cwds.insert(2, "/gone".to_string());

    state.panes = Some(manifest(vec![(0, vec![terminal_pane(1, "pane1")])]));
    state.prune_stale_agents();

    assert!(state.agents.contains_key(&1));
    assert!(!state.agents.contains_key(&2));
    assert!(!state.pane_cwds.contains_key(&2));
}
