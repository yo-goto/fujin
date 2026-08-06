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
use crate::render::{
    cwd_row, divider_line, fold_highlight_indices, overflow_row, pad_to_width, reconcile_scroll,
    shift_highlight_indices, truncate, truncate_start, CounterColumn, Row,
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

#[test]
fn the_brand_row_is_the_same_in_every_mode() {
    // ブランド行はモードによらず内容が変わらない（要件: sidebar-header.feature）
    let mut state = searchable_state();
    let plain = state.brand_line(32).content().to_string();
    assert_eq!(plain, "▲ fujin");

    state.nav_mode = true;
    assert_eq!(state.brand_line(32).content(), plain);
    state.handle_nav_key(key(BareKey::Char('/')));
    assert_eq!(state.brand_line(32).content(), plain);
}

#[test]
fn the_brand_triangle_carries_the_mode_color() {
    // モード名を読まなくても三角の色だけでモードが判別できるようにする
    //（docs/concept/ui-design.md の「ヘッダ」）
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);

    // ツリー表示は dim のみ。色は乗らない
    state.nav_mode = false;
    let tree = state.brand_line(SIDEBAR);
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
    assert_eq!(ink_at(&state.brand_line(SIDEBAR), 2), vec![0]);
    state.handle_nav_key(key(BareKey::Char('p')));
    assert_eq!(ink_at(&state.brand_line(SIDEBAR), 0), vec![0]);
}

#[test]
fn the_brand_name_stays_dim_in_every_mode() {
    // 色を乗せるのは三角1文字だけ。ブランド名まで色を付けると、ツリーの
    // 状態アイコンの色分けと喧嘩する
    let mut state = state_with_panes(2);
    state.nav_mode = true;

    let brand = state.brand_line(SIDEBAR);
    let dim = ink_at(&brand, DIM_LEVEL);
    // "▲ fujin" の1文字目以降（空白 + ブランド名）が dim
    assert_eq!(
        dim,
        (1..brand.content().chars().count()).collect::<Vec<_>>()
    );
    assert_eq!(ink_at(&brand, 2), vec![0], "色は三角だけ");
}

