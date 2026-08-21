// ペイン行の描画（要件: docs/requirements/req-sidebar-tree.md）: カウンタ列のレイアウト・cwd行・
// ペイン名が空のときのフォールバック。フローティングの区別表示は floating_pane_indicator.rs 側

use crate::agent::AgentState;
use crate::render::cwd_row;
use crate::render::CounterColumn;
use crate::render::HeadCells;
use crate::render::Row;
use crate::render::NO_AGENT_ICON;
use crate::test_support::*;
use crate::*;

// --- ペイン行のレイアウト（決定202608060052・決定202608060053） ---
//
// カウンタ列は右端に揃え、幅はフレーム全体で共有する。ペイン名はその残り幅に
// 収めるので、名前が長くてもサブエージェント数 `+N`・未完了タスク数 `[M]` は
// 消えない。cwd はペイン行に混ぜず、続く cwd行に出す

#[test]
fn a_pane_without_a_status_gets_the_no_agent_marker() {
    // 状態アイコン列を空白のままにすると、エージェントが乗る行と並べたときに
    // 左端が欠けて見える（docs/issues/issue-sidebar-cwd-row-legibility.md）
    let state = state_with_one_pane("shell");

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.starts_with(&format!("  {} shell", NO_AGENT_ICON)),
        "状態アイコンの位置に印を出す: {}",
        content
    );
    // 状態色（0/1/2/3/6）も dim も乗せない。意味の軸が違うものに状態色を
    // 割り当てないための印なので、装飾は持たせない
    assert!(!ink_at(&text, DIM_LEVEL).contains(&2), "{}", content);
    for level in [0, 1, 2, 3, ERROR_LEVEL] {
        assert!(
            !ink_at(&text, level).contains(&2),
            "状態色は乗せない（レベル{}）: {}",
            level,
            content
        );
    }
}

#[test]
fn a_status_replaces_the_no_agent_marker() {
    // 印はあくまで「状態が無いとき」の埋め草。状態が来たらアイコンごと譲る
    let mut state = state_with_one_pane("claude");
    set_agent_state(&mut state, 1, AgentState::Working);

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.starts_with(&format!("  {} claude", AgentState::Working.icon())),
        "{}",
        content
    );
    assert!(!content.contains(NO_AGENT_ICON), "{}", content);
    assert!(
        ink_at(&text, AgentState::Working.color()).contains(&2),
        "{}",
        content
    );
    assert!(!ink_at(&text, DIM_LEVEL).contains(&2), "{}", content);
}

#[test]
fn the_status_icon_keeps_its_color_on_the_highlighted_row() {
    // カーソルが乗っても状態色は変えない。行が変わるたびにアイコンの色が
    // 動くと、色だけで状態を判別できるという前提（決定202608062201）が崩れる。
    //
    // `selected()` と `opaque()` を併用していたころは、レベル0（idle）の位置指定
    // だけが zellij 本体のパースで壊れ、選択行の idle アイコンがテーマの base 色
    //（白系）に落ちていた（docs/issues/issue-idle-icon-color-on-selection.md）
    for target in [
        AgentState::Idle,
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Done,
        AgentState::Error,
    ] {
        for highlighted in [false, true] {
            let mut state = state_with_one_pane("claude");
            set_agent_state(&mut state, 1, target);
            let entry = state.selectable[0].clone();

            let pane = state.pane_row(
                &entry,
                highlighted,
                None,
                column_of(&state),
                HeadCells::default(),
                SIDEBAR,
            );
            let triage = state.triage_row(&entry, "tab1", highlighted, 4, None, SIDEBAR);
            for text in [&pane, &triage] {
                assert!(
                    ink_at(text, target.color()).contains(&2),
                    "{:?} highlighted={} のアイコンにレベル{}が乗らない: {:?}",
                    target,
                    highlighted,
                    target.color(),
                    text.content()
                );
            }
        }
    }
}

