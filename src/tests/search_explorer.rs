use crate::search::SearchPhase;
use crate::test_support::*;
use crate::*;

// --- 検索サブモード（要件: .docs/requirements/req-search-explorer.md） ---
//
// match_one / match_pane の単体テストは src/search.rs 側にある。
// ここでは State を通したキー処理と絞り込みの追従を見る。

#[test]
fn slash_enters_search_with_an_empty_query_matching_everything() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));

    let search = state.search.as_ref().unwrap();
    assert!(state.nav_mode, "検索サブモードはnavモードの内側");
    assert_eq!(search.query, "");
    assert_eq!(search.hits.len(), 3, "空クエリは全件一致");
    assert_eq!(search.cursor, Some(1), "カーソルは検索前の選択から始まる");
}

#[test]
fn printable_chars_feed_the_query_not_the_selection() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    // navモードでは j は移動キーだが、検索サブモード中はクエリになる
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.search.as_ref().unwrap().query, "j");
    assert_eq!(state.selected, 0, "選択は動かない");
}

#[test]
fn backspace_deletes_the_last_char_and_refilters() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    // "al" だと charlie（ch"a"r"l"ie）にもサブシーケンス一致するので "alp" を使う
    type_query(&mut state, "alpx");
    assert!(state.search.as_ref().unwrap().hits.is_empty());

    state.handle_nav_key(key(BareKey::Backspace));
    let search = state.search.as_ref().unwrap();
    assert_eq!(search.query, "alp");
    assert_eq!(search.hits.keys().copied().collect::<Vec<_>>(), vec![1]);
}

#[test]
fn query_matches_titles() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alpha");
    let search = state.search.as_ref().unwrap();
    assert_eq!(search.hits.keys().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(search.cursor, Some(1));
}

#[test]
fn a_tab_name_match_keeps_all_its_panes() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "tab1");
    let search = state.search.as_ref().unwrap();
    // tab1 配下の全ペインが残り、tab2 配下は消える
    assert_eq!(search.hits.keys().copied().collect::<Vec<_>>(), vec![1, 2]);
    assert!(search
        .hits
        .values()
        .all(|h| h.field == crate::search::Field::Tab));
}

#[test]
fn cwd_matches_even_when_show_cwd_is_off() {
    let mut state = searchable_state();
    assert!(!state.show_cwd);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");
    let search = state.search.as_ref().unwrap();
    // cwd を持つ bravo だけが当たる。cwd の無いペインは cwd では一致しない
    assert_eq!(search.hits.keys().copied().collect::<Vec<_>>(), vec![2]);
    assert_eq!(search.hits[&2].field, crate::search::Field::Cwd);
}

#[test]
fn cwd_hits_index_into_the_string_that_gets_drawn() {
    // 設定 show_cwd_tilde が効いていると cwd行に出るのは `~` 化した文字列。
    // `Hit::indices` はその field が指す文字列に対する char index なので、
    // 照合も畳んだ側で行わないとハイライトが別の文字に付く
    //（.docs/issues/issue-sidebar-cwd-tilde-home.md）
    let mut state = searchable_state();
    state.show_cwd_tilde = true;
    state.home_dir = Some("/Users/example".to_string());
    state
        .pane_cwds
        .insert(2, "/Users/example/work/fujin".to_string());

    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "fujin");

    let hit = &state.search.as_ref().unwrap().hits[&2];
    assert_eq!(hit.field, crate::search::Field::Cwd);
    let shown: Vec<char> = state.display_cwd(2).unwrap().chars().collect();
    assert_eq!(shown.iter().collect::<String>(), "~/work/fujin");
    let matched: String = hit.indices.iter().map(|&i| shown[i]).collect();
    assert_eq!(matched, "fujin", "畳んだ文字列の上で一致位置が合っている");
}

#[test]
fn cursor_moves_in_tree_order_and_stops_at_the_edges() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));

    state.handle_nav_key(key(BareKey::Down));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(2));
    state.handle_nav_key(key(BareKey::Tab));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(3));
    // 末尾で止まる
    state.handle_nav_key(key(BareKey::Down));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(3));

    state.handle_nav_key(key(BareKey::Up));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(2));
    state.handle_nav_key(key(BareKey::Tab).with_shift_modifier());
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(1));
    // 先頭で止まる
    state.handle_nav_key(key(BareKey::Up));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(1));

    assert!(state.nav_mode, "カーソル移動でモードを抜けない");
    assert!(state.search.is_some());
}

