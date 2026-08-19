// 単体テスト。
//
// 実行はホストターゲットで行う（`make test`）。既定ターゲットの wasm32-wasip1 では
// テストバイナリを走らせるランタイムがないため。テストできる範囲（ホスト関数の
// スタブと、呼べない問い合わせ系）の説明と、複数のセクションから使うヘルパ・定数は
// `src/test_support.rs` に集約してある。

mod agent_status;
mod click_to_focus;
mod floating_pane_indicator;
mod focus_sync;
mod ime_input;
mod instance_sync;
mod nav_mode;
mod pane_close_kill;
mod pane_number_jump;
mod pipe_protocol;
mod preview;
mod sidebar_tree;
mod sidebar_width;
mod summon;

use crate::agent::AgentState;
use crate::command::CommandState;
use crate::command::PaneStatus;
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
use crate::termination::Termination;
use crate::test_support::*;
use crate::*;

// --- 複数選択（マーク。決定202608080250、要件:
// docs/requirements/pane-termination-multi-select/） ---
//
// 一括操作の対象として選んだペインの集合。単一のナビゲーションカーソルである
// 選択（`State::selected`）とは別概念で、タブをまたぎ、navモードを退場しても残る

// マークの入場キー（トグル）と全解除キー
fn mark_key() -> KeyWithModifier {
    key(BareKey::Char('m'))
}

fn clear_marks_key() -> KeyWithModifier {
    key(BareKey::Char('M'))
}

// マーク済みペインをツリー順に並べたもの（集合そのものの比較用）
fn marks(state: &State) -> Vec<u32> {
    state.marked_in_tree_order()
}

#[test]
fn the_mark_key_toggles_the_pane_under_the_selection() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;

    state.handle_nav_key(mark_key());
    assert_eq!(marks(&state), vec![1]);
    // 同じキーでマークが外れる
    state.handle_nav_key(mark_key());
    assert!(marks(&state).is_empty());
}

#[test]
fn marks_pile_up_row_by_row_across_tabs() {
    // マークはタブをまたいでよい（決定202608080250。selectable が元々タブ横断のため）
    let mut state = searchable_state(); // タブ0に1・2、タブ1に3
    state.handle_nav_key(mark_key());
    state.handle_nav_key(key(BareKey::Char('G'))); // 別タブの末尾へ
    state.handle_nav_key(mark_key());

    assert_eq!(marks(&state), vec![1, 3]);
}

#[test]
fn the_clear_key_drops_every_mark() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(mark_key());
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(mark_key());
    assert_eq!(marks(&state), vec![1, 2]);

    state.handle_nav_key(clear_marks_key());
    assert!(marks(&state).is_empty());
    assert!(state.nav_mode, "全解除はnavモードを抜けない");
}

#[test]
fn marks_survive_leaving_nav_mode() {
    // 検索カーソル・トリアージカーソルと違い、タブをまたいで持ち回る前提の状態
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(mark_key());

    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.nav_mode);
    assert_eq!(marks(&state), vec![1]);
    assert!(
        state.mark_column(&state.visible_rows()),
        "印はnavモードの外でも出し続ける"
    );
}

#[test]
fn a_pane_that_leaves_the_list_loses_its_mark() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(mark_key());
    state.handle_nav_key(key(BareKey::Char('j')));
    state.handle_nav_key(mark_key());

    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(2, "pane2"), terminal_pane(3, "pane3")],
    )]));
    state.rebuild_selectable();

    assert_eq!(marks(&state), vec![2], "閉じたペイン1のマークは残さない");
}

#[test]
fn the_triage_list_marks_the_row_under_the_cursor() {
    let mut state = triage_state();
    set_agent_state(&mut state, 3, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('t')));
    assert_eq!(state.triage_cursor(), Some(3));

    state.handle_nav_key(mark_key());
    assert_eq!(marks(&state), vec![3]);
    assert!(state.triage.is_some(), "マークでトリアージ一覧を抜けない");
}

#[test]
fn the_filtered_results_mark_with_the_alt_key() {
    // 検索サブモードは印字可能文字をすべてクエリに使うので、マークだけ Alt付き。
    // 素の `m` はクエリの文字になる（マークにはならない）
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "cha");
    assert_eq!(state.search.as_ref().and_then(|s| s.cursor), Some(3));

    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('m')).with_alt_modifier());
    assert_eq!(marks(&state), vec![3]);
    assert!(state.search.is_some(), "マークで検索サブモードを抜けない");

    state.handle_nav_key(mark_key());
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("cham"),
        "素の `m` はクエリの文字"
    );
}

#[test]
fn the_mark_key_is_undefined_in_the_number_jump_submode() {
    // 数字専用の入力空間の安全弁がそのまま効く（特別扱いのコードは足さない）
    let mut state = jump_state(3);
    state.handle_nav_key(mark_key());

    assert!(!state.nav_mode, "未定義キーとして navモードごと退場する");
    assert!(marks(&state).is_empty());
}

