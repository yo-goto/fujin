// フォーカス同期（要件: .docs/requirements/req-focus-sync.md）と、そこから派生したフォーカスの預かり（決定202608072359）
use crate::agent::AgentState;

use crate::host::{take_host_calls, HostCall};
use crate::test_support::*;
use crate::*;

// --- フォーカス同期（要件: .docs/requirements/req-focus-sync.md） ---
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
    // 戻ると「フォーカスは動いていない」と誤判定し、その間に決定202608012141の同期で
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
    state.interrupt_nav_mode();

    assert!(!state.nav_mode);
    assert_eq!(
        state.selectable[state.selected].pane_id, 3,
        "ハイライトは新しい実フォーカスへ揃う"
    );
}

#[test]
fn nav_entry_after_an_interrupted_exit_starts_from_the_new_focus() {
    // 強制退場は「探索をやめて作業に戻った」合図なので、次の入場は Esc 退場と違って
    // 探索位置を復元しない（.docs/issues/issue-nav-reentry-restores-stale-selection.md）。
    // 退場の記録は exit_nav_mode() が控えるが、そのとき focus_at_nav_exit だけが
    // 新しいフォーカスになり selection_at_nav_exit は古い探索位置のままだったため、
    // 入り直すと「Escで抜けたままフォーカスに触れていない」と誤読されていた
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j'))); // pane2 まで探索して
    assert_eq!(state.selectable[state.selected].pane_id, 2);

    // マウスで pane3 をクリック → refresh_focus() が強制退場させる
    state.focused_pane = Some(3);
    state.interrupt_nav_mode();
    assert_eq!(state.selectable[state.selected].pane_id, 3);

    // 直後に入り直す。フォーカスは pane3 のまま動かしていない
    state.enter_nav_mode();
    assert_eq!(
        state.selectable[state.selected].pane_id, 3,
        "強制退場のあとはクリック先から始める（古い探索位置を復元しない）"
    );
}

#[test]
fn nav_entry_follows_focus_moved_after_an_interrupted_exit() {
    // 強制退場のあと、さらに通常のzellij操作でフォーカスが動いた組み合わせ。
    // 控えの focus_at_nav_exit（強制退場時の新フォーカス）と一致しないので、
    // Esc退場と同じくフォーカスの不一致側の分岐で現在のフォーカスから始まる
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    state.focused_pane = Some(3);
    state.interrupt_nav_mode();

    state.focused_pane = Some(2);
    state.enter_nav_mode();
    assert_eq!(
        state.selectable[state.selected].pane_id, 2,
        "強制退場のあとフォーカスがさらに動いたら現在のフォーカスから始める"
    );
}

#[test]
fn an_esc_exit_still_restores_the_exploring_position_after_an_interrupted_one() {
    // 強制退場で探索位置を捨てても、その次の Esc 退場では復元される
    //（interrupt_nav_mode() が控えるのを止めるのは自分の退場ぶんだけ）
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    state.focused_pane = Some(3);
    state.interrupt_nav_mode();

    // pane3 から入り直して pane1 まで探索し、今度は Esc で抜ける
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('k')));
    state.handle_nav_key(key(BareKey::Char('k')));
    assert_eq!(state.selectable[state.selected].pane_id, 1);
    state.handle_nav_key(key(BareKey::Esc));
    assert_eq!(state.selectable[state.selected].pane_id, 3);

    state.enter_nav_mode();
    assert_eq!(
        state.selectable[state.selected].pane_id, 1,
        "Esc退場のあとフォーカスを動かしていなければ探索位置を復元する"
    );
}

// --- ジャンプ退場のあとジャンプ元へ戻ってからの入場
//     （.docs/issues/issue-nav-entry-restores-stale-jump-target.md） ---
//
// ジャンプ経路は退場を控えたあとで実フォーカスを動かすので、控えを直さないと
// `focus_at_nav_exit` がジャンプ前のフォーカスのままになる。ジャンプ元へ戻ると
// それが現在のフォーカスと偶然一致し、「退場後フォーカスを動かしていない」と
// 誤判定してジャンプ先を復元してしまう。4つのジャンプ経路すべてを踏む

#[test]
fn nav_entry_after_a_jump_starts_from_the_pane_the_jump_came_from() {
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(key(BareKey::Enter)); // pane2 へジャンプして退場
    state.focused_pane = Some(2);

    // pane2 で作業したあと、通常のzellij操作でジャンプ元の pane1 へ戻る
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    assert_eq!(
        state.selectable[state.selected].pane_id, 1,
        "ジャンプ元へ戻ったら現在のフォーカスから始める"
    );
}

#[test]
fn nav_entry_after_a_number_jump_starts_from_the_pane_the_jump_came_from() {
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('n')));
    state.handle_nav_key(key(BareKey::Char('2'))); // 番号ジャンプで確定
    assert!(!state.nav_mode);
    state.focused_pane = Some(2);

    state.focused_pane = Some(1);
    state.enter_nav_mode();
    assert_eq!(state.selectable[state.selected].pane_id, 1);
}

#[test]
fn nav_entry_after_a_search_jump_starts_from_the_pane_the_jump_came_from() {
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "pane2");
    state.handle_nav_key(key(BareKey::Enter)); // 検索確定でジャンプ
    assert!(!state.nav_mode);
    state.focused_pane = Some(2);

    state.focused_pane = Some(1);
    state.enter_nav_mode();
    assert_eq!(state.selectable[state.selected].pane_id, 1);
}