#[test]
fn esc_is_three_staged_and_does_not_close_a_summoned_instance() {
    // 決定202608131200で編集状態→操作状態の1段が挟まった（要件:
    // search-mode-entry-exit.feature / summoned-instance-search.feature）
    let mut state = searchable_state();
    state.summoned = true;
    state.own_plugin_id = Some(9);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "charlie");

    // 1段目: 操作状態へ移るだけ。クエリは破棄しない
    state.handle_nav_key(key(BareKey::Esc));
    assert_eq!(search_phase(&state), Some(SearchPhase::Navigating));
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("charlie"),
        "編集状態のEscではクエリを破棄しない"
    );

    // 2段目: クエリ破棄のみ。召喚インスタンスでも navモードに留まる
    state.handle_nav_key(key(BareKey::Esc));
    assert!(state.search.is_none());
    assert!(
        state.nav_mode,
        "検索サブモードのEscでnavモードごと抜けてはいけない"
    );

    // 3段目: navモードから退場（召喚インスタンスならここで自分を閉じる）
    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.nav_mode);
}

#[test]
fn slash_starts_in_the_editing_phase() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    assert_eq!(
        search_phase(&state),
        Some(SearchPhase::Editing),
        "`/` の直後は編集状態から始まる"
    );
}

#[test]
fn a_question_mark_feeds_the_query_while_editing() {
    // 決定202608031908の「クエリに `?` は打てない」という制約を決定202608131200で解消した
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "a?");

    assert!(!state.help_overlay, "編集状態の `?` でヘルプは開かない");
    assert_eq!(state.search.as_ref().map(|s| s.query.as_str()), Some("a?"));
}

#[test]
fn the_navigating_phase_moves_the_cursor_with_j_and_k() {
    let mut state = navigating_search("");
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(1));

    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(2));
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(3));
    state.handle_nav_key(key(BareKey::Char('k')));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(2));

    // 矢印・Tab も編集状態と同じく効く
    state.handle_nav_key(key(BareKey::Down));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(3));
    state.handle_nav_key(key(BareKey::Tab).with_shift_modifier());
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(2));
    state.handle_nav_key(key(BareKey::Up));
    assert_eq!(state.search.as_ref().unwrap().cursor, Some(1));

    assert_eq!(search_phase(&state), Some(SearchPhase::Navigating));
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some(""),
        "移動キーはクエリに入らない"
    );
}

#[test]
fn the_navigating_phase_ignores_everything_but_its_command_keys() {
    // vimのnormalモードに近い予測可能性を優先し、押し間違いで状態が黙って
    // 変わる事故を避ける（決定202608131200。要件: search-mode-key-handling.feature）
    let mut state = navigating_search("cha");
    state.handle_nav_key(key(BareKey::Char('j'))); // 端で止まるので動かない
    let cursor = state.search.as_ref().unwrap().cursor;

    for bare in [
        BareKey::Char('x'),
        BareKey::Char('/'),
        BareKey::Char('t'),
        BareKey::Char('n'),
        BareKey::Char('d'),
        BareKey::Char('g'),
        BareKey::Backspace,
    ] {
        state.handle_nav_key(key(bare));
        assert!(state.nav_mode, "{:?} で退場してはいけない", bare);
        assert_eq!(
            state.search.as_ref().map(|s| s.query.as_str()),
            Some("cha"),
            "{:?} でクエリが変わった",
            bare
        );
        assert_eq!(state.search.as_ref().unwrap().cursor, cursor);
        assert_eq!(
            search_phase(&state),
            Some(SearchPhase::Navigating),
            "{:?} で編集状態へ戻ってはいけない",
            bare
        );
    }
}

#[test]
fn the_i_key_resumes_editing_with_the_query_intact() {
    let mut state = navigating_search("cha");
    state.handle_nav_key(key(BareKey::Char('i')));

    assert_eq!(search_phase(&state), Some(SearchPhase::Editing));
    assert_eq!(state.search.as_ref().map(|s| s.query.as_str()), Some("cha"));
    // 続きが打てる（`i` 自体はクエリに入らない）
    type_query(&mut state, "r");
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("char")
    );
}

#[test]
fn the_navigating_phase_opens_the_help_overlay_with_a_question_mark() {
    let mut state = navigating_search("cha");
    state.handle_nav_key(key(BareKey::Char('?')));

    assert!(state.help_overlay);
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("cha"),
        "操作状態の `?` はクエリに入らない"
    );
}

#[test]
fn enter_jumps_from_the_navigating_phase_too() {
    let mut state = navigating_search("charlie");
    state.handle_nav_key(key(BareKey::Enter));

    assert!(!state.nav_mode, "確定はnavモードごと抜ける");
    assert_eq!(state.selectable[state.selected].pane_id, 3);
}

#[test]
fn modified_keys_leave_nav_mode_from_the_navigating_phase_too() {
    // 安全弁（決定202607310311）は操作状態でも効く（無反応にするのはコマンドキー以外の
    // 素のキーだけ。要件: search-mode-key-handling.feature）
    let mut state = navigating_search("cha");
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier());

    assert!(!state.nav_mode);
    assert!(state.search.is_none());
}