#[test]
fn the_highlighted_row_keeps_its_bar_and_background() {
    // 選択行の見た目は「左端のバー（レベル2）」と opaque な背景の2つ。
    // アイコンの色を直すために `selected()` を落としたときも、この2つは残す
    let mut state = state_with_one_pane("claude");
    set_agent_state(&mut state, 1, AgentState::Idle);
    let entry = state.selectable[0].clone();
    let text = state.pane_row(
        &entry,
        true,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert!(
        ink_at(&text, 2).contains(&0),
        "左端のバーが消えている: {:?}",
        text.content()
    );
    // opaque が付いていることを見る。背景が塗られないと帯にならず、
    // 幅いっぱいへ伸ばした空白（pad_to_width）が無駄になる
    assert!(
        is_opaque(&text),
        "opaque が落ちている: {:?}",
        Text::from(&text).serialize()
    );
}

#[test]
fn counters_are_flush_with_the_right_edge() {
    let mut state = state_with_one_pane("要件定義とドキュメント整理タスクの続き");
    repeat_status(&mut state, 1, "SubagentStart", 2);
    repeat_status(&mut state, 1, "TaskCreated", 3);

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.ends_with("+2 [3]"),
        "カウンタ列は右端に揃える: {}",
        content
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(content),
        CONTENT,
        "右端には常にマージンを空ける: {}",
        content
    );
    assert!(
        content.contains('…'),
        "収まらないぶんはペイン名側を畳む: {}",
        content
    );
}

#[test]
fn every_tree_row_leaves_a_right_margin() {
    // 文字がサイドバーの縁に貼り付くと窮屈に見える。タブ見出し行・ペイン行・
    // cwd行のどれも、収まらないときは右マージンの手前で畳む
    let mut state = state_with_one_pane("要件定義とドキュメント整理タスクの続き");
    state.tabs = vec![TabInfo {
        position: 0,
        name: "とても長い名前のタブがここにある".to_string(),
        active: true,
        ..Default::default()
    }];
    state.show_cwd = true;
    state.pane_cwds.insert(
        1,
        "/Users/example/development/oss/zellij-plugins/fujin".to_string(),
    );
    repeat_status(&mut state, 1, "SubagentStart", 2);

    let rows = state.visible_rows();
    let column = state.counter_column(&rows);
    for row in &rows {
        let line = match row {
            Row::Tab(tab) => state.tab_heading(tab, SIDEBAR),
            Row::Pane { entry, hit, .. } => {
                state.pane_row(entry, false, *hit, column, HeadCells::default(), SIDEBAR)
            }
            Row::Cwd { cwd, hit, .. } => cwd_row(cwd, false, *hit, SIDEBAR),
            _ => continue,
        };
        assert!(
            unicode_width::UnicodeWidthStr::width(line.content()) <= CONTENT,
            "右端にマージンが残っていない: {:?}",
            line.content()
        );
    }

    // 選択行の背景だけは右マージンも塗る。塗らないと帯が途中で切れて見える
    let selected = state.pane_row(
        &state.selectable[0],
        true,
        None,
        column,
        HeadCells::default(),
        SIDEBAR,
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(selected.content()),
        SIDEBAR
    );
}

#[test]
fn rows_with_any_counter_still_share_the_frames_column_width() {
    // サブエージェント数だけのペインと、未完了タスク数だけのペイン。
    // 桁がずれると一覧を縦に舐められないので、**カウンタを持つ行同士**は列を共有する
    // （2026-08-08改訂: 以前は「フレーム全体」で共有していたが、カウンタを一切
    // 持たない行まで巻き込んでいたのは不具合だった。そちらは
    // `a_counter_less_row_is_unaffected_by_other_rows_counters` で検証する）
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(1, "alpha"), terminal_pane(2, "bravo")],
    )]));
    state.rebuild_selectable();
    repeat_status(&mut state, 1, "SubagentStart", 12);
    repeat_status(&mut state, 2, "TaskCreated", 3);

    let column = column_of(&state);
    let first = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column,
        HeadCells::default(),
        SIDEBAR,
    );
    let second = state.pane_row(
        &state.selectable[1],
        false,
        None,
        column,
        HeadCells::default(),
        SIDEBAR,
    );

    // 列幅は `+12`（3）と `[3]`（3）、あいだの空白1つで計7セル
    assert_eq!(column_at(first.content(), "+12"), CONTENT - 7);
    assert_eq!(column_at(second.content(), "[3]"), CONTENT - 3);
    assert!(
        !second.content().contains('+'),
        "サブエージェント数を持たない行は、その位置を空けたままにする: {}",
        second.content()
    );
}

