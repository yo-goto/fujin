// サイドバーの枠の不変条件（.docs/issues/issue-ui-requirements-approach.md の層1）。
// `.feature` はランナーを持たずドキュメントとしてのみ存在するので、そこに書かれた
// 不変条件へ実行可能な裏付けを与えるのがこの層の役割。
//
// **守るのは「枠の不変」だけ。** no-reflow は境界線・ヘッダー・フッターの位置と
// 高さがモードによらず変わらないことを指し、ツリー領域の中身の増減・並べ替え・
// 差し替えは対象外（決定202608210035）。検索の絞り込みもトリアージの並べ替えも、
// 行を動かすことがモードの目的そのものなので、行の位置はここでは検査しない。
//
// **単一の要件スラッグに属さないため独立したファイルにしてある**（決定202608192347の
// 「機能・要件単位で1ファイル」に対する例外）。対応する `.feature` は sidebar-header・
// sidebar-footer・sidebar-scroll の3つに跨り、どれか1つの下へ置くと残り2つから
// 見えなくなる。

use crate::agent::AgentState;
use crate::render::Row;
use crate::test_support::*;
use crate::*;

// 検査するモード・サブモードの全部。sidebar-header.feature の Scenario Outline
// 「モードを出入りしてもヘッダー・フッターの枠は位置がずれない」の Examples と
// 1対1に対応させる（ツリー表示は、その Given にあたる基準の状態）
fn every_mode() -> Vec<(&'static str, State)> {
    vec![
        ("ツリー表示", tree_view()),
        ("navモード", searchable_state()),
        ("検索サブモード", search_submode()),
        ("トリアージモード", triage_submode()),
        ("番号ジャンプサブモード", jump_state(3)),
        ("終了操作サブモード", termination_state(3)),
        ("ヘルプオーバーレイ", help_overlay()),
    ]
}

fn tree_view() -> State {
    let mut state = searchable_state();
    state.nav_mode = false;
    state
}

// 絞り込みで行が減る状態を作る（枠が中身の行数に引きずられないことを見るため）。
// クエリは charlie だけに当たる "ch" — "a" のような全ペイン（タブ名含む）に
// 当たる文字だと絞り込みが起きず、この fixture の意図を満たさない
fn search_submode() -> State {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, "ch");
    state
}

fn triage_submode() -> State {
    let mut state = triage_state();
    set_agent_state(&mut state, 2, AgentState::Blocked);
    set_agent_state(&mut state, 3, AgentState::Working);
    state.handle_nav_key(key(BareKey::Char('t')));
    state
}

fn help_overlay() -> State {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('?')));
    state
}

// 総当たりする幅。fold_to_width・truncate・append_right_column・counter_column は
// どれも幅依存で、バグは境界幅に出る（既定幅は32）。
//
// **幅 5 以下は入れていない。** 行の先頭に固定で載るセル（選択バー2 + 記号2、
// 番号ジャンプサブモード中はさらに番号列2）がそもそも収まらず、タブ見出し行・
// ペイン行・トリアージ行がそのままあふれる（2026-08-21 に実測。幅 3〜5 で66件、
// **幅 6〜19 はゼロ**）。実機のサイドバーがそこまで縮むことはなく、極端な引数で
// 算術が破綻しないことは既存の `render_survives_*` が見ている。
//
// 6 は「行頭の固定セルがちょうど収まる」実測済みの境界なので総当たりへ含める
// ——バグは境界幅で出る、というこの配列の選定理由そのものに当たる幅
const WIDTHS: [usize; 6] = [6, 20, 28, 32, 48, 120];
// 枠ぶん（FRAME_TOP + FRAME_BOTTOM = 6行）の直後から、一覧が余る高さまで
const HEIGHTS: [usize; 5] = [7, 8, 12, 24, 60];

// 要件: sidebar-scroll.feature「一覧の下に余白は残らない」
#[test]
fn the_screen_is_always_filled_to_the_requested_height() {
    for (mode, state) in every_mode() {
        for rows in HEIGHTS {
            assert_eq!(
                state.screen_rows(rows).len(),
                rows,
                "{mode}: 画面高 {rows} 行を埋めきるべき"
            );
        }
    }
}

// 要件: sidebar-header.feature「モードを出入りしてもヘッダー・フッターの枠は
// 位置がずれない」・sidebar-footer.feature「フッターの高さと位置はモードによらず
// 常に同じ」・sidebar-scroll.feature「ヘッダの3行は最上部に固定」
#[test]
fn the_frame_sits_at_the_same_index_in_every_mode() {
    for (mode, state) in every_mode() {
        for rows in HEIGHTS {
            let screen = state.screen_rows(rows);
            assert!(matches!(screen[0], Row::Divider), "{mode}: 最上部は境界線");
            assert!(matches!(screen[1], Row::Header), "{mode}: ヘッダーは2行目");
            assert!(
                matches!(screen[2], Row::Divider),
                "{mode}: ヘッダーの下は境界線"
            );
            assert!(
                matches!(screen[rows - 3], Row::Divider),
                "{mode}: フッターの上は境界線"
            );
            assert!(
                matches!(screen[rows - 2], Row::Footer),
                "{mode}: フッターは下から2行目"
            );
            assert!(
                matches!(screen[rows - 1], Row::Blank),
                "{mode}: 最下部は status-bar と離すための空行"
            );
        }
    }
}

