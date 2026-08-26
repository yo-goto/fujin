use crate::agent::AgentState;
use crate::config::Config;
use crate::config::Kind;
use crate::config::CONFIG_WARNING_SECS;
use crate::config::SETTINGS;
use crate::render::Row;
use crate::render::NO_AGENT_ICON;
use crate::render::NO_AGENT_LABEL;
use crate::render::{overflow_row, Line};
use crate::search::SearchPhase;
use crate::test_support::*;
use crate::*;

// --- 設定の取り込みと警告（決定202608080346。要件: configuration） ---

#[test]
fn the_property_form_of_kdl_is_normalised() {
    // zellij はプロパティ書式（`show_cwd="true"`）の値を引用符込みで渡す
    //（子ノード書式は素の文字列）。取り込み口で剥がして同じ意味にする
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("show_cwd", "\"true\""),
        ("up_key", "\"Alt u\""),
    ]));

    assert!(state.show_cwd, "引用符付きでも真として読む");
    assert_eq!(state.footer_line(SIDEBAR).content(), "  alt+u:up");
    assert!(
        state.config_warnings.is_empty(),
        "正規化できたので警告は無い"
    );
}

#[test]
fn a_flag_setting_takes_only_true_and_false() {
    // 真偽値の受け口は広げない（決定202608080346）。`1` や `yes` を真と見なすと、
    // 「効かない書き方」の一覧がユーザーからは推測できなくなる
    let mut state = state_with_panes(2);
    for (value, expected) in [("true", true), ("false", false)] {
        state.apply_config(&plugin_config(&[("show_cwd", value)]));
        assert_eq!(state.show_cwd, expected, "{}", value);
        assert!(state.config_warnings.is_empty(), "{}", value);
    }

    for value in ["1", "yes", "TRUE"] {
        state.apply_config(&plugin_config(&[("show_cwd", value)]));
        assert!(!state.show_cwd, "{}", value);
        assert_eq!(state.config_warnings, vec!["show_cwd"], "{}", value);
    }
}

#[test]
fn every_flag_setting_reaches_the_config() {
    // `SETTINGS` に Flag を足したのに `Config::set_flag` へ配線し忘れると、
    // その設定は警告も出さずに黙って無視される。既定値のずれもここで落ちる
    for setting in &SETTINGS {
        let Kind::Flag { default } = setting.kind else {
            continue;
        };
        assert_eq!(
            Config::parse(&plugin_config(&[(setting.key, &default.to_string())])),
            Config::default(),
            "既定と同じ値を書いたのに既定と違う結果になる: {}",
            setting.key
        );
        assert_ne!(
            Config::parse(&plugin_config(&[(setting.key, &(!default).to_string())])),
            Config::default(),
            "既定と逆の値が効いていない（配線漏れ）: {}",
            setting.key
        );
    }
}

#[test]
fn an_unusable_value_warns_in_the_footer() {
    // 既定値へ黙って倒すと、書いた設定が効かない理由が分からない（決定202608080346）
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);
    state.apply_config(&plugin_config(&[("show_cwd", "1"), ("up_key", "alt+u")]));
    state.arm_config_warning();

    let footer = state.footer_line(SIDEBAR);
    assert_eq!(footer.content(), "  !bad value: show_cwd");
    assert!(
        !ink_at(&footer, ERROR_LEVEL).is_empty(),
        "警告色（error_color）で出す"
    );
    // ヘッダーの三角も同じ状態色に揃う（決定202608070119）
    assert!(!ink_at(&state.header_line(SIDEBAR), ERROR_LEVEL).is_empty());
}

#[test]
fn several_unusable_values_fold_into_a_count() {
    // 幅32のフッターには全部は載らない。末尾を `…` で切らず、残りは件数に畳む
    let mut state = state_with_panes(2);
    state.apply_config(&plugin_config(&[
        ("show_cwd", "1"),
        ("up_key", "Meta q"),
        ("down_key", "Meta w"),
    ]));
    state.arm_config_warning();

    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert_eq!(footer, "  !bad values: show_cwd +2");
    assert!(!footer.contains('…'), "{}", footer);
}