#[test]
fn the_mark_column_shows_up_only_when_something_is_marked() {
    let mut state = state_with_panes(2);
    let column = CounterColumn::default();
    assert!(
        !state.mark_column(&state.visible_rows()),
        "マークが無いフレームには列を出さない"
    );

    state.nav_mode = true;
    state.handle_nav_key(mark_key());
    assert!(state.mark_column(&state.visible_rows()));

    let marked = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column,
        HeadCells {
            mark: Some(true),
            ..HeadCells::default()
        },
        SIDEBAR,
    );
    assert!(
        marked.content().starts_with("  ✓ › pane1"),
        "マーク列はアイコンの前: {}",
        marked.content()
    );
    // 同じフレームのマークされていない行も、列ぶんを空白で空けて桁を揃える
    let plain = state.pane_row(
        &state.selectable[1],
        false,
        None,
        column,
        HeadCells {
            mark: Some(false),
            ..HeadCells::default()
        },
        SIDEBAR,
    );
    assert_eq!(
        column_at(plain.content(), "pane2"),
        column_at(marked.content(), "pane1")
    );
}

#[test]
fn a_marked_row_still_leaves_the_right_margin() {
    // マーク列ぶんペイン名の残り幅が縮む。畳み損ねると選択背景が端末側で
    // 折り返して次の行を汚す
    let mut state = state_with_one_pane("要件定義とドキュメント整理タスクの続き");
    state.marked.insert(1);
    repeat_status(&mut state, 1, "SubagentStart", 2);

    let row = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells {
            mark: Some(true),
            ..HeadCells::default()
        },
        SIDEBAR,
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(row.content()),
        CONTENT,
        "{}",
        row.content()
    );
    assert!(row.content().ends_with("+2"), "{}", row.content());
}

#[test]
fn the_mark_column_sits_between_the_number_and_the_icon() {
    // 列順序は 選択バー → 番号列 → マーク列 → アイコン → ペイン名（決定202608080250）。
    // マーク自体は番号ジャンプサブモードへ入る前に立てたもの
    let mut state = jump_state(12);
    state.marked.insert(1);

    let cell = state.jump_number(0);
    let cell = cell.as_ref().map(|(n, m)| (n.as_str(), *m));
    let row = state.pane_row(
        &state.selectable[0],
        false,
        None,
        CounterColumn::default(),
        HeadCells {
            number: cell,
            mark: Some(true),
        },
        SIDEBAR,
    );
    assert!(
        row.content().starts_with("  01 ✓ › pane1"),
        "{}",
        row.content()
    );
    // 状態アイコンの色位置がマーク列ぶんずれても、番号はレベル2のまま
    assert_eq!(ink_at(&row, 2), vec![2, 3, 5]);
}

#[test]
fn marked_triage_rows_wear_the_mark_too() {
    // トリアージ一覧の上でもマークできる以上、印も同じ位置に出す
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('t')));
    state.handle_nav_key(mark_key());

    let rows = state.visible_rows();
    assert!(state.mark_column(&rows));
    let entry = state
        .selectable
        .iter()
        .find(|e| e.pane_id == 2)
        .expect("マークしたペイン");
    let row = state.triage_row(entry, "tab1", false, 4, Some(true), SIDEBAR);
    assert!(row.content().starts_with("  ✓ ×"), "{}", row.content());
}

// --- マークと終了操作の連携（決定202608080250） ---

#[test]
fn the_termination_takes_the_marks_when_there_are_any() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('j'))); // 選択は2へ
    state.marked.extend([3, 1]);

    state.handle_nav_key(key(BareKey::Char('d')));
    assert_eq!(
        state.termination_plan(Termination::Kill),
        vec![(1, true, false), (3, true, false)],
        "対象はマーク集合で、順序はツリー順"
    );
}

#[test]
fn without_marks_the_termination_falls_back_to_the_selection() {
    // 単一版のフローはマーク0件の特殊ケースとして包含する（決定202608080250）
    let state = termination_state(3);
    assert_eq!(
        state.termination_plan(Termination::Close),
        vec![(1, false, true)]
    );
    assert_eq!(state.termination_marked_count(), 0);
}

#[test]
fn the_marked_targets_are_pinned_at_entry() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.marked.insert(1);
    state.handle_nav_key(key(BareKey::Char('d')));

    // 確認の途中でマークが変わっても（兄弟インスタンスからの配布など）
    // 対象は入場時のまま
    state.apply_marks("2,3");
    assert_eq!(
        state.termination_plan(Termination::Close),
        vec![(1, false, true)]
    );
}

#[test]
fn a_marked_pane_closed_during_the_prompt_is_left_alone() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.marked.extend([1, 2]);
    state.handle_nav_key(key(BareKey::Char('d')));

    state.panes = Some(manifest(vec![(0, vec![terminal_pane(2, "pane2")])]));
    state.rebuild_selectable();

    assert_eq!(
        state.termination_plan(Termination::Close),
        vec![(2, false, true)],
        "消えたペインには何も送らず、残りには送る"
    );
}

