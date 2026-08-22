// サイドバーの見た目の回帰検出（.docs/issues/issue-ui-requirements-approach.md の層2）。
//
// **正本は生成物。** 人が絵を書き起こすのではなく、`render_ui()` が返した行を
// そのまま golden file（insta の `.snap`）へ落とし、人は差分を承認する。
// 期待値を手で書かないので、実装を直したときに絵と実装が食い違う余地が無い。
//
// **枚数を増やしすぎない。** 幅の総当たりは層1（frame_invariants.rs）の担当で、
// こちらは幅32を中心に、畳み込みを見たい状態だけ狭幅を足す方針。1枚ごとに
// 「その状態でしか出ない要素」を必ず持たせる——カウンタ列・cwd行・絞り込み・
// 並べ替え・番号列・折り返しが、それぞれどれか1枚にしか出ないようにしてある。
//
// **スナップショット名は内容由来で明示する**（`assert_snapshot!` の第1引数）。
// テスト関数名から導かせない理由は2つ——関数名を変えただけで `.snap` が孤児になること、
// `.feature` から `Then the sidebar renders as ui-state "nav_two_tabs_w32"` の形で
// 一方向に参照する計画（同ノート「Gherkinとの結び」）の参照先がこの文字列であること。
//
// 承認は `cargo insta review`（`cargo-insta` を入れている場合）か、
// `INSTA_UPDATE=always make test` で回す。`.snap.new` を放置するとテストは
// 落ち続けるので、必ずどちらかで畳むこと。

use crate::agent::AgentState;
use crate::test_support::*;
use crate::*;

// 2タブ・3ペインに状態を配ったnavモード。**カウンタ列と cwd行が出る唯一の絵**なので、
// ペイン行のレイアウト（アイコン・ペイン名・右端揃えのカウンタ・6セル字下げのcwd）は
// ここが基準になる。cwd行はエージェントが乗っているペインにしか出ないので
// （render/rows.rs の `show_for_agent`）、cwd を持つ bravo にも状態を与えている
fn nav_state() -> State {
    let mut state = searchable_state();
    state.show_cwd = true;
    // 共有 fixture の cwd（`/work/fujin`）は幅32でも20でも収まってしまい、
    // 先頭省略（`truncate_start`）が絵に出ない。狭幅の1枚に仕事をさせるため、
    // ここでだけ深いパスへ差し替える
    state
        .pane_cwds
        .insert(2, "/work/example/zellij-plugins/fujin".to_string());
    set_agent_state(&mut state, 1, AgentState::Working);
    // カウンタ列は `+N`（サブエージェント）と `[M]`（未完了タスク）の2フィールド。
    // 桁揃えを見たいので、片方だけでなく両方を持つ行を1つ作る
    state.apply_status(status(1, "SubagentStart"));
    state.apply_status(status(1, "SubagentStart"));
    state.apply_status(status(1, "TaskCreated"));
    state.apply_status(status(1, "TaskCreated"));
    state.apply_status(status(1, "TaskCreated"));
    set_agent_state(&mut state, 2, AgentState::Blocked);
    set_agent_state(&mut state, 3, AgentState::Idle);
    state
}

// 最初の1枚。目的は「`render_ui` → `Line` → golden file」の配線が通ることの確認で、
// 網羅性ではない（要件の網羅は `.feature` と層1の担当）
#[test]
fn the_sidebar_renders_a_single_pane() {
    let state = state_with_panes(1);
    insta::assert_snapshot!(
        "single_pane_w32",
        screen_snapshot(&state.render_ui(12, SIDEBAR))
    );
}

// navモードの基準の絵。ツリーが画面高に届かないぶんは `Row::Blank` で埋まり、
// 下の枠が最下部に貼り付く——**この埋め草が no-reflow の実体**（決定202608210035）
#[test]
fn the_sidebar_renders_two_tabs_with_agents() {
    let state = nav_state();
    insta::assert_snapshot!(
        "nav_two_tabs_w32",
        screen_snapshot(&state.render_ui(16, SIDEBAR))
    );
}

// 検索サブモード。クエリは charlie だけに当たる "ch" — 一致ペインを持たないタブは
// 見出しごと落ちるので、**ツリーの行が丸ごと差し替わる**ことが絵に出る。
// 枠が中身の行数に引きずられないことは層1が別途アサートしている
#[test]
fn the_sidebar_renders_the_search_submode() {
    let mut state = nav_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "ch");
    insta::assert_snapshot!(
        "search_filtered_w32",
        screen_snapshot(&state.render_ui(16, SIDEBAR))
    );
}

// トリアージモード。タブ見出しを持たないフラット一覧へ差し替わり、緊急度順に
// 並べ替わる。**idle は落ちる**ので、絞り込みと並べ替えの両方がこの1枚に出る。
// 所属タブ名は右端の列に付く——タブをまたぐ並びでないと列の意味が絵に出ないので、
// 別タブの delta にも状態を与えてある（idle の charlie が落ちる側）
#[test]
fn the_sidebar_renders_the_triage_list() {
    let mut state = triage_state();
    set_agent_state(&mut state, 1, AgentState::Error);
    set_agent_state(&mut state, 2, AgentState::Blocked);
    set_agent_state(&mut state, 3, AgentState::Idle);
    set_agent_state(&mut state, 4, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));
    insta::assert_snapshot!(
        "triage_by_urgency_w32",
        screen_snapshot(&state.render_ui(16, SIDEBAR))
    );
}

// 番号ジャンプサブモード。行頭に番号列が挟まり、**後続の列が2セルぶん右へずれる**
//（決定202608080250の並び: 選択バー → 番号列 → マーク列 → アイコン → ペイン名）。
// 行頭の固定セルが6になるのはこのサブモードで、層1の幅の境界6の出どころでもある
#[test]
fn the_sidebar_renders_the_number_jump_submode() {
    let mut state = nav_state();
    state.handle_nav_key(key(BareKey::Char('n')));
    insta::assert_snapshot!(
        "number_jump_w32",
        screen_snapshot(&state.render_ui(16, SIDEBAR))
    );
}

// 狭幅の1枚。幅32の絵と同じ状態を20セルで描き、切り詰め（ペイン名の `…`・
// cwd の先頭省略）とカウンタ列の押し出しがどう効くかを見る。**幅の総当たりは
// 層1の担当**なので、こちらは畳み込みの見た目が変わったことに気づくためだけに1枚置く
#[test]
fn the_sidebar_folds_rows_at_a_narrow_width() {
    let state = nav_state();
    insta::assert_snapshot!(
        "nav_two_tabs_w20",
        screen_snapshot(&state.render_ui(16, 20))
    );
}