#[test]
fn the_config_warning_gives_way_to_an_input_footer() {
    // 入力欄・確認プロンプト・ヘルプはそこに出ていないと操作が成立しない。
    // 警告が譲るのはこれらに対してだけで、静的なヒントには譲らない
    let mut state = searchable_state();
    state.apply_config(&plugin_config(&[("show_cwd", "1")]));
    state.arm_config_warning();

    state.nav_mode = true;
    assert_eq!(
        state.footer_line(SIDEBAR).content(),
        "  !bad value: show_cwd",
        "navモードの静的ヒントよりは警告が優先する"
    );

    state.handle_nav_key(key(BareKey::Char('/')));
    let footer = state.footer_line(SIDEBAR).content().to_string();
    assert!(
        footer.starts_with("  /"),
        "検索クエリ入力欄が勝つ: {}",
        footer
    );
}

#[test]
fn the_config_warning_stops_after_its_deadline() {
    // 起動直後の一定時間だけ（決定202608080346）。期限が切れたら通常の表示へ戻る
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);
    state.apply_config(&plugin_config(&[
        ("show_cwd", "1"),
        ("up_key", "alt+up"),
        ("down_key", "alt+down"),
    ]));
    state.arm_config_warning();
    assert!(state.showing_config_warning());

    // 期限までは出したまま
    assert!(!state.on_timer(CONFIG_WARNING_SECS / 2.0));
    assert!(state.showing_config_warning());

    // 期限を跨いだ Timer で消え、フッターを描き直させる
    assert!(state.on_timer(CONFIG_WARNING_SECS), "描き直しを要求する");
    assert!(!state.showing_config_warning());
    assert_eq!(
        state.footer_line(SIDEBAR).content(),
        "  alt + › up:up  down:down"
    );
}

#[test]
fn the_readme_settings_section_is_generated_from_the_table() {
    // 設定一覧の正本はコード（決定202608080346）。README へ手で転記した表は必ずいつか
    // ずれるので、生成物との一致をテストで縛る。差分が出たら `make readme`
    use crate::config::doc::{settings_doc, splice, Lang};

    for (file, lang) in [("README.ja.md", Lang::Ja), ("README.md", Lang::En)] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        let current = std::fs::read_to_string(&path).unwrap();
        let updated = splice(&current, &settings_doc(lang))
            .unwrap_or_else(|| panic!("{} に settings の目印が無い", file));

        if std::env::var_os("UPDATE_README").is_some() {
            std::fs::write(&path, updated).unwrap();
            continue;
        }
        assert_eq!(
            current, updated,
            "{} の設定節が config.rs とずれている。`make readme` で再生成する",
            file
        );
    }
}

#[test]
fn the_nav_footer_shows_the_help_and_exit_hints() {
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);
    state.nav_mode = true;

    let footer = state.footer_line(32);
    let footer = footer.content();
    assert_eq!(footer, "  ?:help  esc:exit");
    assert!(
        !footer.contains("jump"),
        "モード中は direct-keys のヒントに戻らない"
    );
}

#[test]
fn the_triage_footer_says_esc_goes_back() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));

    // Esc の行き先はツリー表示であってnavモードの退場ではない
    assert_eq!(state.footer_line(SIDEBAR).content(), "  ?:help  esc:back");
}

#[test]
fn the_footer_becomes_the_query_field_while_searching() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alp");

    let footer = state.footer_line(32);
    let footer = footer.content();
    // 地の文の疑似カーソルは描かない（位置はテキストカーソルに任せる。決定202608131200、
    // 2026-08-13 に廃止）。`?` はクエリの文字なのでヘルプの案内は出さない
    //（要件: nav-mode-hints.feature）
    assert!(footer.starts_with("  /alp"), "{}", footer);
    assert!(!footer.contains('▏'), "{}", footer);
    assert!(footer.contains("esc:browse"), "{}", footer);
    assert!(!footer.contains("?:help"), "{}", footer);
    assert!(footer.chars().count() <= 32);
}

