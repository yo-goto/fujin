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
use crate::render::{shift_highlight_indices, truncate};
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

// 臨時召喚されたインスタンス（決定16）。常駐との違いはフローティングかどうか
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
        cwd: None,
        detail: None,
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
fn truncate_counts_chars_not_bytes() {
    assert_eq!(truncate("日本語テスト", 6), "日本語テスト");
    assert_eq!(truncate("日本語テスト", 3), "日本…");
}

#[test]
fn truncate_with_zero_width_is_empty() {
    // 省略記号1文字だけがはみ出すとサイドバー幅を壊す
    assert_eq!(truncate("abc", 0), "");
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

#[test]
fn stop_clears_running_subagents() {
    let mut state = State::default();
    state.apply_status(status(1, "SubagentStart"));
    state.apply_status(status(1, "SubagentStart"));
    assert_eq!(state.agents[&1].subagents, 2);

    state.apply_status(status(1, "SubagentStop"));
    assert_eq!(state.agents[&1].subagents, 1);

    // ターン終了時点でサブエージェントは全て終わっている
    state.apply_status(status(1, "Stop"));
    assert_eq!(state.agents[&1].state, AgentState::Done);
    assert_eq!(state.agents[&1].subagents, 0);
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
    source.apply_status(status(2, "TaskCreated"));
    source.pane_cwds.insert(1, "/work/fujin".to_string());

    let mut restored = State::default();
    restored.apply_state_dump(&source.state_dump());

    assert_eq!(restored.agents.len(), 2);
    assert_eq!(restored.agents[&1].state, AgentState::Working);
    assert_eq!(restored.agents[&1].subagents, 1);
    assert_eq!(restored.agents[&1].agent, "claude");
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

    assert_eq!(state.agents[&1].state, AgentState::Idle);
    // フォーカスしていないペインは既読にしない
    assert_eq!(state.agents[&2].state, AgentState::Done);
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
fn nav_digit_jumps_and_leaves_the_mode() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('2')));
    assert_eq!(state.selected, 1);
    assert!(!state.nav_mode);
}

#[test]
fn nav_digit_out_of_range_is_ignored() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('9')));
    assert_eq!(state.selected, 0);
    assert!(state.nav_mode, "範囲外のときはモードに留まる");
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

    // navモード中の選択は探索カーソルなので追従させない
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
    // 臨時サイドバー（フローティング）は自分がフローティング層のフォーカスを
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
    assert!(state.nav_mode, "検索はnavモードの内側");
    assert_eq!(search.query, "");
    assert_eq!(search.hits.len(), 3, "空クエリは全件一致");
    assert_eq!(search.cursor, Some(1), "カーソルは検索前の選択から始まる");
}

#[test]
fn printable_chars_feed_the_query_not_the_selection() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    // navモードでは j は移動キーだが、検索中はクエリになる
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.search.as_ref().unwrap().query, "j");
    assert_eq!(state.selected, 0, "選択行は動かない");
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
    assert!(state.nav_mode, "検索のEscでnavモードごと抜けてはいけない");

    // 2段目: navモードから離脱（召喚ならここで自分を閉じる）
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

    // 検索中に先頭へペインが増えてインデックスがずれても、ペインIDで戻す
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
        "消えたら結果の先頭へ寄せる"
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
fn unknown_pipes_are_ignored() {
    let mut state = State::default();
    assert!(!state.pipe(pipe_message("some_other_plugin", "payload")));
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

    // 0件（「一致なし」の行）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "zzz");
    state.render(40, 20);
    state.render(1, 1);
}

// --- ペイン行のクリック（要件: docs/requirements/click-to-focus/） ---
//
// 実際にフォーカスが移るかはホスト側の仕事（focus_pane_with_id はスタブで
// 何もしない）なので、ここでは「どの行がどのペインに対応するか」と
// クリックが選択・モードに与える影響を見る。

// 通常表示（navモード外）の searchable_state。行の並びは
// 0: tab1見出し / 1: alpha / 2: bravo / 3: tab2見出し / 4: charlie
fn clickable_state() -> State {
    let mut state = searchable_state();
    state.nav_mode = false;
    state
}

#[test]
fn clicking_a_pane_row_selects_that_pane() {
    let mut state = clickable_state();

    assert!(state.handle_click(2));
    assert_eq!(state.selectable[state.selected].pane_id, 2);

    // タブをまたいだ行も同じように引ける
    assert!(state.handle_click(4));
    assert_eq!(state.selectable[state.selected].pane_id, 3);
}

#[test]
fn clicking_a_tab_heading_does_nothing() {
    let mut state = clickable_state();
    state.selected = 1;

    assert!(!state.handle_click(0));
    assert!(!state.handle_click(3));
    assert_eq!(state.selected, 1, "選択は動かない");
}

#[test]
fn clicking_outside_the_list_does_nothing() {
    let mut state = clickable_state();
    state.selected = 1;

    // 一覧より下の余白
    assert!(!state.handle_click(5));
    assert!(!state.handle_click(99));
    // 負の行（サイドバーの外）
    assert!(!state.handle_click(-1));
    assert_eq!(state.selected, 1);
}

#[test]
fn clicking_in_nav_mode_jumps_and_leaves_the_mode() {
    let mut state = searchable_state(); // nav_mode = true
                                        // navモードではヘッダが1行入るぶん、行がひとつ下へずれる
    assert!(state.handle_click(3));

    assert_eq!(state.selectable[state.selected].pane_id, 2);
    assert!(!state.nav_mode, "ジャンプしたら横取りは解除する");
}

#[test]
fn clicking_follows_the_filtered_layout_while_searching() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alp");
    // 0: クエリ行 / 1: tab1見出し / 2: alpha（bravo と tab2 は絞り込みで消える）
    assert!(!state.handle_click(1), "見出し行は対象外");

    assert!(state.handle_click(2));
    assert_eq!(state.selectable[state.selected].pane_id, 1);
    assert!(!state.nav_mode);
    assert!(state.search.is_none(), "検索も一緒に畳む");
}

#[test]
fn clicking_is_ignored_before_permissions_are_granted() {
    let mut state = clickable_state();
    state.permissions_granted = false;
    // 承認前は「permissions required」しか描いていないので、そこに行は無い
    assert!(!state.handle_click(1));
}
