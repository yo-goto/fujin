// 単体テスト。
//
// 実行はホストターゲットで行う（`make test`）。既定ターゲットの wasm32-wasip1 では
// テストバイナリを走らせるランタイムがないため。テストできる範囲（ホスト関数の
// スタブと、呼べない問い合わせ系）の説明と、複数のセクションから使うヘルパ・定数は
// `src/test_support.rs` に集約してある。

mod agent_status;
mod click_to_focus;
mod command_status;
mod floating_pane_indicator;
mod focus_sync;
mod ime_input;
mod instance_sync;
mod nav_mode;
mod pane_close_kill;
mod pane_number_jump;
mod pane_termination_multi_select;
mod pipe_protocol;
mod preview;
mod search_explorer;
mod sidebar_tree;
mod sidebar_width;
mod summon;
mod triage_mode;

use crate::agent::AgentState;
use crate::config::Kind;
use crate::config::SETTINGS;
use crate::deploy::TROOP;
use crate::nav::SearchPhase;
use crate::render::cwd_row;
use crate::render::overflow_row;
use crate::render::CounterColumn;
use crate::render::HeadCells;
use crate::render::Row;
use crate::render::NO_AGENT_ICON;
use crate::render::NO_AGENT_LABEL;
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
        "  alt+up:up  alt+down:down"
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
    // 地の文の疑似カーソルを廃止。経緯: docs/issues/search-input-cursor-shape.md）
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
    // 2026-08-13。経緯: docs/issues/search-input-cursor-shape.md）ので、状態を
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
    // ヒントと同じ削り方。docs/issues/direct-keys-hint-overflow.md）
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
    // 操作ヒントが端からはみ出す（docs/concept/ui-design.md のレイアウト規則）
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
    // UI文言は英語で統一する（docs/concept/ui-design.md の「文言」）。
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
    // docs/dev/implementation-notes.md）
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));

    let overlay: Vec<Text> = state
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

    let overlay: Vec<Text> = state
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

// --- ペイン行のレイアウト（決定202608060052・決定202608060053） ---
//
// カウンタ列は右端に揃え、幅はフレーム全体で共有する。ペイン名はその残り幅に
// 収めるので、名前が長くてもサブエージェント数 `+N`・未完了タスク数 `[M]` は
// 消えない。cwd はペイン行に混ぜず、続く cwd行に出す

#[test]
fn a_pane_without_a_status_gets_the_no_agent_marker() {
    // 状態アイコン列を空白のままにすると、エージェントが乗る行と並べたときに
    // 左端が欠けて見える（docs/issues/sidebar-cwd-row-legibility.md）
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
    //（白系）に落ちていた（docs/issues/idle-icon-color-on-selection.md）
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
    // opaque のプレフィックス（`z`）が付いていることを直接見る。背景が塗られないと
    // 帯にならず、幅いっぱいへ伸ばした空白（pad_to_width）が無駄になる
    assert!(
        text.serialize().starts_with('z'),
        "opaque が落ちている: {:?}",
        text.serialize()
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
    // （docs/issues/counter-column-collateral-truncation.md）。
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
    //（docs/issues/sidebar-cwd-persists-after-exit.md）
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
// ペイン名が空のまま残る（docs/issues/pane-title-blank-on-exit.md）

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

// --- 配置演出（要件: docs/requirements/header-animation/） ---

// 兵が使える領域の実測値（幅32セル・右マージン2セル・`▲ fujin` は7セル）。
// 発進位置は本文の右端の1つ先、いちばん奥の着地列は内容幅の右端
const LAUNCH: usize = 8;
const DEEPEST: usize = CONTENT - 1;

// 新規エージェント検出から配置演出の発火までを通す。
//
// 本番では検出（`apply_status` の戻り値）と発火（`begin_deployment`）の間に
// 可視インスタンス判定（`State::is_visible_instance`）が挟まるが、これはホスト関数
// `get_focused_pane_info()` を呼ぶのでテストから通せない
//（docs/dev/build-and-test.md「テストで検証できない範囲」）。ここでは判定を通った
// 後の発火だけを見る
fn deploy_agents(state: &mut State, troops: usize) {
    state.begin_deployment(troops);
}

// いま画面に出ている兵の列
fn troop_columns(state: &State) -> Vec<usize> {
    let (launch, width) = state.troop_field(SIDEBAR);
    state
        .deployment
        .map(|deployment| deployment.columns(launch, width))
        .unwrap_or_default()
}

// 演出が終わるまでフレームを送る。返すのは要したフレーム数
fn play_out(state: &mut State) -> usize {
    for frame in 1.. {
        state.advance_deployment();
        if state.deployment.is_none() {
            return frame;
        }
        assert!(frame < 100, "演出が終わらない");
    }
    unreachable!()
}

#[test]
fn new_panes_alone_do_not_start_a_deployment() {
    // 判定材料はフック通知だけで、ペインが増えたかどうかは見ない。旧実装（増えた
    // ターミナルペインで判定）では `vim` やビルドコマンドでも演出が出ていた
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    observe_panes(&mut state, &[1, 2, 3]);

    assert!(state.deployment.is_none());
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");
}

#[test]
fn a_session_start_is_a_new_agent_detection() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);

    assert!(
        state.apply_status(status(2, "SessionStart")),
        "SessionStart は新規エージェント検出になる"
    );
}