#[test]
fn the_footer_hints_change_with_the_search_phase() {
    // 状態インジケータは入力文字列の明暗とヒント文言の2つ（決定202608131200、2026-08-13に
    // 地の文の疑似カーソルを廃止。経緯: .docs/issues/issue-search-input-cursor-shape.md）
    let mut state = navigating_search("");
    let footer = state.footer_line(32);
    let footer = footer.content();
    assert!(footer.starts_with("  /"), "{}", footer);
    // 移動キー・ヘルプキー・編集再開キー（要件: nav-mode-hints.feature）
    for hint in ["j/k:move", "?:help", "i:edit"] {
        assert!(footer.contains(hint), "{}: {}", hint, footer);
    }
    assert!(
        unicode_width::UnicodeWidthStr::width(footer) <= 32 - 2,
        "{}",
        footer
    );

    // `i` で編集状態へ戻せばヒントが編集中のものへ戻る
    state.handle_nav_key(key(BareKey::Char('i')));
    type_query(&mut state, "alp");
    let footer = state.footer_line(32);
    let footer = footer.content();
    assert!(footer.starts_with("  /alp"), "{}", footer);
    assert!(!footer.contains("j/k:move"), "{}", footer);
}

#[test]
fn the_query_dims_while_navigating_but_the_cursor_stays_lit() {
    // 地の文の疑似カーソルは無くテキストカーソルへ位置表示を一本化した（決定202608131200、
    // 2026-08-13。経緯: .docs/issues/issue-search-input-cursor-shape.md）ので、状態を
    // 見分ける手がかりは入力文字列の明暗とフッターのヒント文言。打てない状態でも
    // 位置を見失わせないよう、沈めるのはクエリだけでテキストカーソルは点いたまま残す
    let mut state = searchable_state();
    // テキストカーソルの行は描画で決まる（描く前は位置が定まらない）
    state.render(20, SIDEBAR);
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "alp");

    // 編集中のクエリは本文の明るさのまま
    let footer = state.footer_line(SIDEBAR);
    assert!(
        ink_at(&footer, DIM_LEVEL).is_empty(),
        "編集中は沈めない: {}",
        footer.content()
    );

    // 操作状態ではクエリだけが dim。テキストカーソル（位置の手がかり）は消さない
    let query_end = "  /alp".chars().count();
    let query: Vec<usize> = ("  /".chars().count()..query_end).collect();
    state.handle_nav_key(key(BareKey::Esc));
    let footer = state.footer_line(SIDEBAR);
    assert_eq!(
        ink_at(&footer, DIM_LEVEL),
        query,
        "クエリだけを沈める: {}",
        footer.content()
    );
    assert_eq!(
        state.input_cursor_position().map(|(x, _)| x),
        Some(query_end),
        "操作状態でもテキストカーソルはクエリ末尾に残す"
    );
}

#[test]
fn a_long_query_wins_over_the_hints() {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "0123456789");

    // 幅12には操作ヒントを置く余地が無い。入力中のクエリのほうを残す
    let footer = state.footer_line(12);
    let footer = footer.content();
    assert!(!footer.contains("esc"), "{}", footer);
    assert!(footer.chars().count() <= 12);
}

#[test]
fn the_hints_drop_whole_items_when_they_do_not_fit() {
    // 幅が足りないときは `…` で切らず末尾の項目ごと落とす（direct-keys の
    // ヒントと同じ削り方。.docs/issues/issue-direct-keys-hint-overflow.md）
    let state = navigating_search("");
    let footer = state.footer_line(32);
    let footer = footer.content();
    assert!(!footer.contains('…'), "{}", footer);
    // 予算に入りきらない末尾（`esc:cancel` 以降）だけが落ち、前は壊れずに残る
    assert!(footer.contains("j/k:move  ?:help  i:edit"), "{}", footer);
    assert!(!footer.contains("esc:cancel"), "{}", footer);

    // クエリが伸びればさらに末尾から落ちる（項目ごと落とすので形は壊れない）
    let mut state = state;
    state.handle_nav_key(key(BareKey::Char('i')));
    type_query(&mut state, "alp");
    state.handle_nav_key(key(BareKey::Esc));
    let footer = state.footer_line(32);
    let footer = footer.content();
    assert!(footer.contains("j/k:move  ?:help"), "{}", footer);
    assert!(!footer.contains("i:edit"), "{}", footer);
}

