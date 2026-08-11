// 単体テスト。
//
// 実行はホストターゲットで行う（`make test`）。既定ターゲットの wasm32-wasip1 では
// テストバイナリを走らせるランタイムがないため。
//
// テストできる範囲について:
// - 副作用だけのホストコマンド（focus_pane_with_id, pipe_message_to_plugin,
//   intercept_key_presses 等）は下のスタブで何もしなくなるので、呼ばれても安全
// - **戻り値を stdin から読み返す問い合わせ系は呼べない**（get_plugin_ids,
//   get_focused_pane_info 等）。テスト中に呼ぶと stdin の読み取りに失敗して panic する。
//   したがって refresh_focus() とそれを経由する pipe ハンドラ（NAV_*）は
//   ここでは検証しない。フォーカス同期（要件: focus-sync）のうち、
//   問い合わせ結果を畳んだ先（State::focused_pane）から先のロジックは
//   フィールドを直接立てて検証する

use super::*;
use crate::agent::{AgentState, StatusPayload};
use crate::command::{CommandState, PaneStatus};
use crate::config::{Kind, SETTINGS};
use crate::deploy::TROOP;
use crate::render::{
    cwd_row, divider_line, overflow_row, reconcile_scroll, CounterColumn, HeadCells, Row,
    NO_AGENT_ICON, NO_AGENT_LABEL,
};
use crate::termination::Termination;
use crate::width::{
    fold_highlight_indices, pad_to_width, shift_highlight_indices, truncate, truncate_start,
};
use std::collections::HashMap;

// zellij-tile の shim は wasm ホストが提供する `host_run_plugin_command` を参照する。
// ホスト向けにリンクするにはこのシンボルを埋めてやる必要がある。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn host_run_plugin_command() {}

// --- ヘルパ ---

fn terminal_pane(id: u32, title: &str) -> PaneInfo {
    PaneInfo {
        id,
        title: title.to_string(),
        ..Default::default()
    }
}

fn plugin_pane(id: u32, url: &str) -> PaneInfo {
    PaneInfo {
        id,
        is_plugin: true,
        plugin_url: Some(url.to_string()),
        ..Default::default()
    }
}

// フォーカスしたまま滞在猶予（READ_DELAY）が満ちるまで居座る
//（docs/issues/transit-focus-clears-read-state.md）。
//
// 実機では 0.15 秒刻みで Timer が届くが、期限は経過時間で見るので
// 1回にまとめてよい。**目的地としてフォーカスした**ことの表明として、
// 既読を期待するテストはこれを挟む
fn settle_read(state: &mut State) {
    state.elapsed += READ_DELAY;
    state.apply_pending_reads();
}

// 召喚インスタンス（決定16）。常駐との違いはフローティングかどうか
fn floating_plugin_pane(id: u32, url: &str) -> PaneInfo {
    PaneInfo {
        is_floating: true,
        ..plugin_pane(id, url)
    }
}

fn manifest(tabs: Vec<(usize, Vec<PaneInfo>)>) -> PaneManifest {
    PaneManifest {
        panes: tabs.into_iter().collect::<HashMap<_, _>>(),
    }
}

fn tab(position: usize, active: bool) -> TabInfo {
    TabInfo {
        position,
        name: format!("tab{}", position + 1),
        active,
        ..Default::default()
    }
}

fn status(pane_id: u32, event: &str) -> StatusPayload {
    StatusPayload {
        pane_id,
        event: event.to_string(),
        agent: "claude".to_string(),
        source: None,
        cwd: None,
        detail: None,
    }
}

// `SessionStart` に起動理由を添えたもの（配置演出のトリガー判定用）
fn session_start(pane_id: u32, source: &str) -> StatusPayload {
    StatusPayload {
        source: Some(source.to_string()),
        ..status(pane_id, "SessionStart")
    }
}

// 番号列だけを持つ先頭列（マーク列は出さないフレーム）
fn number_cells(number: Option<(&str, bool)>) -> HeadCells<'_> {
    HeadCells {
        number,
        ..HeadCells::default()
    }
}

// タブ0に count 個のターミナルペイン（ID 1..=count）を持つ状態
fn state_with_panes(count: u32) -> State {
    let panes: Vec<PaneInfo> = (1..=count)
        .map(|i| terminal_pane(i, &format!("pane{}", i)))
        .collect();
    let mut state = State {
        tabs: vec![tab(0, true)],
        panes: Some(manifest(vec![(0, panes)])),
        permissions_granted: true,
        ..Default::default()
    };
    state.rebuild_selectable();
    state
}

// --- truncate ---

#[test]
fn truncate_leaves_short_strings_alone() {
    assert_eq!(truncate("abc", 5), "abc");
    // 境界: ちょうど収まるときは省略記号を付けない
    assert_eq!(truncate("abcde", 5), "abcde");
}

#[test]
fn truncate_appends_ellipsis_within_budget() {
    assert_eq!(truncate("abcdef", 5), "abcd…");
    assert_eq!(truncate("abcdef", 5).chars().count(), 5);
}

#[test]
fn truncate_counts_display_width_not_chars() {
    // 全角文字（CJK）は2セル分として数える。6文字でも表示幅は12あるので、
    // 文字数ベースだった旧実装ではここが誤って「そのまま返す」になっていた
    // （docs/issues/sidebar-bottom-highlight-glitch.md）
    assert_eq!(truncate("日本語テスト", 12), "日本語テスト");
    assert_eq!(truncate("日本語テスト", 6), "日本…");
}

#[test]
fn truncate_with_zero_width_is_empty() {
    // 省略記号1文字だけがはみ出すとサイドバー幅を壊す
    assert_eq!(truncate("abc", 0), "");
}

// --- pad_to_width ---

#[test]
fn pad_to_width_fills_with_spaces_up_to_the_column_count() {
    assert_eq!(pad_to_width("abc".to_string(), 5), "abc  ");
}

#[test]
fn pad_to_width_counts_cjk_chars_as_two_cells() {
    // 全角文字混じりのラベルを文字数でパディングすると表示幅が cols を
    // 超えてしまい、選択背景が端末側で折り返されて次の行にはみ出す
    // （docs/issues/sidebar-bottom-highlight-glitch.md）。
    // 「日本語」は3文字・表示幅6なので、cols=10 なら空白4個で埋まるのが正しい
    let padded = pad_to_width("日本語".to_string(), 10);
    assert_eq!(padded, "日本語    ");
    assert_eq!(unicode_width::UnicodeWidthStr::width(padded.as_str()), 10);
}

#[test]
fn pad_to_width_does_not_underflow_when_already_wide_enough() {
    // 表示幅がすでに cols 以上のときは空白を足さない（saturating_sub）
    assert_eq!(pad_to_width("日本語テスト".to_string(), 3), "日本語テスト");
}

// --- AgentState ---

#[test]
fn agent_state_wire_round_trip() {
    for state in [
        AgentState::Idle,
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Done,
        AgentState::Error,
    ] {
        assert_eq!(AgentState::from_str(state.as_str()), state);
    }
}

#[test]
fn agent_state_from_unknown_is_idle() {
    assert_eq!(AgentState::from_str(""), AgentState::Idle);
    assert_eq!(AgentState::from_str("bogus"), AgentState::Idle);
}

// --- StatusPayload::parse ---

#[test]
fn parse_status_reads_all_fields() {
    let raw = r#"{"pane_id":"12","event":"Notification","agent":"claude","cwd":"/tmp/x","detail":"needs input"}"#;
    let payload = StatusPayload::parse(raw).expect("parses");
    assert_eq!(payload.pane_id, 12);
    assert_eq!(payload.event, "Notification");
    assert_eq!(payload.agent, "claude");
    assert_eq!(payload.cwd.as_deref(), Some("/tmp/x"));
    assert_eq!(payload.detail.as_deref(), Some("needs input"));
}

#[test]
fn parse_status_accepts_unquoted_numbers_and_spaces() {
    let raw = r#"{ "pane_id": 7, "event": "Stop" }"#;
    let payload = StatusPayload::parse(raw).expect("parses");
    assert_eq!(payload.pane_id, 7);
    assert_eq!(payload.event, "Stop");
}

#[test]
fn parse_status_defaults_missing_agent() {
    let raw = r#"{"pane_id":"3","event":"Stop"}"#;
    let payload = StatusPayload::parse(raw).expect("parses");
    assert_eq!(payload.agent, "unknown");
    assert_eq!(payload.cwd, None);
    assert_eq!(payload.detail, None);
}

#[test]
fn parse_status_reads_the_session_start_source() {
    // フックスクリプト（extras/claude-hooks/fujin-hook.sh）の jq が実際に吐く形。
    // `source` は SessionStart にだけ入り、他のイベントでは with_entries で落ちる
    let raw = r#"{"pane_id":7,"agent":"claude","event":"SessionStart","source":"startup","cwd":"/tmp/x"}"#;
    let payload = StatusPayload::parse(raw).expect("parses");
    assert_eq!(payload.source.as_deref(), Some("startup"));
    assert!(deploy::detect_new_agent(payload.source.as_deref()));

    let raw =
        r#"{"pane_id":7,"agent":"claude","event":"SessionStart","source":"clear","cwd":"/tmp/x"}"#;
    let payload = StatusPayload::parse(raw).expect("parses");
    assert_eq!(payload.source.as_deref(), Some("clear"));
    assert!(!deploy::detect_new_agent(payload.source.as_deref()));

    // `source` を持たないイベントは None のまま
    let raw = r#"{"pane_id":7,"agent":"claude","event":"Stop","cwd":"/tmp/x"}"#;
    assert_eq!(StatusPayload::parse(raw).expect("parses").source, None);
}

#[test]
fn parse_status_rejects_incomplete_payloads() {
    // pane_id / event はどちらも必須
    assert!(StatusPayload::parse(r#"{"event":"Stop"}"#).is_none());
    assert!(StatusPayload::parse(r#"{"pane_id":"3"}"#).is_none());
    // 数値にできない pane_id は捨てる
    assert!(StatusPayload::parse(r#"{"pane_id":"abc","event":"Stop"}"#).is_none());
    assert!(StatusPayload::parse("not json at all").is_none());
}

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

// --- 状態ダンプ（インスタンス間同期のワイヤ形式） ---

#[test]
fn state_dump_round_trips() {
    let mut source = State::default();
    source.apply_status(status(1, "UserPromptSubmit"));
    source.apply_status(status(1, "SubagentStart"));
    // バックグラウンドのサブエージェントを残したままターンが終わった状態
    source.apply_status(status(1, "Stop"));
    source.apply_status(status(2, "TaskCreated"));
    source.pane_cwds.insert(1, "/work/fujin".to_string());

    let mut restored = State::default();
    restored.apply_state_dump(&source.state_dump());

    assert_eq!(restored.agents.len(), 2);
    assert_eq!(restored.agents[&1].state, AgentState::Working);
    assert_eq!(restored.agents[&1].subagents, 1);
    assert_eq!(restored.agents[&1].agent, "claude");
    // ターン終了済みも運ぶ。運ばないと同期先が最後の SubagentStop で done にできない
    assert!(restored.agents[&1].turn_ended);
    // シーケンス番号も運ぶ。運ばないと同期先のトリアージ一覧で同一階層内の
    // 並びが総崩れになる（全員0でツリー順に潰れる）
    assert_eq!(
        restored.agents[&1].state_change_seq,
        source.agents[&1].state_change_seq
    );
    assert!(
        restored.state_seq >= source.agents[&1].state_change_seq,
        "受け手のカウンタは配られた最大値まで進める"
    );
    restored.apply_status(status(1, "SubagentStop"));
    assert_eq!(restored.agents[&1].state, AgentState::Done);
    assert_eq!(
        restored.pane_cwds.get(&1).map(String::as_str),
        Some("/work/fujin")
    );
    assert_eq!(restored.agents[&2].open_tasks, 1);
    // cwd を持たないペインは登録しない
    assert!(!restored.pane_cwds.contains_key(&2));
}

#[test]
fn state_dump_skips_malformed_lines() {
    let mut state = State::default();
    state.apply_state_dump("garbage\nxx\tworking\t0\t0\tclaude\t\n3\tdone\t0\t0\tclaude\t\n");
    assert_eq!(state.agents.len(), 1);
    assert_eq!(state.agents[&3].state, AgentState::Done);
}

#[test]
fn state_dump_drops_panes_that_no_longer_exist() {
    // 送り手が閉じたばかりのペインを載せていても、受け手の一覧で間引く
    let mut state = state_with_panes(2);
    state.apply_state_dump("1\tdone\t0\t0\tclaude\t\n99\tdone\t0\t0\tclaude\t\n");
    assert!(state.agents.contains_key(&1));
    assert!(!state.agents.contains_key(&99));
}

// --- rebuild_selectable ---

#[test]
fn selectable_skips_plugins_and_suppressed_panes() {
    let suppressed = PaneInfo {
        id: 3,
        is_suppressed: true,
        ..Default::default()
    };
    let mut state = State {
        panes: Some(manifest(vec![(
            0,
            vec![
                terminal_pane(1, "shell"),
                plugin_pane(2, "file:/x/fujin.wasm"),
                suppressed,
            ],
        )])),
        ..Default::default()
    };
    state.rebuild_selectable();
    assert_eq!(
        state
            .selectable
            .iter()
            .map(|e| e.pane_id)
            .collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn selectable_is_ordered_by_tab_position() {
    let mut state = State {
        panes: Some(manifest(vec![
            (2, vec![terminal_pane(30, "c")]),
            (0, vec![terminal_pane(10, "a")]),
            (1, vec![terminal_pane(20, "b")]),
        ])),
        ..Default::default()
    };
    state.rebuild_selectable();
    assert_eq!(
        state
            .selectable
            .iter()
            .map(|e| e.pane_id)
            .collect::<Vec<_>>(),
        vec![10, 20, 30]
    );
    assert_eq!(
        state
            .selectable
            .iter()
            .map(|e| e.tab_position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[test]
fn selection_is_clamped_when_panes_disappear() {
    let mut state = state_with_panes(3);
    state.selected = 2;
    state.panes = Some(manifest(vec![(0, vec![terminal_pane(1, "pane1")])]));
    state.rebuild_selectable();
    assert_eq!(state.selected, 0);
}

#[test]
fn selection_is_zero_when_nothing_is_selectable() {
    let mut state = state_with_panes(3);
    state.selected = 2;
    state.panes = Some(manifest(vec![(0, vec![])]));
    state.rebuild_selectable();
    assert_eq!(state.selected, 0);
    assert!(state.selectable.is_empty());
}

// --- 既読モデル（決定10） ---

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

// --- 滞在猶予（docs/issues/transit-focus-clears-read-state.md） ---
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
    // コマンド状態も同じ既読モデルに乗る（決定32）ので、猶予も同じく効く
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

// --- navモードのキー操作（決定12） ---

#[test]
fn nav_keys_move_the_selection() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;

    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('j')));
    assert_eq!(state.selected, 1);
    state.handle_nav_key(KeyWithModifier::new(BareKey::Down));
    assert_eq!(state.selected, 2);
    // 末尾で止まる
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('j')));
    assert_eq!(state.selected, 2);

    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('k')));
    assert_eq!(state.selected, 1);
    state.handle_nav_key(KeyWithModifier::new(BareKey::Up));
    assert_eq!(state.selected, 0);
    // 先頭で止まる
    state.handle_nav_key(KeyWithModifier::new(BareKey::Up));
    assert_eq!(state.selected, 0);

    assert!(state.nav_mode, "移動キーではモードを抜けない");
}

#[test]
fn nav_g_jumps_to_the_edges() {
    let mut state = state_with_panes(4);
    state.nav_mode = true;

    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('G')).with_shift_modifier());
    assert_eq!(state.selected, 3);
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('g')));
    assert_eq!(state.selected, 0);
    assert!(state.nav_mode);
}

#[test]
fn nav_digits_no_longer_jump_directly() {
    // かつての 1-9 直行ジャンプは番号ジャンプサブモードへ一本化した（決定29）。
    // navモード最上位の数字は未定義キー＝安全弁で退場する
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('2')));
    assert!(!state.nav_mode);
    assert_eq!(state.selected, 0, "選択は動かさない");
}

#[test]
fn nav_enter_leaves_the_mode() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(KeyWithModifier::new(BareKey::Enter));
    assert!(!state.nav_mode);
}

#[test]
fn nav_leaves_on_undefined_keys() {
    // 未定義キーで抜けるのは、キー横取りが取り残されないための安全弁
    for key in [
        KeyWithModifier::new(BareKey::Esc),
        KeyWithModifier::new(BareKey::Char('q')),
        KeyWithModifier::new(BareKey::Char('z')),
        KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier(),
        KeyWithModifier::new(BareKey::Char('j')).with_alt_modifier(),
    ] {
        let mut state = state_with_panes(3);
        state.nav_mode = true;
        state.handle_nav_key(key.clone());
        assert!(!state.nav_mode, "{:?} でモードを抜けるべき", key);
        assert_eq!(state.selected, 0, "{:?} で選択を動かすべきでない", key);
    }
}

// --- 番号ジャンプサブモード（決定29、要件: docs/requirements/pane-number-jump/） ---

fn jump_state(count: u32) -> State {
    let mut state = state_with_panes(count);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('n')));
    state
}

fn jump_buffer(state: &State) -> Option<&str> {
    state.jump.as_ref().map(|j| j.buffer.as_str())
}

#[test]
fn n_enters_the_number_jump_submode() {
    let state = jump_state(3);
    assert!(state.jump.is_some());
    assert!(state.nav_mode, "サブモードに入ってもnavモードは継続する");
}

#[test]
fn a_unique_number_jumps_and_leaves_nav_mode() {
    let mut state = jump_state(3);
    state.handle_nav_key(key(BareKey::Char('2')));
    assert_eq!(state.selected, 1);
    assert!(!state.nav_mode, "ジャンプはnavモードの退場を伴う");
    assert!(state.jump.is_none());
}

#[test]
fn numbers_are_zero_padded_to_the_digit_count_of_the_total() {
    // 桁数を総数に固定すると番号どうしが互いの前方一致にならず（prefix-free）、
    // 「1 を打ったが 10 があるので確定できない」という行き止まりが起きない（決定29）
    let state = state_with_panes(12);
    assert_eq!(state.pane_number_width(), 2);
    assert_eq!(state.pane_number(0), "01");
    assert_eq!(state.pane_number(11), "12");
    let single = state_with_panes(9);
    assert_eq!(single.pane_number(0), "1", "総数が1桁ならゼロ埋めしない");
}

#[test]
fn an_ambiguous_prefix_waits_for_the_next_digit() {
    let mut state = jump_state(12);
    state.handle_nav_key(key(BareKey::Char('1')));
    assert!(state.nav_mode, "10-12 が残っているので確定しない");
    assert_eq!(jump_buffer(&state), Some("1"));
    state.handle_nav_key(key(BareKey::Char('2')));
    assert_eq!(state.selected, 11);
    assert!(!state.nav_mode);
}

#[test]
fn a_leading_zero_reaches_the_early_panes() {
    let mut state = jump_state(12);
    state.handle_nav_key(key(BareKey::Char('0')));
    assert!(state.nav_mode, "01-09 が残っているので確定しない");
    state.handle_nav_key(key(BareKey::Char('3')));
    assert_eq!(state.selected, 2);
    assert!(!state.nav_mode);
}

#[test]
fn a_dead_end_input_resets_the_buffer() {
    // 存在しない番号（候補0件）はその場で空に戻す。Backspace での訂正を強制しない
    let mut state = jump_state(12);
    state.handle_nav_key(key(BareKey::Char('9')));
    assert!(state.nav_mode, "リセットしてサブモードには留まる");
    assert_eq!(jump_buffer(&state), Some(""));
    state.handle_nav_key(key(BareKey::Char('0')));
    state.handle_nav_key(key(BareKey::Char('4')));
    assert_eq!(state.selected, 3, "リセット後は最初から入力し直せる");
}

#[test]
fn backspace_deletes_the_last_digit() {
    let mut state = jump_state(12);
    state.handle_nav_key(key(BareKey::Char('1')));
    state.handle_nav_key(key(BareKey::Backspace));
    assert_eq!(jump_buffer(&state), Some(""));
    state.handle_nav_key(key(BareKey::Char('0')));
    state.handle_nav_key(key(BareKey::Char('5')));
    assert_eq!(state.selected, 4);
}

#[test]
fn esc_leaves_only_the_jump_submode() {
    let mut state = jump_state(3);
    state.handle_nav_key(key(BareKey::Esc));
    assert!(state.jump.is_none());
    assert!(state.nav_mode, "navモードは継続している");
}

#[test]
fn the_safety_valve_reaches_the_jump_submode() {
    // 未定義キー・修飾キー付きは navモードごと退場（安全弁は最上位まで効かせる）
    for key_press in [
        KeyWithModifier::new(BareKey::Char('x')),
        KeyWithModifier::new(BareKey::Enter),
        KeyWithModifier::new(BareKey::Char('1')).with_ctrl_modifier(),
    ] {
        let mut state = jump_state(3);
        state.handle_nav_key(key_press.clone());
        assert!(!state.nav_mode, "{:?} でnavモードごと抜ける", key_press);
        assert_eq!(state.selected, 0, "{:?} で選択は動かさない", key_press);
    }
}

#[test]
fn the_number_column_appears_only_in_the_jump_submode() {
    let mut state = state_with_panes(12);
    let column = CounterColumn::default();
    let plain = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column,
        HeadCells::default(),
        SIDEBAR,
    );
    assert!(
        plain.content().starts_with("  › pane1"),
        "{}",
        plain.content()
    );

    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('n')));
    let cell = state.jump_number(0);
    let cell = cell.as_ref().map(|(n, m)| (n.as_str(), *m));
    let numbered = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column,
        number_cells(cell),
        SIDEBAR,
    );
    assert!(
        numbered.content().starts_with("  01 › pane1"),
        "番号列はアイコンの前に挟む: {}",
        numbered.content()
    );
}

#[test]
fn numbers_off_the_candidate_list_are_dimmed() {
    // vimiumのリンクヒントと同じ提示: バッファに前方一致しなくなった番号は
    // 落とし、残っている候補だけがキーの色（レベル2）で目に入る（決定29）
    let mut state = jump_state(12);
    state.handle_nav_key(key(BareKey::Char('1')));

    let column = CounterColumn::default();
    let render_with_number = |state: &State, index: usize| {
        let cell = state.jump_number(index);
        let cell = cell.as_ref().map(|(n, m)| (n.as_str(), *m));
        state.pane_row(
            &state.selectable[index],
            false,
            None,
            column,
            number_cells(cell),
            SIDEBAR,
        )
    };
    let candidate = render_with_number(&state, 9); // "10"
    assert_eq!(ink_at(&candidate, 2), vec![2, 3]);
    let dropped = render_with_number(&state, 0); // "01"
                                                 // アイコン列の未起動の印も dim なので、番号の位置だけを取り出して見る
    let number_dim: Vec<usize> = ink_at(&dropped, DIM_LEVEL)
        .into_iter()
        .filter(|at| *at < 4)
        .collect();
    assert_eq!(number_dim, vec![2, 3]);
    assert!(ink_at(&dropped, 2).is_empty());
}

#[test]
fn the_footer_shows_the_number_buffer_while_jumping() {
    let mut state = jump_state(12);
    state.handle_nav_key(key(BareKey::Char('1')));
    let footer = state.footer_line(SIDEBAR);
    let content = footer.content().to_string();
    assert!(content.starts_with("  n 1▏"), "{}", content);
    assert!(content.ends_with("?:help"), "{}", content);
}

// --- 終了操作サブモード（決定35、要件: docs/requirements/pane-close-kill/） ---
//
// 実行そのもの（send_sigkill_to_pane_id / close_pane_with_id）は副作用だけの
// ホスト関数で結果を観測できないので、その手前で畳んだ `termination_plan()`
//（対象ペインと効果の内訳）を検証対象にする

fn termination_state(count: u32) -> State {
    let mut state = state_with_panes(count);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('d')));
    state
}

#[test]
fn the_entry_key_opens_the_confirmation_prompt() {
    let state = termination_state(3);
    assert!(state.termination.is_some());
    assert!(state.nav_mode, "サブモードはnavモードの内側");
    // フッターが確認プロンプトに転用され、3操作とも選べる
    assert_eq!(
        state.footer_line(SIDEBAR).content(),
        "  c:close k:kill x:kill+close"
    );
}

#[test]
fn each_key_picks_its_own_termination() {
    // close はプロセスに触れず閉じるだけ、kill はSIGKILLだけ、
    // kill→close は両方（順序は kill → close）
    for (pressed, kind, effects) in [
        ('c', Termination::Close, (false, true)),
        ('k', Termination::Kill, (true, false)),
        ('x', Termination::KillThenClose, (true, true)),
    ] {
        let mut state = termination_state(3);
        assert_eq!(Termination::from_key(BareKey::Char(pressed)), Some(kind));
        assert_eq!(
            state.termination_plan(kind),
            vec![(1, effects.0, effects.1)],
            "{} の効果",
            pressed
        );

        state.handle_nav_key(key(BareKey::Char(pressed)));
        assert!(
            state.termination.is_none(),
            "{} でサブモードを抜ける",
            pressed
        );
        assert!(state.nav_mode, "{} で戻る先はnavモード", pressed);
    }
}

