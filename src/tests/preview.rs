use crate::agent::AgentState;
use crate::test_support::*;
use crate::*;

// --- プレビュー（決定202608082045、要件: docs/requirements/req-preview.md） ---
//
// navモード中、いま光っている行のペインの内容をフローティングペインへ
// スナップショット表示するトグル可能な横断的機能。
//
// **フローティングペインを開く経路と `get_pane_scrollback` はテストから呼べない**
// （どちらも戻り値を stdin から読み返すホスト関数）。ここで検証するのは
// 「いつオン/オフになるか」「どのペインを対象に選ぶか」「いつ更新を止めるか」
// までで、`own_plugin_url` を立てていない状態ではペインを開きにいかないので
// ホスト関数には触れない

fn preview_key() -> KeyWithModifier {
    key(BareKey::Char('p'))
}

fn preview_read_key() -> KeyWithModifier {
    key(BareKey::Char('r'))
}

// プレビューが表示している対象ペイン
fn preview_target(state: &State) -> Option<u32> {
    state.preview.as_ref().and_then(|preview| preview.target)
}

#[test]
fn the_preview_key_toggles_the_preview() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;

    state.handle_nav_key(preview_key());
    assert!(state.preview.is_some());
    assert_eq!(preview_target(&state), Some(1), "対象は選択行のペイン");
    assert!(state.nav_mode, "プレビューはモードではなく表示状態");

    state.handle_nav_key(preview_key());
    assert!(state.preview.is_none());
}

#[test]
fn the_preview_follows_the_selection() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());

    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(preview_target(&state), Some(2));
    state.handle_nav_key(key(BareKey::Char('G')));
    assert_eq!(preview_target(&state), Some(3));
}

#[test]
fn the_filtered_results_preview_with_the_alt_key() {
    // 検索サブモードは印字可能文字をすべてクエリに使うので、マークと同じく
    // プレビューのトグルだけ Alt付き
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "cha");
    assert_eq!(state.search.as_ref().and_then(|s| s.cursor), Some(3));

    state.handle_nav_key(KeyWithModifier::new(BareKey::Char('p')).with_alt_modifier());
    assert_eq!(
        preview_target(&state),
        Some(3),
        "対象は絞り込み結果のカーソル"
    );
    assert!(
        state.search.is_some(),
        "プレビューで検索サブモードを抜けない"
    );

    state.handle_nav_key(preview_key());
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("chap"),
        "素の `p` はクエリの文字"
    );
}

#[test]
fn the_triage_list_previews_the_row_under_the_cursor() {
    let mut state = triage_state();
    set_agent_state(&mut state, 3, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('t')));
    assert_eq!(state.triage_cursor(), Some(3));

    state.handle_nav_key(preview_key());
    assert_eq!(preview_target(&state), Some(3));
    assert!(
        state.triage.is_some(),
        "プレビューでトリアージ一覧を抜けない"
    );
}

#[test]
fn the_preview_key_is_undefined_in_the_number_jump_submode() {
    // マークと同じ理由（数字専用の入力空間の安全弁がそのまま効く）
    let mut state = jump_state(3);
    state.handle_nav_key(preview_key());

    assert!(!state.nav_mode, "未定義キーとして navモードごと退場する");
    assert!(state.preview.is_none());
}

#[test]
fn the_preview_stops_updating_in_the_number_jump_submode() {
    // オンのまま番号ジャンプサブモードへ入ることはできる。候補を絞っている
    // あいだは対象ペインが定まらないので、表示は直前のまま動かさない
    // 通し番号が2桁になる件数にしておく。1桁だと最初の数字で候補が1件に
    // 確定して即ジャンプしてしまい、「入力中」の状態を作れない（決定202608070342）
    let mut state = state_with_panes(12);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());
    assert_eq!(preview_target(&state), Some(1));

    state.handle_nav_key(key(BareKey::Char('n')));
    state.handle_nav_key(key(BareKey::Char('0'))); // 候補は 01..09 でまだ曖昧
    assert!(state.jump.is_some(), "番号ジャンプサブモードは続いている");
    assert!(state.preview.is_some(), "表示自体は維持する");
    assert_eq!(preview_target(&state), Some(1), "内容は更新しない");
}

#[test]
fn leaving_nav_mode_closes_the_preview() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());

    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.nav_mode);
    assert!(
        state.preview.is_none(),
        "navモードの外にプレビューだけ残さない（決定202608072359と整合させる）"
    );
}

#[test]
fn jumping_closes_the_preview() {
    let mut state = state_with_panes(3);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());

    state.handle_nav_key(key(BareKey::Enter));
    assert!(!state.nav_mode);
    assert!(state.preview.is_none());
}