#[test]
fn a_full_width_query_does_not_push_the_hint_off_the_edge() {
    // 右寄せの余白は表示セル幅で数える。文字数で数えると全角のクエリで
    // 操作ヒントが端からはみ出す（.docs/concept/ui-design.md のレイアウト規則）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "日本語のペイン名");

    let footer = state.footer_line(32);
    assert!(
        unicode_width::UnicodeWidthStr::width(footer.content()) <= 32,
        "{}",
        footer.content()
    );
}

#[test]
fn the_footer_takes_over_the_help_overlays_closing_hint() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    assert_eq!(
        state.footer_line(SIDEBAR).content(),
        "  press any key to close"
    );
    // 本文側からは消してある（同じ文言を2箇所に出さない）
    for row in state.help_lines() {
        let line = state.help_line(row, SIDEBAR).content().to_string();
        assert!(!line.contains("press any key"), "{}", line);
    }
}

#[test]
fn the_footer_wears_the_state_color_including_its_keys() {
    // 「キーは常にレベル2固定」の色役割はフッターに限り例外（決定202608070119）。
    // ヘッダーの三角とトーンを揃えるほうを取る
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));

    let footer = state.footer_line(SIDEBAR);
    let indent = 2;
    let end = footer.content().chars().count();
    assert_eq!(
        ink_at(&footer, 0),
        (indent..end).collect::<Vec<_>>(),
        "キーも説明もトリアージの状態色: {}",
        footer.content()
    );

    // 非フォーカス時は落とした色（＝ヘッダーの三角と同じ dim）
    let mut state = state_with_panes(2);
    with_direct_keys(&mut state);
    let footer = state.footer_line(SIDEBAR);
    let end = footer.content().chars().count();
    assert_eq!(
        ink_at(&footer, DIM_LEVEL),
        (indent..end).collect::<Vec<_>>()
    );
}

#[test]
fn the_footer_never_runs_off_the_right_margin() {
    let mut state = searchable_state();
    with_direct_keys(&mut state);
    let mut widths = vec![state.footer_line(SIDEBAR)];
    state.nav_mode = true;
    widths.push(state.footer_line(SIDEBAR));
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "日本語のクエリで幅を埋める");
    widths.push(state.footer_line(SIDEBAR));
    // 操作状態（ヒントが長い）でも同じ（決定202608131200）
    state.handle_nav_key(key(BareKey::Esc));
    widths.push(state.footer_line(SIDEBAR));

    for footer in widths {
        assert!(
            unicode_width::UnicodeWidthStr::width(footer.content()) <= SIDEBAR - 2,
            "右マージンを食う: {}",
            footer.content()
        );
    }
}

#[test]
fn question_mark_opens_the_help_overlay() {
    for key in [
        KeyWithModifier::new(BareKey::Char('?')),
        // Shift+/ として届く端末もある
        KeyWithModifier::new(BareKey::Char('?')).with_shift_modifier(),
    ] {
        let mut state = state_with_panes(3);
        state.nav_mode = true;
        state.handle_nav_key(key.clone());

        assert!(state.help_overlay, "{:?} でヘルプを開くべき", key);
        assert!(state.nav_mode, "ヘルプはnavモードの内側");
        assert_eq!(state.selected, 0, "選択は動かさない");
    }
}

#[test]
fn the_help_overlay_covers_the_tree_but_keeps_the_frame() {
    // 覆うのは content だけ。ヘッダー・フッターと境界線は出したままにする（決定202608070119）
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let rows = state.visible_rows();
    assert_frame(&rows, "ヘルプ表示中");
    assert!(
        rows[HEADER_ROWS..rows.len() - FOOTER_ROWS]
            .iter()
            .all(|r| matches!(r, Row::Help(_))),
        "ヘルプ表示中はツリーを出さない"
    );
    // 行クリックの逆引きも当たらない（要件: click-to-focus と食い違わせない）
    assert!((0..rows.len()).all(|y| state.pane_at_row(y).is_none()));
}