#[test]
fn the_prompt_targets_the_selected_pane() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(key(BareKey::Char('d')));

    assert_eq!(
        state.termination_plan(Termination::Close),
        vec![(2, false, true)]
    );
}

#[test]
fn the_target_is_pinned_at_entry() {
    // 確認の途中で選択が動いても（兄弟インスタンスからの配布・ペインの増減）、
    // 確認したときに見えていたペイン以外を対象にしない
    let mut state = termination_state(3);
    state.select_pane_id(3);

    assert_eq!(
        state.termination_plan(Termination::Kill),
        vec![(1, true, false)]
    );
}

#[test]
fn a_pane_closed_during_the_prompt_is_left_alone() {
    let mut state = termination_state(3);
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(2, "pane2"), terminal_pane(3, "pane3")],
    )]));
    state.rebuild_selectable();

    assert!(state.termination_plan(Termination::Close).is_empty());
}

#[test]
fn the_prompt_needs_a_selected_pane() {
    let mut state = state_with_panes(0);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('d')));

    assert!(state.termination.is_none(), "対象が無ければ入場しない");
    assert!(state.nav_mode);
}

#[test]
fn esc_cancels_the_termination() {
    let mut state = termination_state(3);
    state.handle_nav_key(key(BareKey::Esc));

    assert!(state.termination.is_none());
    assert!(state.nav_mode, "取り消しでnavモードごと抜けはしない");
    assert_eq!(state.selected, 0, "選択は動かさない");
    // フッターは元の表示に戻る
    assert_eq!(state.footer_line(SIDEBAR).content(), "  ?:help  esc:exit");
}

#[test]
fn the_safety_valve_reaches_the_termination_submode() {
    for key_press in [
        KeyWithModifier::new(BareKey::Char('z')),
        KeyWithModifier::new(BareKey::Enter),
        KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier(),
    ] {
        let mut state = termination_state(3);
        state.handle_nav_key(key_press.clone());
        assert!(!state.nav_mode, "{:?} でnavモードごと抜ける", key_press);
        assert!(
            state.termination.is_none(),
            "{:?} で終了操作は実行しない",
            key_press
        );
    }
}

#[test]
fn every_kind_of_pane_offers_all_three_terminations() {
    // 対象種別で出し分けはしない（決定35）。対象プロセスが実質存在しない
    // 場合（終了済みコマンドペインへのkill）も no-op になるだけ
    let mut with_agent = state_with_panes(1);
    set_agent_state(&mut with_agent, 1, AgentState::Working);
    let running = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    let exited = state_with_command_panes(vec![exited_command_pane(1, "ls", Some(0))]);
    let plain = state_with_panes(1);

    for mut state in [with_agent, running, exited, plain] {
        state.nav_mode = true;
        state.handle_nav_key(key(BareKey::Char('d')));
        for kind in [
            Termination::Close,
            Termination::Kill,
            Termination::KillThenClose,
        ] {
            assert!(!state.termination_plan(kind).is_empty(), "{:?}", kind);
        }
    }
}

#[test]
fn the_confirmation_prompt_wears_the_warning_color() {
    // 新しい色は増やさず、状態アイコン `error` と同じ error_color を借りる（決定35）
    let state = termination_state(3);
    let footer = state.footer_line(SIDEBAR);
    let indent = 2;
    let end = footer.content().chars().count();
    assert_eq!(
        ink_at(&footer, ERROR_LEVEL),
        (indent..end).collect::<Vec<_>>(),
        "{}",
        footer.content()
    );
    // ヘッダーの三角も同じ状態色（決定27）。モードラベルは navモードのまま
    let header = state.header_line(SIDEBAR);
    assert!(ink_at(&header, ERROR_LEVEL).contains(&0), "{:?}", header);
    assert!(header.content().contains("[nav]"), "{}", header.content());
}

#[test]
fn the_prompt_falls_back_to_bare_keys_when_it_does_not_fit() {
    // 項目の途中で切り詰めると `キー:動作` の形が壊れて読めなくなる
    let state = termination_state(3);
    assert_eq!(state.footer_line(16).content(), "  c k x");
}

// --- 複数選択（マーク。決定39、要件:
// docs/requirements/pane-termination-multi-select/） ---
//
// 一括操作の対象として選んだペインの集合。単一のナビゲーションカーソルである
// 選択（`State::selected`）とは別概念で、タブをまたぎ、navモードを退場しても残る

// マークの入場キー（トグル）と全解除キー
fn mark_key() -> KeyWithModifier {
    key(BareKey::Char('m'))
}

fn clear_marks_key() -> KeyWithModifier {
    key(BareKey::Char('M'))
}

// マーク済みペインをツリー順に並べたもの（集合そのものの比較用）
fn marks(state: &State) -> Vec<u32> {
    state.marked_in_tree_order()
}

#[test]
fn the_mark_key_toggles_the_pane_under_the_selection() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;

    state.handle_nav_key(mark_key());
    assert_eq!(marks(&state), vec![1]);
    // 同じキーでマークが外れる
    state.handle_nav_key(mark_key());
    assert!(marks(&state).is_empty());
}

#[test]
fn marks_pile_up_row_by_row_across_tabs() {
    // マークはタブをまたいでよい（決定39。selectable が元々タブ横断のため）
    let mut state = searchable_state(); // タブ0に1・2、タブ1に3
    state.handle_nav_key(mark_key());
    state.handle_nav_key(key(BareKey::Char('G'))); // 別タブの末尾へ
    state.handle_nav_key(mark_key());

    assert_eq!(marks(&state), vec![1, 3]);
}

#[test]
fn the_clear_key_drops_every_mark() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(mark_key());
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(mark_key());
    assert_eq!(marks(&state), vec![1, 2]);

    state.handle_nav_key(clear_marks_key());
    assert!(marks(&state).is_empty());
    assert!(state.nav_mode, "全解除はnavモードを抜けない");
}

#[test]
fn marks_survive_leaving_nav_mode() {
    // 検索カーソル・トリアージカーソルと違い、タブをまたいで持ち回る前提の状態
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(mark_key());

    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.nav_mode);
    assert_eq!(marks(&state), vec![1]);
    assert!(
        state.mark_column(&state.visible_rows()),
        "印はnavモードの外でも出し続ける"
    );
}

#[test]
fn a_pane_that_leaves_the_list_loses_its_mark() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(mark_key());
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(mark_key());

    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(2, "pane2"), terminal_pane(3, "pane3")],
    )]));
    state.rebuild_selectable();

    assert_eq!(marks(&state), vec![2], "閉じたペイン1のマークは残さない");
}

#[test]
fn the_triage_list_marks_the_row_under_the_cursor() {
    let mut state = triage_state();
    set_agent_state(&mut state, 3, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('p')));
    assert_eq!(state.triage_cursor(), Some(3));

    state.handle_nav_key(mark_key());
    assert_eq!(marks(&state), vec![3]);
    assert!(state.triage.is_some(), "マークでトリアージ一覧を抜けない");
}

#[test]
fn the_filtered_results_mark_with_the_alt_key() {
    // 検索サブモードは印字可能文字をすべてクエリに使うので、マークだけ Alt付き。
    // 素の `m` はクエリの文字になる（マークにはならない）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "cha");
    assert_eq!(state.search.as_ref().and_then(|s| s.cursor), Some(3));

    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('m')).with_alt_modifier());
    assert_eq!(marks(&state), vec![3]);
    assert!(state.search.is_some(), "マークで検索サブモードを抜けない");

    state.handle_nav_key(mark_key());
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("cham"),
        "素の `m` はクエリの文字"
    );
}

#[test]
fn the_mark_key_is_undefined_in_the_number_jump_submode() {
    // 数字専用の入力空間の安全弁がそのまま効く（特別扱いのコードは足さない）
    let mut state = jump_state(3);
    state.handle_nav_key(mark_key());

    assert!(!state.nav_mode, "未定義キーとして navモードごと退場する");
    assert!(marks(&state).is_empty());
}

#[test]
fn the_mark_column_shows_up_only_when_something_is_marked() {
    let mut state = state_with_panes(2);
    let column = CounterColumn::default();
    assert!(
        !state.mark_column(&state.visible_rows()),
        "マークが無いフレームには列を出さない"
    );

    state.nav_mode = true;
    state.handle_nav_key(mark_key());
    assert!(state.mark_column(&state.visible_rows()));

    let marked = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column,
        HeadCells {
            mark: Some(true),
            ..HeadCells::default()
        },
        SIDEBAR,
    );
    assert!(
        marked.content().starts_with("  ✓ › pane1"),
        "マーク列はアイコンの前: {}",
        marked.content()
    );
    // 同じフレームのマークされていない行も、列ぶんを空白で空けて桁を揃える
    let plain = state.pane_row(
        &state.selectable[1],
        false,
        None,
        column,
        HeadCells {
            mark: Some(false),
            ..HeadCells::default()
        },
        SIDEBAR,
    );
    assert_eq!(
        column_at(plain.content(), "pane2"),
        column_at(marked.content(), "pane1")
    );
}

#[test]
fn a_marked_row_still_leaves_the_right_margin() {
    // マーク列ぶんペイン名の残り幅が縮む。畳み損ねると選択背景が端末側で
    // 折り返して次の行を汚す
    let mut state = state_with_one_pane("要件定義とドキュメント整理タスクの続き");
    state.marked.insert(1);
    repeat_status(&mut state, 1, "SubagentStart", 2);

    let row = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells {
            mark: Some(true),
            ..HeadCells::default()
        },
        SIDEBAR,
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(row.content()),
        CONTENT,
        "{}",
        row.content()
    );
    assert!(row.content().ends_with("+2"), "{}", row.content());
}

#[test]
fn the_mark_column_sits_between_the_number_and_the_icon() {
    // 列順序は 選択バー → 番号列 → マーク列 → アイコン → ペイン名（決定39）。
    // マーク自体は番号ジャンプサブモードへ入る前に立てたもの
    let mut state = jump_state(12);
    state.marked.insert(1);

    let cell = state.jump_number(0);
    let cell = cell.as_ref().map(|(n, m)| (n.as_str(), *m));
    let row = state.pane_row(
        &state.selectable[0],
        false,
        None,
        CounterColumn::default(),
        HeadCells {
            number: cell,
            mark: Some(true),
        },
        SIDEBAR,
    );
    assert!(
        row.content().starts_with("  01 ✓ › pane1"),
        "{}",
        row.content()
    );
    // 状態アイコンの色位置がマーク列ぶんずれても、番号はレベル2のまま
    assert_eq!(ink_at(&row, 2), vec![2, 3, 5]);
}

#[test]
fn marked_triage_rows_wear_the_mark_too() {
    // トリアージ一覧の上でもマークできる以上、印も同じ位置に出す
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('p')));
    state.handle_nav_key(mark_key());

    let rows = state.visible_rows();
    assert!(state.mark_column(&rows));
    let entry = state
        .selectable
        .iter()
        .find(|e| e.pane_id == 2)
        .expect("マークしたペイン");
    let row = state.triage_row(entry, "tab1", false, 4, Some(true), SIDEBAR);
    assert!(row.content().starts_with("  ✓ ×"), "{}", row.content());
}

// --- マークと終了操作の連携（決定39） ---

#[test]
fn the_termination_takes_the_marks_when_there_are_any() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('j'))); // 選択は2へ
    state.marked.extend([3, 1]);

    state.handle_nav_key(key(BareKey::Char('d')));
    assert_eq!(
        state.termination_plan(Termination::Kill),
        vec![(1, true, false), (3, true, false)],
        "対象はマーク集合で、順序はツリー順"
    );
}

#[test]
fn without_marks_the_termination_falls_back_to_the_selection() {
    // 単一版のフローはマーク0件の特殊ケースとして包含する（決定39）
    let state = termination_state(3);
    assert_eq!(
        state.termination_plan(Termination::Close),
        vec![(1, false, true)]
    );
    assert_eq!(state.termination_marked_count(), 0);
}

#[test]
fn the_marked_targets_are_pinned_at_entry() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.marked.insert(1);
    state.handle_nav_key(key(BareKey::Char('d')));

    // 確認の途中でマークが変わっても（兄弟インスタンスからの配布など）
    // 対象は入場時のまま
    state.apply_marks("2,3");
    assert_eq!(
        state.termination_plan(Termination::Close),
        vec![(1, false, true)]
    );
}

#[test]
fn a_marked_pane_closed_during_the_prompt_is_left_alone() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.marked.extend([1, 2]);
    state.handle_nav_key(key(BareKey::Char('d')));

    state.panes = Some(manifest(vec![(0, vec![terminal_pane(2, "pane2")])]));
    state.rebuild_selectable();

    assert_eq!(
        state.termination_plan(Termination::Close),
        vec![(2, false, true)],
        "消えたペインには何も送らず、残りには送る"
    );
}

#[test]
fn running_a_termination_clears_every_mark() {
    // 成功・no-op を問わずクリアする（決定39）
    for pressed in ['c', 'k', 'x'] {
        let mut state = state_with_panes(3);
        state.nav_mode = true;
        state.marked.extend([1, 2]);
        state.handle_nav_key(key(BareKey::Char('d')));
        state.handle_nav_key(key(BareKey::Char(pressed)));

        assert!(marks(&state).is_empty(), "{} のあと", pressed);
        assert!(state.termination.is_none());
    }
}

#[test]
fn cancelling_a_termination_keeps_the_marks() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.marked.extend([1, 2]);
    state.handle_nav_key(key(BareKey::Char('d')));
    state.handle_nav_key(key(BareKey::Esc));

    assert!(state.termination.is_none());
    assert_eq!(marks(&state), vec![1, 2]);
}

#[test]
fn the_prompt_counts_the_marked_panes() {
    // マーク1件以上のときだけ件数を前置する。幅28セルに3項目とも入らないので
    // キーだけの2段目に落ちるが、件数は残す（決定39）
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.marked.extend([1, 2]);
    state.handle_nav_key(key(BareKey::Char('d')));
    assert_eq!(state.footer_line(SIDEBAR).content(), "  2 panes  c k x");

    let mut single = state_with_panes(3);
    single.nav_mode = true;
    single.marked.insert(2);
    single.handle_nav_key(key(BareKey::Char('d')));
    assert_eq!(single.footer_line(SIDEBAR).content(), "  1 pane  c k x");
}

// --- マークの同期（決定39・決定13） ---

#[test]
fn marks_travel_to_siblings_as_pane_ids() {
    let mut state = state_with_panes(3);
    state.marked.extend([1, 3]);
    assert_eq!(state.mark_dump(), "1,3");

    let mut sibling = state_with_panes(3);
    assert!(sibling.apply_marks(&state.mark_dump()));
    assert_eq!(marks(&sibling), vec![1, 3]);
    // 同じ集合を配られても再描画は要らない
    assert!(!sibling.apply_marks(&state.mark_dump()));
}

#[test]
fn an_empty_payload_clears_the_marks_on_siblings() {
    let mut state = state_with_panes(3);
    state.marked.insert(2);
    state.clear_marks();

    let mut sibling = state_with_panes(3);
    sibling.marked.insert(2);
    assert!(sibling.apply_marks(&state.mark_dump()));
    assert!(marks(&sibling).is_empty());
}

#[test]
fn a_sibling_keeps_marks_for_panes_it_has_not_seen_yet() {
    // 非可視インスタンスの一覧は古い（PaneUpdate が届かない）。受け取った時点で
    // 絞り込むと、配ったそばからマークが消える。掃除は一覧を持つ側の責務
    let mut sibling = state_with_panes(1);
    assert!(sibling.apply_marks("1,9"));
    assert!(sibling.is_marked(9));
}

#[test]
fn the_nav_help_advertises_the_mark_keys() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    assert!(lines.contains("mark / clear all"), "{}", lines);
}

// --- プレビュー（決定42、要件: docs/requirements/preview/） ---
//
// navモード中、いま光っている行のペインの内容をフローティングペインへ
// スナップショット表示するトグル可能な横断的機能。
//
// **フローティングペインを開く経路と `get_pane_scrollback` はテストから呼べない**
// （どちらも戻り値を stdin から読み返すホスト関数）。ここで検証するのは
// 「いつオン/オフになるか」「どのペインを対象に選ぶか」「いつ更新を止めるか」
// までで、`own_plugin_url` を立てていない状態ではペインを開きにいかないので
// ホスト関数には触れない

fn preview_key() -> KeyWithModifier {
    key(BareKey::Char('v'))
}

fn preview_read_key() -> KeyWithModifier {
    key(BareKey::Char('r'))
}

// プレビューが表示している対象ペイン
fn preview_target(state: &State) -> Option<u32> {
    state.preview.as_ref().and_then(|preview| preview.target)
}

#[test]
fn the_preview_key_toggles_the_preview() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;

    state.handle_nav_key(preview_key());
    assert!(state.preview.is_some());
    assert_eq!(preview_target(&state), Some(1), "対象は選択行のペイン");
    assert!(state.nav_mode, "プレビューはモードではなく表示状態");

    state.handle_nav_key(preview_key());
    assert!(state.preview.is_none());
}

#[test]
fn the_preview_follows_the_selection() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());

    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(preview_target(&state), Some(2));
    state.handle_nav_key(key(BareKey::Char('G')));
    assert_eq!(preview_target(&state), Some(3));
}

#[test]
fn the_filtered_results_preview_with_the_alt_key() {
    // 検索サブモードは印字可能文字をすべてクエリに使うので、マークと同じく
    // プレビューのトグルだけ Alt付き
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "cha");
    assert_eq!(state.search.as_ref().and_then(|s| s.cursor), Some(3));

    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('v')).with_alt_modifier());
    assert_eq!(
        preview_target(&state),
        Some(3),
        "対象は絞り込み結果のカーソル"
    );
    assert!(
        state.search.is_some(),
        "プレビューで検索サブモードを抜けない"
    );

    state.handle_nav_key(preview_key());
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("chav"),
        "素の `v` はクエリの文字"
    );
}

#[test]
fn the_triage_list_previews_the_row_under_the_cursor() {
    let mut state = triage_state();
    set_agent_state(&mut state, 3, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('p')));
    assert_eq!(state.triage_cursor(), Some(3));

    state.handle_nav_key(preview_key());
    assert_eq!(preview_target(&state), Some(3));
    assert!(
        state.triage.is_some(),
        "プレビューでトリアージ一覧を抜けない"
    );
}

#[test]
fn the_preview_key_is_undefined_in_the_number_jump_submode() {
    // マークと同じ理由（数字専用の入力空間の安全弁がそのまま効く）
    let mut state = jump_state(3);
    state.handle_nav_key(preview_key());

    assert!(!state.nav_mode, "未定義キーとして navモードごと退場する");
    assert!(state.preview.is_none());
}

#[test]
fn the_preview_stops_updating_in_the_number_jump_submode() {
    // オンのまま番号ジャンプサブモードへ入ることはできる。候補を絞っている
    // あいだは対象ペインが定まらないので、表示は直前のまま動かさない
    // 通し番号が2桁になる件数にしておく。1桁だと最初の数字で候補が1件に
    // 確定して即ジャンプしてしまい、「入力中」の状態を作れない（決定29）
    let mut state = state_with_panes(12);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());
    assert_eq!(preview_target(&state), Some(1));

    state.handle_nav_key(key(BareKey::Char('n')));
    state.handle_nav_key(key(BareKey::Char('0'))); // 候補は 01..09 でまだ曖昧
    assert!(state.jump.is_some(), "番号ジャンプサブモードは続いている");
    assert!(state.preview.is_some(), "表示自体は維持する");
    assert_eq!(preview_target(&state), Some(1), "内容は更新しない");
}

#[test]
fn leaving_nav_mode_closes_the_preview() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());

    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.nav_mode);
    assert!(
        state.preview.is_none(),
        "navモードの外にプレビューだけ残さない（決定34と整合させる）"
    );
}

#[test]
fn jumping_closes_the_preview() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());

    state.handle_nav_key(key(BareKey::Enter));
    assert!(!state.nav_mode);
    assert!(state.preview.is_none());
}

#[test]
fn previewing_does_not_change_the_notification_state() {
    // プレビューはフォーカスもキー横取りも対象ペインに及ぼさないので、
    // 見て回るだけでは既読にならない（決定10・決定37はそのまま無関係に動く）
    let mut state = state_with_panes(3);
    set_agent_state(&mut state, 2, AgentState::Done);
    state.nav_mode = true;

    state.handle_nav_key(preview_key());
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(preview_target(&state), Some(2));
    assert_eq!(
        state.agents.get(&2).map(|a| a.state),
        Some(AgentState::Done),
        "プレビューしただけでは既読にしない"
    );
}

#[test]
fn the_read_key_clears_the_state_of_the_previewed_pane() {
    for target in [AgentState::Done, AgentState::Blocked, AgentState::Error] {
        let mut state = state_with_panes(3);
        set_agent_state(&mut state, 1, target);
        state.nav_mode = true;
        state.handle_nav_key(preview_key());

        state.handle_nav_key(preview_read_key());
        assert_eq!(
            state.agents.get(&1).map(|a| a.state),
            Some(AgentState::Idle),
            "{:?} は既読化キーで idle に戻る",
            target
        );
        assert!(state.nav_mode, "既読化でnavモードを抜けない");
        assert!(state.preview.is_some(), "プレビューも開いたまま");
    }
}

#[test]
fn the_read_key_leaves_working_alone() {
    let mut state = state_with_panes(3);
    set_agent_state(&mut state, 1, AgentState::Working);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());

    state.handle_nav_key(preview_read_key());
    assert_eq!(
        state.agents.get(&1).map(|a| a.state),
        Some(AgentState::Working),
        "走っている最中のペインは人の対応を待っていない"
    );
}

#[test]
fn the_read_key_is_undefined_while_the_preview_is_off() {
    // プレビュー専用のキーはプレビュー文脈の外では存在しない、の一貫性
    let mut state = state_with_panes(3);
    set_agent_state(&mut state, 1, AgentState::Done);
    state.nav_mode = true;

    state.handle_nav_key(preview_read_key());
    assert!(!state.nav_mode, "未定義キーとして navモードごと退場する");
    assert_eq!(
        state.agents.get(&1).map(|a| a.state),
        Some(AgentState::Done),
        "既読にもしない"
    );
}

#[test]
fn the_read_key_is_undefined_in_the_number_jump_submode() {
    let mut state = jump_state(3);
    state.handle_nav_key(preview_read_key());
    assert!(!state.nav_mode);
}

#[test]
fn the_triage_list_reads_the_previewed_pane_too() {
    let mut state = triage_state();
    set_agent_state(&mut state, 3, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('p')));
    state.handle_nav_key(preview_key());

    state.handle_nav_key(preview_read_key());
    assert_eq!(
        state.agents.get(&3).map(|a| a.state),
        Some(AgentState::Idle)
    );
    assert!(state.triage.is_some());
}

#[test]
fn the_nav_help_advertises_the_preview_keys() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    assert!(lines.contains("preview"), "{}", lines);
    assert!(
        !lines.contains("mark read"),
        "プレビューがオフの間は既読化キーを出さない: {}",
        lines
    );

    state.handle_nav_key(key(BareKey::Char('?'))); // いったん閉じる
    state.handle_nav_key(preview_key());
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    assert!(lines.contains("mark read"), "{}", lines);
    for line in overlay_lines(&state, SIDEBAR) {
        assert!(!line.contains('…'), "プレビュー中: {}", line);
    }
}

#[test]
fn the_snapshot_carries_the_pane_name_and_its_contents() {
    let mut state = State::default();
    state.apply_preview_snapshot("alpha\n$ cargo test\nok");

    assert_eq!(state.preview_content.title, "alpha");
    assert_eq!(state.preview_content.lines, vec!["$ cargo test", "ok"]);
}

#[test]
fn the_snapshot_keeps_the_tail_when_it_overflows() {
    // 見たいのは直近の出力なので、溢れるときに落とすのは古い側
    let mut state = State::default();
    state.apply_preview_snapshot("alpha\none\ntwo\nthree\nfour");

    assert_eq!(state.preview_body(2), ["three", "four"]);
    assert_eq!(state.preview_body(9), ["one", "two", "three", "four"]);
}

#[test]
fn the_snapshot_drops_the_blank_tail() {
    // プロンプトの下の余白がそのまま入ってくると、画面いっぱいの空行で終わる
    let mut state = State::default();
    state.apply_preview_snapshot("alpha\nrunning\n\n   \n");

    assert_eq!(state.preview_body(4), ["running"]);
}

// --- 操作ヒントとヘルプオーバーレイ（要件: docs/requirements/nav-mode/） ---
//
// サイドバー幅は32文字（決定3）。ヘッダもヘルプもこの幅を前提に文言を決めてある

// Text の装飾を「レベル → 文字位置」に戻す。serialize() は
// 「レベルごとの位置列を `$` 区切りで並べ、そのあとに本文」の形。
// 色は 0-3、dim は 4（zellij-tile の Text の取り決め）
fn ink_levels(text: &Text) -> Vec<Vec<usize>> {
    let serialized = text.serialize();
    let Some((indices, _body)) = serialized.rsplit_once('$') else {
        return Vec::new();
    };
    indices
        .split('$')
        .map(|level| {
            level
                .split(',')
                .filter_map(|position| position.parse().ok())
                .collect()
        })
        .collect()
}