// 要件: sidebar-tree.feature「行の右端には余白が残る」の弱い側（あふれないこと）。
// 余白そのものの幅は pane_row.rs 側のテストが見る
#[test]
fn no_row_overflows_the_sidebar_width() {
    for (mode, state) in every_mode() {
        for cols in WIDTHS {
            for rows in HEIGHTS {
                for (y, line) in state.render_ui(rows, cols).into_iter().enumerate() {
                    // 高さを占めるだけの行は印字されない
                    let Some(line) = line else { continue };
                    let body = line.content();
                    let width = unicode_width::UnicodeWidthStr::width(body);
                    assert!(
                        width <= cols,
                        "{mode}: 幅 {cols} に対し {y} 行目が {width} セルある: {body:?}"
                    );
                }
            }
        }
    }
}

// 要件: sidebar-header.feature「権限が未承認のときは枠自体が表示されない」。
// 枠が常に出るという上の3つに対する唯一の例外なので、境界として明示しておく
#[test]
fn the_frame_is_absent_until_permissions_are_granted() {
    let mut state = searchable_state();
    state.permissions_granted = false;
    for rows in HEIGHTS {
        assert!(
            state.screen_rows(rows).is_empty(),
            "権限が未承認のうちは枠の3+3行も出さない"
        );
    }
}

// --- 行位置の不変（決定202608210035の対象外だが、要件としては別に存在する） ---

// 行の同一性を、位置の比較ができる形へ落とす。**中身（色・番号列・マーク列）ではなく
// 「どの行が何行目にいるか」だけ**を見たいので、行種と指し先のIDまでに畳む。
// 番号ジャンプの番号列・終了操作のマーク列は行の中身を変えるが位置は変えないので、
// ここで畳んでしまうのが正しい
fn row_identity(row: &Row) -> String {
    match row {
        Row::Divider => "divider".to_string(),
        Row::Header => "header".to_string(),
        Row::Footer => "footer".to_string(),
        Row::Blank => "blank".to_string(),
        Row::Help(_) => "help".to_string(),
        Row::Notice(text) => format!("notice:{text}"),
        Row::Overflow { above, .. } => format!("overflow:{above}"),
        Row::Tab(tab) => format!("tab:{}", tab.position),
        Row::Pane { entry, .. } => format!("pane:{}", entry.pane_id),
        Row::Triage { entry, .. } => format!("triage:{}", entry.pane_id),
        Row::Cwd { entry, .. } => format!("cwd:{}", entry.pane_id),
    }
}

fn row_identities(state: &State, rows: usize) -> Vec<String> {
    state.screen_rows(rows).iter().map(row_identity).collect()
}

// ツリー領域の中身が入れ替わらないモードだけ。検索は一致しないタブを見出しごと落とし、
// トリアージはフラット一覧へ差し替え、ヘルプはツリーを覆うので、**行が動くことが
// そのモードの目的**にあたる（決定202608210035）。要件側の Examples もこの3つ
fn modes_that_keep_row_positions() -> Vec<(&'static str, State)> {
    vec![
        ("navモード", searchable_state()),
        ("番号ジャンプサブモード", {
            let mut state = searchable_state();
            state.handle_nav_key(key(BareKey::Char('n')));
            state
        }),
        ("終了操作サブモード", {
            let mut state = searchable_state();
            state.handle_nav_key(key(BareKey::Char('d')));
            state
        }),
    ]
}

// 要件: sidebar-header.feature「モードを出入りしてもタブ見出し行・ペイン行の位置は
// 上下にずれない」。**枠の不変（上の3テスト）とは別の要件**で、こちらはツリー領域の
// 中身の話。golden file では表現できない——2つの状態のあいだの関係性なので、
// 1状態1枚の絵をいくら並べても「ずれていない」ことは機械が言えない
#[test]
fn entering_a_mode_does_not_shift_tree_rows() {
    let baseline = tree_view();
    for (mode, state) in modes_that_keep_row_positions() {
        for rows in HEIGHTS {
            assert_eq!(
                row_identities(&state, rows),
                row_identities(&baseline, rows),
                "{mode}: 画面高 {rows} 行で、ツリーの行が上下にずれた"
            );
        }
    }
}
