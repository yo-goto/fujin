// navモード（要件: .docs/requirements/req-nav-mode.md）: 入退場とキー操作、操作ヒントとヘルプオーバーレイ

use crate::agent::AgentState;
use crate::host::{take_host_calls, HostCall};
use crate::render::divider_line;
use crate::test_support::*;
use crate::*;

// --- navモードのキー操作（決定202607310311） ---

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
fn nav_digits_no_longer_jump_directly() {
    // かつての 1-9 直行ジャンプは番号ジャンプサブモードへ一本化した（決定202608070342）。
    // navモード最上位の数字は未定義キー＝安全弁で退場する
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('2')));
    assert!(!state.nav_mode);
    assert_eq!(state.selected, 0, "選択は動かさない");
}

#[test]
fn nav_enter_leaves_the_mode() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(KeyWithModifier::new(BareKey::Enter));
    assert!(!state.nav_mode);
}

// --- ジャンプで発行されるホストコマンド（要件: nav-mode-selection-jump.feature） ---
//
// host.rs の間接層で発行を記録して検証する。従来はリンクスタブが握り潰すため
// 「呼ばれたことも引数も観測できない」領域だった
// （.docs/issues/issue-cucumber-test-automation.md の再検討条件1）。
// フローティング層の出し入れは focus_pane_with_id の第2引数
// should_float_if_hidden が決める（focus_selected のコメント参照）

#[test]
fn nav_enter_focuses_the_selected_tiled_pane_and_hides_the_floating_layer() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('j')));
    take_host_calls();

    state.handle_nav_key(key(BareKey::Enter));
    assert_eq!(
        take_host_calls(),
        vec![
            // フォーカス移動でタブが変わりうるので、横取り解除が先（handle_nav_key の順序）
            HostCall::ClearKeyPressesIntercepts,
            HostCall::FocusPaneWithId {
                pane_id: PaneId::Terminal(2),
                // false = タイルペインへのジャンプではフローティング層を出さない（隠れる）
                should_float_if_hidden: false,
                should_be_in_place_if_hidden: false,
            },
        ]
    );
}

#[test]
fn nav_enter_focuses_the_selected_floating_pane_and_shows_the_floating_layer() {
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(
        0,
        vec![
            terminal_pane(1, "alpha"),
            PaneInfo {
                is_floating: true,
                ..terminal_pane(2, "bravo")
            },
        ],
    )]));
    state.rebuild_selectable();
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('j')));
    take_host_calls();

    state.handle_nav_key(key(BareKey::Enter));
    assert_eq!(
        take_host_calls(),
        vec![
            HostCall::ClearKeyPressesIntercepts,
            HostCall::FocusPaneWithId {
                pane_id: PaneId::Terminal(2),
                // true = 隠れていたフローティング層ごと表示してフォーカスする
                should_float_if_hidden: true,
                should_be_in_place_if_hidden: false,
            },
        ]
    );
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

// --- 操作ヒントとヘルプオーバーレイ（要件: .docs/requirements/req-nav-mode.md） ---
//
// サイドバー幅は32文字（決定202607302256）。ヘッダもヘルプもこの幅を前提に文言を決めてある

#[test]
fn the_header_keeps_the_brand_in_every_mode() {
    // `▲ fujin` はモードによらず常に出る。変わるのは後ろに付くモードラベル
    //（要件: sidebar-header.feature）
    let mut state = searchable_state();
    state.nav_mode = false;
    assert_eq!(state.header_line(32).content(), "▲ fujin");

    state.nav_mode = true;
    assert_eq!(state.header_line(32).content(), "▲ fujin  [nav]");
    state.handle_nav_key(key(BareKey::Char('/')));
    assert_eq!(
        state.header_line(32).content(),
        "▲ fujin  [nav]",
        "検索サブモードは navモードの内側なのでラベルを変えない"
    );
}

#[test]
fn the_header_labels_the_triage_mode() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('t')));

    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin  [tri]");
}