#[test]
fn the_help_lines_fit_the_sidebar_width() {
    // 幅32（決定202607302256）に収まらないと、キー列か説明のどちらかが … で消える
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('?')));
    for line in overlay_lines(&state, 32) {
        assert!(!line.contains('…'), "navモード: {}", line);
    }
    state.handle_nav_key(key(BareKey::Char('?'))); // いったん閉じる
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Esc)); // ヘルプを開けるのは操作状態から（決定202608131200）
    state.handle_nav_key(key(BareKey::Char('?')));
    for line in overlay_lines(&state, 32) {
        assert!(!line.contains('…'), "検索サブモード: {}", line);
    }
    state.handle_nav_key(key(BareKey::Char('?'))); // いったん閉じる
    state.handle_nav_key(key(BareKey::Esc)); // 検索サブモードを抜ける
    state.handle_nav_key(key(BareKey::Char('n')));
    state.handle_nav_key(key(BareKey::Char('?')));
    for line in overlay_lines(&state, 32) {
        assert!(!line.contains('…'), "番号ジャンプサブモード: {}", line);
    }
    state.handle_nav_key(key(BareKey::Char('?'))); // いったん閉じる
    state.handle_nav_key(key(BareKey::Esc)); // 番号ジャンプサブモードを抜ける
    state.handle_nav_key(key(BareKey::Char('d')));
    state.handle_nav_key(key(BareKey::Char('?')));
    for line in overlay_lines(&state, 32) {
        assert!(!line.contains('…'), "終了操作サブモード: {}", line);
    }
}

#[test]
fn the_help_overlay_explains_each_termination() {
    // フッターの確認プロンプトに収まらない Esc の行き先もここで補う
    let mut state = termination_state(3);
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    for expected in ["close pane", "kill process", "kill & close", "cancel"] {
        assert!(lines.contains(expected), "{}: {}", expected, lines);
    }
    // 開いたヘルプは終了操作を実行せずに閉じ、確認プロンプトへ戻る
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(state.termination.is_some());
}

#[test]
fn the_nav_help_advertises_the_termination_key() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    assert!(lines.contains("terminate pane"), "{}", lines);
}

// 文字列に日本語（ひらがな・カタカナ・漢字）が混ざっているか。
// `▲` `▌` `…` や状態アイコンは英語の文言と一緒に使うので弾かない
fn has_japanese(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c))
}

// fujin が自分で書く行（ヘッダー・フッター・ヘルプ・通知行）。ペイン名・タブ名・
// cwd はユーザーのデータなので、日本語が入っていて当然で対象から外す
fn chrome_lines(state: &State) -> Vec<String> {
    let mut lines = vec![
        state.header_line(SIDEBAR).content().to_string(),
        state.footer_line(SIDEBAR).content().to_string(),
    ];
    lines.extend(overlay_lines(state, SIDEBAR));
    for row in state.visible_rows() {
        if let Row::Notice(notice) = row {
            lines.push(notice.to_string());
        }
    }
    lines
}

#[test]
fn the_sidebar_never_shows_japanese_text() {
    // UI文言は英語で統一する（.docs/concept/ui-design.md の「文言」）。
    // コメントとドキュメントは日本語なので、画面に出る側だけを一度に見る
    let mut lines = vec![overflow_row(3, true, SIDEBAR).content().to_string()];

    let agent_state = |nav: bool| {
        let mut state = triage_state();
        set_agent_state(&mut state, 1, AgentState::Working);
        state.nav_mode = nav;
        state
    };
    // ツリー表示（非フォーカス）と navモード
    lines.extend(chrome_lines(&agent_state(false)));
    lines.extend(chrome_lines(&agent_state(true)));

    // 各サブモードと、そこで開いたヘルプオーバーレイ（状態アイコン凡例を含む）
    for entry in ['/', 'p', 'n', 'd'] {
        let mut state = agent_state(true);
        state.handle_nav_key(key(BareKey::Char(entry)));
        lines.extend(chrome_lines(&state));
        state.handle_nav_key(key(BareKey::Char('?')));
        lines.extend(chrome_lines(&state));
    }

    // 通知行（検索の0件・トリアージの対象なし）
    let mut no_hits = agent_state(true);
    no_hits.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut no_hits, "zzz");
    lines.extend(chrome_lines(&no_hits));
    let mut nothing_to_triage = triage_state(); // 状態を持つペインが1つも無い
    nothing_to_triage.handle_nav_key(key(BareKey::Char('t')));
    lines.extend(chrome_lines(&nothing_to_triage));

    for line in &lines {
        assert!(
            !has_japanese(line),
            "UI文言に日本語が混ざっている: {}",
            line
        );
    }
    // 通知行を拾えていることの確認（拾えていないと上のループが素通しになる）
    assert!(
        lines.iter().any(|l| l == "no matches"),
        "検索の0件通知が無い: {:?}",
        lines
    );
    assert!(
        lines.iter().any(|l| l == "nothing to triage"),
        "トリアージの対象なし通知が無い: {:?}",
        lines
    );
}