#[test]
fn the_header_keeps_its_three_rows_across_every_mode_boundary() {
    // モードの入退場でヘッダの行数が変わると、ツリー全体がそのぶん
    // 上下にずれる（要件: sidebar-header.feature）
    let mut state = searchable_state();
    set_agent_state(&mut state, 1, AgentState::Done);
    state.nav_mode = false;
    let plain = {
        let rows = state.visible_rows();
        assert!(
            matches!(
                (&rows[0], &rows[1], &rows[2]),
                (Row::Brand, Row::Mode, Row::Divider)
            ),
            "通常表示にもヘッダ3行がある"
        );
        rows.len()
    };

    for enter in ["nav", "search", "triage"] {
        state.nav_mode = true;
        match enter {
            "search" => {
                state.handle_nav_key(key(BareKey::Char('/')));
            }
            "triage" => {
                state.handle_nav_key(key(BareKey::Char('p')));
            }
            _ => {}
        }
        let rows = state.visible_rows();
        assert!(
            matches!(
                (&rows[0], &rows[1], &rows[2]),
                (Row::Brand, Row::Mode, Row::Divider)
            ),
            "{} でヘッダ3行が崩れた",
            enter
        );
        if enter == "nav" {
            assert_eq!(rows.len(), plain, "navモードで行数が変わった");
        }
        state.search = None;
        state.triage = None;
    }
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

#[test]
fn the_tree_mode_row_counts_the_waiting_panes() {
    // 待ち件数に数えるのは注意を引く状態（done/blocked/error）だけ。
    // working は数えない（要件: sidebar-header.feature）
    let mut state = state_with_panes(4);
    set_agent_state(&mut state, 1, AgentState::Done);
    set_agent_state(&mut state, 2, AgentState::Blocked);
    set_agent_state(&mut state, 3, AgentState::Working);

    assert_eq!(state.waiting_count(), 2);
    assert_eq!(state.mode_line(32).content(), "  2 waiting");
}

#[test]
fn the_waiting_count_carries_the_triage_color() {
    // 数字だけトリアージモードの色を薄く乗せる（色 + dim の併用）
    let mut state = state_with_panes(2);
    set_agent_state(&mut state, 1, AgentState::Error);

    let mode = state.mode_line(SIDEBAR);
    assert_eq!(ink_at(&mode, 0), vec![2], "数字にレベル0の色");
    assert!(
        ink_at(&mode, DIM_LEVEL).contains(&2),
        "数字は dim も併用する"
    );
}

#[test]
fn a_two_digit_waiting_count_keeps_its_color_and_its_width() {
    // 件数が桁上がりしても、色は数字の位置に追従し、幅もモード行に収まる
    //（`  12 waiting` で12セル。サイドバー幅32の内容幅30に対して余裕がある）
    let mut state = state_with_panes(12);
    for pane_id in 1..=12 {
        set_agent_state(&mut state, pane_id, AgentState::Done);
    }

    let mode = state.mode_line(SIDEBAR);
    assert_eq!(mode.content(), "  12 waiting");
    assert!(
        unicode_width::UnicodeWidthStr::width(mode.content()) <= SIDEBAR - 2,
        "右マージンを食わない: {}",
        mode.content()
    );
    assert_eq!(ink_at(&mode, 0), vec![2, 3], "2桁とも色が乗る");
}

#[test]
fn the_mode_row_stays_empty_when_nothing_waits() {
    // 対応が不要であることを伝える文言は出さない（装飾のための装飾）
    let mut state = state_with_panes(2);
    set_agent_state(&mut state, 1, AgentState::Working);

    assert_eq!(state.waiting_count(), 0);
    assert_eq!(state.mode_line(32).content(), "");
}

#[test]
fn a_mode_replaces_the_waiting_count() {
    // モード行は1行しかないので、モード中は待ち件数を諦める
    let mut state = state_with_panes(2);
    set_agent_state(&mut state, 1, AgentState::Done);
    state.nav_mode = true;

    let mode = state.mode_line(32).content().to_string();
    assert!(!mode.contains("waiting"), "{}", mode);
    assert!(mode.contains("[nav]"), "{}", mode);
}

#[test]
fn the_nav_mode_row_shows_the_help_and_exit_hints() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;

    let mode = state.mode_line(32);
    let mode = mode.content();
    assert!(mode.starts_with("  [nav]"), "{}", mode);
    assert!(mode.contains("?:help"), "{}", mode);
    assert!(mode.contains("esc:exit"), "{}", mode);
    assert!(mode.chars().count() <= 32, "サイドバー幅に収まる");
}

#[test]
fn the_search_mode_row_keeps_the_help_hint_beside_the_query() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alp");

    let mode = state.mode_line(32);
    let mode = mode.content();
    assert!(mode.starts_with("  /alp"), "{}", mode);
    assert!(mode.contains("?:help"), "{}", mode);
    assert!(mode.chars().count() <= 32);
}

#[test]
fn a_long_query_wins_over_the_help_hint() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "0123456789");

    // 幅12には操作ヒントを置く余地が無い。入力中のクエリのほうを残す
    let mode = state.mode_line(12);
    let mode = mode.content();
    assert!(!mode.contains("?:help"), "{}", mode);
    assert!(mode.chars().count() <= 12);
}