#[test]
fn running_a_termination_clears_every_mark() {
    // 成功・no-op を問わずクリアする（決定202608080250）
    for pressed in ['c', 'k', 'x'] {
        let mut state = state_with_panes(3);
        state.nav_mode = true;
        state.marked.extend([1, 2]);
        state.handle_nav_key(key(BareKey::Char('d')));
        state.handle_nav_key(key(BareKey::Char(pressed)));

        assert!(marks(&state).is_empty(), "{} のあと", pressed);
        assert!(state.termination.is_none());
    }
}

#[test]
fn cancelling_a_termination_keeps_the_marks() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.marked.extend([1, 2]);
    state.handle_nav_key(key(BareKey::Char('d')));
    state.handle_nav_key(key(BareKey::Esc));

    assert!(state.termination.is_none());
    assert_eq!(marks(&state), vec![1, 2]);
}

#[test]
fn the_prompt_counts_the_marked_panes() {
    // マーク1件以上のときだけ件数を前置する。幅28セルに3項目とも入らないので
    // キーだけの2段目に落ちるが、件数は残す（決定202608080250）
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.marked.extend([1, 2]);
    state.handle_nav_key(key(BareKey::Char('d')));
    assert_eq!(state.footer_line(SIDEBAR).content(), "  2 panes  c k x");

    let mut single = state_with_panes(3);
    single.nav_mode = true;
    single.marked.insert(2);
    single.handle_nav_key(key(BareKey::Char('d')));
    assert_eq!(single.footer_line(SIDEBAR).content(), "  1 pane  c k x");
}

// --- マークの同期（決定202608080250・決定202608012141） ---

#[test]
fn marks_travel_to_siblings_as_pane_ids() {
    let mut state = state_with_panes(3);
    state.marked.extend([1, 3]);
    assert_eq!(state.mark_dump(), "1,3");

    let mut sibling = state_with_panes(3);
    assert!(sibling.apply_marks(&state.mark_dump()));
    assert_eq!(marks(&sibling), vec![1, 3]);
    // 同じ集合を配られても再描画は要らない
    assert!(!sibling.apply_marks(&state.mark_dump()));
}

#[test]
fn an_empty_payload_clears_the_marks_on_siblings() {
    let mut state = state_with_panes(3);
    state.marked.insert(2);
    state.clear_marks();

    let mut sibling = state_with_panes(3);
    sibling.marked.insert(2);
    assert!(sibling.apply_marks(&state.mark_dump()));
    assert!(marks(&sibling).is_empty());
}

#[test]
fn a_sibling_keeps_marks_for_panes_it_has_not_seen_yet() {
    // 非可視インスタンスの一覧は古い（PaneUpdate が届かない）。受け取った時点で
    // 絞り込むと、配ったそばからマークが消える。掃除は一覧を持つ側の責務
    let mut sibling = state_with_panes(1);
    assert!(sibling.apply_marks("1,9"));
    assert!(sibling.is_marked(9));
}

#[test]
fn the_nav_help_advertises_the_mark_keys() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    assert!(lines.contains("mark / clear all"), "{}", lines);
}

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

// --- 検索サブモード（要件: docs/requirements/search-explorer/） ---
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

// --- トリアージモード（要件: docs/requirements/triage-mode/） ---
//
// navモードの内側で `p` から入る、エージェント状態の緊急度順のフラット一覧。
// ツリー表示の並び順（決定202607302256）には手を触れず、切り替えて使う

fn triage_ids(state: &State) -> Vec<u32> {
    state.triage_entries().iter().map(|e| e.pane_id).collect()
}

#[test]
fn triage_lists_only_panes_that_need_attention() {
    let mut state = triage_state();
    // 1: 通知を受けていない（状態を持たない）/ 2: idle（既読）/ 3: blocked
    set_agent_state(&mut state, 2, AgentState::Idle);
    set_agent_state(&mut state, 3, AgentState::Blocked);

    assert_eq!(
        triage_ids(&state),
        vec![3],
        "状態を持たないペインと idle は一覧に出さない"
    );
}

#[test]
fn triage_orders_by_priority_tier() {
    let mut state = triage_state();
    // 投入順は優先度と無関係にしておく（並び替えが効いていることを見る）
    set_agent_state(&mut state, 1, AgentState::Done);
    set_agent_state(&mut state, 2, AgentState::Working);
    set_agent_state(&mut state, 3, AgentState::Error);
    set_agent_state(&mut state, 4, AgentState::Blocked);

    assert_eq!(
        triage_ids(&state),
        vec![3, 4, 2, 1],
        "error → blocked → working → done"
    );
}

#[test]
fn triage_spans_tabs() {
    let mut state = triage_state();
    set_agent_state(&mut state, 4, AgentState::Working); // タブ1
    set_agent_state(&mut state, 1, AgentState::Working); // タブ0

    // タブの壁を無視して並ぶ。タブ1のペインのほうが先に変化しているので下
    assert_eq!(triage_ids(&state), vec![1, 4]);
}

