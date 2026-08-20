// 複数選択（マーク。要件: docs/requirements/req-pane-termination-multi-select.md）: マークの付け外し・
// 終了操作との連携・インスタンス間の同期

use crate::agent::AgentState;
use crate::render::CounterColumn;
use crate::render::HeadCells;
use crate::termination::Termination;
use crate::test_support::*;
use crate::*;

// --- 複数選択（マーク。決定202608080250、要件:
// docs/requirements/req-pane-termination-multi-select.md） ---
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
    // マークはタブをまたいでよい（決定202608080250。selectable が元々タブ横断のため）
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
    state.handle_nav_key(key(BareKey::Char('t')));
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
    // 列順序は 選択バー → 番号列 → マーク列 → アイコン → ペイン名（決定202608080250）。
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
    state.handle_nav_key(key(BareKey::Char('t')));
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

// --- マークと終了操作の連携（決定202608080250） ---

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
    // 単一版のフローはマーク0件の特殊ケースとして包含する（決定202608080250）
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
    // 成功・no-op を問わずクリアする（決定202608080250）
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
    // キーだけの2段目に落ちるが、件数は残す（決定202608080250）
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

// --- マークの同期（決定202608080250・決定202608012141） ---

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