// そのレベルの装飾が乗っている文字位置（乗っていなければ空）
fn ink_at(text: &Text, level: usize) -> Vec<usize> {
    ink_levels(text).get(level).cloned().unwrap_or_default()
}

const DIM_LEVEL: usize = 4;
// unbold。zellij 側の基底スタイルが bold なので、落とさない＝太いまま残る
const UNBOLD_LEVEL: usize = 5;
// error_color。状態アイコン `error` と終了操作サブモードの警告色（決定35）
const ERROR_LEVEL: usize = 6;

// プラグインの configuration（決定40。取り込みは State::apply_config 1本）
fn plugin_config(settings: &[(&str, &str)]) -> BTreeMap<String, String> {
    settings
        .iter()
        .map(|(setting, value)| (setting.to_string(), value.to_string()))
        .collect()
}

// READMEが例示している direct-keys の移動キー（Alt Up / Alt Down / Alt g）。
// `toggle_cwd_key` は入れない — 幅の詰め方（矢印への退避・末尾の省略）を見る
// テストが多く、移動キー3つで既に幅32を超えるため
fn with_direct_keys(state: &mut State) {
    state.apply_config(&plugin_config(&[
        ("up_key", "alt+up"),
        ("down_key", "alt+down"),
        ("go_key", "alt+g"),
    ]));
}

#[test]
fn the_header_keeps_the_brand_in_every_mode() {
    // `▲ fujin` はモードによらず常に出る。変わるのは後ろに付くモードラベル
    //（要件: sidebar-header.feature）
    let mut state = searchable_state();
    state.nav_mode = false;
    assert_eq!(state.header_line(32).content(), "▲ fujin");

    state.nav_mode = true;
    assert_eq!(state.header_line(32).content(), "▲ fujin  [nav]");
    state.handle_nav_key(key(BareKey::Char('/')));
    assert_eq!(
        state.header_line(32).content(),
        "▲ fujin  [nav]",
        "検索サブモードは navモードの内側なのでラベルを変えない"
    );
}

#[test]
fn the_header_labels_the_triage_mode() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('p')));

    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin  [tri]");
}

#[test]
fn the_header_never_shows_the_waiting_count() {
    // 待ち件数のヘッダ表示は決定27で廃止した。集計そのものは残っている
    //（決定32でコマンド状態も含む形に広げた）
    let mut state = state_with_panes(4);
    set_agent_state(&mut state, 1, AgentState::Done);
    set_agent_state(&mut state, 2, AgentState::Blocked);
    set_agent_state(&mut state, 3, AgentState::Working);

    assert_eq!(state.waiting_count(), 2, "集計は残す");
    let header = state.header_line(SIDEBAR).content().to_string();
    assert!(!header.contains('2'), "件数は出さない: {}", header);
    assert!(!header.contains("waiting"), "{}", header);
}

#[test]
fn the_header_triangle_carries_the_mode_color() {
    // モードラベルを読まなくても三角の色だけでモードが判別できるようにする
    //（docs/concept/ui-design.md の「ヘッダ」）
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);

    // ツリー表示は dim のみ。色は乗らない
    state.nav_mode = false;
    let tree = state.header_line(SIDEBAR);
    assert!(
        ink_at(&tree, DIM_LEVEL).contains(&0),
        "{:?}",
        ink_levels(&tree)
    );
    for level in 0..DIM_LEVEL {
        assert!(
            ink_at(&tree, level).is_empty(),
            "レベル{}に色が乗っている",
            level
        );
    }

    // navモードはレベル2、トリアージモードはレベル0
    state.nav_mode = true;
    assert!(ink_at(&state.header_line(SIDEBAR), 2).contains(&0));
    state.handle_nav_key(key(BareKey::Char('p')));
    assert!(ink_at(&state.header_line(SIDEBAR), 0).contains(&0));
}

#[test]
fn the_mode_label_shares_the_triangle_color() {
    // ラベルは三角の裏取りなので同じ状態色で出す（決定27）。ブランド名だけは
    // dim のまま — 名前まで色を付けるとツリーの状態アイコンの色分けと喧嘩する
    let mut state = state_with_panes(2);
    state.nav_mode = true;

    let header = state.header_line(SIDEBAR);
    // "▲ fujin  [nav]": 0=三角 / 1-6=" fujin" / 7-8=空白 / 9-13="[nav]"
    assert_eq!(ink_at(&header, DIM_LEVEL), (1..=6).collect::<Vec<_>>());
    assert_eq!(
        ink_at(&header, 2),
        vec![0, 9, 10, 11, 12, 13],
        "三角とラベルに状態色"
    );
}

#[test]
fn the_frame_keeps_its_rows_across_every_mode_boundary() {
    // モードの入退場で枠の行数が変わると、ツリー全体がそのぶん上下にずれる
    //（要件: sidebar-header.feature / sidebar-footer.feature）
    let mut state = searchable_state();
    set_agent_state(&mut state, 1, AgentState::Done);
    state.nav_mode = false;
    let plain = {
        let rows = state.visible_rows();
        assert_frame(&rows, "通常表示");
        rows.len()
    };

    for enter in ["nav", "search", "triage", "jump", "help"] {
        state.nav_mode = true;
        match enter {
            "search" => {
                state.handle_nav_key(key(BareKey::Char('/')));
            }
            "triage" => {
                state.handle_nav_key(key(BareKey::Char('p')));
            }
            "jump" => {
                state.handle_nav_key(key(BareKey::Char('n')));
            }
            "help" => {
                state.handle_nav_key(key(BareKey::Char('?')));
            }
            _ => {}
        }
        assert_frame(&state.visible_rows(), enter);
        if enter == "nav" {
            assert_eq!(
                state.visible_rows().len(),
                plain,
                "navモードで行数が変わった"
            );
        }
        state.search = None;
        state.triage = None;
        state.jump = None;
        state.help_overlay = false;
    }
}

// 枠（境界線→ヘッダー→境界線→…→境界線→フッター）が崩れていないこと
fn assert_frame(rows: &[Row<'_>], label: &str) {
    assert!(
        matches!(
            (&rows[0], &rows[1], &rows[2]),
            (Row::Divider, Row::Header, Row::Divider)
        ),
        "{} で上の枠が崩れた",
        label
    );
    assert!(
        matches!(
            (
                &rows[rows.len() - 3],
                &rows[rows.len() - 2],
                &rows[rows.len() - 1]
            ),
            (Row::Divider, Row::Footer, Row::Blank)
        ),
        "{} で下の枠が崩れた",
        label
    );
}

#[test]
fn the_divider_leaves_the_right_margin() {
    // 境界線も他の行と同じく右端2セルを空ける（決定3の右マージン）
    let divider = divider_line(32);
    let line = divider.content();
    assert_eq!(line.chars().count(), 30);
    assert!(line.chars().all(|c| c == '─'), "{}", line);
    assert_eq!(ink_at(&divider, DIM_LEVEL).len(), 30, "dim で引く");
}

// --- フッター（決定27。要件: sidebar-footer.feature） ---

#[test]
fn the_footer_shows_the_configured_direct_keys() {
    // 決め打ちのキー表記はユーザーの設定と食い違いうるので、実際に割り当てた
    // キーの表記を configuration から受け取って出す（決定28）
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("up_key", "f1"),
        ("down_key", "f2"),
        ("go_key", "f3"),
    ]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  f1:up  f2:down  f3:jump");
}

#[test]
fn the_footer_shows_the_toggle_cwd_key_hint() {
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[("toggle_cwd_key", "alt+c")]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  alt+c:cwd");
}

#[test]
fn the_zellij_spelling_of_a_key_is_accepted_as_is() {
    // ユーザーは config.kdl の `bind "Alt u"` からコピーしてくる。そのまま
    // 貼れないと使いにくいので、空白区切りの zellij 表記も受けて画面表記へ均す
    let mut state = state_with_panes(2);
    for (written, shown) in [
        ("Alt u", "  alt+u:up"),
        ("alt+u", "  alt+u:up"),
        ("Ctrl Shift g", "  ctrl+shift+g:up"),
        ("PageUp", "  pgup:up"),
        ("Enter", "  enter:up"),
    ] {
        state.apply_config(&plugin_config(&[("up_key", written)]));
        assert_eq!(state.footer_line(SIDEBAR).content(), shown, "{}", written);
    }
}

#[test]
fn an_unreadable_key_setting_is_shown_as_written() {
    // 黙って落とすとヒントが1つ消えるだけになり、設定を間違えたことに
    // 気づけない。読めない値はそのまま出して気づかせる
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[("up_key", "Meta q")]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  Meta q:up");
}

#[test]
fn direct_key_hints_are_dropped_whole_rather_than_truncated() {
    // 幅28に3項目が収まらないとき、末尾を `…` で切ると `キー:動作` の形が壊れる。
    // 矢印へ落としても足りなければ、項目ごと省いて残りを正しく読ませる。
    // READMEが例示している Alt Up / Alt Down / Alt g がちょうどこれに当たる
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  alt+up:up  alt+down:down");
    assert!(!footer.contains('…'), "{}", footer);
}

#[test]
fn an_unset_direct_key_drops_only_its_own_hint() {
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("up_key", "alt+up"),
        ("down_key", "alt+down"),
    ]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert!(!footer.contains("jump"), "設定の無い項目は省く: {}", footer);
    assert!(footer.contains("alt+up:up"), "{}", footer);
    assert!(footer.contains("alt+down:down"), "{}", footer);
}

#[test]
fn an_empty_direct_key_setting_is_treated_as_unset() {
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[("up_key", "  "), ("down_key", "alt+d")]));

    assert_eq!(state.footer_line(SIDEBAR).content(), "  alt+d:down");
}

#[test]
fn long_direct_keys_fall_back_to_arrows() {
    // 実キーの長さはユーザー依存。英字表記が28セルに収まらないときだけ、
    // up/down を矢印へ落とす（jump に対応する矢印記号は無いので残す）。
    // `pgup:up  pgdn:down  alt+g:jump` は30セルで28に収まらないが、
    // 矢印にすれば26セルで3項目とも残る
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("up_key", "PageUp"),
        ("down_key", "PageDown"),
        ("go_key", "alt+g"),
    ]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  pgup:↑  pgdn:↓  alt+g:jump");
}

// --- 設定の取り込みと警告（決定40。要件: configuration） ---

#[test]
fn the_property_form_of_kdl_is_normalised() {
    // zellij はプロパティ書式（`show_cwd="true"`）の値を引用符込みで渡す
    //（子ノード書式は素の文字列）。取り込み口で剥がして同じ意味にする
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("show_cwd", "\"true\""),
        ("up_key", "\"Alt u\""),
    ]));

    assert!(state.show_cwd, "引用符付きでも真として読む");
    assert_eq!(state.footer_line(SIDEBAR).content(), "  alt+u:up");
    assert!(
        state.config_warnings.is_empty(),
        "正規化できたので警告は無い"
    );
}

#[test]
fn a_flag_setting_takes_only_true_and_false() {
    // 真偽値の受け口は広げない（決定40）。`1` や `yes` を真と見なすと、
    // 「効かない書き方」の一覧がユーザーからは推測できなくなる
    let mut state = state_with_panes(2);
    for (value, expected) in [("true", true), ("false", false)] {
        state.apply_config(&plugin_config(&[("show_cwd", value)]));
        assert_eq!(state.show_cwd, expected, "{}", value);
        assert!(state.config_warnings.is_empty(), "{}", value);
    }

    for value in ["1", "yes", "TRUE"] {
        state.apply_config(&plugin_config(&[("show_cwd", value)]));
        assert!(!state.show_cwd, "{}", value);
        assert_eq!(state.config_warnings, vec!["show_cwd"], "{}", value);
    }
}

#[test]
fn every_flag_setting_reaches_the_config() {
    // `SETTINGS` に Flag を足したのに `Config::set_flag` へ配線し忘れると、
    // その設定は警告も出さずに黙って無視される。既定値のずれもここで落ちる
    for setting in &SETTINGS {
        let Kind::Flag { default } = setting.kind else {
            continue;
        };
        assert_eq!(
            Config::parse(&plugin_config(&[(setting.key, &default.to_string())])),
            Config::default(),
            "既定と同じ値を書いたのに既定と違う結果になる: {}",
            setting.key
        );
        assert_ne!(
            Config::parse(&plugin_config(&[(setting.key, &(!default).to_string())])),
            Config::default(),
            "既定と逆の値が効いていない（配線漏れ）: {}",
            setting.key
        );
    }
}

#[test]
fn an_unusable_value_warns_in_the_footer() {
    // 既定値へ黙って倒すと、書いた設定が効かない理由が分からない（決定40）
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);
    state.apply_config(&plugin_config(&[("show_cwd", "1"), ("up_key", "alt+u")]));
    state.arm_config_warning();

    let footer = state.footer_line(SIDEBAR);
    assert_eq!(footer.content(), "  !bad value: show_cwd");
    assert!(
        !ink_at(&footer, ERROR_LEVEL).is_empty(),
        "警告色（error_color）で出す"
    );
    // ヘッダーの三角も同じ状態色に揃う（決定27）
    assert!(!ink_at(&state.header_line(SIDEBAR), ERROR_LEVEL).is_empty());
}

#[test]
fn several_unusable_values_fold_into_a_count() {
    // 幅32のフッターには全部は載らない。末尾を `…` で切らず、残りは件数に畳む
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("show_cwd", "1"),
        ("up_key", "Meta q"),
        ("down_key", "Meta w"),
    ]));
    state.arm_config_warning();

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  !bad values: show_cwd +2");
    assert!(!footer.contains('…'), "{}", footer);
}

#[test]
fn the_config_warning_gives_way_to_an_input_footer() {
    // 入力欄・確認プロンプト・ヘルプはそこに出ていないと操作が成立しない。
    // 警告が譲るのはこれらに対してだけで、静的なヒントには譲らない
    let mut state = searchable_state();
    state.apply_config(&plugin_config(&[("show_cwd", "1")]));
    state.arm_config_warning();

    state.nav_mode = true;
    assert_eq!(
        state.footer_line(SIDEBAR).content(),
        "  !bad value: show_cwd",
        "navモードの静的ヒントよりは警告が優先する"
    );

    state.handle_nav_key(key(BareKey::Char('/')));
    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert!(
        footer.starts_with("  /"),
        "検索クエリ入力欄が勝つ: {}",
        footer
    );
}

#[test]
fn the_config_warning_stops_after_its_deadline() {
    // 起動直後の一定時間だけ（決定40）。期限が切れたら通常の表示へ戻る
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);
    state.apply_config(&plugin_config(&[
        ("show_cwd", "1"),
        ("up_key", "alt+up"),
        ("down_key", "alt+down"),
    ]));
    state.arm_config_warning();
    assert!(state.showing_config_warning());

    // 期限までは出したまま
    assert!(!state.on_timer(CONFIG_WARNING_SECS / 2.0));
    assert!(state.showing_config_warning());

    // 期限を跨いだ Timer で消え、フッターを描き直させる
    assert!(state.on_timer(CONFIG_WARNING_SECS), "描き直しを要求する");
    assert!(!state.showing_config_warning());
    assert_eq!(
        state.footer_line(SIDEBAR).content(),
        "  alt+up:up  alt+down:down"
    );
}

#[test]
fn the_readme_settings_section_is_generated_from_the_table() {
    // 設定一覧の正本はコード（決定40）。README へ手で転記した表は必ずいつか
    // ずれるので、生成物との一致をテストで縛る。差分が出たら `make readme`
    use crate::config::doc::{settings_doc, splice, Lang};

    for (file, lang) in [("README.ja.md", Lang::Ja), ("README.md", Lang::En)] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        let current = std::fs::read_to_string(&path).unwrap();
        let updated = splice(&current, &settings_doc(lang))
            .unwrap_or_else(|| panic!("{} に settings の目印が無い", file));

        if std::env::var_os("UPDATE_README").is_some() {
            std::fs::write(&path, updated).unwrap();
            continue;
        }
        assert_eq!(
            current, updated,
            "{} の設定節が config.rs とずれている。`make readme` で再生成する",
            file
        );
    }
}

#[test]
fn the_nav_footer_shows_the_help_and_exit_hints() {
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);
    state.nav_mode = true;

    let footer = state.footer_line(32);
    let footer = footer.content();
    assert_eq!(footer, "  ?:help  esc:exit");
    assert!(
        !footer.contains("jump"),
        "モード中は direct-keys のヒントに戻らない"
    );
}

#[test]
fn the_triage_footer_says_esc_goes_back() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('p')));

    // Esc の行き先はツリー表示であってnavモードの退場ではない
    assert_eq!(state.footer_line(SIDEBAR).content(), "  ?:help  esc:back");
}

#[test]
fn the_footer_becomes_the_query_field_while_searching() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alp");

    let footer = state.footer_line(32);
    let footer = footer.content();
    assert!(footer.starts_with("  /alp"), "{}", footer);
    assert!(footer.contains("?:help"), "{}", footer);
    assert!(footer.chars().count() <= 32);
}

#[test]
fn a_long_query_wins_over_the_help_hint() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "0123456789");

    // 幅12には操作ヒントを置く余地が無い。入力中のクエリのほうを残す
    let footer = state.footer_line(12);
    let footer = footer.content();
    assert!(!footer.contains("?:help"), "{}", footer);
    assert!(footer.chars().count() <= 12);
}

#[test]
fn a_full_width_query_does_not_push_the_hint_off_the_edge() {
    // 右寄せの余白は表示セル幅で数える。文字数で数えると全角のクエリで
    // 操作ヒントが端からはみ出す（docs/concept/ui-design.md のレイアウト規則）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "日本語のペイン名");

    let footer = state.footer_line(32);
    assert!(
        unicode_width::UnicodeWidthStr::width(footer.content()) <= 32,
        "{}",
        footer.content()
    );
}

#[test]
fn the_footer_takes_over_the_help_overlays_closing_hint() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    assert_eq!(
        state.footer_line(SIDEBAR).content(),
        "  press any key to close"
    );
    // 本文側からは消してある（同じ文言を2箇所に出さない）
    for row in state.help_lines() {
        let line = state.help_line(row, SIDEBAR).content().to_string();
        assert!(!line.contains("press any key"), "{}", line);
    }
}

#[test]
fn the_footer_wears_the_state_color_including_its_keys() {
    // 「キーは常にレベル2固定」の色役割はフッターに限り例外（決定27）。
    // ヘッダーの三角とトーンを揃えるほうを取る
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('p')));

    let footer = state.footer_line(SIDEBAR);
    let indent = 2;
    let end = footer.content().chars().count();
    assert_eq!(
        ink_at(&footer, 0),
        (indent..end).collect::<Vec<_>>(),
        "キーも説明もトリアージの状態色: {}",
        footer.content()
    );

    // 非フォーカス時は落とした色（＝ヘッダーの三角と同じ dim）
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);
    let footer = state.footer_line(SIDEBAR);
    let end = footer.content().chars().count();
    assert_eq!(
        ink_at(&footer, DIM_LEVEL),
        (indent..end).collect::<Vec<_>>()
    );
}

#[test]
fn the_footer_never_runs_off_the_right_margin() {
    let mut state = searchable_state();
    with_direct_keys(&mut state);
    let mut widths = vec![state.footer_line(SIDEBAR)];
    state.nav_mode = true;
    widths.push(state.footer_line(SIDEBAR));
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "日本語のクエリで幅を埋める");
    widths.push(state.footer_line(SIDEBAR));

    for footer in widths {
        assert!(
            unicode_width::UnicodeWidthStr::width(footer.content()) <= SIDEBAR - 2,
            "右マージンを食う: {}",
            footer.content()
        );
    }
}

#[test]
fn question_mark_opens_the_help_overlay() {
    for key in [
        KeyWithModifier::new(BareKey::Char('?')),
        // Shift+/ として届く端末もある
        KeyWithModifier::new(BareKey::Char('?')).with_shift_modifier(),
    ] {
        let mut state = state_with_panes(3);
        state.nav_mode = true;
        state.handle_nav_key(key.clone());

        assert!(state.help_overlay, "{:?} でヘルプを開くべき", key);
        assert!(state.nav_mode, "ヘルプはnavモードの内側");
        assert_eq!(state.selected, 0, "選択は動かさない");
    }
}

#[test]
fn the_help_overlay_covers_the_tree_but_keeps_the_frame() {
    // 覆うのは content だけ。ヘッダー・フッターと境界線は出したままにする（決定27）
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let rows = state.visible_rows();
    assert_frame(&rows, "ヘルプ表示中");
    assert!(
        rows[HEADER_ROWS..rows.len() - FOOTER_ROWS]
            .iter()
            .all(|r| matches!(r, Row::Help(_))),
        "ヘルプ表示中はツリーを出さない"
    );
    // 行クリックの逆引きも当たらない（要件: click-to-focus と食い違わせない）
    assert!((0..rows.len()).all(|y| state.pane_at_row(y).is_none()));
}

// ヘルプオーバーレイに実際に載る行（モードごとのキー一覧＋共通の状態アイコン凡例）。
// 高さは全部載るだけ渡す — あふれ方の検証は別のテストで見る
fn overlay_lines(state: &State, cols: usize) -> Vec<String> {
    state
        .screen_rows(40)
        .iter()
        .filter_map(|row| match row {
            Row::Help(help) => Some(state.help_line(help, cols).content().to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_help_lines_fit_the_sidebar_width() {
    // 幅32（決定3）に収まらないと、キー列か説明のどちらかが … で消える
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('?')));
    for line in overlay_lines(&state, 32) {
        assert!(!line.contains('…'), "navモード: {}", line);
    }
    state.handle_nav_key(key(BareKey::Char('?'))); // いったん閉じる
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Char('?')));
    for line in overlay_lines(&state, 32) {
        assert!(!line.contains('…'), "検索サブモード: {}", line);
    }
    state.handle_nav_key(key(BareKey::Char('?'))); // いったん閉じる
    state.handle_nav_key(key(BareKey::Esc)); // 検索サブモードを抜ける
    state.handle_nav_key(key(BareKey::Char('n')));
    state.handle_nav_key(key(BareKey::Char('?')));
    for line in overlay_lines(&state, 32) {
        assert!(!line.contains('…'), "番号ジャンプサブモード: {}", line);
    }
    state.handle_nav_key(key(BareKey::Char('?'))); // いったん閉じる
    state.handle_nav_key(key(BareKey::Esc)); // 番号ジャンプサブモードを抜ける
    state.handle_nav_key(key(BareKey::Char('d')));
    state.handle_nav_key(key(BareKey::Char('?')));
    for line in overlay_lines(&state, 32) {
        assert!(!line.contains('…'), "終了操作サブモード: {}", line);
    }
}

#[test]
fn the_help_overlay_explains_each_termination() {
    // フッターの確認プロンプトに収まらない Esc の行き先もここで補う
    let mut state = termination_state(3);
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    for expected in ["close pane", "kill process", "kill & close", "cancel"] {
        assert!(lines.contains(expected), "{}: {}", expected, lines);
    }
    // 開いたヘルプは終了操作を実行せずに閉じ、確認プロンプトへ戻る
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(state.termination.is_some());
}

#[test]
fn the_nav_help_advertises_the_termination_key() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    assert!(lines.contains("terminate pane"), "{}", lines);
}

// 文字列に日本語（ひらがな・カタカナ・漢字）が混ざっているか。
// `▲` `▌` `…` や状態アイコンは英語の文言と一緒に使うので弾かない
fn has_japanese(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c))
}

// fujin が自分で書く行（ヘッダー・フッター・ヘルプ・通知行）。ペイン名・タブ名・
// cwd はユーザーのデータなので、日本語が入っていて当然で対象から外す
fn chrome_lines(state: &State) -> Vec<String> {
    let mut lines = vec![
        state.header_line(SIDEBAR).content().to_string(),
        state.footer_line(SIDEBAR).content().to_string(),
    ];
    lines.extend(overlay_lines(state, SIDEBAR));
    for row in state.visible_rows() {
        if let Row::Notice(notice) = row {
            lines.push(notice.to_string());
        }
    }
    lines
}

#[test]
fn the_sidebar_never_shows_japanese_text() {
    // UI文言は英語で統一する（docs/concept/ui-design.md の「文言」）。
    // コメントとドキュメントは日本語なので、画面に出る側だけを一度に見る
    let mut lines = vec![overflow_row(3, true, SIDEBAR).content().to_string()];

    let agent_state = |nav: bool| {
        let mut state = triage_state();
        set_agent_state(&mut state, 1, AgentState::Working);
        state.nav_mode = nav;
        state
    };
    // ツリー表示（非フォーカス）と navモード
    lines.extend(chrome_lines(&agent_state(false)));
    lines.extend(chrome_lines(&agent_state(true)));

    // 各サブモードと、そこで開いたヘルプオーバーレイ（状態アイコン凡例を含む）
    for entry in ['/', 'p', 'n', 'd'] {
        let mut state = agent_state(true);
        state.handle_nav_key(key(BareKey::Char(entry)));
        lines.extend(chrome_lines(&state));
        state.handle_nav_key(key(BareKey::Char('?')));
        lines.extend(chrome_lines(&state));
    }

    // 通知行（検索の0件・トリアージの対象なし）
    let mut no_hits = agent_state(true);
    no_hits.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut no_hits, "zzz");
    lines.extend(chrome_lines(&no_hits));
    let mut nothing_to_triage = triage_state(); // 状態を持つペインが1つも無い
    nothing_to_triage.handle_nav_key(key(BareKey::Char('p')));
    lines.extend(chrome_lines(&nothing_to_triage));

    for line in &lines {
        assert!(
            !has_japanese(line),
            "UI文言に日本語が混ざっている: {}",
            line
        );
    }
    // 通知行を拾えていることの確認（拾えていないと上のループが素通しになる）
    assert!(
        lines.iter().any(|l| l == "no matches"),
        "検索の0件通知が無い: {:?}",
        lines
    );
    assert!(
        lines.iter().any(|l| l == "nothing to triage"),
        "トリアージの対象なし通知が無い: {:?}",
        lines
    );
}

