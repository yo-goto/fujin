// サイドバーの見た目の回帰検出（docs/issues/issue-ui-requirements-approach.md の層2）。
//
// **正本は生成物。** 人が絵を書き起こすのではなく、`render_ui()` が返した行を
// そのまま golden file（insta の `.snap`）へ落とし、人は差分を承認する。
// 期待値を手で書かないので、実装を直したときに絵と実装が食い違う余地が無い。
//
// **枚数を増やしすぎない。** 幅の総当たりは層1（frame_invariants.rs）の担当で、
// こちらは幅32を中心に、畳み込みを見たい状態だけ狭幅を足す方針。
//
// 承認は `cargo insta review`（`cargo-insta` を入れている場合）か、
// `INSTA_UPDATE=always make test` で回す。`.snap.new` を放置するとテストは
// 落ち続けるので、必ずどちらかで畳むこと。

use crate::test_support::*;

// 最初の1枚。目的は「`render_ui` → `Line` → golden file」の配線が通ることの確認で、
// 網羅性ではない（要件の網羅は `.feature` と層1の担当）
#[test]
fn the_sidebar_renders_a_single_pane() {
    let state = state_with_panes(1);
    insta::assert_snapshot!(screen_snapshot(&state.render_ui(12, SIDEBAR)));
}