#[test]
fn a_counter_less_row_is_unaffected_by_other_rows_counters() {
    // 「+1」のようなカウンタが1行にでも出ると、それを持たない他の行まで右端が
    // 削られていた不具合の回帰テスト
    // （docs/issues/issue-counter-column-collateral-truncation.md）。
    // 同じ行を「静かな列（ゼロ幅）」と「実測した列（非ゼロ幅）」の両方で描画し、
    // 自分自身がカウンタを持たなければ結果が完全に一致することを確認する
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(
        0,
        vec![
            terminal_pane(1, "要件定義とドキュメント整理タスクの続き"),
            terminal_pane(2, "bravo"),
        ],
    )]));
    state.rebuild_selectable();
    repeat_status(&mut state, 2, "SubagentStart", 12);

    let quiet_column = CounterColumn::default();
    let noisy_column = column_of(&state);
    assert_ne!(
        noisy_column, quiet_column,
        "このフレームは他のペインがカウンタを持っている前提"
    );

    let without_counters = state.pane_row(
        &state.selectable[0],
        false,
        None,
        quiet_column,
        HeadCells::default(),
        SIDEBAR,
    );
    let with_counters = state.pane_row(
        &state.selectable[0],
        false,
        None,
        noisy_column,
        HeadCells::default(),
        SIDEBAR,
    );

    assert_eq!(
        without_counters.content(),
        with_counters.content(),
        "カウンタを持たない行は、他の行がカウンタを持っていても表示が変わらない"
    );
}

#[test]
fn the_counter_column_costs_nothing_when_nobody_has_counters() {
    // 静かなフレームでは列を予約しない。予約するとペイン名の幅がその場で失われる
    let state = state_with_one_pane("abcdefghijklmnopqrstuvwxyz0123456789");
    assert_eq!(column_of(&state), CounterColumn::default());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(content.starts_with("  › abcdefghij"), "{}", content);
    assert!(content.ends_with('…'), "{}", content);
    assert_eq!(unicode_width::UnicodeWidthStr::width(content), CONTENT);
}

#[test]
fn a_pane_name_that_is_a_path_keeps_its_tail() {
    // ペイン名に cwd がそのまま入ることがある。末尾を切ると
    // `/Users/example/develo…` のようにどの行も同じ見た目になってしまう
    let state = state_with_one_pane("/Users/example/development/oss/zellij-plugins/fujin");

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.ends_with("zellij-plugins/fujin"),
        "パスは先頭省略で末尾を残す: {}",
        content
    );
    assert!(content.contains('…'), "{}", content);
    assert_eq!(unicode_width::UnicodeWidthStr::width(content), CONTENT);
}

// --- cwd行（決定202608060053） ---

#[test]
fn the_cwd_is_rendered_as_its_own_row() {
    let mut state = state_with_one_pane("claude-worker");
    state.show_cwd = true;
    set_agent_state(&mut state, 1, AgentState::Idle);
    state
        .pane_cwds
        .insert(1, "/work/oss/zellij-plugins/fujin".to_string());

    let rows = state.visible_rows();
    // 0-2: ヘッダ3行 / 3: tab1見出し / 4: ペイン行 / 5: cwd行
    assert!(matches!(rows[HEADER_ROWS + 1], Row::Pane { .. }));
    assert!(matches!(rows[HEADER_ROWS + 2], Row::Cwd { .. }));
    assert_eq!(
        state.pane_at_row(HEADER_ROWS + 2),
        Some(1),
        "cwd行のクリックも同じペインに当たる（要件: click-to-focus）"
    );

    let pane_text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert!(
        !pane_text.content().contains("/work"),
        "cwd はペイン行には出ない: {}",
        pane_text.content()
    );
}