#[test]
fn a_full_width_query_does_not_push_the_hint_off_the_edge() {
    // 右寄せの余白は表示セル幅で数える。文字数で数えると全角のクエリで
    // 操作ヒントが端からはみ出す（docs/concept/ui-design.md のレイアウト規則）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "日本語のペイン名");

    let mode = state.mode_line(32);
    assert!(
        unicode_width::UnicodeWidthStr::width(mode.content()) <= 32,
        "{}",
        mode.content()
    );
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
fn the_help_overlay_covers_the_whole_sidebar() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let rows = state.visible_rows();
    assert!(rows.len() > 1);
    assert!(
        rows.iter().all(|r| matches!(r, Row::Help(_))),
        "ヘルプ表示中はツリーを出さない"
    );
    // 行クリックの逆引きも当たらない（要件: click-to-focus と食い違わせない）
    assert!((0..rows.len()).all(|y| state.pane_at_row(y).is_none()));
}

#[test]
fn the_help_lines_fit_the_sidebar_width() {
    // 幅32（決定3）に収まらないと、キー列か説明のどちらかが … で消える
    let mut state = searchable_state();
    for row in state.help_lines() {
        let line = state.help_line(row, 32);
        assert!(
            !line.content().contains('…'),
            "navモード: {}",
            line.content()
        );
    }
    state.handle_nav_key(key(BareKey::Char('/')));
    for row in state.help_lines() {
        let line = state.help_line(row, 32);
        assert!(
            !line.content().contains('…'),
            "検索サブモード: {}",
            line.content()
        );
    }
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
    let title = state
        .help_lines()
        .first()
        .map(|row| state.help_line(row, 32));
    assert_eq!(
        title.as_ref().map(|t| t.content()),
        Some("  [search] keys"),
        "検索サブモードのキーを出す"
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

// --- ペイン行のレイアウト（決定21・決定22） ---
//
// カウンタ列は右端に揃え、幅はフレーム全体で共有する。ペイン名はその残り幅に
// 収めるので、名前が長くてもサブエージェント数 `+N`・未完了タスク数 `[M]` は
// 消えない。cwd はペイン行に混ぜず、続く cwd行に出す

const SIDEBAR: usize = 32; // 既定のサイドバー幅（決定3）
                           // ヘッダが常時占める行数（ブランド行・モード行・境界線）。ツリーの行番号は
                           // すべてこの下から数える（要件: sidebar-header.feature）
const HEADER_ROWS: usize = 3;
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
fn counters_are_flush_with_the_right_edge() {
    let mut state = state_with_one_pane("要件定義とドキュメント整理タスクの続き");
    repeat_status(&mut state, 1, "SubagentStart", 2);
    repeat_status(&mut state, 1, "TaskCreated", 3);

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
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
            Row::Pane { entry, hit, .. } => state.pane_row(entry, false, *hit, column, SIDEBAR),
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
    let selected = state.pane_row(&state.selectable[0], true, None, column, SIDEBAR);
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(selected.content()),
        SIDEBAR
    );
}

#[test]
fn the_counter_column_is_shared_by_every_row() {
    // サブエージェント数だけのペインと、未完了タスク数だけのペイン。
    // 桁がずれると一覧を縦に舐められないので、列はフレーム全体で共有する
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(1, "alpha"), terminal_pane(2, "bravo")],
    )]));
    state.rebuild_selectable();
    repeat_status(&mut state, 1, "SubagentStart", 12);
    repeat_status(&mut state, 2, "TaskCreated", 3);

    let column = column_of(&state);
    let first = state.pane_row(&state.selectable[0], false, None, column, SIDEBAR);
    let second = state.pane_row(&state.selectable[1], false, None, column, SIDEBAR);

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
fn the_counter_column_costs_nothing_when_nobody_has_counters() {
    // 静かなフレームでは列を予約しない。予約するとペイン名の幅がその場で失われる
    let state = state_with_one_pane("abcdefghijklmnopqrstuvwxyz0123456789");
    assert_eq!(column_of(&state), CounterColumn::default());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        SIDEBAR,
    );
    let content = text.content();
    assert!(content.starts_with("    abcdefghij"), "{}", content);
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
        HEADER_ROWS + 2,
        "ヘッダ・タブ見出し行・ペイン行だけ"
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
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "bravo");

    let rows = state.visible_rows();
    let Some(Row::Cwd { hit, .. }) = rows.iter().find(|r| matches!(r, Row::Cwd { .. })) else {
        panic!("cwd行が無い");
    };
    assert!(hit.is_none());
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

