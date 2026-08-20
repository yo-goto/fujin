use crate::agent::AgentState;
use crate::render::HeadCells;
use crate::render::Row;
use crate::render::NO_AGENT_ICON;
use crate::test_support::*;
use crate::*;

// --- フローティングペインの区別表示（要件: docs/requirements/req-floating-pane-indicator.md） ---
//
// フローティングペインはフローティング層ごと隠れうるので、一覧の上で見分けられる
// ようにペイン名を丸括弧で囲む。色・dim は使わない（決定202608080027）

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
    // パス形式は先頭省略（決定202608060053）。省略記号は括弧の内側に入る
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
    // 括弧はカウンタ列と同じく先に確保する（決定202608060052の考え方）。名前が長くても
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
    // ペイン名の位置に出ているのが cwd でも、囲むものには変わりない（決定202608070102）
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
    state.handle_nav_key(key(BareKey::Char('t')));

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