#[test]
fn the_help_overlay_ends_with_the_status_icon_legend() {
    // 要件: nav-mode-hints.feature「ヘルプオーバーレイに状態アイコン凡例が
    // 表示される」。README を見に行かなくても記号の意味を引けるようにする（決定202608062201）
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let lines = overlay_lines(&state, SIDEBAR);
    let heading = lines
        .iter()
        .position(|line| line.trim() == "status")
        .expect("凡例の見出しがある");
    let legend: Vec<&String> = lines[heading + 1..]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();

    // 並び・アイコン・説明はすべて AgentState のテーブル由来（決定202608062201）。
    // 末尾の1行だけは AgentState に無い「エージェントが乗っていないペイン」の印
    assert_eq!(legend.len(), AgentState::ALL.len() + 1, "{:?}", legend);
    for (line, agent_state) in legend.iter().zip(AgentState::ALL.iter()) {
        assert!(
            line.starts_with(&format!("  {}", agent_state.icon())),
            "アイコンがキー列の位置に出る: {}",
            line
        );
        assert!(
            line.ends_with(agent_state.label()),
            "状態名が説明として続く: {}",
            line
        );
    }
    let no_agent = legend.last().expect("未起動の行がある");
    assert!(
        no_agent.starts_with(&format!("  {}", NO_AGENT_ICON)) && no_agent.ends_with(NO_AGENT_LABEL),
        "エージェントが乗っていないペインの印も同じ節で引ける: {}",
        no_agent
    );
    // キー一覧より後ろに置く。先に読むべきは操作のほう
    let last_key = lines
        .iter()
        .position(|line| line.contains("this help"))
        .expect("キー一覧がある");
    assert!(last_key < heading);
}

#[test]
fn the_status_legend_lines_up_with_the_key_column() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let lines = overlay_lines(&state, SIDEBAR);
    let key_entry = lines
        .iter()
        .find(|line| line.contains("this help"))
        .expect("キー一覧がある");
    let legend = lines
        .iter()
        .find(|line| line.ends_with("working"))
        .expect("凡例がある");
    assert_eq!(
        column_at(legend, "working"),
        column_at(key_entry, "this help"),
        "説明の開始位置が揃っている: {:?} / {:?}",
        legend,
        key_entry
    );
}

#[test]
fn the_help_headings_leave_the_mode_name_to_the_header() {
    // ヘッダーの `▲ fujin [tri]` と重複するので、オーバーレイの見出しは
    // 節名だけにする（決定202608071906）
    let mut nav = searchable_state();
    // 検索サブモードのヘルプは操作状態から開く（決定202608131200）
    let mut search = navigating_search("");
    let mut jump = searchable_state();
    jump.handle_nav_key(key(BareKey::Char('n')));
    let mut triage = triage_state();
    set_agent_state(&mut triage, 1, AgentState::Working);
    triage.handle_nav_key(key(BareKey::Char('t')));

    for (label, state) in [
        ("nav", &mut nav),
        ("search", &mut search),
        ("jump", &mut jump),
        ("triage", &mut triage),
    ] {
        state.handle_nav_key(key(BareKey::Char('?')));
        let lines = overlay_lines(state, SIDEBAR);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("  keys"),
            "{}",
            label
        );
        for line in &lines {
            assert!(
                !line.contains('['),
                "{}: モード名が残っている: {}",
                label,
                line
            );
        }
    }
}