#[test]
fn the_help_overlay_ends_with_the_status_icon_legend() {
    // 要件: nav-mode-hints.feature「ヘルプオーバーレイに状態アイコン凡例が
    // 表示される」。README を見に行かなくても記号の意味を引けるようにする（決定25）
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let lines = overlay_lines(&state, SIDEBAR);
    let heading = lines
        .iter()
        .position(|line| line.trim() == "status")
        .expect("凡例の見出しがある");
    let legend: Vec<&String> = lines[heading + 1..]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();

    // 並び・アイコン・説明はすべて AgentState のテーブル由来（決定25）。
    // 末尾の1行だけは AgentState に無い「エージェントが乗っていないペイン」の印
    assert_eq!(legend.len(), AgentState::ALL.len() + 1, "{:?}", legend);
    for (line, agent_state) in legend.iter().zip(AgentState::ALL.iter()) {
        assert!(
            line.starts_with(&format!("  {}", agent_state.icon())),
            "アイコンがキー列の位置に出る: {}",
            line
        );
        assert!(
            line.ends_with(agent_state.label()),
            "状態名が説明として続く: {}",
            line
        );
    }
    let no_agent = legend.last().expect("未起動の行がある");
    assert!(
        no_agent.starts_with(&format!("  {}", NO_AGENT_ICON)) && no_agent.ends_with(NO_AGENT_LABEL),
        "エージェントが乗っていないペインの印も同じ節で引ける: {}",
        no_agent
    );
    // キー一覧より後ろに置く。先に読むべきは操作のほう
    let last_key = lines
        .iter()
        .position(|line| line.contains("this help"))
        .expect("キー一覧がある");
    assert!(last_key < heading);
}

#[test]
fn the_status_legend_lines_up_with_the_key_column() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let lines = overlay_lines(&state, SIDEBAR);
    let key_entry = lines
        .iter()
        .find(|line| line.contains("this help"))
        .expect("キー一覧がある");
    let legend = lines
        .iter()
        .find(|line| line.ends_with("working"))
        .expect("凡例がある");
    assert_eq!(
        column_at(legend, "working"),
        column_at(key_entry, "this help"),
        "説明の開始位置が揃っている: {:?} / {:?}",
        legend,
        key_entry
    );
}

#[test]
fn the_help_headings_leave_the_mode_name_to_the_header() {
    // ヘッダーの `▲ fujin [tri]` と重複するので、オーバーレイの見出しは
    // 節名だけにする（決定30）
    let mut nav = searchable_state();
    let mut search = searchable_state();
    search.handle_nav_key(key(BareKey::Char('/')));
    let mut jump = searchable_state();
    jump.handle_nav_key(key(BareKey::Char('n')));
    let mut triage = triage_state();
    set_agent_state(&mut triage, 1, AgentState::Working);
    triage.handle_nav_key(key(BareKey::Char('p')));

    for (label, state) in [
        ("nav", &mut nav),
        ("search", &mut search),
        ("jump", &mut jump),
        ("triage", &mut triage),
    ] {
        state.handle_nav_key(key(BareKey::Char('?')));
        let lines = overlay_lines(state, SIDEBAR);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("  keys"),
            "{}",
            label
        );
        for line in &lines {
            assert!(
                !line.contains('['),
                "{}: モード名が残っている: {}",
                label,
                line
            );
        }
    }
}

#[test]
fn the_key_column_fits_the_widest_key_of_the_mode() {
    // 17セル固定をやめ、モードごとの実測最大＋空白2にした（決定30）。
    // いちばん長いキーのためだけに全行が空白を払う状態を解消する
    let mut nav = state_with_panes(2);
    nav.nav_mode = true;
    nav.handle_nav_key(key(BareKey::Char('?')));
    let nav_lines = overlay_lines(&nav, SIDEBAR);
    let widest = nav_lines
        .iter()
        .find(|line| line.ends_with("jump & exit"))
        .expect("いちばん長いキーの行がある");
    // 左マージン2 + "enter"(5) + 空白2
    assert_eq!(column_at(widest, "jump & exit"), 9);

    // キーが長いモードでは列も広がる（`backspace` が最長）
    let mut search = searchable_state();
    search.handle_nav_key(key(BareKey::Char('/')));
    search.handle_nav_key(key(BareKey::Char('?')));
    let search_lines = overlay_lines(&search, SIDEBAR);
    let delete = search_lines
        .iter()
        .find(|line| line.ends_with("delete char"))
        .expect("backspace の行がある");
    assert_eq!(column_at(delete, "delete char"), 2 + 9 + 2);
}

#[test]
fn the_no_agent_legend_carries_no_decoration() {
    // 他の凡例行はアイコンに状態色が乗るが、この行は状態ではないので何も乗せない。
    // dim も掛けない — 掛けると zellij が dim の解除に出す `\e[22m` が端末側で
    // bold まで消し、説明文だけ他の行と太さが揃わなくなる（実測。
    // docs/dev/implementation-notes.md）
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let overlay: Vec<Text> = state
        .screen_rows(40)
        .iter()
        .filter_map(|row| match row {
            Row::Help(help) => Some(state.help_line(help, SIDEBAR)),
            _ => None,
        })
        .collect();
    let line = overlay
        .iter()
        .find(|text| text.content().ends_with(NO_AGENT_LABEL))
        .expect("未起動の凡例がある");
    assert!(ink_at(line, DIM_LEVEL).is_empty(), "{}", line.content());
    assert!(ink_at(line, UNBOLD_LEVEL).is_empty(), "{}", line.content());
    for level in [0, 1, 2, 3, ERROR_LEVEL] {
        assert!(
            ink_at(line, level).is_empty(),
            "凡例のアイコンに状態色は乗せない（レベル{}）: {}",
            level,
            line.content()
        );
    }
}

#[test]
fn the_status_legend_carries_the_state_colors() {
    // 凡例の目的は意味と色を結びつけることなので、アイコンにだけは状態色を乗せる
    //（キーは常にレベル2固定、というヘルプの色役割の例外・決定25）
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let overlay: Vec<Text> = state
        .screen_rows(40)
        .iter()
        .filter_map(|row| match row {
            Row::Help(help) => Some(state.help_line(help, SIDEBAR)),
            _ => None,
        })
        .collect();
    for agent_state in AgentState::ALL {
        let line = overlay
            .iter()
            .find(|text| text.content().ends_with(agent_state.label()))
            .unwrap_or_else(|| panic!("{} の凡例がある", agent_state.label()));
        // 色が乗るのは左マージン(2)の直後、アイコン1文字だけ
        assert_eq!(
            ink_at(line, agent_state.color()),
            vec![2],
            "{} のアイコンに状態色が乗る",
            agent_state.label()
        );
    }
}

#[test]
fn every_agent_state_gets_a_color_of_its_own() {
    // 要件: agent-status-icon.feature「状態ごとに異なる色で判別できる」。
    // `error` が `blocked` とレベル3で重複していたのを解消した（決定25）
    let levels: Vec<usize> = AgentState::ALL.iter().map(|s| s.color()).collect();
    let mut unique = levels.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), levels.len(), "色が重複している: {:?}", levels);
    assert_eq!(
        AgentState::Error.color(),
        6,
        "error はテーマのエラー色（レベル6）"
    );
}

#[test]
fn the_status_legend_shows_up_in_every_overlay() {
    // ツリーにもトリアージ一覧にも状態アイコンが出るので、どのモードの
    // オーバーレイからでも同じ表を引けるようにする
    let mut state = triage_state();
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('p')));
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(
        overlay_lines(&state, SIDEBAR)
            .iter()
            .any(|line| line.trim() == "status"),
        "トリアージモード"
    );

    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(
        overlay_lines(&state, SIDEBAR)
            .iter()
            .any(|line| line.trim() == "status"),
        "検索サブモード"
    );

    let mut state = jump_state(3);
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(
        overlay_lines(&state, SIDEBAR)
            .iter()
            .any(|line| line.trim() == "status"),
        "番号ジャンプサブモード"
    );
}

#[test]
fn a_short_sidebar_marks_the_hidden_help_lines() {
    // オーバーレイは「任意のキーで閉じる」のでスクロール用のキーを持てない。
    // 収まらないぶんはツリーと同じあふれマーカーで、隠れていることだけ示す
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    state.render(16, SIDEBAR);

    let markers = overflow_markers(&state, 16);
    assert_eq!(markers.len(), 1, "下端にだけ出る: {:?}", markers);
    assert!(!markers[0].1, "上には隠れない（先頭から出す）");
}

#[test]
fn the_help_entries_line_up_under_a_left_margin() {
    let state = state_with_panes(2);
    // 左端に貼り付けず余白を空ける。説明の開始位置は行をまたいで揃える
    let lines: Vec<String> = state
        .help_lines()
        .iter()
        .map(|row| state.help_line(row, 32).content().to_string())
        .collect();
    let entries: Vec<&String> = lines.iter().filter(|l| l.contains("  ")).collect();
    assert!(!entries.is_empty());
    for line in &lines {
        if line.is_empty() {
            continue;
        }
        assert!(line.starts_with("  "), "左マージンが無い: {}", line);
    }
    let jump = lines
        .iter()
        .find(|l| l.contains("jump & exit"))
        .expect("ジャンプの行がある");
    let search = lines.iter().find(|l| l.contains("search")).unwrap();
    assert_eq!(
        jump.find("jump & exit"),
        search.find("search"),
        "説明の開始位置が揃っている: {:?} / {:?}",
        jump,
        search
    );
}

#[test]
fn any_key_closes_the_help_overlay_without_acting_on_it() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    state.handle_nav_key(key(BareKey::Char('j')));

    assert!(!state.help_overlay);
    assert!(state.nav_mode, "閉じてもnavモードは継続する");
    assert_eq!(state.selected, 0, "閉じるためのキーは操作として解釈しない");
}

#[test]
fn modified_keys_only_close_the_help_overlay() {
    // 安全弁（決定12）より手前で閉じる。閲覧をやめただけで退場させるのは筋が通らない
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier());

    assert!(!state.help_overlay);
    assert!(state.nav_mode);
}

#[test]
fn the_help_overlay_opens_from_the_search_submode_too() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alp");
    state.handle_nav_key(key(BareKey::Char('?')));

    assert!(state.help_overlay);
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("alp"),
        "? はクエリに入らない"
    );
    let lines = overlay_lines(&state, SIDEBAR);
    assert_eq!(lines.first().map(String::as_str), Some("  keys"));
    assert!(
        lines.iter().any(|line| line.contains("filter panes")),
        "検索サブモードのキーを出す: {:?}",
        lines
    );

    // 閉じたら開く前の表示（検索サブモード）に戻る。Esc も閉じるだけで、
    // 検索サブモードの取り消しにはならない
    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.help_overlay);
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("alp"),
        "検索サブモードは中断されない"
    );
}

#[test]
fn leaving_nav_mode_closes_the_help_overlay() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.help_overlay = true;
    state.leave_nav_mode();

    assert!(
        !state.help_overlay,
        "開いたまま退場するとツリー表示へ戻れない"
    );
}

// --- フォーカス同期（要件: docs/requirements/focus-sync/） ---
//
// 実フォーカスの問い合わせ（refresh_focus）はここでは呼べないので、
// 観測結果を畳んだ State::focused_pane を直接立てて先のロジックを見る。

#[test]
fn nav_entry_starts_from_the_focused_pane() {
    // 退場の記録が無い初回の入場は、常にフォーカス中のペインから始まる
    let mut state = state_with_panes(3);
    state.focused_pane = Some(3);

    state.enter_nav_mode();
    assert_eq!(state.selectable[state.selected].pane_id, 3);
}

#[test]
fn nav_entry_follows_focus_moved_after_leaving() {
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j'))); // pane2 まで探索して
    state.handle_nav_key(key(BareKey::Esc)); // 退場（フォーカスは pane1 のまま）

    // 退場後に通常のzellij操作でフォーカスが動いた
    state.focused_pane = Some(3);
    state.enter_nav_mode();
    assert_eq!(
        state.selectable[state.selected].pane_id, 3,
        "作業場所が変わったら現在のフォーカスから始める"
    );
}

#[test]
fn nav_entry_restores_the_exploring_position_when_focus_did_not_move() {
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.selectable[state.selected].pane_id, 2);
    state.handle_nav_key(key(BareKey::Esc));

    state.enter_nav_mode();
    assert_eq!(
        state.selectable[state.selected].pane_id, 2,
        "フォーカスを動かしていないなら探索位置を復元する"
    );
}

#[test]
fn leaving_nav_mode_puts_the_highlight_back_on_the_focused_pane() {
    // navモード外のハイライトは常に実フォーカスと一致する
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(key(BareKey::Esc));

    assert_eq!(state.selectable[state.selected].pane_id, 1);
}

#[test]
fn jumping_leaves_the_highlight_on_the_jump_target() {
    // ジャンプでは実フォーカスが選択行へ移るので、選択は戻さない
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(key(BareKey::Enter));
    assert!(!state.nav_mode);
    assert_eq!(state.selectable[state.selected].pane_id, 2);

    // ジャンプ後のフォーカスは選択行と一致するので、次の入場もそこから
    state.focused_pane = Some(2);
    state.enter_nav_mode();
    assert_eq!(state.selectable[state.selected].pane_id, 2);
}

#[test]
fn select_pane_id_ignores_unknown_panes() {
    let mut state = state_with_panes(3);
    state.selected = 1;

    assert!(!state.select_pane_id(99), "一覧に無いペインでは動かさない");
    assert_eq!(state.selected, 1);
    assert!(!state.select_pane_id(2), "同じ行なら「動いた」にはしない");
    assert!(state.select_pane_id(3));
    assert_eq!(state.selected, 2);
}

#[test]
fn focus_follows_only_when_it_moved() {
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);

    assert_eq!(state.focus_to_follow(Some(2), false), Some(2));
    // 動いていないフォーカスで引き直すと、fujin_up / fujin_down で動かした
    // 選択がペイン名の変化のたびに巻き戻る
    assert_eq!(state.focus_to_follow(Some(1), false), None);
    // プラグインペインへのフォーカスは selectable に無いので対象外
    assert_eq!(state.focus_to_follow(None, false), None);

    // navモード中の選択は探索位置なので追従させない
    state.nav_mode = true;
    assert_eq!(state.focus_to_follow(Some(2), false), None);
}

#[test]
fn a_newly_visible_instance_re_reads_the_focus() {
    // 非可視の間は PaneUpdate が届かずキャッシュが凍る。タブを切り替えて
    // 戻ると「フォーカスは動いていない」と誤判定し、その間に決定13の同期で
    // 受け取った別タブの選択が残ったままになる（実測での症状）
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);

    assert_eq!(state.focus_to_follow(Some(1), true), Some(1));
    // 可視化の合図でも、navモード中とプラグインペインは対象外のまま
    assert_eq!(state.focus_to_follow(None, true), None);
    state.nav_mode = true;
    assert_eq!(state.focus_to_follow(Some(1), true), None);
}

#[test]
fn a_focus_move_during_nav_mode_leaves_the_mode() {
    // マウスでペインを選ぶとキー横取り中でもフォーカスが動く。抜けないと
    // クリックした先で j/k がサイドバー操作として食われ続ける
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.selectable[state.selected].pane_id, 2);

    // refresh_focus() はホスト問い合わせを含むのでここでは呼べない。
    // 「フォーカスが動いた」と観測した後の処理だけを再現する
    state.focused_pane = Some(3);
    state.leave_nav_mode();

    assert!(!state.nav_mode);
    assert_eq!(
        state.selectable[state.selected].pane_id, 3,
        "ハイライトは新しい実フォーカスへ揃う"
    );
}

#[test]
fn the_focused_terminal_comes_from_the_tiled_layer() {
    // 召喚インスタンス（フローティング）は自分がフローティング層のフォーカスを
    // 持つので、問い合わせでは作業ペインが分からない。PaneInfo.is_focused は
    // レイヤごとなので、タイル層のフォーカスを一覧から拾い直す
    let focused_terminal = PaneInfo {
        is_focused: true,
        ..terminal_pane(2, "pane2")
    };
    let focused_floating_sidebar = PaneInfo {
        is_focused: true,
        ..floating_plugin_pane(9, "fujin.wasm")
    };
    let state = State {
        panes: Some(manifest(vec![(
            0,
            vec![
                terminal_pane(1, "pane1"),
                focused_terminal,
                focused_floating_sidebar,
            ],
        )])),
        ..Default::default()
    };

    assert_eq!(state.focused_terminal_in_tab(0), Some(2));
    assert_eq!(state.focused_terminal_in_tab(1), None, "存在しないタブ");
}

#[test]
fn a_focused_floating_terminal_is_used_when_no_tile_is_focused() {
    // フローティングのターミナルで作業していた場合の受け皿
    let focused_floating_terminal = PaneInfo {
        is_focused: true,
        is_floating: true,
        ..terminal_pane(3, "floating")
    };
    let state = State {
        panes: Some(manifest(vec![(
            0,
            vec![terminal_pane(1, "pane1"), focused_floating_terminal],
        )])),
        ..Default::default()
    };

    assert_eq!(state.focused_terminal_in_tab(0), Some(3));
}

#[test]
fn owns_tab_only_matches_the_tab_holding_this_instance() {
    // 権威判定（決定14）の材料。フォーカス中のタブに自分が居るかだけを見る
    let state = State {
        own_plugin_id: Some(9),
        panes: Some(manifest(vec![
            (
                0,
                vec![terminal_pane(1, "pane1"), plugin_pane(9, "fujin.wasm")],
            ),
            (1, vec![terminal_pane(2, "pane2")]),
        ])),
        ..Default::default()
    };

    assert!(state.owns_tab(0));
    assert!(!state.owns_tab(1));
    assert!(!state.owns_tab(2), "存在しないタブ");
}

// --- フォーカスの預かり（決定34） ---
//
// `set_selectable()` / `focus_plugin_pane()` / `focus_pane_with_id()` は副作用
// だけのホストコマンドなのでテストから呼んでも安全だが、効き目は観測できない。
// したがって「誰からフォーカスを預かっているか」を畳んだ State::focus_parked で
// 預かりと返却の対応を見る。

// 預かっている相手のペインID。テストから見たいのはこれだけなので、
// State 側にアクセサは置かない
fn parked_pane(state: &State) -> Option<u32> {
    state.focus_parked.map(|parked| parked.pane_id)
}

// フォーカスを預かれる状態（実フォーカスがターミナル側にあり、自分のIDも判明済み）
fn state_ready_to_park(count: u32) -> State {
    let mut state = state_with_panes(count);
    state.own_plugin_id = Some(9);
    state.focused_pane = Some(1);
    state.focus_on_terminal = true;
    state
}

#[test]
fn entering_nav_mode_parks_the_focus_on_the_sidebar() {
    let mut state = state_ready_to_park(3);

    state.enter_nav_mode();
    assert_eq!(parked_pane(&state), Some(1));

    state.handle_nav_key(key(BareKey::Esc));
    assert_eq!(parked_pane(&state), None, "退場でフォーカスを返す");
}

#[test]
fn jumping_out_of_nav_mode_also_releases_the_parked_focus() {
    // 退場は2系統ある（exit_nav_mode / leave_nav_mode）。ジャンプ側でも手放す
    let mut state = state_ready_to_park(3);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(key(BareKey::Enter));

    assert!(!state.nav_mode);
    assert_eq!(parked_pane(&state), None);
}

#[test]
fn the_focus_goes_back_to_the_pane_it_was_taken_from() {
    // 探索で選択を動かしても、返す相手は預かった当のペイン
    let mut state = state_ready_to_park(3);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(parked_pane(&state), Some(1));

    state.handle_nav_key(key(BareKey::Esc));
    assert_eq!(parked_pane(&state), None);

    // 返したあとの入場は、そのときのフォーカスから預かり直す
    state.focused_pane = Some(2);
    state.enter_nav_mode();
    assert_eq!(parked_pane(&state), Some(2));
}

#[test]
fn a_floating_work_pane_is_remembered_as_floating() {
    // 返すときの should_float_if_hidden。タイル扱いで返すと、フローティング層が
    // 隠れているタブでは戻り先に届かない（`focus_selected` と同じ罠）
    let mut state = state_ready_to_park(2);
    if let Some(panes) = state.panes.as_mut().and_then(|m| m.panes.get_mut(&0)) {
        panes[0].is_floating = true;
    }
    state.rebuild_selectable();

    state.enter_nav_mode();
    assert_eq!(
        state.focus_parked.map(|parked| parked.is_floating),
        Some(true)
    );
}

#[test]
fn a_summoned_instance_does_not_park_anything() {
    // 召喚インスタンスは最初から自分がフォーカスを持っている（決定16）
    let mut state = state_ready_to_park(3);
    state.summoned = true;

    state.enter_nav_mode();
    assert_eq!(parked_pane(&state), None);
}

#[test]
fn the_focus_is_left_alone_when_a_plugin_pane_already_holds_it() {
    // フォーカス枠はセッション内で1枚しか点かない（実測）。プラグインペイン側に
    // フォーカスがあるなら作業ペインの枠は既に非フォーカス色で、預かる理由が無い
    let mut state = state_ready_to_park(3);
    state.focus_on_terminal = false;

    state.enter_nav_mode();
    assert_eq!(parked_pane(&state), None);
}

#[test]
fn a_park_counts_as_lost_only_after_it_was_confirmed() {
    // フォーカスの移動は非同期。預けた直後は「自分にフォーカスが無い」のが正常で、
    // ここで持って行かれたと判定すると入場した直後に退場してしまう
    let mut state = state_ready_to_park(3);
    state.enter_nav_mode();

    assert!(
        !state.park_taken_over(false),
        "預かりの成立を観測する前は判定しない"
    );
    assert!(!state.park_taken_over(true));

    // 一度自分にフォーカスが来たのを観測した後は、離れたら持って行かれた扱い
    if let Some(parked) = state.focus_parked.as_mut() {
        parked.confirmed = true;
    }
    assert!(state.park_taken_over(false));
    assert!(!state.park_taken_over(true));

    // 預かっていなければ、そもそも判定の対象外
    state.release_parked_focus(false);
    assert!(!state.park_taken_over(false));
}

#[test]
fn the_focus_falls_back_to_the_selected_row_when_the_parked_pane_is_gone() {
    // navモード中に預かった相手が閉じられたケース。unselectable な自分に
    // フォーカスを残すと、横取りも解いた後なのでキーの行き先が無くなる
    let mut state = state_ready_to_park(3);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j'))); // 選択は pane2 へ
    let parked = state.focus_parked.expect("預かっているはず");

    // pane1（預かった相手）が閉じられた
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(2, "pane2"), terminal_pane(3, "pane3")],
    )]));
    state.rebuild_selectable();
    state.select_pane_id(2);

    assert_eq!(
        state.refocus_target(parked),
        Some((2, false)),
        "閉じられていたら選択行へ返す"
    );
    // 生きていれば当のペインへ返す
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(1, "pane1"), terminal_pane(2, "pane2")],
    )]));
    state.rebuild_selectable();
    assert_eq!(state.refocus_target(parked), Some((1, false)));
}

#[test]
fn an_interrupted_nav_mode_does_not_take_the_focus_back() {
    // マウスで別のペインを選んだ場合。預かりを「返して」しまうと、ユーザーが
    // 選んだ先からフォーカスを奪い返すことになる
    let mut state = state_ready_to_park(3);
    state.enter_nav_mode();
    assert_eq!(parked_pane(&state), Some(1));

    // refresh_focus() はホスト問い合わせを含むのでここでは呼べない。
    // 「フォーカスが動いた」と観測した後の処理だけを再現する
    state.release_parked_focus(false);
    state.focused_pane = Some(3);
    state.leave_nav_mode();

    assert!(!state.nav_mode);
    assert_eq!(parked_pane(&state), None);
    assert_eq!(
        state.selectable[state.selected].pane_id, 3,
        "ハイライトは新しい実フォーカスへ揃う"
    );
}

// --- 検索サブモード（要件: docs/requirements/search-explorer/） ---
//
// match_one / match_pane の単体テストは src/search.rs 側にある。
// ここでは State を通したキー処理と絞り込みの追従を見る。