#[test]
fn an_agent_started_in_an_existing_pane_is_detected() {
    // 空のシェルペインを先に開いておき、後から `claude` を打つ使い方（要件:
    // 前から開いてあるペインで後からエージェントを起動しても配置演出が始まる）。
    // ペインの側は何も変わらないまま通知だけが届く
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    // 一覧を何度観測しても増減が無い状態を作ってから通知を受ける
    observe_panes(&mut state, &[1, 2]);

    assert!(state.apply_status(status(2, "SessionStart")));
}

#[test]
fn a_restarted_conversation_is_not_a_new_agent() {
    // `/clear` とコンパクトは稼働中のエージェントの仕切り直しで、着任ではない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for source in ["clear", "compact"] {
        assert!(
            !state.apply_status(session_start(1, source)),
            "source={} は新規エージェント検出にしない",
            source
        );
    }
}

#[test]
fn a_fresh_session_is_a_new_agent() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for source in ["startup", "resume", "fork"] {
        assert!(
            state.apply_status(session_start(1, source)),
            "source={} は新規エージェント検出になる",
            source
        );
    }
}

#[test]
fn a_session_start_without_a_source_still_counts() {
    // `source` を送らない旧フックスクリプトのままでも演出は出る。判定を
    // ホワイトリストではなく除外方式にしてあるのはこのため
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    assert!(state.apply_status(status(1, "SessionStart")));
}

#[test]
fn other_hook_events_are_never_new_agent_detections() {
    // **リロード直後の誤検出を防いでいるのがこの性質。** プラグインをリロードすると
    // `agents` マップは空になる（docs/issues/redeploy-resets-agent-state.md）が、
    // 稼働中のエージェントから次に届くのは SessionStart 以外のイベントなので、
    // 既存エージェントが新規と誤検出されることはない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for event in [
        "UserPromptSubmit",
        "Stop",
        "StopFailure",
        "Notification",
        "SubagentStart",
        "SubagentStop",
        "TaskCreated",
        "TaskCompleted",
        "SessionEnd",
    ] {
        assert!(
            !state.apply_status(status(1, event)),
            "{} は新規エージェント検出にしない",
            event
        );
    }
}

#[test]
fn a_new_agent_starts_the_deployment_animation() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    deploy_agents(&mut state, 1);

    assert_eq!(state.deployment.map(|d| d.troops), Some(1));
    // 兵はブランド行の `fujin` の右側から現れる
    assert_eq!(troop_columns(&state), vec![LAUNCH]);
    let header = state.header_line(SIDEBAR).content().to_string();
    assert!(header.starts_with("▲ fujin "), "{}", header);
    assert_eq!(
        header.chars().position(|c| c.to_string() == TROOP),
        Some(LAUNCH)
    );
}

#[test]
fn every_detected_agent_gets_a_troop() {
    for detected in [1usize, 3, 6] {
        let mut state = sidebar_state();
        observe_panes(&mut state, &[1]);
        deploy_agents(&mut state, detected);

        assert_eq!(
            state.deployment.map(|d| d.troops),
            Some(detected),
            "{}体の検出",
            detected
        );
    }
}