#[test]
fn the_cwd_row_survives_a_sidebar_narrower_than_its_indent() {
    // 字下げより狭い幅でも算術が破綻しない
    let text = cwd_row("/work/fujin", false, None, 3);
    assert!(unicode_width::UnicodeWidthStr::width(text.content()) <= 6);
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
    let (folded, dropped) = truncate_start("/aa/bb/cc", 5);
    assert_eq!(folded, "…b/cc");
    assert_eq!(dropped, 5);
    assert_eq!(folded.chars().count(), 5);
    // 全角は2セルぶん食う
    let (folded, dropped) = truncate_start("/あ/いう", 5);
    assert_eq!(folded, "…いう");
    assert_eq!(dropped, 3);
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

    // 0件（「一致なし」の行）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "zzz");
    state.render(40, 20);
    state.render(1, 1);
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
    assert_eq!(state.screen_rows(40).len(), HEADER_ROWS + 13);
}

#[test]
fn the_selection_never_leaves_the_screen() {
    // 「見えない行へ選択だけが進む」のが元の不具合。上下どちらへ動かしても
    // 選択行が画面に残ることを、全行ぶん確かめる
    const ROWS: usize = 8;
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
    const ROWS: usize = 8;
    let mut state = overflowing_state();
    state.show_cwd = true;
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
fn the_header_stays_pinned_while_the_list_scrolls() {
    const ROWS: usize = 8;
    let mut state = overflowing_state();
    state.selected = 11;
    state.render(ROWS, 32);

    let screen = state.screen_rows(ROWS);
    assert!(
        matches!(
            (&screen[0], &screen[1], &screen[2]),
            (Row::Brand, Row::Mode, Row::Divider)
        ),
        "ヘッダ3行は流さず固定する"
    );
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
    const ROWS: usize = 8;
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
        markers[0].0 + markers[1].0 + (ROWS - HEADER_ROWS - 2),
        13,
        "隠れている行数と出ている行数の合計が一覧の行数になる"
    );
}

#[test]
fn clicking_follows_the_scrolled_layout() {
    // 描画とクリックの逆引きが同じ切り出しを見ていないと行がずれる
    const ROWS: usize = 8;
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
    const ROWS: usize = 8;
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
        AgentState::Idle => state.apply_status(status(pane_id, "SessionStart")),
        AgentState::Working => state.apply_status(status(pane_id, "UserPromptSubmit")),
        AgentState::Blocked => state.apply_status(status(pane_id, "Notification")),
        AgentState::Done => {
            state.apply_status(status(pane_id, "UserPromptSubmit"));
            state.apply_status(status(pane_id, "Stop"));
        }
        AgentState::Error => state.apply_status(status(pane_id, "StopFailure")),
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
    assert!(
        matches!(rows.get(HEADER_ROWS), Some(Row::Notice(_))),
        "空リストのままだと壊れて見える"
    );
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
    let text = state.triage_row(entry, tab_name, false, tab_column, SIDEBAR);
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
        .triage_row(entry, tab_name, false, tab_column, SIDEBAR)
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
fn the_triage_mode_row_replaces_the_mode_name() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('p')));
    let mode = state.mode_line(SIDEBAR).content().to_string();
    assert!(mode.starts_with("  [tri]"), "{}", mode);
    // Esc の行き先はツリー表示であってnavモードの退場ではない
    assert!(mode.contains("esc:back"), "{}", mode);
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
    assert!(lines[0].contains("[tri]"), "{:?}", lines);
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

    // 対象なしの行も通す
    let mut state = triage_state();
    state.handle_nav_key(key(BareKey::Char('p')));
    state.render(40, 20);
    state.render(1, 1);
}