// タブ2枚（tab1: alpha, bravo / tab2: charlie）。bravo だけ cwd を持つ
fn searchable_state() -> State {
    let mut state = State {
        tabs: vec![tab(0, true), tab(1, false)],
        panes: Some(manifest(vec![
            (
                0,
                vec![terminal_pane(1, "alpha"), terminal_pane(2, "bravo")],
            ),
            (1, vec![terminal_pane(3, "charlie")]),
        ])),
        permissions_granted: true,
        nav_mode: true,
        ..Default::default()
    };
    state.pane_cwds.insert(2, "/work/fujin".to_string());
    state.rebuild_selectable();
    state
}

fn key(bare: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare)
}

fn type_query(state: &mut State, query: &str) {
    for c in query.chars() {
        state.handle_nav_key(key(BareKey::Char(c)));
    }
}

#[test]
fn slash_enters_search_with_an_empty_query_matching_everything() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));

    let search = state.search.as_ref().unwrap();
    assert!(state.nav_mode, "検索サブモードはnavモードの内側");
    assert_eq!(search.query, "");
    assert_eq!(search.hits.len(), 3, "空クエリは全件一致");
    assert_eq!(search.cursor, Some(1), "カーソルは検索前の選択から始まる");
}

#[test]
fn printable_chars_feed_the_query_not_the_selection() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    // navモードでは j は移動キーだが、検索サブモード中はクエリになる
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.search.as_ref().unwrap().query, "j");
    assert_eq!(state.selected, 0, "選択は動かない");
}

#[test]
fn backspace_deletes_the_last_char_and_refilters() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    // "al" だと charlie（ch"a"r"l"ie）にもサブシーケンス一致するので "alp" を使う
    type_query(&mut state, "alpx");
    assert!(state.search.as_ref().unwrap().hits.is_empty());

    state.handle_nav_key(key(BareKey::Backspace));
    let search = state.search.as_ref().unwrap();
    assert_eq!(search.query, "alp");
    assert_eq!(search.hits.keys().copied().collect::<Vec<_>>(), vec![1]);
}

#[test]
fn query_matches_titles() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alpha");
    let search = state.search.as_ref().unwrap();
    assert_eq!(search.hits.keys().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(search.cursor, Some(1));
}

#[test]
fn a_tab_name_match_keeps_all_its_panes() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "tab1");
    let search = state.search.as_ref().unwrap();
    // tab1 配下の全ペインが残り、tab2 配下は消える
    assert_eq!(search.hits.keys().copied().collect::<Vec<_>>(), vec![1, 2]);
    assert!(search
        .hits
        .values()
        .all(|h| h.field == crate::search::Field::Tab));
}

#[test]
fn cwd_matches_even_when_show_cwd_is_off() {
    let mut state = searchable_state();
    assert!(!state.show_cwd);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");
    let search = state.search.as_ref().unwrap();
    // cwd を持つ bravo だけが当たる。cwd の無いペインは cwd では一致しない
    assert_eq!(search.hits.keys().copied().collect::<Vec<_>>(), vec![2]);
    assert_eq!(search.hits[&2].field, crate::search::Field::Cwd);
}

#[test]
fn cursor_moves_in_tree_order_and_stops_at_the_edges() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));

    state.handle_nav_key(key(BareKey::Down));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(2));
    state.handle_nav_key(key(BareKey::Tab));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(3));
    // 末尾で止まる
    state.handle_nav_key(key(BareKey::Down));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(3));

    state.handle_nav_key(key(BareKey::Up));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(2));
    state.handle_nav_key(key(BareKey::Tab).with_shift_modifier());
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(1));
    // 先頭で止まる
    state.handle_nav_key(key(BareKey::Up));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(1));

    assert!(state.nav_mode, "カーソル移動でモードを抜けない");
    assert!(state.search.is_some());
}

#[test]
fn esc_is_two_staged_and_does_not_close_a_summoned_instance() {
    let mut state = searchable_state();
    state.summoned = true;
    state.own_plugin_id = Some(9);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "charlie");

    // 1段目: クエリ破棄のみ。召喚インスタンスでも navモードに留まる
    state.handle_nav_key(key(BareKey::Esc));
    assert!(state.search.is_none());
    assert!(
        state.nav_mode,
        "検索サブモードのEscでnavモードごと抜けてはいけない"
    );

    // 2段目: navモードから退場（召喚インスタンスならここで自分を閉じる）
    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.nav_mode);
}

#[test]
fn esc_restores_the_selection_saved_on_entry() {
    let mut state = searchable_state();
    state.selected = 1; // bravo
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Down));
    state.handle_nav_key(key(BareKey::Down));

    // 検索サブモード中に先頭へペインが増えてインデックスがずれても、ペインIDで戻す
    state.panes = Some(manifest(vec![
        (
            0,
            vec![
                terminal_pane(9, "newcomer"),
                terminal_pane(1, "alpha"),
                terminal_pane(2, "bravo"),
            ],
        ),
        (1, vec![terminal_pane(3, "charlie")]),
    ]));
    state.rebuild_selectable();

    state.handle_nav_key(key(BareKey::Esc));
    assert_eq!(state.selectable[state.selected].pane_id, 2);
}

#[test]
fn enter_jumps_and_leaves_nav_mode_entirely() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "charlie");

    state.handle_nav_key(key(BareKey::Enter));
    assert!(!state.nav_mode, "確定はnavモードごと抜ける");
    assert!(state.search.is_none(), "クエリは破棄される");
    assert_eq!(state.selectable[state.selected].pane_id, 3);
}

#[test]
fn enter_with_no_hits_does_nothing() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "zzz");
    assert!(state.search.as_ref().unwrap().hits.is_empty());

    state.handle_nav_key(key(BareKey::Enter));
    assert!(state.nav_mode, "0件ヒットでは検索サブモードに留まる");
    assert!(state.search.is_some());
    assert_eq!(state.selected, 0, "ジャンプは起きない");
}

#[test]
fn modified_keys_leave_nav_mode_from_search_too() {
    // 安全弁は最上位まで効かせる（決定12と同様）
    for key in [
        KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier(),
        KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier(),
    ] {
        let mut state = searchable_state();
        state.handle_nav_key(KeyWithModifier::new(BareKey::Char('/')));
        state.handle_nav_key(key.clone());
        assert!(!state.nav_mode, "{:?} でnavモードごと抜けるべき", key);
        assert!(state.search.is_none(), "{:?} で検索状態は破棄すべき", key);
    }
}

#[test]
fn reentering_search_starts_with_an_empty_query() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alpha");
    state.handle_nav_key(key(BareKey::Esc));

    state.handle_nav_key(key(BareKey::Char('/')));
    assert_eq!(state.search.as_ref().unwrap().query, "");
}

#[test]
fn the_cursor_follows_its_pane_through_list_updates() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Down)); // cursor = bravo(2)

    // 別タブでペインが増えても、カーソルは同じペインに留まる
    state.panes = Some(manifest(vec![
        (
            0,
            vec![terminal_pane(1, "alpha"), terminal_pane(2, "bravo")],
        ),
        (
            1,
            vec![terminal_pane(3, "charlie"), terminal_pane(4, "delta")],
        ),
    ]));
    state.rebuild_selectable();
    let search = state.search.as_ref().unwrap();
    assert_eq!(search.cursor, Some(2));
    assert_eq!(search.hits.len(), 4, "一覧の更新で絞り込みも引き直す");
}

#[test]
fn the_cursor_falls_back_to_the_first_hit_when_its_pane_closes() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Down)); // cursor = bravo(2)

    state.panes = Some(manifest(vec![
        (0, vec![terminal_pane(1, "alpha")]),
        (1, vec![terminal_pane(3, "charlie")]),
    ]));
    state.rebuild_selectable();
    assert_eq!(
        state.search.as_ref().unwrap().cursor,
        Some(1),
        "消えたら絞り込み結果の先頭へ寄せる"
    );
}

#[test]
fn refiltering_with_no_hits_clears_the_cursor() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alpha");
    state.panes = Some(manifest(vec![(1, vec![terminal_pane(3, "charlie")])]));
    state.rebuild_selectable();
    assert_eq!(state.search.as_ref().unwrap().cursor, None);
}

// --- shift_highlight_indices（ハイライト位置の変換） ---

#[test]
fn highlight_indices_are_shifted_by_the_label_prefix() {
    // "▌ ○ alpha" — タイトルは4文字目から
    assert_eq!(
        shift_highlight_indices(&[0, 2], 4, "▌ ○ alpha", 9),
        vec![4, 6]
    );
}

#[test]
fn highlight_indices_beyond_the_truncation_are_dropped() {
    // 元9文字を6文字に切り詰めると、末尾は … になる（実位置5が省略記号）
    let truncated = truncate("▌ ○ alpha", 6);
    assert_eq!(truncated.chars().count(), 6);
    assert_eq!(
        shift_highlight_indices(&[0, 1, 2, 3, 4], 4, &truncated, 9),
        vec![4],
        "省略記号とその先の位置には色を乗せない"
    );
}

// --- 兄弟インスタンスの検出（決定13） ---

#[test]
fn own_plugin_url_is_learned_from_the_manifest() {
    let url = "file:/x/fujin.wasm";
    let mut state = State {
        own_plugin_id: Some(5),
        panes: Some(manifest(vec![(0, vec![plugin_pane(5, url)])])),
        ..Default::default()
    };
    state.learn_own_plugin_url();
    assert_eq!(state.own_plugin_url.as_deref(), Some(url));
}

#[test]
fn siblings_are_the_same_plugin_in_other_tabs() {
    let url = "file:/x/fujin.wasm";
    let mut state = State {
        own_plugin_id: Some(5),
        own_plugin_url: Some(url.to_string()),
        panes: Some(manifest(vec![
            (0, vec![plugin_pane(5, url), terminal_pane(1, "shell")]),
            (1, vec![plugin_pane(6, url)]),
            // 別プラグイン（status-bar 等）は兄弟ではない
            (2, vec![plugin_pane(7, "zellij:status-bar")]),
        ])),
        ..Default::default()
    };
    state.push_state_to_new_siblings();
    assert_eq!(
        state.known_siblings.iter().copied().collect::<Vec<_>>(),
        vec![6]
    );
}

// --- 臨時召喚（決定16） ---
//
// 召喚そのもの（summon_floating_if_absent）は get_focused_pane_info /
// get_pane_info を経由するのでここでは検証できない。召喚された側の
// 入場と、取り残しの掃除だけを見る。

#[test]
fn a_summoned_instance_enters_nav_mode_once_ready() {
    let mut state = state_with_panes(3);
    state.summoned = true;
    state.pending_nav_entry = true;

    state.enter_nav_mode_if_pending();
    assert!(state.nav_mode);
    assert!(!state.pending_nav_entry, "予約は使い切る");
}

#[test]
fn a_pending_entry_waits_for_permissions() {
    // 承認前に入場すると横取りの要求が通らず、キーが戻らないまま無反応になる
    let mut state = state_with_panes(3);
    state.summoned = true;
    state.pending_nav_entry = true;
    state.permissions_granted = false;

    state.enter_nav_mode_if_pending();
    assert!(!state.nav_mode);
    assert!(state.pending_nav_entry, "予約は次の機会まで残す");
}

#[test]
fn a_pending_entry_waits_for_a_non_empty_list() {
    // 一覧が空のまま入ると j/k が効かず、抜けるしかない状態になる
    let mut state = State {
        summoned: true,
        pending_nav_entry: true,
        permissions_granted: true,
        ..Default::default()
    };

    state.enter_nav_mode_if_pending();
    assert!(!state.nav_mode);
    assert!(state.pending_nav_entry);
}

#[test]
fn a_pending_entry_does_not_re_enter_after_leaving() {
    // 入場の機会は PermissionRequestResult / PaneUpdate / TabUpdate の
    // 3箇所から来る。予約を使い切らないと、抜けた直後に入り直してしまう
    let mut state = state_with_panes(3);
    state.summoned = true;
    state.pending_nav_entry = true;

    state.enter_nav_mode_if_pending();
    state.handle_nav_key(KeyWithModifier::new(BareKey::Esc));
    assert!(!state.nav_mode);

    state.enter_nav_mode_if_pending();
    assert!(!state.nav_mode, "一度抜けたら予約では戻らない");
}

#[test]
fn dismiss_makes_a_summoned_instance_leave_nav_mode() {
    let mut state = state_with_panes(3);
    state.summoned = true;
    state.own_plugin_id = Some(6);
    state.nav_mode = true;

    state.pipe(pipe_message(DISMISS_PIPE, ""));
    assert!(!state.nav_mode, "横取りしたまま消えるとキーが戻らない");
}

#[test]
fn dismiss_leaves_the_resident_sidebar_in_place() {
    // 常駐が消えると復帰手段が無くなる
    let url = "file:/x/fujin.wasm";
    let mut state = State {
        own_plugin_id: Some(5),
        own_plugin_url: Some(url.to_string()),
        nav_mode: true,
        panes: Some(manifest(vec![(0, vec![plugin_pane(5, url)])])),
        ..Default::default()
    };

    state.pipe(pipe_message(DISMISS_PIPE, ""));
    assert!(state.nav_mode, "常駐は掃除の対象外");
}

#[test]
fn dismiss_forgets_what_it_summoned() {
    // 記録が残ると、閉じたはずのタブで召喚が抑止されてしまう
    let url = "file:/x/fujin.wasm";
    let mut state = State {
        own_plugin_id: Some(5),
        own_plugin_url: Some(url.to_string()),
        panes: Some(manifest(vec![(
            0,
            vec![plugin_pane(5, url), floating_plugin_pane(9, url)],
        )])),
        summoned_panes: BTreeMap::from([(0, 9)]),
        ..Default::default()
    };

    state.pipe(pipe_message(DISMISS_PIPE, ""));
    assert!(state.summoned_panes.is_empty());
}

#[test]
fn a_tab_that_already_has_fujin_is_recognized() {
    // 「そのタブに居るなら召喚しない」の判定そのもの。常駐（タイル）でも
    // 既に出ている召喚（フローティング）でも、居るなら重ねない
    let url = "file:/x/fujin.wasm";
    let other = "file:/x/other.wasm";
    let manifest = manifest(vec![
        (0, vec![plugin_pane(1, url), terminal_pane(10, "pane1")]),
        (1, vec![terminal_pane(11, "pane2"), plugin_pane(2, other)]),
        (2, vec![floating_plugin_pane(3, url)]),
    ]);

    assert!(State::tab_has_fujin(&manifest, 0, url));
    // 別プラグインは兄弟ではない
    assert!(!State::tab_has_fujin(&manifest, 1, url));
    assert!(State::tab_has_fujin(&manifest, 2, url));
    // 一覧に無いタブ（＝一覧が古いと新規タブがこうなる。だから召喚の判定では
    // 凍った self.panes ではなくサーバへ問い合わせ直した一覧を使う）
    assert!(!State::tab_has_fujin(&manifest, 3, url));
}

#[test]
fn summon_candidates_are_ordered_and_deduped() {
    // 代表は「昇順で最初に生きているID」。順序が揺れるとインスタンスごとに
    // 結論が食い違い、誰も召喚しない／全員が召喚するのどちらにもなる
    let url = "file:/x/fujin.wasm";
    let other = "file:/x/other.wasm";
    let manifest = manifest(vec![
        (0, vec![plugin_pane(7, url), terminal_pane(1, "pane1")]),
        (1, vec![plugin_pane(3, url), plugin_pane(9, other)]),
        (2, vec![plugin_pane(7, url), floating_plugin_pane(5, url)]),
    ]);

    assert_eq!(State::sibling_plugin_ids(&manifest, url), vec![3, 5, 7]);
}

// --- pipe（ワイヤプロトコル） ---

fn pipe_message(name: &str, payload: &str) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(0),
        name: name.to_string(),
        payload: Some(payload.to_string()),
        args: BTreeMap::new(),
        is_private: true,
    }
}

// キーバインドからの `fujin_toggle_cwd` はユーザー操作なので payload を持たない
//（NAV_UP_PIPE等と同じ）。同期用の明示セットとの分岐を試すのに必要
fn pipe_message_no_payload(name: &str) -> PipeMessage {
    PipeMessage {
        payload: None,
        ..pipe_message(name, "")
    }
}

#[test]
fn status_pipe_applies_the_payload() {
    let mut state = State::default();
    let rendered = state.pipe(pipe_message(
        STATUS_PIPE,
        r#"{"pane_id":"4","event":"UserPromptSubmit","agent":"claude"}"#,
    ));
    assert!(rendered);
    assert_eq!(state.agents[&4].state, AgentState::Working);
}

#[test]
fn status_pipe_ignores_garbage() {
    let mut state = State::default();
    assert!(!state.pipe(pipe_message(STATUS_PIPE, "{}")));
    assert!(state.agents.is_empty());
}

#[test]
fn selection_pipe_is_addressed_by_pane_id() {
    // インデックスではなくペインIDで運ぶので、一覧が違っても同じ行を指す
    let mut state = state_with_panes(3);
    assert!(state.pipe(pipe_message(SELECTION_PIPE, "3")));
    assert_eq!(state.selected, 2);
    // 同じ位置への再指定は再描画不要
    assert!(!state.pipe(pipe_message(SELECTION_PIPE, "3")));
    // 知らないペインIDは無視する
    assert!(!state.pipe(pipe_message(SELECTION_PIPE, "99")));
    assert_eq!(state.selected, 2);
}

#[test]
fn read_clear_pipe_clears_listed_panes() {
    let mut state = state_with_panes(3);
    state.apply_status(status(1, "Stop"));
    state.apply_status(status(2, "Stop"));
    state.apply_status(status(3, "UserPromptSubmit"));

    assert!(state.pipe(pipe_message(READ_CLEAR_PIPE, "1,3")));
    assert_eq!(state.agents[&1].state, AgentState::Idle);
    assert_eq!(state.agents[&2].state, AgentState::Done);
    // working は既読対象ではないので触らない
    assert_eq!(state.agents[&3].state, AgentState::Working);
}

#[test]
fn sync_pipe_only_fills_an_empty_state() {
    let dump = "1\tdone\t0\t0\tclaude\t\n";

    let mut empty = state_with_panes(1);
    assert!(empty.pipe(pipe_message(SYNC_STATE_PIPE, dump)));
    assert_eq!(empty.agents[&1].state, AgentState::Done);

    // 既に自前の状態を持っているなら、古いダンプで上書きしない
    let mut populated = state_with_panes(1);
    populated.apply_status(status(1, "UserPromptSubmit"));
    assert!(!populated.pipe(pipe_message(SYNC_STATE_PIPE, dump)));
    assert_eq!(populated.agents[&1].state, AgentState::Working);
}

#[test]
fn toggle_cwd_pipe_flips_the_local_value_when_unprompted() {
    // ユーザーのキー操作からはpayloadが付かない（決定6）。全インスタンスが
    // 同じ値から出発している前提で、権威を立てず各自が独立に反転する
    // （docs/issues/toggle-cwd-key.md）
    let mut state = State::default();
    assert!(!state.show_cwd);

    assert!(state.pipe(pipe_message_no_payload(TOGGLE_CWD_PIPE)));
    assert!(state.show_cwd);

    assert!(state.pipe(pipe_message_no_payload(TOGGLE_CWD_PIPE)));
    assert!(!state.show_cwd);
}

#[test]
fn toggle_cwd_pipe_applies_an_explicit_sync_value() {
    // 新入りインスタンスへの現在値push（決定13）は明示セット。反転にすると
    // 押し付けるたびに向きがずれる
    let mut state = State::default();
    assert!(state.pipe(pipe_message(TOGGLE_CWD_PIPE, "true")));
    assert!(state.show_cwd);

    // 既に同じ値なら再描画不要
    assert!(!state.pipe(pipe_message(TOGGLE_CWD_PIPE, "true")));

    assert!(state.pipe(pipe_message(TOGGLE_CWD_PIPE, "false")));
    assert!(!state.show_cwd);
}

#[test]
fn toggle_cwd_pipe_treats_an_empty_payload_as_a_key_press() {
    // CLI から `zellij pipe` で叩くと payload が空文字で届きうる。config.rs の
    // 正規化と同じく未設定扱いにして、キー操作（反転）として読む
    let mut state = State::default();
    for payload in ["", "  ", "\n"] {
        let before = state.show_cwd;
        assert!(
            state.pipe(pipe_message(TOGGLE_CWD_PIPE, payload)),
            "{payload:?}"
        );
        assert_eq!(state.show_cwd, !before, "{payload:?}");
    }
}

#[test]
fn toggle_cwd_pipe_ignores_an_unparsable_payload() {
    // 真偽値の受け口は広げない（決定40）。黙って false へ倒すと、cwd が消えた
    // 結果だけが残って原因を追えない
    let mut state = State {
        show_cwd: true,
        ..Default::default()
    };

    for payload in ["1", "yes", "TRUE", "false ish"] {
        assert!(
            !state.pipe(pipe_message(TOGGLE_CWD_PIPE, payload)),
            "{payload:?}"
        );
        assert!(state.show_cwd, "{payload:?}");
    }
}

#[test]
fn unknown_pipes_are_ignored() {
    let mut state = State::default();
    assert!(!state.pipe(pipe_message("some_other_plugin", "payload")));
}

// --- ペイン行のレイアウト（決定21・決定22） ---
//
// カウンタ列は右端に揃え、幅はフレーム全体で共有する。ペイン名はその残り幅に
// 収めるので、名前が長くてもサブエージェント数 `+N`・未完了タスク数 `[M]` は
// 消えない。cwd はペイン行に混ぜず、続く cwd行に出す

const SIDEBAR: usize = 32; // 既定のサイドバー幅（決定3）
                           // ツリーの上に常時居る枠（境界線・ヘッダー・境界線）。ツリーの行番号は
                           // すべてこの下から数える（要件: sidebar-header.feature）
const HEADER_ROWS: usize = 3;
// ツリーの下に常時居る枠（境界線・フッター・status-bar と離すための余白）
const FOOTER_ROWS: usize = 3;
const CONTENT: usize = SIDEBAR - 2; // 右マージン2セルを除いた、文字を置ける幅

// 1ペインだけを持つ状態。ペイン名を指定して作る
fn state_with_one_pane(title: &str) -> State {
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(0, vec![terminal_pane(1, title)])]));
    state.rebuild_selectable();
    state
}

// そのフレームのカウンタ列（描画と同じ手順で測る）
fn column_of(state: &State) -> CounterColumn {
    state.counter_column(&state.visible_rows())
}

// content 内で needle が始まる列（表示セル基準）
fn column_at(content: &str, needle: &str) -> usize {
    let byte = content
        .find(needle)
        .unwrap_or_else(|| panic!("{:?} が {:?} に無い", needle, content));
    unicode_width::UnicodeWidthStr::width(&content[..byte])
}

fn repeat_status(state: &mut State, pane_id: u32, event: &str, times: usize) {
    for _ in 0..times {
        state.apply_status(status(pane_id, event));
    }
}

#[test]
fn a_pane_without_a_status_gets_the_no_agent_marker() {
    // 状態アイコン列を空白のままにすると、エージェントが乗る行と並べたときに
    // 左端が欠けて見える（docs/issues/sidebar-cwd-row-legibility.md）
    let state = state_with_one_pane("shell");

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.starts_with(&format!("  {} shell", NO_AGENT_ICON)),
        "状態アイコンの位置に印を出す: {}",
        content
    );
    // 状態色（0/1/2/3/6）も dim も乗せない。意味の軸が違うものに状態色を
    // 割り当てないための印なので、装飾は持たせない
    assert!(!ink_at(&text, DIM_LEVEL).contains(&2), "{}", content);
    for level in [0, 1, 2, 3, ERROR_LEVEL] {
        assert!(
            !ink_at(&text, level).contains(&2),
            "状態色は乗せない（レベル{}）: {}",
            level,
            content
        );
    }
}

#[test]
fn a_status_replaces_the_no_agent_marker() {
    // 印はあくまで「状態が無いとき」の埋め草。状態が来たらアイコンごと譲る
    let mut state = state_with_one_pane("claude");
    set_agent_state(&mut state, 1, AgentState::Working);

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.starts_with(&format!("  {} claude", AgentState::Working.icon())),
        "{}",
        content
    );
    assert!(!content.contains(NO_AGENT_ICON), "{}", content);
    assert!(
        ink_at(&text, AgentState::Working.color()).contains(&2),
        "{}",
        content
    );
    assert!(!ink_at(&text, DIM_LEVEL).contains(&2), "{}", content);
}

#[test]
fn counters_are_flush_with_the_right_edge() {
    let mut state = state_with_one_pane("要件定義とドキュメント整理タスクの続き");
    repeat_status(&mut state, 1, "SubagentStart", 2);
    repeat_status(&mut state, 1, "TaskCreated", 3);

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.ends_with("+2 [3]"),
        "カウンタ列は右端に揃える: {}",
        content
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(content),
        CONTENT,
        "右端には常にマージンを空ける: {}",
        content
    );
    assert!(
        content.contains('…'),
        "収まらないぶんはペイン名側を畳む: {}",
        content
    );
}