#[test]
fn triage_breaks_ties_by_the_latest_state_change() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Working);
    set_agent_state(&mut state, 3, AgentState::Working);
    assert_eq!(triage_ids(&state), vec![3, 2, 1], "新しく変わったものが上");

    // 1 が blocked を経て working に戻ると、同一階層内でいちばん新しくなる
    set_agent_state(&mut state, 1, AgentState::Blocked);
    set_agent_state(&mut state, 1, AgentState::Working);
    assert_eq!(triage_ids(&state), vec![1, 3, 2]);
}

#[test]
fn the_sequence_only_advances_on_a_state_change() {
    // カウンタだけが動くイベントで番号を進めると、同一階層内の並びが
    // 「直近の状態変化順」でなくなる
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Working);
    let before = state.agents[&1].state_change_seq;

    state.apply_status(status(1, "TaskCreated"));
    state.apply_status(status(1, "SubagentStart"));

    assert_eq!(state.agents[&1].state_change_seq, before);
    assert_eq!(triage_ids(&state), vec![2, 1], "並びも変わらない");
}

#[test]
fn the_triage_list_follows_state_changes() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));
    assert_eq!(triage_ids(&state), vec![2, 1]);

    // 表示中に別のペインが blocked になったら、次の描画で上に来る
    set_agent_state(&mut state, 1, AgentState::Blocked);
    assert_eq!(triage_ids(&state), vec![1, 2]);
}

#[test]
fn t_switches_the_sidebar_to_the_triage_list() {
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Blocked);
    state.handle_nav_key(key(BareKey::Char('t')));

    assert!(state.triage.is_some());
    assert!(state.nav_mode, "トリアージモードはnavモードの内側");
    let rows = state.visible_rows();
    assert!(
        !rows
            .iter()
            .any(|r| matches!(r, Row::Tab(_) | Row::Pane { .. })),
        "ツリー表示は隠れる"
    );
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(r, Row::Triage { .. }))
            .count(),
        1,
        "一覧はエージェント状態を持つペインだけ"
    );
}

#[test]
fn esc_returns_to_the_tree_view_and_stays_in_nav_mode() {
    let mut state = triage_state();
    set_agent_state(&mut state, 3, AgentState::Blocked);
    state.selected = 1; // bravo を選択した状態で入る
    state.handle_nav_key(key(BareKey::Char('t')));
    state.handle_nav_key(key(BareKey::Esc));

    assert!(state.triage.is_none());
    assert!(state.nav_mode, "navモードは継続している");
    assert_eq!(state.selected, 1, "入る前の選択に戻す");
    assert!(
        state
            .visible_rows()
            .iter()
            .any(|r| matches!(r, Row::Tab(_))),
        "ツリー表示に戻る"
    );
}

#[test]
fn enter_jumps_from_the_triage_list_and_leaves_nav_mode() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 4, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('t')));
    // カーソルは一覧の先頭（error のタブ1のペイン）
    state.handle_nav_key(key(BareKey::Enter));

    assert_eq!(state.selectable[state.selected].pane_id, 4);
    assert!(!state.nav_mode, "ジャンプはnavモードの退場を伴う");
    assert!(state.triage.is_none());
}

#[test]
fn a_triage_jump_clears_the_read_state_through_the_usual_path() {
    // 既読クリアは「PaneUpdate でのフォーカス変化を見る」汎用の仕組みに乗せる。
    // トリアージモード専用のクリア処理を別に書くと、決定202608012141が踏んだ配り漏れの
    // 罠を再発明することになる（要件: triage-mode-entry-exit.feature）
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Blocked);
    state.handle_nav_key(key(BareKey::Char('t')));
    state.handle_nav_key(key(BareKey::Enter));
    assert_eq!(state.selectable[state.selected].pane_id, 2);

    // ジャンプでフォーカスが移った結果が PaneUpdate として返ってくる
    let focused = PaneInfo {
        is_focused: true,
        ..terminal_pane(2, "bravo")
    };
    state.apply_read_model(&manifest(vec![(
        0,
        vec![
            terminal_pane(1, "alpha"),
            focused,
            terminal_pane(3, "charlie"),
        ],
    )]));
    settle_read(&mut state);
    assert_eq!(
        state.agents[&2].state,
        AgentState::Idle,
        "ジャンプ先は既読になる"
    );
}

#[test]
fn the_triage_cursor_moves_within_the_list() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Working);
    set_agent_state(&mut state, 3, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));
    // 一覧は [3, 2, 1]
    assert_eq!(state.triage_cursor(), Some(3), "カーソルは先頭から");

    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.triage_cursor(), Some(2));
    state.handle_nav_key(key(BareKey::Down));
    assert_eq!(state.triage_cursor(), Some(1));
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.triage_cursor(), Some(1), "末尾で止まる");

    state.handle_nav_key(key(BareKey::Char('k')));
    assert_eq!(state.triage_cursor(), Some(2));
    state.handle_nav_key(key(BareKey::Char('G')).with_shift_modifier());
    assert_eq!(state.triage_cursor(), Some(1));
    state.handle_nav_key(key(BareKey::Char('g')));
    assert_eq!(state.triage_cursor(), Some(3));
    assert!(state.nav_mode, "移動キーではモードを抜けない");
    assert!(state.triage.is_some());
}

