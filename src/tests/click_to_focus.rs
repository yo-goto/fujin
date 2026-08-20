use crate::test_support::*;
use crate::*;

// --- 行クリック（要件: docs/requirements/req-click-to-focus.md） ---
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