#[test]
fn the_key_column_fits_the_widest_key_of_the_mode() {
    // 17セル固定をやめ、モードごとの実測最大＋空白2にした（決定202608071906）。
    // いちばん長いキーのためだけに全行が空白を払う状態を解消する
    let mut nav = state_with_panes(2);
    nav.nav_mode = true;
    nav.handle_nav_key(key(BareKey::Char('?')));
    let nav_lines = overlay_lines(&nav, SIDEBAR);
    let widest = nav_lines
        .iter()
        .find(|line| line.ends_with("jump & exit"))
        .expect("いちばん長いキーの行がある");
    // 左マージン2 + "enter"(5) + 空白2
    assert_eq!(column_at(widest, "jump & exit"), 9);

    // キーが長いモードでは列も広がる（`backspace` が最長）
    let mut search = navigating_search("");
    search.handle_nav_key(key(BareKey::Char('?')));
    let search_lines = overlay_lines(&search, SIDEBAR);
    let delete = search_lines
        .iter()
        .find(|line| line.ends_with("delete char"))
        .expect("backspace の行がある");
    assert_eq!(column_at(delete, "delete char"), 2 + 9 + 2);
}

#[test]
fn the_no_agent_legend_carries_no_decoration() {
    // 他の凡例行はアイコンに状態色が乗るが、この行は状態ではないので何も乗せない。
    // dim も掛けない — 掛けると zellij が dim の解除に出す `\e[22m` が端末側で
    // bold まで消し、説明文だけ他の行と太さが揃わなくなる（実測。
    // .docs/dev/implementation-notes.md）
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let overlay: Vec<Line> = state
        .screen_rows(40)
        .iter()
        .filter_map(|row| match row {
            Row::Help(help) => Some(state.help_line(help, SIDEBAR)),
            _ => None,
        })
        .collect();
    let line = overlay
        .iter()
        .find(|text| text.content().ends_with(NO_AGENT_LABEL))
        .expect("未起動の凡例がある");
    assert!(ink_at(line, DIM_LEVEL).is_empty(), "{}", line.content());
    assert!(ink_at(line, UNBOLD_LEVEL).is_empty(), "{}", line.content());
    for level in [0, 1, 2, 3, ERROR_LEVEL] {
        assert!(
            ink_at(line, level).is_empty(),
            "凡例のアイコンに状態色は乗せない（レベル{}）: {}",
            level,
            line.content()
        );
    }
}

#[test]
fn the_status_legend_carries_the_state_colors() {
    // 凡例の目的は意味と色を結びつけることなので、アイコンにだけは状態色を乗せる
    //（キーは常にレベル2固定、というヘルプの色役割の例外・決定202608062201）
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let overlay: Vec<Line> = state
        .screen_rows(40)
        .iter()
        .filter_map(|row| match row {
            Row::Help(help) => Some(state.help_line(help, SIDEBAR)),
            _ => None,
        })
        .collect();
    for agent_state in AgentState::ALL {
        let line = overlay
            .iter()
            .find(|text| text.content().ends_with(agent_state.label()))
            .unwrap_or_else(|| panic!("{} の凡例がある", agent_state.label()));
        // 色が乗るのは左マージン(2)の直後、アイコン1文字だけ
        assert_eq!(
            ink_at(line, agent_state.color()),
            vec![2],
            "{} のアイコンに状態色が乗る",
            agent_state.label()
        );
    }
}

#[test]
fn every_agent_state_gets_a_color_of_its_own() {
    // 要件: agent-status-icon.feature「状態ごとに異なる色で判別できる」。
    // `error` が `blocked` とレベル3で重複していたのを解消した（決定202608062201）
    let levels: Vec<usize> = AgentState::ALL.iter().map(|s| s.color()).collect();
    let mut unique = levels.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), levels.len(), "色が重複している: {:?}", levels);
    assert_eq!(
        AgentState::Error.color(),
        6,
        "error はテーマのエラー色（レベル6）"
    );
}