#[test]
fn the_triage_cursor_starts_at_the_most_urgent_row() {
    // 入る前の選択は継がない。トリアージが答えるのは「今どれに手を入れるか」で、
    // 選択中のペインはたいてい今まさに自分が作業している側
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 2, AgentState::Error);
    state.selected = 0; // alpha（working。一覧では2番目）
    state.handle_nav_key(key(BareKey::Char('t')));
    assert_eq!(state.triage_cursor(), Some(2));
}

#[test]
fn the_triage_cursor_does_not_move_the_selection() {
    // カーソルの移動は兄弟インスタンスへ配らない（要件: triage-cursor.feature）。
    // 配布そのものはホスト関数なのでテストから覗けないため、配る材料である
    // 選択（`selected`）が動かないことで押さえる。動くのは Enter の確定時だけ
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 3, AgentState::Working);
    state.selected = 1; // bravo（一覧には出ないペイン）
    state.handle_nav_key(key(BareKey::Char('t')));

    // 一覧は [3, 1]
    assert_eq!(state.triage_cursor(), Some(3));
    assert_eq!(state.selected, 1, "入場では選択を動かさない");
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(state.triage_cursor(), Some(1));
    assert_eq!(state.selected, 1, "カーソルの移動では選択を動かさない");

    state.handle_nav_key(key(BareKey::Enter));
    assert_eq!(
        state.selectable[state.selected].pane_id, 1,
        "確定したときだけ選択が動く"
    );
}

#[test]
fn the_triage_cursor_falls_back_when_its_pane_leaves_the_list() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Blocked);
    set_agent_state(&mut state, 2, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));
    assert_eq!(state.triage_cursor(), Some(1));

    // 既読になって一覧から消えたら、残った先頭へ寄せる
    state.agents.get_mut(&1).unwrap().mark_read();
    assert_eq!(state.triage_cursor(), Some(2));

    // 一覧が空になれば None（Enter は何も起こさない）
    state.agents.get_mut(&2).unwrap().state = AgentState::Idle;
    assert_eq!(state.triage_cursor(), None);
    state.handle_nav_key(key(BareKey::Enter));
    assert!(state.nav_mode, "空の一覧での Enter はモードに留まる");
    assert!(state.triage.is_some());
}

#[test]
fn triage_leaves_nav_mode_on_undefined_keys() {
    // 安全弁（決定202607310311）はサブモードでも最上位まで効かせる
    for k in [
        key(BareKey::Char('z')),
        key(BareKey::Char('q')),
        key(BareKey::Char('j')).with_ctrl_modifier(),
        // alt+p（プレビュー）は検索サブモード限定の例外で、トリアージ一覧では効かない
        key(BareKey::Char('p')).with_alt_modifier(),
    ] {
        let mut state = triage_state();
        set_agent_state(&mut state, 1, AgentState::Working);
        state.handle_nav_key(key(BareKey::Char('t')));
        state.handle_nav_key(k.clone());
        assert!(!state.nav_mode, "{:?} でnavモードごと抜けるべき", k);
        assert!(state.triage.is_none());
    }
}

#[test]
fn an_empty_triage_list_says_so() {
    let mut state = triage_state();
    state.handle_nav_key(key(BareKey::Char('t')));
    let rows = state.visible_rows();
    let Some(Row::Notice(notice)) = rows.get(HEADER_ROWS) else {
        panic!("空リストのままだと壊れて見える: {}", rows.len());
    };
    assert_eq!(*notice, "nothing to triage");
}

#[test]
fn triage_rows_carry_the_pane_name_and_its_tab_name() {
    let mut state = triage_state();
    set_agent_state(&mut state, 4, AgentState::Blocked); // タブ1の delta
    state.handle_nav_key(key(BareKey::Char('t')));

    let rows = state.visible_rows();
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い: {}", rows.len());
    };
    let text = state.triage_row(entry, tab_name, false, tab_column, None, SIDEBAR);
    let content = text.content();

    assert!(content.contains("delta"), "ペイン名: {}", content);
    assert!(
        content.ends_with("tab2"),
        "所属タブ名を右端に併記: {}",
        content
    );
    assert!(
        content.starts_with("  ◆ "),
        "状態アイコンは通常表示と同じ: {}",
        content
    );
    assert!(
        unicode_width::UnicodeWidthStr::width(content) <= CONTENT,
        "右端にはマージンを空ける: {}",
        content
    );
}

#[test]
fn a_long_pane_name_does_not_push_the_tab_name_off_the_row() {
    let mut state = triage_state();
    state.panes = Some(manifest(vec![(
        0,
        vec![terminal_pane(1, "要件定義とドキュメント整理タスクの続き")],
    )]));
    state.rebuild_selectable();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));

    let rows = state.visible_rows();
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い");
    };
    let content = state
        .triage_row(entry, tab_name, false, tab_column, None, SIDEBAR)
        .content()
        .to_string();
    assert!(content.ends_with("tab1"), "タブ名は残す: {}", content);
    assert!(content.contains('…'), "畳むのはペイン名側: {}", content);
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(content.as_str()),
        CONTENT,
        "右端は揃える: {}",
        content
    );
}

