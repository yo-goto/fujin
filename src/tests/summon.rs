// 臨時召喚（要件: docs/requirements/summon/）

use crate::test_support::*;
use crate::*;

// --- 臨時召喚（決定202608011644） ---
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