#[test]
fn the_header_never_shows_the_waiting_count() {
    // 待ち件数のヘッダ表示は決定202608070119で廃止した。集計そのものは残っている
    //（決定202608072218でコマンド状態も含む形に広げた）
    let mut state = state_with_panes(4);
    set_agent_state(&mut state, 1, AgentState::Done);
    set_agent_state(&mut state, 2, AgentState::Blocked);
    set_agent_state(&mut state, 3, AgentState::Working);

    assert_eq!(state.waiting_count(), 2, "集計は残す");
    let header = state.header_line(SIDEBAR).content().to_string();
    assert!(!header.contains('2'), "件数は出さない: {}", header);
    assert!(!header.contains("waiting"), "{}", header);
}

#[test]
fn the_header_triangle_carries_the_mode_color() {
    // モードラベルを読まなくても三角の色だけでモードが判別できるようにする
    //（.docs/concept/ui-design.md の「ヘッダ」）
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);

    // ツリー表示は dim のみ。色は乗らない
    state.nav_mode = false;
    let tree = state.header_line(SIDEBAR);
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
    assert!(ink_at(&state.header_line(SIDEBAR), 2).contains(&0));
    state.handle_nav_key(key(BareKey::Char('t')));
    assert!(ink_at(&state.header_line(SIDEBAR), 0).contains(&0));
}

#[test]
fn the_mode_label_shares_the_triangle_color() {
    // ラベルは三角の裏取りなので同じ状態色で出す（決定202608070119）。ブランド名だけは
    // dim のまま — 名前まで色を付けるとツリーの状態アイコンの色分けと喧嘩する
    let mut state = state_with_panes(2);
    state.nav_mode = true;

    let header = state.header_line(SIDEBAR);
    // "▲ fujin  [nav]": 0=三角 / 1-6=" fujin" / 7-8=空白 / 9-13="[nav]"
    assert_eq!(ink_at(&header, DIM_LEVEL), (1..=6).collect::<Vec<_>>());
    assert_eq!(
        ink_at(&header, 2),
        vec![0, 9, 10, 11, 12, 13],
        "三角とラベルに状態色"
    );
}

#[test]
fn the_frame_keeps_its_rows_across_every_mode_boundary() {
    // モードの入退場で枠の行数が変わると、ツリー全体がそのぶん上下にずれる
    //（要件: sidebar-header.feature / sidebar-footer.feature）
    let mut state = searchable_state();
    set_agent_state(&mut state, 1, AgentState::Done);
    state.nav_mode = false;
    let plain = {
        let rows = state.visible_rows();
        assert_frame(&rows, "通常表示");
        rows.len()
    };

    for enter in ["nav", "search", "triage", "jump", "help"] {
        state.nav_mode = true;
        match enter {
            "search" => {
                state.handle_nav_key(key(BareKey::Char('/')));
            }
            "triage" => {
                state.handle_nav_key(key(BareKey::Char('t')));
            }
            "jump" => {
                state.handle_nav_key(key(BareKey::Char('n')));
            }
            "help" => {
                state.handle_nav_key(key(BareKey::Char('?')));
            }
            _ => {}
        }
        assert_frame(&state.visible_rows(), enter);
        if enter == "nav" {
            assert_eq!(
                state.visible_rows().len(),
                plain,
                "navモードで行数が変わった"
            );
        }
        state.search = None;
        state.triage = None;
        state.jump = None;
        state.help_overlay = false;
    }
}

#[test]
fn the_divider_leaves_the_right_margin() {
    // 境界線も他の行と同じく右端2セルを空ける（決定202607302256の右マージン）。
    // 選択行の背景もここへ揃えてあるので、罫線の直下に選択行が来ても右端が揃う
    //（.docs/issues/issue-header-divider-selection-margin-mismatch.md）
    let divider = divider_line(32);
    let line = divider.content();
    assert_eq!(line.chars().count(), 30);
    assert!(line.chars().all(|c| c == '─'), "{}", line);
    assert_eq!(ink_at(&divider, DIM_LEVEL).len(), 30, "dim で引く");
}
