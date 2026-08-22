// ファジーマッチ（検索サブモード用）。
//
// zellij のホスト関数に一切依存しない純粋ロジック。外部クレートも使わない
// （候補の比較と根拠は .docs/requirements/req-search-explorer-spec.md 参照）。
//
// 一致判定は fzf v1 と同じ2パスのサブシーケンス:
// forward で一致の終端を見つけ、そこから backward で始端を締める。
// これで「先頭側の文字を拾って間延びした区間」を選ばずに済む。

// マッチ対象のフィールド。宣言順が同点時の優先順（Title > Tab > Cwd）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Title, // ペイン名
    Tab,   // そのペインが属するタブ名
    Cwd,   // フック由来の cwd
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub score: i32,
    pub field: Field,
    // 一致した文字位置。field が指す文字列に対する char index
    pub indices: Vec<usize>,
}

const MATCH: i32 = 16; // 一致1文字ごと
const BONUS_CONSECUTIVE: i32 = 8; // 直前の文字も一致している
const BONUS_BOUNDARY: i32 = 12; // 直前が / - _ . または空白、あるいは先頭
const BONUS_CAMEL: i32 = 8; // 直前が小文字で自分が大文字（camelCase 境界）
const GAP_PENALTY: i32 = -1; // 一致文字の間に挟まった未一致1文字ごと
const GAP_PENALTY_FLOOR: i32 = -20; // ギャップ減点の下限

// 3フィールドを個別に評価し、最良のものを返す。
// 連結して1本の文字列として判定してはいけない — サブシーケンス一致は
// フィールドをまたいだ無意味なヒットを拾ってしまう。
pub fn match_pane(query: &str, title: &str, tab: &str, cwd: Option<&str>) -> Option<Hit> {
    // smart-case: クエリに大文字が1つでもあれば case-sensitive
    let case_sensitive = query.chars().any(|c| c.is_ascii_uppercase());
    let query: Vec<char> = query.chars().collect();
    let candidates = [
        (Field::Title, Some(title), 10),
        (Field::Tab, Some(tab), 8),
        (Field::Cwd, cwd, 6),
    ];
    let mut best: Option<Hit> = None;
    for (field, haystack, weight) in candidates {
        let Some(haystack) = haystack else {
            continue;
        };
        let Some((score, indices)) = match_one(&query, haystack, case_sensitive) else {
            continue;
        };
        let weighted = score * weight / 10;
        // 同点は宣言順（先勝ち）で Title > Tab > Cwd
        if best.as_ref().map(|b| weighted > b.score).unwrap_or(true) {
            best = Some(Hit {
                score: weighted,
                field,
                indices,
            });
        }
    }
    best
}

// 1フィールドに対する一致判定。Some((score, 一致した char index 列)) を返す
fn match_one(query: &[char], haystack: &str, case_sensitive: bool) -> Option<(i32, Vec<usize>)> {
    // 空クエリは全件一致
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let haystack: Vec<char> = haystack.chars().collect();
    let eq = |a: char, b: char| {
        if case_sensitive {
            a == b
        } else {
            // 畳み込みは ASCII のみ。Unicode の完全な畳み込みはテーブルの
            // ぶんだけ wasm が太るので持ち込まない（非 ASCII は素通し）
            a.eq_ignore_ascii_case(&b)
        }
    };

    // pass 1 (forward): クエリを先頭から消費し、一致の終端を見つける
    let mut qi = 0;
    let mut end = 0;
    for (hi, &hc) in haystack.iter().enumerate() {
        if qi < query.len() && eq(query[qi], hc) {
            qi += 1;
            if qi == query.len() {
                end = hi;
                break;
            }
        }
    }
    if qi < query.len() {
        return None;
    }

    // pass 2 (backward): 終端から逆走してクエリを末尾から消費し、始端を締める
    let mut qi = query.len();
    let mut start = end;
    for hi in (0..=end).rev() {
        if eq(query[qi - 1], haystack[hi]) {
            qi -= 1;
            if qi == 0 {
                start = hi;
                break;
            }
        }
    }

    // pass 3: 確定した区間内を forward し、indices を確定する
    let mut indices = Vec::with_capacity(query.len());
    let mut qi = 0;
    for (hi, &hc) in haystack.iter().enumerate().take(end + 1).skip(start) {
        if qi < query.len() && eq(query[qi], hc) {
            indices.push(hi);
            qi += 1;
        }
    }

    Some((score(&haystack, &indices), indices))
}

fn score(haystack: &[char], indices: &[usize]) -> i32 {
    let mut total = 0;
    let mut previous: Option<usize> = None;
    for &i in indices {
        total += MATCH;
        if previous == Some(i.wrapping_sub(1)) {
            total += BONUS_CONSECUTIVE;
        } else {
            total += positional_bonus(haystack, i);
        }
        previous = Some(i);
    }
    // ギャップ減点は区間内の未一致文字数ぶん。下限でクランプする
    let span = indices.last().map(|l| l + 1 - indices[0]).unwrap_or(0);
    let gaps = (span - indices.len()) as i32;
    total + (gaps * GAP_PENALTY).max(GAP_PENALTY_FLOOR)
}

