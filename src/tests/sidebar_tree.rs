// サイドバーのツリー表示（要件: docs/requirements/req-sidebar-tree.md）: 選択可能行の組み立て・
// フッター・描画パスの頑健性・縦スクロール。ペイン行そのものの描画は pane_row.rs 側

use crate::agent::AgentState;
use crate::render::overflow_row;
use crate::render::reconcile_scroll;
use crate::render::Row;
use crate::test_support::*;
use crate::*;

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

// --- フッター（決定202608070119。要件: sidebar-footer.feature） ---

#[test]
fn the_footer_shows_the_configured_direct_keys() {
    // 決め打ちのキー表記はユーザーの設定と食い違いうるので、実際に割り当てた
    // キーの表記を configuration から受け取って出す（決定202608070226）
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("up_key", "f1"),
        ("down_key", "f2"),
        ("go_key", "f3"),
    ]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  f1:up  f2:down  f3:jump");
}

#[test]
fn the_footer_shows_the_toggle_cwd_key_hint() {
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[("toggle_cwd_key", "alt+c")]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  alt+c:cwd");
}

#[test]
fn the_zellij_spelling_of_a_key_is_accepted_as_is() {
    // ユーザーは config.kdl の `bind "Alt u"` からコピーしてくる。そのまま
    // 貼れないと使いにくいので、空白区切りの zellij 表記も受けて画面表記へ均す
    let mut state = state_with_panes(2);
    for (written, shown) in [
        ("Alt u", "  alt+u:up"),
        ("alt+u", "  alt+u:up"),
        ("Ctrl Shift g", "  ctrl+shift+g:up"),
        ("PageUp", "  pgup:up"),
        ("Enter", "  enter:up"),
    ] {
        state.apply_config(&plugin_config(&[("up_key", written)]));
        assert_eq!(state.footer_line(SIDEBAR).content(), shown, "{}", written);
    }
}

#[test]
fn an_unreadable_key_setting_is_shown_as_written() {
    // 黙って落とすとヒントが1つ消えるだけになり、設定を間違えたことに
    // 気づけない。読めない値はそのまま出して気づかせる
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[("up_key", "Meta q")]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  Meta q:up");
}

#[test]
fn direct_key_hints_are_dropped_whole_rather_than_truncated() {
    // 幅28に3項目が収まらないとき、末尾を `…` で切ると `キー:動作` の形が壊れる。
    // 矢印へ落としても足りなければ、項目ごと省いて残りを正しく読ませる。
    // READMEが例示している Alt Up / Alt Down / Alt g がちょうどこれに当たる
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  alt+up:up  alt+down:down");
    assert!(!footer.contains('…'), "{}", footer);
}

#[test]
fn an_unset_direct_key_drops_only_its_own_hint() {
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("up_key", "alt+up"),
        ("down_key", "alt+down"),
    ]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert!(!footer.contains("jump"), "設定の無い項目は省く: {}", footer);
    assert!(footer.contains("alt+up:up"), "{}", footer);
    assert!(footer.contains("alt+down:down"), "{}", footer);
}

#[test]
fn an_empty_direct_key_setting_is_treated_as_unset() {
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[("up_key", "  "), ("down_key", "alt+d")]));

    assert_eq!(state.footer_line(SIDEBAR).content(), "  alt+d:down");
}

#[test]
fn long_direct_keys_fall_back_to_arrows() {
    // 実キーの長さはユーザー依存。英字表記が28セルに収まらないときだけ、
    // up/down を矢印へ落とす（jump に対応する矢印記号は無いので残す）。
    // `pgup:up  pgdn:down  alt+g:jump` は30セルで28に収まらないが、
    // 矢印にすれば26セルで3項目とも残る
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("up_key", "PageUp"),
        ("down_key", "PageDown"),
        ("go_key", "alt+g"),
    ]));

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  pgup:↑  pgdn:↓  alt+g:jump");
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

    // 0件（`no matches` の通知行）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "zzz");
    state.render(40, 20);
    state.render(1, 1);
}

#[test]
fn render_survives_the_preview_pane() {
    // プレビュー用フローティングペイン（決定202608082045）は枠を持たない別の描画パス
    let mut state = State {
        is_preview: true,
        permissions_granted: true,
        ..Default::default()
    };
    // まだ何も配られていない（見出しだけ）
    state.render(40, 20);
    state.apply_preview_snapshot("alpha\n$ cargo test\nrunning 3 tests");
    state.render(40, 20);
    // 見出しすら置けない高さ・幅でも算術が破綻しないこと
    state.render(2, 1);
    state.render(1, 1);
    state.render(0, 0);
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

#[test]
fn scrolling_keeps_everything_in_place_when_it_all_fits() {
    let mut state = overflowing_state();
    state.selected = 11;
    state.render(40, 32);

    assert_eq!(state.scroll, 0, "全部載るならスクロールしない");
    assert!(overflow_markers(&state, 40).is_empty());
    let screen = state.screen_rows(40);
    assert_eq!(screen.len(), 40, "画面高ぶんを返す（余りは空行）");
    // 空行は埋め草と最下部の余白なので、中身の行数からは外して数える
    assert_eq!(
        screen.iter().filter(|r| !matches!(r, Row::Blank)).count(),
        HEADER_ROWS + 13 + (FOOTER_ROWS - 1),
        "中身の行数は変わらない"
    );
}

#[test]
fn the_selection_never_leaves_the_screen() {
    // 「見えない行へ選択だけが進む」のが元の不具合。上下どちらへ動かしても
    // 選択行が画面に残ることを、全行ぶん確かめる
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
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
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
    let mut state = overflowing_state();
    state.show_cwd = true;
    set_agent_state(&mut state, 5, AgentState::Idle);
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
fn the_footer_sits_at_the_bottom_edge_even_when_the_tree_is_short() {
    // ツリーが短いと、下の枠がツリーの直後へ浮いてしまう（実機で確認された
    // 見た目の不具合）。余った高さは空行で埋めて最下部まで押し下げる
    let mut state = state_with_panes(2);
    state.render(20, SIDEBAR);

    let screen = state.screen_rows(20);
    assert_eq!(screen.len(), 20);
    assert_frame(&screen, "ツリーが短いとき");
    assert!(
        matches!(screen[HEADER_ROWS + 3], Row::Blank),
        "ツリーの後ろは空行で埋める"
    );
    // 空行はクリックの対象にならない（要件: click-to-focus と食い違わせない）
    assert_eq!(state.pane_at_row(HEADER_ROWS + 3), None);
    assert_eq!(state.pane_at_row(19), None, "最下部の余白も対象外");
}

#[test]
fn the_frame_stays_pinned_while_the_list_scrolls() {
    const ROWS: usize = 10;
    let mut state = overflowing_state();
    state.selected = 11;
    state.render(ROWS, 32);

    let screen = state.screen_rows(ROWS);
    assert_frame(&screen, "スクロール中");
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
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
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
        markers[0].0 + markers[1].0 + (ROWS - HEADER_ROWS - FOOTER_ROWS - 2),
        13,
        "隠れている行数と出ている行数の合計が一覧の行数になる"
    );
}

#[test]
fn clicking_follows_the_scrolled_layout() {
    // 描画とクリックの逆引きが同じ切り出しを見ていないと行がずれる
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
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
    // 枠6行 + 一覧5行。枠が増えたぶん、旧テストの8行から引き上げてある
    const ROWS: usize = 11;
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
