use crate::render::CounterColumn;
use crate::render::HeadCells;
use crate::test_support::*;
use crate::*;

// --- 番号ジャンプサブモード（決定202608070342、要件: .docs/requirements/req-pane-number-jump.md） ---

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
    // 「1 を打ったが 10 があるので確定できない」という行き止まりが起きない（決定202608070342）
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
    // 落とし、残っている候補だけがキーの色（レベル2）で目に入る（決定202608070342）
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
    assert!(content.starts_with("  n 1"), "{}", content);
    assert!(content.ends_with("?:help"), "{}", content);
}