fn positional_bonus(haystack: &[char], i: usize) -> i32 {
    let Some(&prev) = i.checked_sub(1).and_then(|p| haystack.get(p)) else {
        // 先頭は境界扱い
        return BONUS_BOUNDARY;
    };
    if matches!(prev, '/' | '-' | '_' | '.') || prev.is_whitespace() {
        BONUS_BOUNDARY
    } else if prev.is_lowercase() && haystack[i].is_uppercase() {
        BONUS_CAMEL
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(query: &str, haystack: &str) -> Option<(i32, Vec<usize>)> {
        let case_sensitive = query.chars().any(|c| c.is_ascii_uppercase());
        match_one(&query.chars().collect::<Vec<_>>(), haystack, case_sensitive)
    }

    // --- match_one: 一致判定 ---

    #[test]
    fn subsequence_matches_in_order() {
        assert!(one("fjn", "fujin").is_some());
        assert!(one("fujin", "fujin").is_some());
        // 順序が崩れたら一致しない
        assert!(one("njf", "fujin").is_none());
        // 文字が足りなければ一致しない
        assert!(one("fujinx", "fujin").is_none());
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(one("", "anything"), Some((0, vec![])));
        assert_eq!(one("", ""), Some((0, vec![])));
    }

    #[test]
    fn smart_case_switches_on_uppercase() {
        // 小文字クエリは大文字にも当たる
        assert!(one("main", "MAIN.rs").is_some());
        // 大文字を含むクエリは case-sensitive
        assert!(one("Main", "Main.rs").is_some());
        assert!(one("Main", "main.rs").is_none());
    }

    #[test]
    fn indices_point_at_matched_chars() {
        let (_, indices) = one("fn", "fujin").expect("matches");
        assert_eq!(indices, vec![0, 4]);
    }

    #[test]
    fn indices_are_char_based_for_multibyte() {
        let (_, indices) = one("検索", "全文検索タブ").expect("matches");
        assert_eq!(indices, vec![2, 3]);
    }

    #[test]
    fn consecutive_beats_scattered() {
        let (consecutive, _) = one("abc", "xxabc").expect("matches");
        let (scattered, _) = one("abc", "axbxc").expect("matches");
        assert!(consecutive > scattered);
    }

    #[test]
    fn boundary_beats_middle() {
        let (boundary, _) = one("main", "src/main").expect("matches");
        let (middle, _) = one("main", "xxxmain").expect("matches");
        assert!(boundary > middle);
    }

    #[test]
    fn two_pass_picks_the_tight_tail_region() {
        // 1パスの貪欲な forward だと先頭の m a を拾って区間が間延びする。
        // backward で締め直し、末尾側の "main" を丸ごと選ぶこと
        let (_, indices) = one("main", "mab/main").expect("matches");
        assert_eq!(indices, vec![4, 5, 6, 7]);
        // 仕様の例: src/main.rs × main
        let (_, indices) = one("main", "src/main.rs").expect("matches");
        assert_eq!(indices, vec![4, 5, 6, 7]);
    }

    // --- match_pane: フィールド選択 ---

    #[test]
    fn straddling_fields_does_not_match() {
        // "ペイン名の末尾 + タブ名の先頭" のような連結ヒットを作らない
        assert!(match_pane("cd", "abc", "def", None).is_none());
        assert!(match_pane("fx", "shell-f", "xterm", Some("/tmp")).is_none());
    }

    #[test]
    fn panes_without_cwd_do_not_match_on_cwd() {
        assert!(match_pane("work", "shell", "tab1", None).is_none());
        // cwd があれば当たる
        let hit = match_pane("work", "shell", "tab1", Some("/work")).expect("matches");
        assert_eq!(hit.field, Field::Cwd);
    }

    #[test]
    fn title_wins_ties() {
        // タイトルとタブ名が同じ文字列なら重みの高い Title を採る
        let hit = match_pane("same", "same", "same", Some("same")).expect("matches");
        assert_eq!(hit.field, Field::Title);
    }

    #[test]
    fn field_weights_prefer_title_over_tab_over_cwd() {
        // 同じ一致内容でも重みで Title > Tab > Cwd
        let hit = match_pane("x", "y", "x", Some("x")).expect("matches");
        assert_eq!(hit.field, Field::Tab);
        let hit = match_pane("x", "y", "z", Some("x")).expect("matches");
        assert_eq!(hit.field, Field::Cwd);
    }

    #[test]
    fn empty_query_matches_any_pane_via_title() {
        let hit = match_pane("", "shell", "tab1", None).expect("matches");
        assert_eq!(hit.score, 0);
        assert_eq!(hit.field, Field::Title);
        assert!(hit.indices.is_empty());
    }
}