#[test]
fn every_tree_row_leaves_a_right_margin() {
    // 文字がサイドバーの縁に貼り付くと窮屈に見える。タブ見出し行・ペイン行・
    // cwd行のどれも、収まらないときは右マージンの手前で畳む
    let mut state = state_with_one_pane("要件定義とドキュメント整理タスクの続き");
    state.tabs = vec![TabInfo {
        position: 0,
        name: "とても長い名前のタブがここにある".to_string(),
        active: true,
        ..Default::default()
    }];
    state.show_cwd = true;
    state.pane_cwds.insert(
        1,
        "/Users/example/development/oss/zellij-plugins/fujin".to_string(),
    );
    repeat_status(&mut state, 1, "SubagentStart", 2);

    let rows = state.visible_rows();
    let column = state.counter_column(&rows);
    for row in &rows {
        let line = match row {
            Row::Tab(tab) => state.tab_heading(tab, SIDEBAR),
            Row::Pane { entry, hit, .. } => {
                state.pane_row(entry, false, *hit, column, HeadCells::default(), SIDEBAR)
            }
            Row::Cwd { cwd, hit, .. } => cwd_row(cwd, false, *hit, SIDEBAR),
            _ => continue,
        };
        assert!(
            unicode_width::UnicodeWidthStr::width(line.content()) <= CONTENT,
            "右端にマージンが残っていない: {:?}",
            line.content()
        );
    }

    // 選択行の背景だけは右マージンも塗る。塗らないと帯が途中で切れて見える
    let selected = state.pane_row(
        &state.selectable[0],
        true,
        None,
        column,
        HeadCells::default(),
        SIDEBAR,
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(selected.content()),
        SIDEBAR
    );
}

#[test]
fn rows_with_any_counter_still_share_the_frames_column_width() {
    // サブエージェント数だけのペインと、未完了タスク数だけのペイン。
    // 桁がずれると一覧を縦に舐められないので、**カウンタを持つ行同士**は列を共有する
    // （2026-08-08改訂: 以前は「フレーム全体」で共有していたが、カウンタを一切
    // 持たない行まで巻き込んでいたのは不具合だった。そちらは
    // `a_counter_less_row_is_unaffected_by_other_rows_counters` で検証する）
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(1, "alpha"), terminal_pane(2, "bravo")],
    )]));
    state.rebuild_selectable();
    repeat_status(&mut state, 1, "SubagentStart", 12);
    repeat_status(&mut state, 2, "TaskCreated", 3);

    let column = column_of(&state);
    let first = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column,
        HeadCells::default(),
        SIDEBAR,
    );
    let second = state.pane_row(
        &state.selectable[1],
        false,
        None,
        column,
        HeadCells::default(),
        SIDEBAR,
    );

    // 列幅は `+12`（3）と `[3]`（3）、あいだの空白1つで計7セル
    assert_eq!(column_at(first.content(), "+12"), CONTENT - 7);
    assert_eq!(column_at(second.content(), "[3]"), CONTENT - 3);
    assert!(
        !second.content().contains('+'),
        "サブエージェント数を持たない行は、その位置を空けたままにする: {}",
        second.content()
    );
}

#[test]
fn a_counter_less_row_is_unaffected_by_other_rows_counters() {
    // 「+1」のようなカウンタが1行にでも出ると、それを持たない他の行まで右端が
    // 削られていた不具合の回帰テスト
    // （docs/issues/counter-column-collateral-truncation.md）。
    // 同じ行を「静かな列（ゼロ幅）」と「実測した列（非ゼロ幅）」の両方で描画し、
    // 自分自身がカウンタを持たなければ結果が完全に一致することを確認する
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(
        0,
        vec![
            terminal_pane(1, "要件定義とドキュメント整理タスクの続き"),
            terminal_pane(2, "bravo"),
        ],
    )]));
    state.rebuild_selectable();
    repeat_status(&mut state, 2, "SubagentStart", 12);

    let quiet_column = CounterColumn::default();
    let noisy_column = column_of(&state);
    assert_ne!(
        noisy_column, quiet_column,
        "このフレームは他のペインがカウンタを持っている前提"
    );

    let without_counters = state.pane_row(
        &state.selectable[0],
        false,
        None,
        quiet_column,
        HeadCells::default(),
        SIDEBAR,
    );
    let with_counters = state.pane_row(
        &state.selectable[0],
        false,
        None,
        noisy_column,
        HeadCells::default(),
        SIDEBAR,
    );

    assert_eq!(
        without_counters.content(),
        with_counters.content(),
        "カウンタを持たない行は、他の行がカウンタを持っていても表示が変わらない"
    );
}

#[test]
fn the_counter_column_costs_nothing_when_nobody_has_counters() {
    // 静かなフレームでは列を予約しない。予約するとペイン名の幅がその場で失われる
    let state = state_with_one_pane("abcdefghijklmnopqrstuvwxyz0123456789");
    assert_eq!(column_of(&state), CounterColumn::default());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(content.starts_with("  › abcdefghij"), "{}", content);
    assert!(content.ends_with('…'), "{}", content);
    assert_eq!(unicode_width::UnicodeWidthStr::width(content), CONTENT);
}

#[test]
fn a_pane_name_that_is_a_path_keeps_its_tail() {
    // ペイン名に cwd がそのまま入ることがある。末尾を切ると
    // `/Users/example/develo…` のようにどの行も同じ見た目になってしまう
    let state = state_with_one_pane("/Users/example/development/oss/zellij-plugins/fujin");

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.ends_with("zellij-plugins/fujin"),
        "パスは先頭省略で末尾を残す: {}",
        content
    );
    assert!(content.contains('…'), "{}", content);
    assert_eq!(unicode_width::UnicodeWidthStr::width(content), CONTENT);
}

// --- cwd行（決定22） ---

#[test]
fn the_cwd_is_rendered_as_its_own_row() {
    let mut state = state_with_one_pane("claude-worker");
    state.show_cwd = true;
    set_agent_state(&mut state, 1, AgentState::Idle);
    state
        .pane_cwds
        .insert(1, "/work/oss/zellij-plugins/fujin".to_string());

    let rows = state.visible_rows();
    // 0-2: ヘッダ3行 / 3: tab1見出し / 4: ペイン行 / 5: cwd行
    assert!(matches!(rows[HEADER_ROWS + 1], Row::Pane { .. }));
    assert!(matches!(rows[HEADER_ROWS + 2], Row::Cwd { .. }));
    assert_eq!(
        state.pane_at_row(HEADER_ROWS + 2),
        Some(1),
        "cwd行のクリックも同じペインに当たる（要件: click-to-focus）"
    );

    let pane_text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert!(
        !pane_text.content().contains("/work"),
        "cwd はペイン行には出ない: {}",
        pane_text.content()
    );
}

#[test]
fn a_pane_without_a_cwd_gets_no_extra_row() {
    // 空の cwd行で縦を消費しない（サイドバーはスクロールしないので行数は貴重）
    let mut state = state_with_one_pane("claude-worker");
    state.show_cwd = true;

    let rows = state.visible_rows();
    assert_eq!(
        rows.len(),
        HEADER_ROWS + 2 + FOOTER_ROWS,
        "枠・タブ見出し行・ペイン行だけ"
    );
}

#[test]
fn the_cwd_row_appears_for_a_search_hit_even_when_show_cwd_is_off() {
    // 画面に無い文字列でヒットしたように見せない（決定18）
    let mut state = searchable_state();
    assert!(!state.show_cwd);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");

    let rows = state.visible_rows();
    let cwd_rows: Vec<&Row> = rows
        .iter()
        .filter(|r| matches!(r, Row::Cwd { .. }))
        .collect();
    assert_eq!(cwd_rows.len(), 1, "cwd を持つ bravo の行だけ");
    let Some(Row::Cwd { entry, hit, .. }) = cwd_rows.first() else {
        panic!("cwd行が無い");
    };
    assert_eq!(entry.pane_id, 2);
    assert!(hit.is_some(), "ヒット箇所を提示するのでハイライトを持つ");
}

#[test]
fn the_cwd_row_is_not_highlighted_by_a_pane_name_hit() {
    // ペイン名に当たっただけの行では cwd行を光らせない
    let mut state = searchable_state();
    state.show_cwd = true;
    set_agent_state(&mut state, 2, AgentState::Idle);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "bravo");

    let rows = state.visible_rows();
    let Some(Row::Cwd { hit, .. }) = rows.iter().find(|r| matches!(r, Row::Cwd { .. })) else {
        panic!("cwd行が無い");
    };
    assert!(hit.is_none());
}

#[test]
fn the_cwd_row_disappears_when_the_agent_exits() {
    // エージェントが去ったら cwd行も引っ込める
    //（docs/issues/sidebar-cwd-persists-after-exit.md）
    let mut state = state_with_one_pane("claude-worker");
    state.show_cwd = true;
    set_agent_state(&mut state, 1, AgentState::Idle);
    state
        .pane_cwds
        .insert(1, "/work/oss/zellij-plugins/fujin".to_string());
    assert!(
        state
            .visible_rows()
            .iter()
            .any(|r| matches!(r, Row::Cwd { .. })),
        "動いている間は出る"
    );

    state.apply_status(status(1, "SessionEnd"));

    assert!(
        !state
            .visible_rows()
            .iter()
            .any(|r| matches!(r, Row::Cwd { .. })),
        "終了後は出ない"
    );
    // cwd 自体は捨てない。ペイン名フォールバック（決定26）が使う
    assert!(state.pane_cwds.contains_key(&1));
}

#[test]
fn an_exited_agent_still_gets_a_cwd_row_for_a_search_hit() {
    // 表示条件を絞っても、絞り込み結果の提示は変えない — 一覧に残っている以上、
    // 何に一致したかは示す（決定18）
    let mut state = searchable_state();
    state.show_cwd = true;
    // pane2 は cwd を持つがエージェントは居ない（＝終了後と同じ状態）
    assert!(!state.agents.contains_key(&2));
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");

    let rows = state.visible_rows();
    let Some(Row::Cwd { entry, hit, .. }) = rows.iter().find(|r| matches!(r, Row::Cwd { .. }))
    else {
        panic!("cwd行が無い");
    };
    assert_eq!(entry.pane_id, 2);
    assert!(hit.is_some());
}

#[test]
fn the_cwd_row_keeps_the_tail_of_the_path() {
    let text = cwd_row(
        "/Users/example/development/oss/zellij-plugins/fujin",
        false,
        None,
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.ends_with("zellij-plugins/fujin"),
        "末尾のディレクトリ名を残す: {}",
        content
    );
    assert!(
        content.starts_with("      …"),
        "字下げして続きに見せる: {}",
        content
    );
    assert!(unicode_width::UnicodeWidthStr::width(content) <= CONTENT);
}

// --- ペイン名フォールバック（決定26） ---
//
// claude は終了時に空のタイトルを OSC で送るため、エージェントを落とした瞬間に
// ペイン名が空のまま残る（docs/issues/pane-title-blank-on-exit.md）

#[test]
fn an_empty_pane_name_falls_back_to_the_cwd() {
    let mut state = state_with_one_pane("");
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert!(
        text.content().contains("fujin"),
        "ペイン名の位置に cwd を出す: {}",
        text.content()
    );
}

#[test]
fn a_pane_name_that_is_only_spaces_falls_back_too() {
    let mut state = state_with_one_pane("   ");
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert!(text.content().contains("fujin"), "{}", text.content());
}

#[test]
fn a_non_empty_pane_name_is_left_alone() {
    // 決定19（生のペイン名をそのまま出す）は空でないときは変わらない
    let mut state = state_with_one_pane("claude-worker");
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(content.contains("claude-worker"), "{}", content);
    assert!(
        !content.contains("fujin"),
        "cwd で上書きしない: {}",
        content
    );
}

#[test]
fn an_empty_pane_name_without_a_cwd_stays_blank() {
    // 落とす先が無いペインは名前の位置が空欄のまま。名前を捏造しない
    //（アイコン列の未起動の印だけは出る）
    let state = state_with_one_pane("");

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert_eq!(text.content().trim(), NO_AGENT_ICON, "{}", text.content());
}

#[test]
fn the_cwd_row_is_dropped_while_the_pane_name_falls_back() {
    // 同じパスが2行並んでも情報が増えない
    let mut state = state_with_one_pane("");
    state.show_cwd = true;
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let rows = state.visible_rows();
    assert!(
        !rows.iter().any(|r| matches!(r, Row::Cwd { .. })),
        "cwd行は出さない"
    );
    assert_eq!(
        rows.len(),
        HEADER_ROWS + 2 + FOOTER_ROWS,
        "枠・タブ見出し行・ペイン行だけ"
    );
}

#[test]
fn a_falling_back_pane_row_still_matches_on_the_cwd() {
    // 生のペイン名は空なので、当たるのは cwd。ハイライトはペイン名の位置に
    // 出ている cwd 側へ載るため、ここでも cwd行は足さない
    let mut state = state_with_one_pane("");
    state.nav_mode = true;
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");

    let rows = state.visible_rows();
    assert!(
        rows.iter().any(|r| matches!(r, Row::Pane { .. })),
        "cwd 一致でペイン行が残る"
    );
    assert!(!rows.iter().any(|r| matches!(r, Row::Cwd { .. })));
    // ハイライト位置の算出（畳んだ cwd への index 付け替え）を通す
    state.render(SIDEBAR, 10);
}

#[test]
fn triage_rows_fall_back_to_the_cwd_too() {
    let mut state = state_with_one_pane("");
    state.nav_mode = true;
    set_agent_state(&mut state, 1, AgentState::Blocked);
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());
    state.handle_nav_key(key(BareKey::Char('p')));

    let rows = state.visible_rows();
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い: {}", rows.len());
    };
    let text = state.triage_row(entry, tab_name, false, tab_column, None, SIDEBAR);
    assert!(
        text.content().contains("fujin"),
        "トリアージ行でも cwd へ落とす: {}",
        text.content()
    );
}

#[test]
fn the_cwd_row_survives_a_sidebar_narrower_than_its_indent() {
    // 字下げより狭い幅でも算術が破綻しない
    let text = cwd_row("/work/fujin", false, None, 3);
    assert!(unicode_width::UnicodeWidthStr::width(text.content()) <= 6);
}

// --- フローティングペインの区別表示（要件: docs/requirements/floating-pane-indicator/） ---
//
// フローティングペインはフローティング層ごと隠れうるので、一覧の上で見分けられる
// ようにペイン名を丸括弧で囲む。色・dim は使わない（決定36）

// フローティングなターミナルペイン1つだけを持つ状態
fn state_with_one_floating_pane(title: &str) -> State {
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(
        0,
        vec![PaneInfo {
            is_floating: true,
            ..terminal_pane(1, title)
        }],
    )]));
    state.rebuild_selectable();
    state
}

// 先頭のペイン行を既定の先頭列で描いた中身
fn pane_row_content(state: &State) -> String {
    state
        .pane_row(
            &state.selectable[0],
            false,
            None,
            column_of(state),
            HeadCells::default(),
            SIDEBAR,
        )
        .content()
        .to_string()
}

#[test]
fn floating_panes_wrap_their_name_in_parentheses() {
    let state = state_with_one_floating_pane("claude");
    let content = pane_row_content(&state);
    assert!(
        content.contains("(claude)"),
        "フローティングペインは名前を丸括弧で囲む: {}",
        content
    );
}

#[test]
fn tiled_panes_keep_their_bare_name() {
    let state = state_with_one_pane("claude");
    let content = pane_row_content(&state);
    assert!(content.contains("claude"), "{}", content);
    assert!(
        !content.contains('('),
        "タイルペインには括弧を付けない: {}",
        content
    );
}

#[test]
fn a_long_floating_name_is_folded_inside_the_parentheses() {
    let state = state_with_one_floating_pane("要件定義とドキュメント整理タスクの続き");
    let content = pane_row_content(&state);
    assert!(
        content.contains("(") && content.ends_with("…)"),
        "括弧は前後に残し、畳むのは内側: {}",
        content
    );
    assert!(
        unicode_width::UnicodeWidthStr::width(content.as_str()) <= CONTENT,
        "右端のマージンは括弧を足しても残る: {}",
        content
    );
}

#[test]
fn a_long_floating_path_keeps_the_parentheses_around_the_leading_ellipsis() {
    // パス形式は先頭省略（決定22）。省略記号は括弧の内側に入る
    let state = state_with_one_floating_pane("/Users/example/development/oss/zellij-plugins/fujin");
    let content = pane_row_content(&state);
    assert!(
        content.contains("(…"),
        "先頭省略でも開き括弧が先に来る: {}",
        content
    );
    assert!(
        content.ends_with("fujin)"),
        "閉じ括弧は末尾に残る: {}",
        content
    );
}

#[test]
fn the_parentheses_are_reserved_ahead_of_the_pane_name() {
    // 括弧はカウンタ列と同じく先に確保する（決定21の考え方）。名前が長くても
    // 括弧・カウンタ列のどちらも消えず、畳まれるのはペイン名の側
    let mut state = state_with_one_floating_pane("要件定義とドキュメント整理タスクの続き");
    repeat_status(&mut state, 1, "SubagentStart", 2);
    repeat_status(&mut state, 1, "TaskCreated", 3);

    let content = pane_row_content(&state);
    assert!(
        content.ends_with("+2 [3]"),
        "カウンタ列は右端に揃ったまま: {}",
        content
    );
    assert!(
        content.contains("…)"),
        "ペイン名を括弧の内側で畳む: {}",
        content
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(content.as_str()),
        CONTENT,
        "右端には常にマージンを空ける: {}",
        content
    );
}

#[test]
fn a_floating_pane_falling_back_to_its_cwd_wraps_the_cwd() {
    // ペイン名の位置に出ているのが cwd でも、囲むものには変わりない（決定26）
    let mut state = state_with_one_floating_pane("");
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let content = pane_row_content(&state);
    assert!(
        content.contains("(/work/oss/fujin)"),
        "フォールバック中の cwd も括弧で囲む: {}",
        content
    );
}

#[test]
fn a_floating_pane_without_a_name_or_a_cwd_stays_blank() {
    // 囲むものが無い行に括弧だけ出しても、フローティングだと分かる以前に読めない
    let state = state_with_one_floating_pane("");
    assert_eq!(pane_row_content(&state).trim(), NO_AGENT_ICON);
}

#[test]
fn triage_rows_wrap_floating_pane_names_too() {
    // ペイン名が出る箇所は一貫して同じ見た目にする
    let mut state = state_with_one_floating_pane("claude");
    state.nav_mode = true;
    set_agent_state(&mut state, 1, AgentState::Blocked);
    state.handle_nav_key(key(BareKey::Char('p')));

    let rows = state.visible_rows();
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い: {}", rows.len());
    };
    let text = state.triage_row(entry, tab_name, false, tab_column, None, SIDEBAR);
    assert!(
        text.content().contains("(claude)"),
        "トリアージ一覧でも丸括弧で囲む: {}",
        text.content()
    );
}

#[test]
fn a_floating_row_survives_a_sidebar_too_narrow_for_the_parentheses() {
    // 括弧ぶんの2セルすら無い幅でも算術が破綻しない
    let state = state_with_one_floating_pane("claude");
    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        4,
    );
    assert!(unicode_width::UnicodeWidthStr::width(text.content()) <= 4);
}

// --- 先頭省略とハイライト位置 ---

#[test]
fn truncate_start_leaves_short_strings_alone() {
    assert_eq!(truncate_start("/a/b", 5), ("/a/b".to_string(), 0));
    // 境界: ちょうど収まるときは省略記号を付けない
    assert_eq!(truncate_start("/a/bc", 5), ("/a/bc".to_string(), 0));
}

#[test]
fn truncate_start_drops_the_head_within_budget() {
    // "/bb" までは境界候補だが幅に収まらないので、収まる直近の境界 "/cc" へ丸める
    let (folded, dropped) = truncate_start("/aa/bb/cc", 5);
    assert_eq!(folded, "…/cc");
    assert_eq!(dropped, 6);
    // 収まる `/` 境界が無いときだけ、従来どおり文字幅で機械的に末尾を残す
    // （全角は2セルぶん食う）
    let (folded, dropped) = truncate_start("/あ/いう", 5);
    assert_eq!(folded, "…いう");
    assert_eq!(dropped, 3);
}

#[test]
fn truncate_start_rounds_to_a_slash_boundary() {
    // ディレクトリ名の途中で切らず、`…` の直後が必ず `/` になるように
    // 収まる範囲でいちばん手前の区切りへ丸める（中途半端な文字列を避ける調整）
    let (folded, dropped) =
        truncate_start("/Users/example/development/oss/zellij-plugins/fujin", 24);
    assert_eq!(folded, "…/zellij-plugins/fujin");
    assert_eq!(dropped, 30);
}

#[test]
fn truncate_start_falls_back_to_character_width_when_no_boundary_fits() {
    // 区切りが無い（か、区切りまで残しても収まらない）ほど1セグメントが
    // 長いときは、`…/` を諦めて文字幅で機械的に末尾を残す
    let (folded, dropped) = truncate_start("/aaaaaaaaaa", 5);
    assert_eq!(folded, "…aaaa");
    assert_eq!(dropped, 7);
}

#[test]
fn highlight_indices_follow_a_leading_ellipsis() {
    // "/aa/bb/cc" を先頭省略すると "…b/cc"。落ちた側（0..5）のヒットは捨て、
    // 残った側は省略記号1文字ぶん右へずれる
    assert_eq!(
        fold_highlight_indices(&[0, 4, 5, 8], "…b/cc", 5, 9, 6),
        vec![6 + 1, 6 + 1 + 3]
    );
}

#[test]
fn highlight_indices_after_a_trailing_ellipsis_are_dropped() {
    // 末尾切り詰め側は既存の規則のまま。"abcdefghi" を "abcd…" に詰めたので、
    // 見えている 0..4 は offset ぶんずらし、省略記号に重なる 4 以降は捨てる
    assert_eq!(
        fold_highlight_indices(&[0, 3, 4, 8], "abcd…", 0, 9, 4),
        vec![4, 7]
    );
}

// --- render（描画パスが panic しないこと） ---

#[test]
fn render_survives_a_cramped_sidebar() {
    let mut state = state_with_panes(3);
    state.apply_status(status(1, "Stop"));
    state.show_cwd = true;
    state
        .pane_cwds
        .insert(1, "/very/long/path/to/somewhere".to_string());
    // 行も桁も足りない状況で切り詰め・パディングの算術が破綻しないこと
    state.render(2, 1);
    state.render(0, 0);
    state.render(40, 20);
}

#[test]
fn render_survives_search_mode() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    // cwd ヒット（show_cwd=false でも cwd 行が出る経路）とハイライトを通す
    type_query(&mut state, "fujin");
    state.render(40, 20);
    state.render(2, 1);
    state.render(0, 0);

    // タブ名ヒット（見出しのハイライト経路）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "tab1");
    state.render(40, 20);
    state.render(3, 2);

    // 0件（`no matches` の通知行）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "zzz");
    state.render(40, 20);
    state.render(1, 1);
}

#[test]
fn render_survives_the_preview_pane() {
    // プレビュー用フローティングペイン（決定42）は枠を持たない別の描画パス
    let mut state = State {
        is_preview: true,
        permissions_granted: true,
        ..Default::default()
    };
    // まだ何も配られていない（見出しだけ）
    state.render(40, 20);
    state.apply_preview_snapshot("alpha\n$ cargo test\nrunning 3 tests");
    state.render(40, 20);
    // 見出しすら置けない高さ・幅でも算術が破綻しないこと
    state.render(2, 1);
    state.render(1, 1);
    state.render(0, 0);
}

// --- 縦スクロール（docs/issues/sidebar-vertical-overflow.md） ---
//
// 行番号の勘定は「画面高に収まるぶんだけを切り出す」ところに集まっているので、
// 純粋関数（reconcile_scroll）と、描画・クリックの逆引きが同じ並びを見ているか
// の両方を見る。

// 画面高8行に対して行が余る状態。visible_rows は
// ヘッダ1 + タブ見出し1 + ペイン12 = 14行になる
fn overflowing_state() -> State {
    let mut state = state_with_panes(12);
    state.nav_mode = true;
    state
}

// 画面のどこかにそのペインの行があるか
fn on_screen(state: &State, rows: usize, pane_id: u32) -> bool {
    (0..rows).any(|y| state.pane_at_row(y) == Some(pane_id))
}

fn overflow_markers(state: &State, rows: usize) -> Vec<(usize, bool)> {
    state
        .screen_rows(rows)
        .iter()
        .filter_map(|row| match row {
            Row::Overflow { hidden, above } => Some((*hidden, *above)),
            _ => None,
        })
        .collect()
}

#[test]
fn scrolling_keeps_everything_in_place_when_it_all_fits() {
    let mut state = overflowing_state();
    state.selected = 11;
    state.render(40, 32);

    assert_eq!(state.scroll, 0, "全部載るならスクロールしない");
    assert!(overflow_markers(&state, 40).is_empty());
    let screen = state.screen_rows(40);
    assert_eq!(screen.len(), 40, "画面高ぶんを返す（余りは空行）");
    // 空行は埋め草と最下部の余白なので、中身の行数からは外して数える
    assert_eq!(
        screen.iter().filter(|r| !matches!(r, Row::Blank)).count(),
        HEADER_ROWS + 13 + (FOOTER_ROWS - 1),
        "中身の行数は変わらない"
    );
}