#[test]
fn triage_rows_drop_the_counter_column_and_the_cwd_row() {
    // サイドバー幅32にタブ名とカウンタ列の両方は載らないので、タブ名を優先する。
    // cwd行も同じ理由で出さない（要件: triage-list-display.feature）
    let mut state = triage_state();
    state.show_cwd = true;
    state.pane_cwds.insert(4, "/work/oss/fujin".to_string());
    set_agent_state(&mut state, 4, AgentState::Working);
    state.apply_status(status(4, "SubagentStart")); // サブエージェント数 +1
    state.apply_status(status(4, "TaskCreated")); // 未完了タスク数 [1]
    state.handle_nav_key(key(BareKey::Char('t')));

    let rows = state.visible_rows();
    assert!(
        !rows.iter().any(|r| matches!(r, Row::Cwd { .. })),
        "cwd行は出さない"
    );
    let tab_column = state.triage_tab_column(&rows, SIDEBAR);
    let Some(Row::Triage { entry, tab_name }) = rows.get(HEADER_ROWS) else {
        panic!("トリアージ行が無い: {}", rows.len());
    };
    let content = state
        .triage_row(entry, tab_name, false, tab_column, None, SIDEBAR)
        .content()
        .to_string();
    assert_eq!(state.agents[&4].subagents, 1, "カウンタ自体は数えている");
    assert_eq!(state.agents[&4].open_tasks, 1);
    assert!(
        !content.contains("+1"),
        "サブエージェント数は出さない: {}",
        content
    );
    assert!(
        !content.contains("[1]"),
        "未完了タスク数は出さない: {}",
        content
    );
    assert!(content.ends_with("tab2"), "タブ名を優先する: {}", content);
}

#[test]
fn the_triage_help_overlay_lists_its_own_keys() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));
    state.handle_nav_key(key(BareKey::Char('?')));

    let lines: Vec<String> = state
        .help_lines()
        .iter()
        .map(|row| state.help_line(row, SIDEBAR).content().to_string())
        .collect();
    assert!(
        lines.iter().any(|line| line.contains("back to tree")),
        "トリアージモードのキーを出す: {:?}",
        lines
    );
    for line in &lines {
        assert!(!line.contains('…'), "幅32に収まらない: {}", line);
    }
    // ヘルプを閉じてもトリアージモードには留まる
    state.handle_nav_key(key(BareKey::Char('j')));
    assert!(state.triage.is_some());
    assert!(!state.help_overlay);
}

#[test]
fn the_nav_help_advertises_the_triage_key() {
    let state = triage_state();
    let lines: Vec<String> = state
        .help_lines()
        .iter()
        .map(|row| state.help_line(row, SIDEBAR).content().to_string())
        .collect();
    assert!(
        lines.iter().any(|l| l.contains("triage")),
        "navモードのヘルプから辿れないと気づけない: {:?}",
        lines
    );
}

#[test]
fn clicking_a_triage_row_jumps_to_that_pane() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 4, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('t')));
    // 0-2: ヘッダ / 3: delta（error）/ 4: alpha（working）
    assert!(state.handle_click(HEADER_ROWS as isize + 1));
    assert_eq!(state.selectable[state.selected].pane_id, 1);
    assert!(!state.nav_mode);
    assert!(state.triage.is_none(), "トリアージモードも一緒に畳む");
}

#[test]
fn render_survives_triage_mode() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Working);
    set_agent_state(&mut state, 4, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('t')));
    state.render(40, 20);
    state.render(3, 2);
    state.render(2, 1);
    state.render(0, 0);

    // 対象なしの通知行（`nothing to triage`）も通す
    let mut state = triage_state();
    state.handle_nav_key(key(BareKey::Char('t')));
    state.render(40, 20);
    state.render(1, 1);
}

// --- コマンド状態（決定202608072218。要件: docs/requirements/command-status/） ---
//
// コマンドペインの走行・終了を PaneManifest から導出する。エージェント状態とは
// 別概念だが、記号・既読モデル・待ち件数・トリアージ一覧は共用する

#[test]
fn a_running_command_pane_is_working() {
    let state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
}

#[test]
fn a_command_pane_that_exits_cleanly_is_done() {
    let state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Done))
    );
}

#[test]
fn any_other_exit_is_an_error() {
    // 非0コードもシグナル終了（exit_status なし）も区別せず error（決定202608072218）
    let failed = state_with_command_panes(vec![exited_command_pane(1, "make", Some(1))]);
    assert_eq!(
        failed.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
    let signalled = state_with_command_panes(vec![exited_command_pane(1, "make", None)]);
    assert_eq!(
        signalled.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

#[test]
fn a_shell_pane_has_no_command_state() {
    // コマンドペインでなければ状態を持たない（フックが無ければ状態を持たない
    // エージェント状態と対称）
    let state = state_with_command_panes(vec![terminal_pane(1, "zsh")]);
    assert_eq!(state.pane_status(1), None);
    assert!(state.commands.is_empty());
}

#[test]
fn trivial_commands_are_tracked_too() {
    // 絞り込みは行わない（決定202608072218）。実行時間の閾値のようなヒューリスティックは持たない
    let state = state_with_command_panes(vec![exited_command_pane(1, "ls", Some(0))]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Done))
    );
}

#[test]
fn re_running_a_command_returns_it_to_working() {
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(1))]);
    let seq_before = state.status_seq(1);

    state.apply_command_states(&manifest(vec![(0, vec![command_pane(1, "make")])]));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
    assert!(
        state.status_seq(1) > seq_before,
        "状態が変わったのでシーケンス番号も進む"
    );
}