#[test]
fn the_status_legend_shows_up_in_every_overlay() {
    // ツリーにもトリアージ一覧にも状態アイコンが出るので、どのモードの
    // オーバーレイからでも同じ表を引けるようにする
    let mut state = triage_state();
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('t')));
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(
        overlay_lines(&state, SIDEBAR)
            .iter()
            .any(|line| line.trim() == "status"),
        "トリアージモード"
    );

    let mut state = navigating_search("");
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(
        overlay_lines(&state, SIDEBAR)
            .iter()
            .any(|line| line.trim() == "status"),
        "検索サブモード"
    );

    let mut state = jump_state(3);
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(
        overlay_lines(&state, SIDEBAR)
            .iter()
            .any(|line| line.trim() == "status"),
        "番号ジャンプサブモード"
    );
}

#[test]
fn a_short_sidebar_marks_the_hidden_help_lines() {
    // オーバーレイは「任意のキーで閉じる」のでスクロール用のキーを持てない。
    // 収まらないぶんはツリーと同じあふれマーカーで、隠れていることだけ示す
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    state.render(16, SIDEBAR);

    let markers = overflow_markers(&state, 16);
    assert_eq!(markers.len(), 1, "下端にだけ出る: {:?}", markers);
    assert!(!markers[0].1, "上には隠れない（先頭から出す）");
}

#[test]
fn the_help_entries_line_up_under_a_left_margin() {
    let state = state_with_panes(2);
    // 左端に貼り付けず余白を空ける。説明の開始位置は行をまたいで揃える
    let lines: Vec<String> = state
        .help_lines()
        .iter()
        .map(|row| state.help_line(row, 32).content().to_string())
        .collect();
    let entries: Vec<&String> = lines.iter().filter(|l| l.contains("  ")).collect();
    assert!(!entries.is_empty());
    for line in &lines {
        if line.is_empty() {
            continue;
        }
        assert!(line.starts_with("  "), "左マージンが無い: {}", line);
    }
    let jump = lines
        .iter()
        .find(|l| l.contains("jump & exit"))
        .expect("ジャンプの行がある");
    let search = lines.iter().find(|l| l.contains("search")).unwrap();
    assert_eq!(
        jump.find("jump & exit"),
        search.find("search"),
        "説明の開始位置が揃っている: {:?} / {:?}",
        jump,
        search
    );
}

#[test]
fn any_key_closes_the_help_overlay_without_acting_on_it() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    state.handle_nav_key(key(BareKey::Char('j')));

    assert!(!state.help_overlay);
    assert!(state.nav_mode, "閉じてもnavモードは継続する");
    assert_eq!(state.selected, 0, "閉じるためのキーは操作として解釈しない");
}

#[test]
fn modified_keys_only_close_the_help_overlay() {
    // 安全弁（決定202607310311）より手前で閉じる。閲覧をやめただけで退場させるのは筋が通らない
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier());

    assert!(!state.help_overlay);
    assert!(state.nav_mode);
}

#[test]
fn the_help_overlay_opens_from_the_search_submode_too() {
    // 開けるのは操作状態から（編集状態の `?` はクエリの文字。決定202608131200）
    let mut state = navigating_search("alp");
    state.handle_nav_key(key(BareKey::Char('?')));

    assert!(state.help_overlay);
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("alp"),
        "? はクエリに入らない"
    );
    let lines = overlay_lines(&state, SIDEBAR);
    assert_eq!(lines.first().map(String::as_str), Some("  keys"));
    assert!(
        lines.iter().any(|line| line.contains("filter panes")),
        "検索サブモードのキーを出す: {:?}",
        lines
    );
    assert!(
        lines.iter().any(|line| line.contains("edit query")),
        "編集状態へ戻るキーも出す: {:?}",
        lines
    );

    // 閉じたら開く前の表示（検索サブモードの操作状態）に戻る。Esc も閉じる
    // だけで、検索サブモードの取り消しにはならない
    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.help_overlay);
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("alp"),
        "検索サブモードは中断されない"
    );
    assert_eq!(search_phase(&state), Some(SearchPhase::Navigating));
}

#[test]
fn leaving_nav_mode_closes_the_help_overlay() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.help_overlay = true;
    state.leave_nav_mode();

    assert!(
        !state.help_overlay,
        "開いたまま退場するとツリー表示へ戻れない"
    );
}