#[test]
fn pasted_text_is_ignored_in_the_navigating_phase() {
    // キーと同じ扱い（決定202608131200）。IMEの確定・貼り付けもテキスト入力なので、
    // 打てない状態では受け取らない
    let mut state = navigating_search("cha");
    assert!(!state.handle_pasted_text("日本語"));
    assert_eq!(state.search.as_ref().map(|s| s.query.as_str()), Some("cha"));
    assert_eq!(search_phase(&state), Some(SearchPhase::Navigating));

    // `i` で編集状態へ戻せばまた入る
    state.handle_nav_key(key(BareKey::Char('i')));
    assert!(state.handle_pasted_text("ん"));
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("chaん")
    );
}

#[test]
fn esc_restores_the_selection_saved_on_entry() {
    let mut state = searchable_state();
    state.selected = 1; // bravo
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Down));
    state.handle_nav_key(key(BareKey::Down));

    // 検索サブモード中に先頭へペインが増えてインデックスがずれても、ペインIDで戻す
    state.panes = Some(manifest(vec![
        (
            0,
            vec![
                terminal_pane(9, "newcomer"),
                terminal_pane(1, "alpha"),
                terminal_pane(2, "bravo"),
            ],
        ),
        (1, vec![terminal_pane(3, "charlie")]),
    ]));
    state.rebuild_selectable();

    // 1段目のEscは操作状態へ移るだけ。選択が戻るのは2段目（決定202608131200）
    state.handle_nav_key(key(BareKey::Esc));
    state.handle_nav_key(key(BareKey::Esc));
    assert_eq!(state.selectable[state.selected].pane_id, 2);
}

#[test]
fn enter_jumps_and_leaves_nav_mode_entirely() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "charlie");

    state.handle_nav_key(key(BareKey::Enter));
    assert!(!state.nav_mode, "確定はnavモードごと抜ける");
    assert!(state.search.is_none(), "クエリは破棄される");
    assert_eq!(state.selectable[state.selected].pane_id, 3);
}

#[test]
fn enter_with_no_hits_does_nothing() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "zzz");
    assert!(state.search.as_ref().unwrap().hits.is_empty());

    state.handle_nav_key(key(BareKey::Enter));
    assert!(state.nav_mode, "0件ヒットでは検索サブモードに留まる");
    assert!(state.search.is_some());
    assert_eq!(state.selected, 0, "ジャンプは起きない");
}

#[test]
fn modified_keys_leave_nav_mode_from_search_too() {
    // 安全弁は最上位まで効かせる（決定202607310311と同様）
    for key in [
        KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier(),
        KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier(),
    ] {
        let mut state = searchable_state();
        state.handle_nav_key(KeyWithModifier::new(BareKey::Char('/')));
        state.handle_nav_key(key.clone());
        assert!(!state.nav_mode, "{:?} でnavモードごと抜けるべき", key);
        assert!(state.search.is_none(), "{:?} で検索状態は破棄すべき", key);
    }
}

#[test]
fn reentering_search_starts_with_an_empty_query() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alpha");
    state.handle_nav_key(key(BareKey::Esc)); // 操作状態へ
    state.handle_nav_key(key(BareKey::Esc)); // ツリー表示へ

    state.handle_nav_key(key(BareKey::Char('/')));
    let search = state.search.as_ref().unwrap();
    assert_eq!(search.query, "");
    assert_eq!(search.phase, SearchPhase::Editing, "入り直しも編集状態から");
}

#[test]
fn the_cursor_follows_its_pane_through_list_updates() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Down)); // cursor = bravo(2)

    // 別タブでペインが増えても、カーソルは同じペインに留まる
    state.panes = Some(manifest(vec![
        (
            0,
            vec![terminal_pane(1, "alpha"), terminal_pane(2, "bravo")],
        ),
        (
            1,
            vec![terminal_pane(3, "charlie"), terminal_pane(4, "delta")],
        ),
    ]));
    state.rebuild_selectable();
    let search = state.search.as_ref().unwrap();
    assert_eq!(search.cursor, Some(2));
    assert_eq!(search.hits.len(), 4, "一覧の更新で絞り込みも引き直す");
}

#[test]
fn the_cursor_falls_back_to_the_first_hit_when_its_pane_closes() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Down)); // cursor = bravo(2)

    state.panes = Some(manifest(vec![
        (0, vec![terminal_pane(1, "alpha")]),
        (1, vec![terminal_pane(3, "charlie")]),
    ]));
    state.rebuild_selectable();
    assert_eq!(
        state.search.as_ref().unwrap().cursor,
        Some(1),
        "消えたら絞り込み結果の先頭へ寄せる"
    );
}

#[test]
fn refiltering_with_no_hits_clears_the_cursor() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alpha");
    state.panes = Some(manifest(vec![(1, vec![terminal_pane(3, "charlie")])]));
    state.rebuild_selectable();
    assert_eq!(state.search.as_ref().unwrap().cursor, None);
}
