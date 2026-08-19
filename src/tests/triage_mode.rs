use crate::agent::AgentState;
use crate::render::Row;
use crate::test_support::*;
use crate::*;

// --- トリアージモード（要件: docs/requirements/triage-mode/） ---
//
// navモードの内側で `p` から入る、エージェント状態の緊急度順のフラット一覧。
// ツリー表示の並び順（決定202607302256）には手を触れず、切り替えて使う

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
    state.handle_nav_key(key(BareKey::Char('t')));
    assert_eq!(triage_ids(&state), vec![2, 1]);

    // 表示中に別のペインが blocked になったら、次の描画で上に来る
    set_agent_state(&mut state, 1, AgentState::Blocked);
    assert_eq!(triage_ids(&state), vec![1, 2]);
}

#[test]
fn t_switches_the_sidebar_to_the_triage_list() {
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Blocked);
    state.handle_nav_key(key(BareKey::Char('t')));

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
    state.handle_nav_key(key(BareKey::Char('t')));
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
    state.handle_nav_key(key(BareKey::Char('t')));
    // カーソルは一覧の先頭（error のタブ1のペイン）
    state.handle_nav_key(key(BareKey::Enter));

    assert_eq!(state.selectable[state.selected].pane_id, 4);
    assert!(!state.nav_mode, "ジャンプはnavモードの退場を伴う");
    assert!(state.triage.is_none());
}

#[test]
fn a_triage_jump_clears_the_read_state_through_the_usual_path() {
    // 既読クリアは「PaneUpdate でのフォーカス変化を見る」汎用の仕組みに乗せる。
    // トリアージモード専用のクリア処理を別に書くと、決定202608012141が踏んだ配り漏れの
    // 罠を再発明することになる（要件: triage-mode-entry-exit.feature）
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Blocked);
    state.handle_nav_key(key(BareKey::Char('t')));
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
    state.handle_nav_key(key(BareKey::Char('t')));
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
    state.handle_nav_key(key(BareKey::Char('t')));
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
    state.handle_nav_key(key(BareKey::Char('t')));

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
    state.handle_nav_key(key(BareKey::Char('t')));
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
    // 安全弁（決定202607310311）はサブモードでも最上位まで効かせる
    for k in [
        key(BareKey::Char('z')),
        key(BareKey::Char('q')),
        key(BareKey::Char('j')).with_ctrl_modifier(),
        // alt+p（プレビュー）は検索サブモード限定の例外で、トリアージ一覧では効かない
        key(BareKey::Char('p')).with_alt_modifier(),
    ] {
        let mut state = triage_state();
        set_agent_state(&mut state, 1, AgentState::Working);
        state.handle_nav_key(key(BareKey::Char('t')));
        state.handle_nav_key(k.clone());
        assert!(!state.nav_mode, "{:?} でnavモードごと抜けるべき", k);
        assert!(state.triage.is_none());
    }
}

#[test]
fn an_empty_triage_list_says_so() {
    let mut state = triage_state();
    state.handle_nav_key(key(BareKey::Char('t')));
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
    state.handle_nav_key(key(BareKey::Char('t')));

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
    state.handle_nav_key(key(BareKey::Char('t')));

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
    state.handle_nav_key(key(BareKey::Char('t')));

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
    state.handle_nav_key(key(BareKey::Char('t')));
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
    state.handle_nav_key(key(BareKey::Char('t')));
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
    state.handle_nav_key(key(BareKey::Char('t')));
    state.render(40, 20);
    state.render(3, 2);
    state.render(2, 1);
    state.render(0, 0);

    // 対象なしの通知行（`nothing to triage`）も通す
    let mut state = triage_state();
    state.handle_nav_key(key(BareKey::Char('t')));
    state.render(40, 20);
    state.render(1, 1);
}