#[test]
fn show_deploy_animation_can_switch_the_animation_off() {
    // 演出は情報を運ばないので、切っても見える情報は変わらない（決定202608080346）
    let mut state = sidebar_state();
    state.apply_config(&plugin_config(&[("show_deploy_animation", "false")]));
    observe_panes(&mut state, &[1, 2]);

    // 新規エージェントの検出そのものは、切っている間も動く
    //（要件: But 新規エージェントの検出そのものは行われる）
    assert!(state.apply_status(status(2, "SessionStart")));
    deploy_agents(&mut state, 1);
    assert!(state.deployment.is_none(), "配置演出は再生されない");
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");

    // 戻せば次の検出から再生される
    state.apply_config(&plugin_config(&[("show_deploy_animation", "true")]));
    deploy_agents(&mut state, 1);
    assert_eq!(state.deployment.map(|d| d.troops), Some(1));
}

#[test]
fn detections_in_the_same_window_join_one_deployment() {
    // 新規タブ作成のように一括で着任するときは、検出が複数回に割れて届く。
    // 検出のたびに発火させると演出が重なって騒がしくなる
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 1);
    state.advance_deployment();
    deploy_agents(&mut state, 2);

    let deployment = state.deployment.expect("演出は続いている");
    assert_eq!(deployment.troops, 3, "検出した数の合計ぶんの兵が出る");
    assert_eq!(deployment.frame, 1, "演出は最初から巻き直さない");
}

#[test]
fn the_troops_line_up_with_the_first_launched_deepest() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 3);

    // 先に発進した兵ほど奥へ着く。着地列は右端から2セル間隔
    let mut landed = Vec::new();
    for _ in 0..20 {
        state.advance_deployment();
        let columns = troop_columns(&state);
        if columns.len() == 3 && columns.iter().all(|c| *c >= DEEPEST - 4) {
            landed = columns;
            break;
        }
    }
    assert_eq!(landed, vec![DEEPEST - 4, DEEPEST - 2, DEEPEST]);

    // 静止したあとは動かない
    state.advance_deployment();
    assert_eq!(troop_columns(&state), landed);
}

#[test]
fn the_troops_stay_clear_of_the_brand() {
    // 幅が足りないぶんの兵は着地列を確保できない。ブランド行に重ねるくらいなら
    // 出さない（着地列は発進位置より左には作らない）
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 39);

    // 出せるだけ出た瞬間（＝いちばん多く並んだフレーム）を見る
    let mut columns = Vec::new();
    while state.deployment.is_some() {
        state.advance_deployment();
        let frame = troop_columns(&state);
        if frame.len() > columns.len() {
            columns = frame;
        }
    }
    assert!(columns.iter().all(|c| *c >= LAUNCH), "{:?}", columns);
    assert_eq!(columns.first(), Some(&LAUNCH));
    assert_eq!(columns.last(), Some(&DEEPEST));
}

#[test]
fn the_header_returns_to_normal_when_the_deployment_ends() {
    // 着地点は完全に元へ戻る。稼働数のような情報は残さない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 2);
    assert!(state.header_line(SIDEBAR).content().contains(TROOP));

    let frames = play_out(&mut state);
    assert!(
        frames > 3,
        "整列した状態を見せる間もなく畳んでいる: {}",
        frames
    );
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");
    // 取り残されたタイマーが来ても何も起きない
    assert!(!state.advance_deployment());
}

// 「サイドバーが表示されていない間の検出では演出は再生されない」「見逃した検出は
// 後から遡って演出されない」の2要件は、可視インスタンス判定（`is_visible_instance`）
// が担っている。ホスト関数 `get_focused_pane_info()` を呼ぶためユニットテストからは
// 通せない（docs/dev/build-and-test.md「テストで検証できない範囲」）ので、実機での
// 手動確認に頼る。

#[test]
fn the_deployment_leaves_the_mode_label_readable() {
    // navモード中は `[nav]` のぶんだけ発進位置が右へずれる。兵がラベルに
    // 重なるとどちらも読めなくなる
    let mut state = sidebar_state();
    state.nav_mode = true;
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 1);

    let header = state.header_line(SIDEBAR).content().to_string();
    assert!(header.starts_with("▲ fujin  [nav] "), "{}", header);
    assert_eq!(
        troop_columns(&state),
        vec!["▲ fujin  [nav]".chars().count() + 1]
    );
}

#[test]
fn render_survives_the_deployment() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2, 3]);
    deploy_agents(&mut state, 2);
    state.render(40, SIDEBAR);
    state.render(40, 12);
    state.render(3, 2);
    state.render(0, 0);
    // 幅0で描いたあともフレーム送りは止まらない
    state.advance_deployment();
}