#[test]
fn focusing_a_finished_command_pane_marks_it_read() {
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);

    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);

    assert_eq!(state.pane_status(1), None, "既読は状態を持たない側へ戻る");
}

#[test]
fn a_read_command_state_does_not_come_back() {
    // 終了したコマンドペインは exited が立ちっぱなしなので、既読を旗で持たないと
    // 次の PaneUpdate で同じ done が再導出されて復活する
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);

    state.apply_command_states(&manifest(vec![(
        0,
        vec![exited_command_pane(1, "make", Some(0))],
    )]));

    assert_eq!(state.pane_status(1), None);
}

#[test]
fn a_running_command_is_not_marked_read() {
    // working は既読にならない（走っている最中のコマンドは人を待っていない）
    let mut state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    let focused = PaneInfo {
        is_focused: true,
        ..command_pane(1, "docker build .")
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
}

#[test]
fn the_agent_state_wins_over_the_command_state() {
    // `zellij run -- claude` のようにコマンドペイン経由でエージェントを起動した
    // ケース。フック由来の状態が常に優先で、コマンド状態は無視する（決定202608072218）
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "claude", Some(1))]);
    state.apply_status(status(1, "UserPromptSubmit"));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Agent(AgentState::Working))
    );
}

#[test]
fn command_states_join_the_waiting_count() {
    let state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        exited_command_pane(2, "cargo test", Some(101)),
        command_pane(3, "docker build ."),
        terminal_pane(4, "zsh"),
    ]);
    // done と error だけを数える（working は人の対応を待っていない）
    assert_eq!(state.waiting_count(), 2);
}

#[test]
fn command_states_join_the_triage_list() {
    let mut state = state_with_command_panes(vec![
        command_pane(1, "docker build ."),
        exited_command_pane(2, "cargo test", Some(101)),
        terminal_pane(3, "zsh"),
    ]);
    state.apply_status(status(3, "Notification"));

    let ids: Vec<u32> = state.triage_entries().iter().map(|e| e.pane_id).collect();
    // 優先度階層はソースを問わず共通（error → blocked → working → done）
    assert_eq!(ids, vec![2, 3, 1]);
}

#[test]
fn a_command_pane_without_a_name_shows_its_command() {
    // 決定202608072218: ペイン名が空ならコマンド文字列を代わりに出す
    let state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(content.contains("docker build ."), "{}", content);
    assert!(content.contains('»'), "走行中のアイコンも出る: {}", content);
}

#[test]
fn a_named_command_pane_keeps_its_name() {
    // `zellij run --name` やリネームで名前が付いていればそちらが優先
    let named = PaneInfo {
        title: "build".to_string(),
        ..command_pane(1, "docker build .")
    };
    let state = state_with_command_panes(vec![named]);
    let content = state
        .pane_row(
            &state.selectable[0],
            false,
            None,
            column_of(&state),
            HeadCells::default(),
            SIDEBAR,
        )
        .content()
        .to_string();
    assert!(content.contains("build"), "{}", content);
    assert!(!content.contains("docker"), "{}", content);
}

#[test]
fn closed_command_panes_lose_their_state() {
    let mut state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        command_pane(2, "docker build ."),
    ]);
    state.panes = Some(manifest(vec![(0, vec![command_pane(2, "docker build .")])]));
    state.prune_stale_agents();

    assert!(!state.commands.contains_key(&1));
    assert!(state.commands.contains_key(&2));
}

// --- コマンド状態のインスタンス間同期（決定202608012141・決定202608072218） ---

#[test]
fn the_command_dump_round_trips() {
    let mut state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        command_pane(2, "docker build ."),
    ]);
    // 1つを既読にしてから配る
    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);
    let dump = state.command_dump();

    let mut peer = State::default();
    assert!(peer.apply_command_dump(&dump));

    assert_eq!(peer.pane_status(1), None, "既読も一緒に配る");
    assert_eq!(
        peer.pane_status(2),
        Some(PaneStatus::Command(CommandState::Working))
    );
    assert_eq!(peer.status_seq(2), state.status_seq(2));
    // 取り込んだ番号より古い番号を後から振らないよう、カウンタを進めておく
    assert!(peer.state_seq >= state.status_seq(2));
}

#[test]
fn a_broken_command_dump_line_is_skipped() {
    let mut state = State::default();
    assert!(!state.apply_command_dump("garbage\nx\tdone\t0\t1\n"));
    assert!(state.commands.is_empty());
}

