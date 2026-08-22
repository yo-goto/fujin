use crate::agent::AgentState;
use crate::termination::Termination;
use crate::test_support::*;
use crate::*;

// --- 終了操作サブモード（決定202608080140、要件: .docs/requirements/req-pane-close-kill.md） ---
//
// 実行そのもの（send_sigkill_to_pane_id / close_pane_with_id）は副作用だけの
// ホスト関数で結果を観測できないので、その手前で畳んだ `termination_plan()`
//（対象ペインと効果の内訳）を検証対象にする

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
    // 対象種別で出し分けはしない（決定202608080140）。対象プロセスが実質存在しない
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
    // 新しい色は増やさず、状態アイコン `error` と同じ error_color を借りる（決定202608080140）
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
    // ヘッダーの三角も同じ状態色（決定202608070119）。モードラベルは navモードのまま
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