#[test]
fn the_selection_never_leaves_the_screen() {
    // 「見えない行へ選択だけが進む」のが元の不具合。上下どちらへ動かしても
    // 選択行が画面に残ることを、全行ぶん確かめる
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
    let mut state = overflowing_state();

    for _ in 0..11 {
        state.handle_nav_key(key(BareKey::Char('j')));
        state.render(ROWS, 32);
        let pane_id = state.selectable[state.selected].pane_id;
        assert!(
            on_screen(&state, ROWS, pane_id),
            "下へ移動中に選択行が画面外へ出た: pane {}",
            pane_id
        );
    }
    for _ in 0..11 {
        state.handle_nav_key(key(BareKey::Char('k')));
        state.render(ROWS, 32);
        let pane_id = state.selectable[state.selected].pane_id;
        assert!(
            on_screen(&state, ROWS, pane_id),
            "上へ移動中に選択行が画面外へ出た: pane {}",
            pane_id
        );
    }
    assert_eq!(state.scroll, 0, "先頭まで戻ったらスクロールも戻る");
}

#[test]
fn the_cwd_row_stays_with_its_pane_row_at_the_bottom_edge() {
    // ペイン行だけが入って cwd行が切れると、選択の帯が画面の端で切れて見える
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
    let mut state = overflowing_state();
    state.show_cwd = true;
    set_agent_state(&mut state, 5, AgentState::Idle);
    state.pane_cwds.insert(5, "/work/fujin".to_string());
    state.selected = 4; // pane5
    state.render(ROWS, 32);

    let screen = state.screen_rows(ROWS);
    let last_pane = screen
        .iter()
        .rposition(|row| matches!(row, Row::Pane { .. }))
        .expect("ペイン行が1つも無い");
    assert!(
        matches!(screen.get(last_pane + 1), Some(Row::Cwd { .. })),
        "選択行の cwd行まで画面に入っていない"
    );
}

#[test]
fn the_footer_sits_at_the_bottom_edge_even_when_the_tree_is_short() {
    // ツリーが短いと、下の枠がツリーの直後へ浮いてしまう（実機で確認された
    // 見た目の不具合）。余った高さは空行で埋めて最下部まで押し下げる
    let mut state = state_with_panes(2);
    state.render(20, SIDEBAR);

    let screen = state.screen_rows(20);
    assert_eq!(screen.len(), 20);
    assert_frame(&screen, "ツリーが短いとき");
    assert!(
        matches!(screen[HEADER_ROWS + 3], Row::Blank),
        "ツリーの後ろは空行で埋める"
    );
    // 空行はクリックの対象にならない（要件: click-to-focus と食い違わせない）
    assert_eq!(state.pane_at_row(HEADER_ROWS + 3), None);
    assert_eq!(state.pane_at_row(19), None, "最下部の余白も対象外");
}

#[test]
fn the_frame_stays_pinned_while_the_list_scrolls() {
    const ROWS: usize = 10;
    let mut state = overflowing_state();
    state.selected = 11;
    state.render(ROWS, 32);

    let screen = state.screen_rows(ROWS);
    assert_frame(&screen, "スクロール中");
    assert_eq!(screen.len(), ROWS, "画面高ぴったりまで使う");
}

#[test]
fn the_overflow_marker_sits_in_the_tab_heading_column() {
    // マーカーは一覧の1項目ではなく「一覧がそこで打ち切られている」ことを示す行なので、
    // ペイン行の階段ではなくタブ見出しと同じ x=0 に置く（docs/concept/ui-design.md）。
    // タブ見出し行の `▾` と記号がぶつかるため、続く `…` で見分けさせている
    for (above, marker) in [(true, '▴'), (false, '▾')] {
        let row = overflow_row(7, above, SIDEBAR);
        let chars: Vec<char> = row.content().chars().collect();
        assert_eq!(chars[0], marker, "記号は x=0: {}", row.content());
        assert_eq!(chars[2], '…', "x=2 に省略記号: {}", row.content());
        assert_eq!(chars[4], '7', "行数は名前の列 x=4: {}", row.content());
        assert!(row.content().ends_with(" more"), "{}", row.content());
        // 一覧の行そのものではないので全体を落として出す
        assert_eq!(ink_at(&row, DIM_LEVEL).len(), chars.len());
    }

    // 桁が増えても右マージンを食わない
    let wide = overflow_row(123, true, SIDEBAR);
    assert_eq!(wide.content(), "▴ … 123 more");
    assert!(unicode_width::UnicodeWidthStr::width(wide.content()) <= SIDEBAR - 2);
}

#[test]
fn overflow_markers_report_the_hidden_rows() {
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
    let mut state = overflowing_state();

    // 先頭を選択中: 下だけが隠れる。ヘッダ3 + タブ見出し1 + ペイン3 + 下端マーカー1 = 8行
    state.selected = 0;
    state.render(ROWS, 32);
    assert_eq!(overflow_markers(&state, ROWS), vec![(9, false)]);

    // 末尾を選択中: 上だけが隠れる（タブ見出し行も隠れる側に入る）
    state.selected = 11;
    state.render(ROWS, 32);
    assert_eq!(overflow_markers(&state, ROWS), vec![(9, true)]);

    // 途中まで送ったところ: 上下ともマーカーが出る
    let mut state = overflowing_state();
    state.selected = 8;
    state.render(ROWS, 32);
    let markers = overflow_markers(&state, ROWS);
    assert_eq!(markers.len(), 2, "上下ともマーカーが出る: {:?}", markers);
    assert!(markers[0].1 && !markers[1].1);
    assert_eq!(
        markers[0].0 + markers[1].0 + (ROWS - HEADER_ROWS - FOOTER_ROWS - 2),
        13,
        "隠れている行数と出ている行数の合計が一覧の行数になる"
    );
}

#[test]
fn clicking_follows_the_scrolled_layout() {
    // 描画とクリックの逆引きが同じ切り出しを見ていないと行がずれる
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
    let mut state = overflowing_state();
    state.nav_mode = false;
    state.selected = 11;
    state.render(ROWS, 32);

    // 0-2: ヘッダ3行 / 3: 上端マーカー / 4..7: pane9..pane12
    assert_eq!(state.pane_at_row(HEADER_ROWS), None, "マーカー行は対象外");
    assert_eq!(state.pane_at_row(HEADER_ROWS + 1), Some(9));
    assert_eq!(state.pane_at_row(7), Some(12));
    assert_eq!(state.pane_at_row(8), None, "画面の外");

    assert!(state.handle_click(HEADER_ROWS as isize + 1));
    assert_eq!(state.selectable[state.selected].pane_id, 9);
}

#[test]
fn scroll_stays_within_the_list_when_panes_disappear() {
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
    let mut state = overflowing_state();
    state.selected = 11;
    state.render(ROWS, 32);
    assert!(state.scroll > 0);

    // ペインが減って全部載るようになったら、上に寄った表示を残さない
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(1, "alpha"), terminal_pane(2, "bravo")],
    )]));
    state.rebuild_selectable();
    state.render(ROWS, 32);
    assert_eq!(state.scroll, 0);
    assert!(on_screen(&state, ROWS, 1));
}

#[test]
fn reconcile_scroll_leaves_a_list_that_fits_alone() {
    assert_eq!(reconcile_scroll(5, 8, 0, Some((4, 4))), 0);
    // 一覧が縮んで全部載るようになったら、スクロールは畳む
    assert_eq!(reconcile_scroll(5, 8, 3, None), 0);
}

#[test]
fn reconcile_scroll_pulls_the_selection_into_view() {
    // 13行を7行に出す。上端・下端のマーカーがそれぞれ1行使う
    assert_eq!(
        reconcile_scroll(13, 7, 0, Some((12, 12))),
        7,
        "下に外れた選択は最小限だけ送る"
    );
    assert_eq!(
        reconcile_scroll(13, 7, 7, Some((0, 0))),
        0,
        "上に外れた選択は先頭に置く"
    );
    // 既に見えているなら動かさない
    assert_eq!(reconcile_scroll(13, 7, 7, Some((10, 10))), 7);
}

#[test]
fn reconcile_scroll_does_not_leave_a_gap_at_the_bottom() {
    // 行き過ぎたスクロール位置は、末尾が下端に来るところまで戻す
    assert_eq!(reconcile_scroll(13, 7, 12, None), 7);
}

#[test]
fn reconcile_scroll_survives_a_screen_with_no_room() {
    assert_eq!(reconcile_scroll(13, 0, 3, Some((5, 5))), 0);
    assert_eq!(reconcile_scroll(13, 1, 0, Some((12, 12))), 12);
}

// --- 行クリック（要件: docs/requirements/click-to-focus/） ---
//
// 実際にフォーカスが移るかはホスト側の仕事（focus_pane_with_id はスタブで
// 何もしない）なので、ここでは「どの行がどのペインに対応するか」と
// クリックが選択・モードに与える影響を見る。

// navモード外の searchable_state。ヘッダは常時3行あるので、行の並びは
// 0-2: ヘッダ / 3: tab1見出し / 4: alpha / 5: bravo / 6: tab2見出し / 7: charlie
fn clickable_state() -> State {
    let mut state = searchable_state();
    state.nav_mode = false;
    state
}

#[test]
fn clicking_a_pane_row_selects_that_pane() {
    let mut state = clickable_state();

    assert!(state.handle_click(HEADER_ROWS as isize + 2));
    assert_eq!(state.selectable[state.selected].pane_id, 2);

    // タブをまたいだ行も同じように引ける
    assert!(state.handle_click(HEADER_ROWS as isize + 4));
    assert_eq!(state.selectable[state.selected].pane_id, 3);
}

#[test]
fn clicking_a_tab_heading_does_nothing() {
    let mut state = clickable_state();
    state.selected = 1;

    // ヘッダ3行はどれもクリックの対象にならない（要件: sidebar-header.feature）
    for y in 0..HEADER_ROWS as isize {
        assert!(!state.handle_click(y), "ヘッダ{}行目が反応した", y + 1);
    }
    assert!(!state.handle_click(HEADER_ROWS as isize), "tab1見出し");
    assert!(!state.handle_click(HEADER_ROWS as isize + 3), "tab2見出し");
    assert_eq!(state.selected, 1, "選択は動かない");
}

#[test]
fn clicking_outside_the_list_does_nothing() {
    let mut state = clickable_state();
    state.selected = 1;

    // 一覧より下の余白
    assert!(!state.handle_click(HEADER_ROWS as isize + 5));
    assert!(!state.handle_click(99));
    // 負の行（サイドバーの外）
    assert!(!state.handle_click(-1));
    assert_eq!(state.selected, 1);
}

#[test]
fn clicking_in_nav_mode_jumps_and_leaves_the_mode() {
    let mut state = searchable_state(); // nav_mode = true
                                        // ヘッダが3行入るぶん、ツリーはその下から始まる
    assert!(state.handle_click(HEADER_ROWS as isize + 2));

    assert_eq!(state.selectable[state.selected].pane_id, 2);
    assert!(!state.nav_mode, "ジャンプしたら横取りは解除する");
}

#[test]
fn clicking_follows_the_filtered_layout_while_searching() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alp");
    // 0-2: ヘッダ / 3: tab1見出し / 4: alpha（bravo と tab2 は絞り込みで消える）
    assert!(
        !state.handle_click(HEADER_ROWS as isize),
        "見出し行は対象外"
    );

    assert!(state.handle_click(HEADER_ROWS as isize + 1));
    assert_eq!(state.selectable[state.selected].pane_id, 1);
    assert!(!state.nav_mode);
    assert!(state.search.is_none(), "検索サブモードも一緒に畳む");
}

#[test]
fn clicking_is_ignored_before_permissions_are_granted() {
    let mut state = clickable_state();
    state.permissions_granted = false;
    // 承認前は「permissions required」しか描いていないので、そこに行は無い
    assert!(!state.handle_click(1));
}

// --- トリアージモード（要件: docs/requirements/triage-mode/） ---
//
// navモードの内側で `p` から入る、エージェント状態の緊急度順のフラット一覧。
// ツリー表示の並び順（決定3）には手を触れず、切り替えて使う

// タブ0に3ペイン、タブ1に1ペインを持つ navモード中の状態
fn triage_state() -> State {
    let mut state = State {
        tabs: vec![tab(0, true), tab(1, false)],
        panes: Some(manifest(vec![
            (
                0,
                vec![
                    terminal_pane(1, "alpha"),
                    terminal_pane(2, "bravo"),
                    terminal_pane(3, "charlie"),
                ],
            ),
            (1, vec![terminal_pane(4, "delta")]),
        ])),
        permissions_granted: true,
        nav_mode: true,
        ..Default::default()
    };
    state.rebuild_selectable();
    state
}

// フックのイベント列を通してエージェント状態を作る（直接代入せず、
// シーケンス番号も本番と同じ経路で振らせる）
fn set_agent_state(state: &mut State, pane_id: u32, target: AgentState) {
    match target {
        AgentState::Idle => {
            state.apply_status(status(pane_id, "SessionStart"));
        }
        AgentState::Working => {
            state.apply_status(status(pane_id, "UserPromptSubmit"));
        }
        AgentState::Blocked => {
            state.apply_status(status(pane_id, "Notification"));
        }
        AgentState::Done => {
            state.apply_status(status(pane_id, "UserPromptSubmit"));
            state.apply_status(status(pane_id, "Stop"));
        }
        AgentState::Error => {
            state.apply_status(status(pane_id, "StopFailure"));
        }
    }
}

fn triage_ids(state: &State) -> Vec<u32> {
    state.triage_entries().iter().map(|e| e.pane_id).collect()
}

#[test]
fn triage_lists_only_panes_that_need_attention() {
    let mut state = triage_state();
    // 1: 通知を受けていない（状態を持たない）/ 2: idle（既読）/ 3: blocked
    set_agent_state(&mut state, 2, AgentState::Idle);
    set_agent_state(&mut state, 3, AgentState::Blocked);

    assert_eq!(
        triage_ids(&state),
        vec![3],
        "状態を持たないペインと idle は一覧に出さない"
    );
}

#[test]
fn triage_orders_by_priority_tier() {
    let mut state = triage_state();
    // 投入順は優先度と無関係にしておく（並び替えが効いていることを見る）
    set_agent_state(&mut state, 1, AgentState::Done);
    set_agent_state(&mut state, 2, AgentState::Working);
    set_agent_state(&mut state, 3, AgentState::Error);
    set_agent_state(&mut state, 4, AgentState::Blocked);

    assert_eq!(
        triage_ids(&state),
        vec![3, 4, 2, 1],
        "error → blocked → working → done"
    );
}

#[test]
fn triage_spans_tabs() {
    let mut state = triage_state();
    set_agent_state(&mut state, 4, AgentState::Working); // タブ1
    set_agent_state(&mut state, 1, AgentState::Working); // タブ0

    // タブの壁を無視して並ぶ。タブ1のペインのほうが先に変化しているので下
    assert_eq!(triage_ids(&state), vec![1, 4]);
}

#[test]
fn triage_breaks_ties_by_the_latest_state_change() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Working);
    set_agent_state(&mut state, 3, AgentState::Working);
    assert_eq!(triage_ids(&state), vec![3, 2, 1], "新しく変わったものが上");

    // 1 が blocked を経て working に戻ると、同一階層内でいちばん新しくなる
    set_agent_state(&mut state, 1, AgentState::Blocked);
    set_agent_state(&mut state, 1, AgentState::Working);
    assert_eq!(triage_ids(&state), vec![1, 3, 2]);
}

#[test]
fn the_sequence_only_advances_on_a_state_change() {
    // カウンタだけが動くイベントで番号を進めると、同一階層内の並びが
    // 「直近の状態変化順」でなくなる
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Working);
    let before = state.agents[&1].state_change_seq;

    state.apply_status(status(1, "TaskCreated"));
    state.apply_status(status(1, "SubagentStart"));

    assert_eq!(state.agents[&1].state_change_seq, before);
    assert_eq!(triage_ids(&state), vec![2, 1], "並びも変わらない");
}

#[test]
fn the_triage_list_follows_state_changes() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('p')));
    assert_eq!(triage_ids(&state), vec![2, 1]);

    // 表示中に別のペインが blocked になったら、次の描画で上に来る
    set_agent_state(&mut state, 1, AgentState::Blocked);
    assert_eq!(triage_ids(&state), vec![1, 2]);
}

#[test]
fn p_switches_the_sidebar_to_the_triage_list() {
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Blocked);
    state.handle_nav_key(key(BareKey::Char('p')));

    assert!(state.triage.is_some());
    assert!(state.nav_mode, "トリアージモードはnavモードの内側");
    let rows = state.visible_rows();
    assert!(
        !rows
            .iter()
            .any(|r| matches!(r, Row::Tab(_) | Row::Pane { .. })),
        "ツリー表示は隠れる"
    );
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(r, Row::Triage { .. }))
            .count(),
        1,
        "一覧はエージェント状態を持つペインだけ"
    );
}

#[test]
fn esc_returns_to_the_tree_view_and_stays_in_nav_mode() {
    let mut state = triage_state();
    set_agent_state(&mut state, 3, AgentState::Blocked);
    state.selected = 1; // bravo を選択した状態で入る
    state.handle_nav_key(key(BareKey::Char('p')));
    state.handle_nav_key(key(BareKey::Esc));

    assert!(state.triage.is_none());
    assert!(state.nav_mode, "navモードは継続している");
    assert_eq!(state.selected, 1, "入る前の選択に戻す");
    assert!(
        state
            .visible_rows()
            .iter()
            .any(|r| matches!(r, Row::Tab(_))),
        "ツリー表示に戻る"
    );
}

#[test]
fn enter_jumps_from_the_triage_list_and_leaves_nav_mode() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 4, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('p')));
    // カーソルは一覧の先頭（error のタブ1のペイン）
    state.handle_nav_key(key(BareKey::Enter));

    assert_eq!(state.selectable[state.selected].pane_id, 4);
    assert!(!state.nav_mode, "ジャンプはnavモードの退場を伴う");
    assert!(state.triage.is_none());
}

#[test]
fn a_triage_jump_clears_the_read_state_through_the_usual_path() {
    // 既読クリアは「PaneUpdate でのフォーカス変化を見る」汎用の仕組みに乗せる。
    // トリアージモード専用のクリア処理を別に書くと、決定13が踏んだ配り漏れの
    // 罠を再発明することになる（要件: triage-mode-entry-exit.feature）
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Blocked);
    state.handle_nav_key(key(BareKey::Char('p')));
    state.handle_nav_key(key(BareKey::Enter));
    assert_eq!(state.selectable[state.selected].pane_id, 2);

    // ジャンプでフォーカスが移った結果が PaneUpdate として返ってくる
    let focused = PaneInfo {
        is_focused: true,
        ..terminal_pane(2, "bravo")
    };
    state.apply_read_model(&manifest(vec![(
        0,
        vec![
            terminal_pane(1, "alpha"),
            focused,
            terminal_pane(3, "charlie"),
        ],
    )]));
    settle_read(&mut state);
    assert_eq!(
        state.agents[&2].state,
        AgentState::Idle,
        "ジャンプ先は既読になる"
    );
}

#[test]
fn the_triage_cursor_moves_within_the_list() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Working);
    set_agent_state(&mut state, 3, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('p')));
    // 一覧は [3, 2, 1]
    assert_eq!(state.triage_cursor(), Some(3), "カーソルは先頭から");

    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.triage_cursor(), Some(2));
    state.handle_nav_key(key(BareKey::Down));
    assert_eq!(state.triage_cursor(), Some(1));
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.triage_cursor(), Some(1), "末尾で止まる");

    state.handle_nav_key(key(BareKey::Char('k')));
    assert_eq!(state.triage_cursor(), Some(2));
    state.handle_nav_key(key(BareKey::Char('G')).with_shift_modifier());
    assert_eq!(state.triage_cursor(), Some(1));
    state.handle_nav_key(key(BareKey::Char('g')));
    assert_eq!(state.triage_cursor(), Some(3));
    assert!(state.nav_mode, "移動キーではモードを抜けない");
    assert!(state.triage.is_some());
}

#[test]
fn the_triage_cursor_starts_at_the_most_urgent_row() {
    // 入る前の選択は継がない。トリアージが答えるのは「今どれに手を入れるか」で、
    // 選択中のペインはたいてい今まさに自分が作業している側
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Error);
    state.selected = 0; // alpha（working。一覧では2番目）
    state.handle_nav_key(key(BareKey::Char('p')));
    assert_eq!(state.triage_cursor(), Some(2));
}

#[test]
fn the_triage_cursor_does_not_move_the_selection() {
    // カーソルの移動は兄弟インスタンスへ配らない（要件: triage-cursor.feature）。
    // 配布そのものはホスト関数なのでテストから覗けないため、配る材料である
    // 選択（`selected`）が動かないことで押さえる。動くのは Enter の確定時だけ
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 3, AgentState::Working);
    state.selected = 1; // bravo（一覧には出ないペイン）
    state.handle_nav_key(key(BareKey::Char('p')));

    // 一覧は [3, 1]
    assert_eq!(state.triage_cursor(), Some(3));
    assert_eq!(state.selected, 1, "入場では選択を動かさない");
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.triage_cursor(), Some(1));
    assert_eq!(state.selected, 1, "カーソルの移動では選択を動かさない");

    state.handle_nav_key(key(BareKey::Enter));
    assert_eq!(
        state.selectable[state.selected].pane_id, 1,
        "確定したときだけ選択が動く"
    );
}

#[test]
fn the_triage_cursor_falls_back_when_its_pane_leaves_the_list() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Blocked);
    set_agent_state(&mut state, 2, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('p')));
    assert_eq!(state.triage_cursor(), Some(1));

    // 既読になって一覧から消えたら、残った先頭へ寄せる
    state.agents.get_mut(&1).unwrap().mark_read();
    assert_eq!(state.triage_cursor(), Some(2));

    // 一覧が空になれば None（Enter は何も起こさない）
    state.agents.get_mut(&2).unwrap().state = AgentState::Idle;
    assert_eq!(state.triage_cursor(), None);
    state.handle_nav_key(key(BareKey::Enter));
    assert!(state.nav_mode, "空の一覧での Enter はモードに留まる");
    assert!(state.triage.is_some());
}

#[test]
fn triage_leaves_nav_mode_on_undefined_keys() {
    // 安全弁（決定12）はサブモードでも最上位まで効かせる
    for k in [
        key(BareKey::Char('z')),
        key(BareKey::Char('q')),
        key(BareKey::Char('j')).with_ctrl_modifier(),
        key(BareKey::Char('p')).with_alt_modifier(),
    ] {
        let mut state = triage_state();
        set_agent_state(&mut state, 1, AgentState::Working);
        state.handle_nav_key(key(BareKey::Char('p')));
        state.handle_nav_key(k.clone());
        assert!(!state.nav_mode, "{:?} でnavモードごと抜けるべき", k);
        assert!(state.triage.is_none());
    }
}

#[test]
fn an_empty_triage_list_says_so() {
    let mut state = triage_state();
    state.handle_nav_key(key(BareKey::Char('p')));
    let rows = state.visible_rows();
    let Some(Row::Notice(notice)) = rows.get(HEADER_ROWS) else {
        panic!("空リストのままだと壊れて見える: {}", rows.len());
    };
    assert_eq!(*notice, "nothing to triage");
}

#[test]
fn triage_rows_carry_the_pane_name_and_its_tab_name() {
    let mut state = triage_state();
    set_agent_state(&mut state, 4, AgentState::Blocked); // タブ1の delta
    state.handle_nav_key(key(BareKey::Char('p')));

    let rows = state.visible_rows();
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い: {}", rows.len());
    };
    let text = state.triage_row(entry, tab_name, false, tab_column, None, SIDEBAR);
    let content = text.content();

    assert!(content.contains("delta"), "ペイン名: {}", content);
    assert!(
        content.ends_with("tab2"),
        "所属タブ名を右端に併記: {}",
        content
    );
    assert!(
        content.starts_with("  ◆ "),
        "状態アイコンは通常表示と同じ: {}",
        content
    );
    assert!(
        unicode_width::UnicodeWidthStr::width(content) <= CONTENT,
        "右端にはマージンを空ける: {}",
        content
    );
}

#[test]
fn a_long_pane_name_does_not_push_the_tab_name_off_the_row() {
    let mut state = triage_state();
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(1, "要件定義とドキュメント整理タスクの続き")],
    )]));
    state.rebuild_selectable();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('p')));

    let rows = state.visible_rows();
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い");
    };
    let content = state
        .triage_row(entry, tab_name, false, tab_column, None, SIDEBAR)
        .content()
        .to_string();
    assert!(content.ends_with("tab1"), "タブ名は残す: {}", content);
    assert!(content.contains('…'), "畳むのはペイン名側: {}", content);
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(content.as_str()),
        CONTENT,
        "右端は揃える: {}",
        content
    );
}