#[test]
fn the_read_clear_pipe_also_clears_command_states() {
    // 既読クリアの配布（決定202608012141）はソースを区別しない
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    assert!(state.pipe(pipe_message(READ_CLEAR_PIPE, "1")));
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn the_command_state_pipe_takes_a_dump() {
    let mut state = State::default();
    assert!(state.pipe(pipe_message(COMMAND_STATE_PIPE, "7\terror\t0\t3\n")));
    assert_eq!(
        state.pane_status(7),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

// --- 既読の猶予（docs/issues/command-status-error-icon-swallowed.md） ---
//
// `zellij run` は新しいペインへフォーカスを移すので、一瞬で終わるコマンドは
// 必ず「フォーカス中に終了」する。素直に既読モデルを当てると、状態が付いた
// 同じ PaneUpdate の中で既読になり、アイコンが一度も描かれないまま消える

// 実セッションの再現手順（`zellij run -- sh -c 'exit 1'`）をそのままなぞる。
// 新しいコマンドペインがフォーカスを持ったまま走り、そのまま失敗して終わる
fn run_and_fail_while_focused() -> State {
    let mut state = state_with_panes(0);
    // 1回目の PaneUpdate: ペインが出来てフォーカスを持ち、まだ走っている
    let running = manifest(vec![(0, vec![focused(command_pane(1, "sh -c exit 1"))])]);
    state.apply_command_states(&running);
    state.apply_read_model(&running);
    state.panes = Some(running);
    state.rebuild_selectable();
    // 2回目の PaneUpdate: フォーカスを持ったまま失敗して終わる
    let exited = manifest(vec![(
        0,
        vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
    )]);
    state.apply_command_states(&exited);
    state.apply_read_model(&exited);
    state.panes = Some(exited);
    state.rebuild_selectable();
    state
}

#[test]
fn a_command_that_fails_while_focused_still_shows_its_icon() {
    let state = run_and_fail_while_focused();
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error)),
        "状態が付いた瞬間にフォーカスしていても、その場では既読にしない"
    );
    let content = state
        .pane_row(
            &state.selectable[0],
            false,
            None,
            column_of(&state),
            HeadCells::default(),
            SIDEBAR,
        )
        .content()
        .to_string();
    assert!(content.contains('×'), "{}", content);
}

#[test]
fn the_read_grace_survives_repeated_updates() {
    // 描画のたびに PaneUpdate が飛ぶ（実測で数msおきに連続）。猶予が1回きり
    // だと、この連打の中で結局既読になってしまう
    let mut state = run_and_fail_while_focused();
    let exited = manifest(vec![(
        0,
        vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
    )]);
    for _ in 0..5 {
        state.apply_command_states(&exited);
        state.apply_read_model(&exited);
    }
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

#[test]
fn leaving_the_pane_keeps_the_icon_and_coming_back_clears_it() {
    let mut state = run_and_fail_while_focused();
    // ユーザーが別のペインへ移る: 猶予は解けるが、既読にはならない
    let away = manifest(vec![(
        0,
        vec![
            exited_command_pane(1, "sh -c exit 1", Some(1)),
            focused(terminal_pane(2, "zsh")),
        ],
    )]);
    state.apply_command_states(&away);
    state.apply_read_model(&away);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error)),
        "離れただけでは既読にしない"
    );

    // 戻ってきて初めて既読になる
    let back = manifest(vec![(
        0,
        vec![
            focused(exited_command_pane(1, "sh -c exit 1", Some(1))),
            terminal_pane(2, "zsh"),
        ],
    )]);
    state.apply_command_states(&back);
    state.apply_read_model(&back);
    settle_read(&mut state);
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn another_tab_counts_as_having_left_the_pane() {
    // 別タブに居るあいだはフォーカスがそのペインに無いので猶予は解ける。
    // タブを切り替えて戻ってきたときに既読にならない、という取りこぼしを防ぐ
    let mut state = run_and_fail_while_focused();
    state.tabs = vec![tab(0, false), tab(1, true)];
    let elsewhere = manifest(vec![
        (0, vec![exited_command_pane(1, "sh -c exit 1", Some(1))]),
        (1, vec![focused(terminal_pane(2, "zsh"))]),
    ]);
    state.apply_command_states(&elsewhere);
    state.apply_read_model(&elsewhere);

    state.tabs = vec![tab(0, true), tab(1, false)];
    let back = manifest(vec![
        (
            0,
            vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
        ),
        (1, vec![terminal_pane(2, "zsh")]),
    ]);
    state.apply_command_states(&back);
    state.apply_read_model(&back);
    settle_read(&mut state);
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn the_read_clear_pipe_ignores_the_grace() {
    // 既読クリアの配布（決定202608012141）は、送り手が猶予込みで判断した結果。
    // 受け手が猶予で握り潰すと、タブごとにアイコンの有無が食い違う
    let mut state = run_and_fail_while_focused();
    assert!(state.pipe(pipe_message(READ_CLEAR_PIPE, "1")));
    assert_eq!(state.pane_status(1), None);
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