#[test]
fn a_pane_without_a_cwd_gets_no_extra_row() {
    // 空の cwd行で縦を消費しない（サイドバーはスクロールしないので行数は貴重）
    let mut state = state_with_one_pane("claude-worker");
    state.show_cwd = true;

    let rows = state.visible_rows();
    assert_eq!(
        rows.len(),
        HEADER_ROWS + 2 + FOOTER_ROWS,
        "枠・タブ見出し行・ペイン行だけ"
    );
}

#[test]
fn the_cwd_row_appears_for_a_search_hit_even_when_show_cwd_is_off() {
    // 画面に無い文字列でヒットしたように見せない（決定202608031908）
    let mut state = searchable_state();
    assert!(!state.show_cwd);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");

    let rows = state.visible_rows();
    let cwd_rows: Vec<&Row> = rows
        .iter()
        .filter(|r| matches!(r, Row::Cwd { .. }))
        .collect();
    assert_eq!(cwd_rows.len(), 1, "cwd を持つ bravo の行だけ");
    let Some(Row::Cwd { entry, hit, .. }) = cwd_rows.first() else {
        panic!("cwd行が無い");
    };
    assert_eq!(entry.pane_id, 2);
    assert!(hit.is_some(), "ヒット箇所を提示するのでハイライトを持つ");
}

#[test]
fn the_cwd_row_is_not_highlighted_by_a_pane_name_hit() {
    // ペイン名に当たっただけの行では cwd行を光らせない
    let mut state = searchable_state();
    state.show_cwd = true;
    set_agent_state(&mut state, 2, AgentState::Idle);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "bravo");

    let rows = state.visible_rows();
    let Some(Row::Cwd { hit, .. }) = rows.iter().find(|r| matches!(r, Row::Cwd { .. })) else {
        panic!("cwd行が無い");
    };
    assert!(hit.is_none());
}

#[test]
fn the_cwd_row_disappears_when_the_agent_exits() {
    // エージェントが去ったら cwd行も引っ込める
    //（docs/issues/issue-sidebar-cwd-persists-after-exit.md）
    let mut state = state_with_one_pane("claude-worker");
    state.show_cwd = true;
    set_agent_state(&mut state, 1, AgentState::Idle);
    state
        .pane_cwds
        .insert(1, "/work/oss/zellij-plugins/fujin".to_string());
    assert!(
        state
            .visible_rows()
            .iter()
            .any(|r| matches!(r, Row::Cwd { .. })),
        "動いている間は出る"
    );

    state.apply_status(status(1, "SessionEnd"));

    assert!(
        !state
            .visible_rows()
            .iter()
            .any(|r| matches!(r, Row::Cwd { .. })),
        "終了後は出ない"
    );
    // cwd 自体は捨てない。ペイン名フォールバック（決定202608070102）が使う
    assert!(state.pane_cwds.contains_key(&1));
}

#[test]
fn an_exited_agent_still_gets_a_cwd_row_for_a_search_hit() {
    // 表示条件を絞っても、絞り込み結果の提示は変えない — 一覧に残っている以上、
    // 何に一致したかは示す（決定202608031908）
    let mut state = searchable_state();
    state.show_cwd = true;
    // pane2 は cwd を持つがエージェントは居ない（＝終了後と同じ状態）
    assert!(!state.agents.contains_key(&2));
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");

    let rows = state.visible_rows();
    let Some(Row::Cwd { entry, hit, .. }) = rows.iter().find(|r| matches!(r, Row::Cwd { .. }))
    else {
        panic!("cwd行が無い");
    };
    assert_eq!(entry.pane_id, 2);
    assert!(hit.is_some());
}