#[test]
fn previewing_does_not_change_the_notification_state() {
    // プレビューはフォーカスもキー横取りも対象ペインに及ぼさないので、
    // 見て回るだけでは既読にならない（決定202607302302・決定202608080109はそのまま無関係に動く）
    let mut state = state_with_panes(3);
    set_agent_state(&mut state, 2, AgentState::Done);
    state.nav_mode = true;

    state.handle_nav_key(preview_key());
    state.handle_nav_key(key(BareKey::Char('j')));
    assert_eq!(preview_target(&state), Some(2));
    assert_eq!(
        state.agents.get(&2).map(|a| a.state),
        Some(AgentState::Done),
        "プレビューしただけでは既読にしない"
    );
}

#[test]
fn the_read_key_clears_the_state_of_the_previewed_pane() {
    for target in [AgentState::Done, AgentState::Blocked, AgentState::Error] {
        let mut state = state_with_panes(3);
        set_agent_state(&mut state, 1, target);
        state.nav_mode = true;
        state.handle_nav_key(preview_key());

        state.handle_nav_key(preview_read_key());
        assert_eq!(
            state.agents.get(&1).map(|a| a.state),
            Some(AgentState::Idle),
            "{:?} は既読化キーで idle に戻る",
            target
        );
        assert!(state.nav_mode, "既読化でnavモードを抜けない");
        assert!(state.preview.is_some(), "プレビューも開いたまま");
    }
}

#[test]
fn the_read_key_leaves_working_alone() {
    let mut state = state_with_panes(3);
    set_agent_state(&mut state, 1, AgentState::Working);
    state.nav_mode = true;
    state.handle_nav_key(preview_key());

    state.handle_nav_key(preview_read_key());
    assert_eq!(
        state.agents.get(&1).map(|a| a.state),
        Some(AgentState::Working),
        "走っている最中のペインは人の対応を待っていない"
    );
}

#[test]
fn the_read_key_is_undefined_while_the_preview_is_off() {
    // プレビュー専用のキーはプレビュー文脈の外では存在しない、の一貫性
    let mut state = state_with_panes(3);
    set_agent_state(&mut state, 1, AgentState::Done);
    state.nav_mode = true;

    state.handle_nav_key(preview_read_key());
    assert!(!state.nav_mode, "未定義キーとして navモードごと退場する");
    assert_eq!(
        state.agents.get(&1).map(|a| a.state),
        Some(AgentState::Done),
        "既読にもしない"
    );
}

#[test]
fn the_read_key_is_undefined_in_the_number_jump_submode() {
    let mut state = jump_state(3);
    state.handle_nav_key(preview_read_key());
    assert!(!state.nav_mode);
}

#[test]
fn the_triage_list_reads_the_previewed_pane_too() {
    let mut state = triage_state();
    set_agent_state(&mut state, 3, AgentState::Error);
    state.handle_nav_key(key(BareKey::Char('t')));
    state.handle_nav_key(preview_key());

    state.handle_nav_key(preview_read_key());
    assert_eq!(
        state.agents.get(&3).map(|a| a.state),
        Some(AgentState::Idle)
    );
    assert!(state.triage.is_some());
}

#[test]
fn the_nav_help_advertises_the_preview_keys() {
    let mut state = state_with_panes(2);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    assert!(lines.contains("preview"), "{}", lines);
    assert!(
        !lines.contains("mark read"),
        "プレビューがオフの間は既読化キーを出さない: {}",
        lines
    );

    state.handle_nav_key(key(BareKey::Char('?'))); // いったん閉じる
    state.handle_nav_key(preview_key());
    state.handle_nav_key(key(BareKey::Char('?')));
    let lines = overlay_lines(&state, SIDEBAR).join("\n");
    assert!(lines.contains("mark read"), "{}", lines);
    for line in overlay_lines(&state, SIDEBAR) {
        assert!(!line.contains('…'), "プレビュー中: {}", line);
    }
}

#[test]
fn the_snapshot_carries_the_pane_name_and_its_contents() {
    let mut state = State::default();
    state.apply_preview_snapshot("alpha\n$ cargo test\nok");

    assert_eq!(state.preview_content.title, "alpha");
    assert_eq!(state.preview_content.lines, vec!["$ cargo test", "ok"]);
}

#[test]
fn the_snapshot_keeps_the_tail_when_it_overflows() {
    // 見たいのは直近の出力なので、溢れるときに落とすのは古い側
    let mut state = State::default();
    state.apply_preview_snapshot("alpha\none\ntwo\nthree\nfour");

    assert_eq!(state.preview_body(2), ["three", "four"]);
    assert_eq!(state.preview_body(9), ["one", "two", "three", "four"]);
}

#[test]
fn the_snapshot_drops_the_blank_tail() {
    // プロンプトの下の余白がそのまま入ってくると、画面いっぱいの空行で終わる
    let mut state = State::default();
    state.apply_preview_snapshot("alpha\nrunning\n\n   \n");

    assert_eq!(state.preview_body(4), ["running"]);
}