#[test]
fn triage_rows_drop_the_counter_column_and_the_cwd_row() {
    // サイドバー幅32にタブ名とカウンタ列の両方は載らないので、タブ名を優先する。
    // cwd行も同じ理由で出さない（要件: triage-list-display.feature）
    let mut state = triage_state();
    state.show_cwd = true;
    state.pane_cwds.insert(4, "/work/oss/fujin".to_string());
    set_agent_state(&mut state, 4, AgentState::Working);
    state.apply_status(status(4, "SubagentStart")); // サブエージェント数 +1
    state.apply_status(status(4, "TaskCreated")); // 未完了タスク数 [1]
    state.handle_nav_key(key(BareKey::Char('p')));

    let rows = state.visible_rows();
    assert!(
        !rows.iter().any(|r| matches!(r, Row::Cwd { .. })),
        "cwd行は出さない"
    );
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い: {}", rows.len());
    };
    let content = state
        .triage_row(entry, tab_name, false, tab_column, None, SIDEBAR)
        .content()
        .to_string();
    assert_eq!(state.agents[&4].subagents, 1, "カウンタ自体は数えている");
    assert_eq!(state.agents[&4].open_tasks, 1);
    assert!(
        !content.contains("+1"),
        "サブエージェント数は出さない: {}",
        content
    );
    assert!(
        !content.contains("[1]"),
        "未完了タスク数は出さない: {}",
        content
    );
    assert!(content.ends_with("tab2"), "タブ名を優先する: {}", content);
}

#[test]
fn the_triage_help_overlay_lists_its_own_keys() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('p')));
    state.handle_nav_key(key(BareKey::Char('?')));

    let lines: Vec<String> = state
        .help_lines()
        .iter()
        .map(|row| state.help_line(row, SIDEBAR).content().to_string())
        .collect();
    assert!(
        lines.iter().any(|line| line.contains("back to tree")),
        "トリアージモードのキーを出す: {:?}",
        lines
    );
    for line in &lines {
        assert!(!line.contains('…'), "幅32に収まらない: {}", line);
    }
    // ヘルプを閉じてもトリアージモードには留まる
    state.handle_nav_key(key(BareKey::Char('j')));
    assert!(state.triage.is_some());
    assert!(!state.help_overlay);
}

#[test]
fn the_nav_help_advertises_the_triage_key() {
    let state = triage_state();
    let lines: Vec<String> = state
        .help_lines()
        .iter()
        .map(|row| state.help_line(row, SIDEBAR).content().to_string())
        .collect();
    assert!(
        lines.iter().any(|l| l.contains("triage")),
        "navモードのヘルプから辿れないと気づけない: {:?}",
        lines
    );
}

#[test]
fn clicking_a_triage_row_jumps_to_that_pane() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 4, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('p')));
    // 0-2: ヘッダ / 3: delta（error）/ 4: alpha（working）
    assert!(state.handle_click(HEADER_ROWS as isize + 1));
    assert_eq!(state.selectable[state.selected].pane_id, 1);
    assert!(!state.nav_mode);
    assert!(state.triage.is_none(), "トリアージモードも一緒に畳む");
}

#[test]
fn render_survives_triage_mode() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 4, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('p')));
    state.render(40, 20);
    state.render(3, 2);
    state.render(2, 1);
    state.render(0, 0);

    // 対象なしの通知行（`nothing to triage`）も通す
    let mut state = triage_state();
    state.handle_nav_key(key(BareKey::Char('p')));
    state.render(40, 20);
    state.render(1, 1);
}

// --- コマンド状態（決定32。要件: docs/requirements/command-status/） ---
//
// コマンドペインの走行・終了を PaneManifest から導出する。エージェント状態とは
// 別概念だが、記号・既読モデル・待ち件数・トリアージ一覧は共用する

fn command_pane(id: u32, command: &str) -> PaneInfo {
    PaneInfo {
        id,
        terminal_command: Some(command.to_string()),
        ..Default::default()
    }
}

// 終了して残っているコマンドペイン（`close_on_exit` は既定 false なので、
// プロセスが終わってもペインは `exited` のまま残り続ける）
fn exited_command_pane(id: u32, command: &str, exit_status: Option<i32>) -> PaneInfo {
    PaneInfo {
        exited: true,
        exit_status,
        ..command_pane(id, command)
    }
}

// 与えたペインを持つタブ0だけの状態。導出（apply_command_states）まで済ませる
fn state_with_command_panes(panes: Vec<PaneInfo>) -> State {
    let mut state = state_with_panes(0);
    let manifest = manifest(vec![(0, panes)]);
    state.apply_command_states(&manifest);
    state.panes = Some(manifest);
    state.rebuild_selectable();
    state
}

#[test]
fn a_running_command_pane_is_working() {
    let state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
}

#[test]
fn a_command_pane_that_exits_cleanly_is_done() {
    let state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Done))
    );
}

#[test]
fn any_other_exit_is_an_error() {
    // 非0コードもシグナル終了（exit_status なし）も区別せず error（決定32）
    let failed = state_with_command_panes(vec![exited_command_pane(1, "make", Some(1))]);
    assert_eq!(
        failed.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
    let signalled = state_with_command_panes(vec![exited_command_pane(1, "make", None)]);
    assert_eq!(
        signalled.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

#[test]
fn a_shell_pane_has_no_command_state() {
    // コマンドペインでなければ状態を持たない（フックが無ければ状態を持たない
    // エージェント状態と対称）
    let state = state_with_command_panes(vec![terminal_pane(1, "zsh")]);
    assert_eq!(state.pane_status(1), None);
    assert!(state.commands.is_empty());
}

#[test]
fn trivial_commands_are_tracked_too() {
    // 絞り込みは行わない（決定32）。実行時間の閾値のようなヒューリスティックは持たない
    let state = state_with_command_panes(vec![exited_command_pane(1, "ls", Some(0))]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Done))
    );
}

#[test]
fn re_running_a_command_returns_it_to_working() {
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(1))]);
    let seq_before = state.status_seq(1);

    state.apply_command_states(&manifest(vec![(0, vec![command_pane(1, "make")])]));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
    assert!(
        state.status_seq(1) > seq_before,
        "状態が変わったのでシーケンス番号も進む"
    );
}

#[test]
fn focusing_a_finished_command_pane_marks_it_read() {
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);

    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);

    assert_eq!(state.pane_status(1), None, "既読は状態を持たない側へ戻る");
}

#[test]
fn a_read_command_state_does_not_come_back() {
    // 終了したコマンドペインは exited が立ちっぱなしなので、既読を旗で持たないと
    // 次の PaneUpdate で同じ done が再導出されて復活する
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);

    state.apply_command_states(&manifest(vec![(
        0,
        vec![exited_command_pane(1, "make", Some(0))],
    )]));

    assert_eq!(state.pane_status(1), None);
}

#[test]
fn a_running_command_is_not_marked_read() {
    // working は既読にならない（走っている最中のコマンドは人を待っていない）
    let mut state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    let focused = PaneInfo {
        is_focused: true,
        ..command_pane(1, "docker build .")
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
}

#[test]
fn the_agent_state_wins_over_the_command_state() {
    // `zellij run -- claude` のようにコマンドペイン経由でエージェントを起動した
    // ケース。フック由来の状態が常に優先で、コマンド状態は無視する（決定32）
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "claude", Some(1))]);
    state.apply_status(status(1, "UserPromptSubmit"));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Agent(AgentState::Working))
    );
}

#[test]
fn command_states_join_the_waiting_count() {
    let state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        exited_command_pane(2, "cargo test", Some(101)),
        command_pane(3, "docker build ."),
        terminal_pane(4, "zsh"),
    ]);
    // done と error だけを数える（working は人の対応を待っていない）
    assert_eq!(state.waiting_count(), 2);
}

#[test]
fn command_states_join_the_triage_list() {
    let mut state = state_with_command_panes(vec![
        command_pane(1, "docker build ."),
        exited_command_pane(2, "cargo test", Some(101)),
        terminal_pane(3, "zsh"),
    ]);
    state.apply_status(status(3, "Notification"));

    let ids: Vec<u32> = state.triage_entries().iter().map(|e| e.pane_id).collect();
    // 優先度階層はソースを問わず共通（error → blocked → working → done）
    assert_eq!(ids, vec![2, 3, 1]);
}

#[test]
fn a_command_pane_without_a_name_shows_its_command() {
    // 決定32: ペイン名が空ならコマンド文字列を代わりに出す
    let state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(content.contains("docker build ."), "{}", content);
    assert!(content.contains('»'), "走行中のアイコンも出る: {}", content);
}

#[test]
fn a_named_command_pane_keeps_its_name() {
    // `zellij run --name` やリネームで名前が付いていればそちらが優先
    let named = PaneInfo {
        title: "build".to_string(),
        ..command_pane(1, "docker build .")
    };
    let state = state_with_command_panes(vec![named]);
    let content = state
        .pane_row(
            &state.selectable[0],
            false,
            None,
            column_of(&state),
            HeadCells::default(),
            SIDEBAR,
        )
        .content()
        .to_string();
    assert!(content.contains("build"), "{}", content);
    assert!(!content.contains("docker"), "{}", content);
}

#[test]
fn closed_command_panes_lose_their_state() {
    let mut state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        command_pane(2, "docker build ."),
    ]);
    state.panes = Some(manifest(vec![(0, vec![command_pane(2, "docker build .")])]));
    state.prune_stale_agents();

    assert!(!state.commands.contains_key(&1));
    assert!(state.commands.contains_key(&2));
}

// --- コマンド状態のインスタンス間同期（決定13・決定32） ---

#[test]
fn the_command_dump_round_trips() {
    let mut state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        command_pane(2, "docker build ."),
    ]);
    // 1つを既読にしてから配る
    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);
    let dump = state.command_dump();

    let mut peer = State::default();
    assert!(peer.apply_command_dump(&dump));

    assert_eq!(peer.pane_status(1), None, "既読も一緒に配る");
    assert_eq!(
        peer.pane_status(2),
        Some(PaneStatus::Command(CommandState::Working))
    );
    assert_eq!(peer.status_seq(2), state.status_seq(2));
    // 取り込んだ番号より古い番号を後から振らないよう、カウンタを進めておく
    assert!(peer.state_seq >= state.status_seq(2));
}

#[test]
fn a_broken_command_dump_line_is_skipped() {
    let mut state = State::default();
    assert!(!state.apply_command_dump("garbage\nx\tdone\t0\t1\n"));
    assert!(state.commands.is_empty());
}

#[test]
fn the_read_clear_pipe_also_clears_command_states() {
    // 既読クリアの配布（決定13）はソースを区別しない
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    assert!(state.pipe(pipe_message(READ_CLEAR_PIPE, "1")));
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn the_command_state_pipe_takes_a_dump() {
    let mut state = State::default();
    assert!(state.pipe(pipe_message(COMMAND_STATE_PIPE, "7\terror\t0\t3\n")));
    assert_eq!(
        state.pane_status(7),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

// --- 既読の猶予（docs/issues/command-status-error-icon-swallowed.md） ---
//
// `zellij run` は新しいペインへフォーカスを移すので、一瞬で終わるコマンドは
// 必ず「フォーカス中に終了」する。素直に既読モデルを当てると、状態が付いた
// 同じ PaneUpdate の中で既読になり、アイコンが一度も描かれないまま消える

fn focused(pane: PaneInfo) -> PaneInfo {
    PaneInfo {
        is_focused: true,
        ..pane
    }
}

// 実セッションの再現手順（`zellij run -- sh -c 'exit 1'`）をそのままなぞる。
// 新しいコマンドペインがフォーカスを持ったまま走り、そのまま失敗して終わる
fn run_and_fail_while_focused() -> State {
    let mut state = state_with_panes(0);
    // 1回目の PaneUpdate: ペインが出来てフォーカスを持ち、まだ走っている
    let running = manifest(vec![(0, vec![focused(command_pane(1, "sh -c exit 1"))])]);
    state.apply_command_states(&running);
    state.apply_read_model(&running);
    state.panes = Some(running);
    state.rebuild_selectable();
    // 2回目の PaneUpdate: フォーカスを持ったまま失敗して終わる
    let exited = manifest(vec![(
        0,
        vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
    )]);
    state.apply_command_states(&exited);
    state.apply_read_model(&exited);
    state.panes = Some(exited);
    state.rebuild_selectable();
    state
}

#[test]
fn a_command_that_fails_while_focused_still_shows_its_icon() {
    let state = run_and_fail_while_focused();
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error)),
        "状態が付いた瞬間にフォーカスしていても、その場では既読にしない"
    );
    let content = state
        .pane_row(
            &state.selectable[0],
            false,
            None,
            column_of(&state),
            HeadCells::default(),
            SIDEBAR,
        )
        .content()
        .to_string();
    assert!(content.contains('×'), "{}", content);
}

#[test]
fn the_read_grace_survives_repeated_updates() {
    // 描画のたびに PaneUpdate が飛ぶ（実測で数msおきに連続）。猶予が1回きり
    // だと、この連打の中で結局既読になってしまう
    let mut state = run_and_fail_while_focused();
    let exited = manifest(vec![(
        0,
        vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
    )]);
    for _ in 0..5 {
        state.apply_command_states(&exited);
        state.apply_read_model(&exited);
    }
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

#[test]
fn leaving_the_pane_keeps_the_icon_and_coming_back_clears_it() {
    let mut state = run_and_fail_while_focused();
    // ユーザーが別のペインへ移る: 猶予は解けるが、既読にはならない
    let away = manifest(vec![(
        0,
        vec![
            exited_command_pane(1, "sh -c exit 1", Some(1)),
            focused(terminal_pane(2, "zsh")),
        ],
    )]);
    state.apply_command_states(&away);
    state.apply_read_model(&away);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error)),
        "離れただけでは既読にしない"
    );

    // 戻ってきて初めて既読になる
    let back = manifest(vec![(
        0,
        vec![
            focused(exited_command_pane(1, "sh -c exit 1", Some(1))),
            terminal_pane(2, "zsh"),
        ],
    )]);
    state.apply_command_states(&back);
    state.apply_read_model(&back);
    settle_read(&mut state);
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn another_tab_counts_as_having_left_the_pane() {
    // 別タブに居るあいだはフォーカスがそのペインに無いので猶予は解ける。
    // タブを切り替えて戻ってきたときに既読にならない、という取りこぼしを防ぐ
    let mut state = run_and_fail_while_focused();
    state.tabs = vec![tab(0, false), tab(1, true)];
    let elsewhere = manifest(vec![
        (0, vec![exited_command_pane(1, "sh -c exit 1", Some(1))]),
        (1, vec![focused(terminal_pane(2, "zsh"))]),
    ]);
    state.apply_command_states(&elsewhere);
    state.apply_read_model(&elsewhere);

    state.tabs = vec![tab(0, true), tab(1, false)];
    let back = manifest(vec![
        (
            0,
            vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
        ),
        (1, vec![terminal_pane(2, "zsh")]),
    ]);
    state.apply_command_states(&back);
    state.apply_read_model(&back);
    settle_read(&mut state);
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn the_read_clear_pipe_ignores_the_grace() {
    // 既読クリアの配布（決定13）は、送り手が猶予込みで判断した結果。
    // 受け手が猶予で握り潰すと、タブごとにアイコンの有無が食い違う
    let mut state = run_and_fail_while_focused();
    assert!(state.pipe(pipe_message(READ_CLEAR_PIPE, "1")));
    assert_eq!(state.pane_status(1), None);
}

// --- 配置演出（要件: docs/requirements/header-animation/） ---

// 兵が使える領域の実測値（幅32セル・右マージン2セル・`▲ fujin` は7セル）。
// 発進位置は本文の右端の1つ先、いちばん奥の着地列は内容幅の右端
const LAUNCH: usize = 8;
const DEEPEST: usize = CONTENT - 1;

// ペイン一覧を差し替えて1回ぶん観測させる。`Event::PaneUpdate` の扱いと同じ順序。
// **配置演出のトリガーはもう一覧を見ない**（フック通知だけで判定する）ので、ここでは
// ヘッダー描画に要る状態を作るだけ
fn observe_panes(state: &mut State, ids: &[u32]) {
    let panes: Vec<PaneInfo> = ids
        .iter()
        .map(|id| terminal_pane(*id, &format!("pane{}", id)))
        .collect();
    state.panes = Some(manifest(vec![(0, panes)]));
    state.rebuild_selectable();
}

// 新規エージェント検出から配置演出の発火までを通す。
//
// 本番では検出（`apply_status` の戻り値）と発火（`begin_deployment`）の間に
// 可視インスタンス判定（`State::is_visible_instance`）が挟まるが、これはホスト関数
// `get_focused_pane_info()` を呼ぶのでテストから通せない
//（docs/dev/build-and-test.md「テストで検証できない範囲」）。ここでは判定を通った
// 後の発火だけを見る
fn deploy_agents(state: &mut State, troops: usize) {
    state.begin_deployment(troops);
}

// まだ何も観測していない、既定幅で描画済みのサイドバー
fn sidebar_state() -> State {
    State {
        tabs: vec![tab(0, true)],
        permissions_granted: true,
        viewport_cols: SIDEBAR,
        ..Default::default()
    }
}

// いま画面に出ている兵の列
fn troop_columns(state: &State) -> Vec<usize> {
    let (launch, width) = state.troop_field(SIDEBAR);
    state
        .deployment
        .map(|deployment| deployment.columns(launch, width))
        .unwrap_or_default()
}

// 演出が終わるまでフレームを送る。返すのは要したフレーム数
fn play_out(state: &mut State) -> usize {
    for frame in 1.. {
        state.advance_deployment();
        if state.deployment.is_none() {
            return frame;
        }
        assert!(frame < 100, "演出が終わらない");
    }
    unreachable!()
}

#[test]
fn new_panes_alone_do_not_start_a_deployment() {
    // 判定材料はフック通知だけで、ペインが増えたかどうかは見ない。旧実装（増えた
    // ターミナルペインで判定）では `vim` やビルドコマンドでも演出が出ていた
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    observe_panes(&mut state, &[1, 2, 3]);

    assert!(state.deployment.is_none());
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");
}

#[test]
fn a_session_start_is_a_new_agent_detection() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);

    assert!(
        state.apply_status(status(2, "SessionStart")),
        "SessionStart は新規エージェント検出になる"
    );
}

#[test]
fn an_agent_started_in_an_existing_pane_is_detected() {
    // 空のシェルペインを先に開いておき、後から `claude` を打つ使い方（要件:
    // 前から開いてあるペインで後からエージェントを起動しても配置演出が始まる）。
    // ペインの側は何も変わらないまま通知だけが届く
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    // 一覧を何度観測しても増減が無い状態を作ってから通知を受ける
    observe_panes(&mut state, &[1, 2]);

    assert!(state.apply_status(status(2, "SessionStart")));
}

#[test]
fn a_restarted_conversation_is_not_a_new_agent() {
    // `/clear` とコンパクトは稼働中のエージェントの仕切り直しで、着任ではない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for source in ["clear", "compact"] {
        assert!(
            !state.apply_status(session_start(1, source)),
            "source={} は新規エージェント検出にしない",
            source
        );
    }
}

#[test]
fn a_fresh_session_is_a_new_agent() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for source in ["startup", "resume", "fork"] {
        assert!(
            state.apply_status(session_start(1, source)),
            "source={} は新規エージェント検出になる",
            source
        );
    }
}

#[test]
fn a_session_start_without_a_source_still_counts() {
    // `source` を送らない旧フックスクリプトのままでも演出は出る。判定を
    // ホワイトリストではなく除外方式にしてあるのはこのため
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    assert!(state.apply_status(status(1, "SessionStart")));
}

#[test]
fn other_hook_events_are_never_new_agent_detections() {
    // **リロード直後の誤検出を防いでいるのがこの性質。** プラグインをリロードすると
    // `agents` マップは空になる（docs/issues/redeploy-resets-agent-state.md）が、
    // 稼働中のエージェントから次に届くのは SessionStart 以外のイベントなので、
    // 既存エージェントが新規と誤検出されることはない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for event in [
        "UserPromptSubmit",
        "Stop",
        "StopFailure",
        "Notification",
        "SubagentStart",
        "SubagentStop",
        "TaskCreated",
        "TaskCompleted",
        "SessionEnd",
    ] {
        assert!(
            !state.apply_status(status(1, event)),
            "{} は新規エージェント検出にしない",
            event
        );
    }
}

#[test]
fn a_new_agent_starts_the_deployment_animation() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    deploy_agents(&mut state, 1);

    assert_eq!(state.deployment.map(|d| d.troops), Some(1));
    // 兵はブランド行の `fujin` の右側から現れる
    assert_eq!(troop_columns(&state), vec![LAUNCH]);
    let header = state.header_line(SIDEBAR).content().to_string();
    assert!(header.starts_with("▲ fujin "), "{}", header);
    assert_eq!(
        header.chars().position(|c| c.to_string() == TROOP),
        Some(LAUNCH)
    );
}

#[test]
fn every_detected_agent_gets_a_troop() {
    for detected in [1usize, 3, 6] {
        let mut state = sidebar_state();
        observe_panes(&mut state, &[1]);
        deploy_agents(&mut state, detected);

        assert_eq!(
            state.deployment.map(|d| d.troops),
            Some(detected),
            "{}体の検出",
            detected
        );
    }
}

#[test]
fn show_deploy_animation_can_switch_the_animation_off() {
    // 演出は情報を運ばないので、切っても見える情報は変わらない（決定40）
    let mut state = sidebar_state();
    state.apply_config(&plugin_config(&[("show_deploy_animation", "false")]));
    observe_panes(&mut state, &[1, 2]);

    // 新規エージェントの検出そのものは、切っている間も動く
    //（要件: But 新規エージェントの検出そのものは行われる）
    assert!(state.apply_status(status(2, "SessionStart")));
    deploy_agents(&mut state, 1);
    assert!(state.deployment.is_none(), "配置演出は再生されない");
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");

    // 戻せば次の検出から再生される
    state.apply_config(&plugin_config(&[("show_deploy_animation", "true")]));
    deploy_agents(&mut state, 1);
    assert_eq!(state.deployment.map(|d| d.troops), Some(1));
}

#[test]
fn detections_in_the_same_window_join_one_deployment() {
    // 新規タブ作成のように一括で着任するときは、検出が複数回に割れて届く。
    // 検出のたびに発火させると演出が重なって騒がしくなる
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 1);
    state.advance_deployment();
    deploy_agents(&mut state, 2);

    let deployment = state.deployment.expect("演出は続いている");
    assert_eq!(deployment.troops, 3, "検出した数の合計ぶんの兵が出る");
    assert_eq!(deployment.frame, 1, "演出は最初から巻き直さない");
}

#[test]
fn the_troops_line_up_with_the_first_launched_deepest() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 3);

    // 先に発進した兵ほど奥へ着く。着地列は右端から2セル間隔
    let mut landed = Vec::new();
    for _ in 0..20 {
        state.advance_deployment();
        let columns = troop_columns(&state);
        if columns.len() == 3 && columns.iter().all(|c| *c >= DEEPEST - 4) {
            landed = columns;
            break;
        }
    }
    assert_eq!(landed, vec![DEEPEST - 4, DEEPEST - 2, DEEPEST]);

    // 静止したあとは動かない
    state.advance_deployment();
    assert_eq!(troop_columns(&state), landed);
}

#[test]
fn the_troops_stay_clear_of_the_brand() {
    // 幅が足りないぶんの兵は着地列を確保できない。ブランド行に重ねるくらいなら
    // 出さない（着地列は発進位置より左には作らない）
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 39);

    // 出せるだけ出た瞬間（＝いちばん多く並んだフレーム）を見る
    let mut columns = Vec::new();
    while state.deployment.is_some() {
        state.advance_deployment();
        let frame = troop_columns(&state);
        if frame.len() > columns.len() {
            columns = frame;
        }
    }
    assert!(columns.iter().all(|c| *c >= LAUNCH), "{:?}", columns);
    assert_eq!(columns.first(), Some(&LAUNCH));
    assert_eq!(columns.last(), Some(&DEEPEST));
}

#[test]
fn the_header_returns_to_normal_when_the_deployment_ends() {
    // 着地点は完全に元へ戻る。稼働数のような情報は残さない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 2);
    assert!(state.header_line(SIDEBAR).content().contains(TROOP));

    let frames = play_out(&mut state);
    assert!(
        frames > 3,
        "整列した状態を見せる間もなく畳んでいる: {}",
        frames
    );
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");
    // 取り残されたタイマーが来ても何も起きない
    assert!(!state.advance_deployment());
}

// 「サイドバーが表示されていない間の検出では演出は再生されない」「見逃した検出は
// 後から遡って演出されない」の2要件は、可視インスタンス判定（`is_visible_instance`）
// が担っている。ホスト関数 `get_focused_pane_info()` を呼ぶためユニットテストからは
// 通せない（docs/dev/build-and-test.md「テストで検証できない範囲」）ので、実機での
// 手動確認に頼る。

#[test]
fn the_deployment_leaves_the_mode_label_readable() {
    // navモード中は `[nav]` のぶんだけ発進位置が右へずれる。兵がラベルに
    // 重なるとどちらも読めなくなる
    let mut state = sidebar_state();
    state.nav_mode = true;
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 1);

    let header = state.header_line(SIDEBAR).content().to_string();
    assert!(header.starts_with("▲ fujin  [nav] "), "{}", header);
    assert_eq!(
        troop_columns(&state),
        vec!["▲ fujin  [nav]".chars().count() + 1]
    );
}

#[test]
fn render_survives_the_deployment() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2, 3]);
    deploy_agents(&mut state, 2);
    state.render(40, SIDEBAR);
    state.render(40, 12);
    state.render(3, 2);
    state.render(0, 0);
    // 幅0で描いたあともフレーム送りは止まらない
    state.advance_deployment();
}