#[test]
fn nav_entry_after_a_triage_jump_starts_from_the_pane_the_jump_came_from() {
    let mut state = triage_state();
    state.nav_mode = false;
    set_agent_state(&mut state, 4, AgentState::Error);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('t')));
    state.handle_nav_key(key(BareKey::Enter)); // トリアージ確定でジャンプ
    assert_eq!(state.selectable[state.selected].pane_id, 4);
    state.focused_pane = Some(4);

    state.focused_pane = Some(1);
    state.enter_nav_mode();
    assert_eq!(state.selectable[state.selected].pane_id, 1);
}

#[test]
fn nav_entry_right_after_a_jump_still_starts_from_the_jump_target() {
    // ジャンプ先に留まったまま入り直した場合は、これまでどおりジャンプ先から。
    // 控えを直しても「一致する」側の分岐が同じ行を指す
    let mut state = state_with_panes(3);
    state.focused_pane = Some(1);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(key(BareKey::Enter));
    state.focused_pane = Some(2);

    state.enter_nav_mode();
    assert_eq!(state.selectable[state.selected].pane_id, 2);
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
    // 権威判定（決定202608012142）の材料。フォーカス中のタブに自分が居るかだけを見る
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

// --- フォーカスの預かり（決定202608072359） ---
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
    // 召喚インスタンスは最初から自分がフォーカスを持っている（決定202608011644）
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
    state.interrupt_nav_mode();

    assert!(!state.nav_mode);
    assert_eq!(parked_pane(&state), None);
    assert_eq!(
        state.selectable[state.selected].pane_id, 3,
        "ハイライトは新しい実フォーカスへ揃う"
    );
}

// --- 権威を失ったことによる退場（.docs/issues/issue-nav-mode-survives-cross-tab-click.md） ---
//
// フォーカスが**自分の居ないタブへ**移った場合。`refresh_focus()` は権威判定
//（決定202608012142）で早期に返るので、退場だけを切り出した `abandon_nav_mode()` を
// 直接呼んで先のロジックを見る（問い合わせ自体はここでは呼べない）。

#[test]
fn abandoning_nav_mode_clears_the_interception_without_taking_the_focus_back() {
    // 横取りは登録した自分にタブの可視性と関係なく届き続けるので解除は自分でやるが、
    // 預かりを「返して」しまうとユーザーが選んだタブからフォーカスを奪い返す
    let mut state = state_ready_to_park(3);
    state.enter_nav_mode();
    assert_eq!(parked_pane(&state), Some(1));
    take_host_calls();

    state.abandon_nav_mode();

    assert!(!state.nav_mode);
    assert_eq!(parked_pane(&state), None);
    assert_eq!(
        take_host_calls(),
        vec![
            // 預かりの後始末（決定202607302258へ戻す）だけで、返却の
            // FocusPaneWithId は出さない
            HostCall::SetSelectable { selectable: false },
            HostCall::ClearKeyPressesIntercepts,
        ],
        "元タブの作業ペインへフォーカスを返さない"
    );
}

#[test]
fn abandoning_nav_mode_leaves_the_highlight_where_the_exploring_stopped() {
    // 選択を実フォーカスへ戻さない。`focused_pane` は元タブのペインを指したままなので、
    // 戻すと古い行を指すうえ、その古い行を兄弟へ配ってしまう。
    // ハイライトは移動先のタブの権威が配ってくるのを待つ
    let mut state = state_ready_to_park(3);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.selectable[state.selected].pane_id, 2);

    state.abandon_nav_mode();

    assert_eq!(state.selectable[state.selected].pane_id, 2);
}

#[test]
fn nav_entry_after_abandoning_does_not_restore_the_exploring_position() {
    // 別タブへ移った＝作業場所を変えた合図なので、強制退場（interrupt_nav_mode）と
    // 同じく探索位置は持ち越さない
    let mut state = state_ready_to_park(3);
    state.enter_nav_mode();
    state.handle_nav_key(key(BareKey::Char('j'))); // pane2 まで探索して
    state.abandon_nav_mode();

    // フォーカスが自分のタブの pane1 へ戻ってきた（＝退場時の控えと同じペイン）
    state.enter_nav_mode();
    assert_eq!(
        state.selectable[state.selected].pane_id, 1,
        "探索位置ではなくフォーカス中のペインから始める"
    );
}

#[test]
fn a_summoned_instance_closes_itself_when_the_focus_leaves_its_tab() {
    // 召喚インスタンス（決定202608011644）も同じ経路を通る。預かりは持っていないので
    // 手放しは空振りし、横取り解除のあとに自死する順序（exit_nav_mode）は崩れない
    let mut state = state_ready_to_park(3);
    state.summoned = true;
    state.enter_nav_mode();
    assert_eq!(parked_pane(&state), None, "召喚インスタンスは預からない");
    take_host_calls();

    state.abandon_nav_mode();

    assert!(!state.nav_mode);
    assert_eq!(
        take_host_calls(),
        vec![
            HostCall::ClearKeyPressesIntercepts,
            HostCall::ClosePluginPane { plugin_pane_id: 9 },
        ]
    );
}