#[test]
fn the_cwd_row_keeps_the_tail_of_the_path() {
    let text = cwd_row(
        "/Users/example/development/oss/zellij-plugins/fujin",
        false,
        None,
        SIDEBAR,
    );
    let content = text.content();
    assert!(
        content.ends_with("zellij-plugins/fujin"),
        "末尾のディレクトリ名を残す: {}",
        content
    );
    assert!(
        content.starts_with("      …"),
        "字下げして続きに見せる: {}",
        content
    );
    assert!(unicode_width::UnicodeWidthStr::width(content) <= CONTENT);
}

// --- ペイン名フォールバック（決定202608070102） ---
//
// claude は終了時に空のタイトルを OSC で送るため、エージェントを落とした瞬間に
// ペイン名が空のまま残る（docs/issues/issue-pane-title-blank-on-exit.md）

#[test]
fn an_empty_pane_name_falls_back_to_the_cwd() {
    let mut state = state_with_one_pane("");
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert!(
        text.content().contains("fujin"),
        "ペイン名の位置に cwd を出す: {}",
        text.content()
    );
}

#[test]
fn a_pane_name_that_is_only_spaces_falls_back_too() {
    let mut state = state_with_one_pane("   ");
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert!(text.content().contains("fujin"), "{}", text.content());
}

#[test]
fn a_non_empty_pane_name_is_left_alone() {
    // 決定202608050055（生のペイン名をそのまま出す）は空でないときは変わらない
    let mut state = state_with_one_pane("claude-worker");
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(content.contains("claude-worker"), "{}", content);
    assert!(
        !content.contains("fujin"),
        "cwd で上書きしない: {}",
        content
    );
}

#[test]
fn an_empty_pane_name_without_a_cwd_stays_blank() {
    // 落とす先が無いペインは名前の位置が空欄のまま。名前を捏造しない
    //（アイコン列の未起動の印だけは出る）
    let state = state_with_one_pane("");

    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    assert_eq!(text.content().trim(), NO_AGENT_ICON, "{}", text.content());
}

#[test]
fn the_cwd_row_is_dropped_while_the_pane_name_falls_back() {
    // 同じパスが2行並んでも情報が増えない
    let mut state = state_with_one_pane("");
    state.show_cwd = true;
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());

    let rows = state.visible_rows();
    assert!(
        !rows.iter().any(|r| matches!(r, Row::Cwd { .. })),
        "cwd行は出さない"
    );
    assert_eq!(
        rows.len(),
        HEADER_ROWS + 2 + FOOTER_ROWS,
        "枠・タブ見出し行・ペイン行だけ"
    );
}

#[test]
fn a_falling_back_pane_row_still_matches_on_the_cwd() {
    // 生のペイン名は空なので、当たるのは cwd。ハイライトはペイン名の位置に
    // 出ている cwd 側へ載るため、ここでも cwd行は足さない
    let mut state = state_with_one_pane("");
    state.nav_mode = true;
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");

    let rows = state.visible_rows();
    assert!(
        rows.iter().any(|r| matches!(r, Row::Pane { .. })),
        "cwd 一致でペイン行が残る"
    );
    assert!(!rows.iter().any(|r| matches!(r, Row::Cwd { .. })));
    // ハイライト位置の算出（畳んだ cwd への index 付け替え）を通す
    state.render(SIDEBAR, 10);
}

#[test]
fn triage_rows_fall_back_to_the_cwd_too() {
    let mut state = state_with_one_pane("");
    state.nav_mode = true;
    set_agent_state(&mut state, 1, AgentState::Blocked);
    state.pane_cwds.insert(1, "/work/oss/fujin".to_string());
    state.handle_nav_key(key(BareKey::Char('t')));

    let rows = state.visible_rows();
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い: {}", rows.len());
    };
    let text = state.triage_row(entry, tab_name, false, tab_column, None, SIDEBAR);
    assert!(
        text.content().contains("fujin"),
        "トリアージ行でも cwd へ落とす: {}",
        text.content()
    );
}

#[test]
fn the_cwd_row_survives_a_sidebar_narrower_than_its_indent() {
    // 字下げより狭い幅でも算術が破綻しない
    let text = cwd_row("/work/fujin", false, None, 3);
    assert!(unicode_width::UnicodeWidthStr::width(text.content()) <= 6);
}
