// 表示幅ユーティリティ。
//
// サイドバーの幅計算は表示セル幅（全角=2セル）で行う。文字数で数えると
// CJK 混在時にセル幅を超え、選択背景が端末側で折り返されて次の行を汚す
//（.docs/issues/issue-sidebar-bottom-highlight-glitch.md）。
// ここに置くのは zellij のホスト関数に依存しない純粋関数だけ。

use std::borrow::Cow;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// 表示セル幅ベースの切り詰め（末尾 `…`）。全角文字（CJK）は2セル分として数える
pub(crate) fn truncate(s: &str, max: usize) -> String {
    // 幅0のときに省略記号だけがはみ出さないようにする
    if max == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    // 省略記号（幅1）ぶんの余地を残しながら、幅が max-1 を超える手前まで詰める
    let limit = max.saturating_sub(1);
    let mut out = String::new();
    let mut width = 0;
    for c in s.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w > limit {
            break;
        }
        width += w;
        out.push(c);
    }
    out.push('…');
    out
}

// 表示セル幅ベースの先頭省略。先頭を落として `…` に畳み、末尾を残す（決定202608060053）。
// `/` の位置で丸め、`…` の直後が必ず `/` になるようにする（`…/development/…`
// の形。詳細は .docs/issues/issue-sidebar-cwd-path-boundary.md）。どのセグメント境界でも
// 収まらないほど狭いときだけ、文字幅で機械的に末尾を残す。
// 返り値は (畳んだ文字列, 落とした文字数)
pub(crate) fn truncate_start(s: &str, max: usize) -> (String, usize) {
    let total = s.chars().count();
    // 幅0のときに省略記号だけがはみ出さないようにする
    if max == 0 {
        return (String::new(), total);
    }
    if UnicodeWidthStr::width(s) <= max {
        return (s.to_string(), 0);
    }
    if let Some((dropped, tail)) = truncate_start_at_boundary(s, max) {
        return (format!("…{tail}"), dropped);
    }
    // フォールバック: 最後のセグメント自体が `…/` を付けても収まらないほど
    // 長い。区切りでは畳めないので、文字幅で機械的に末尾を残す
    let limit = max.saturating_sub(1);
    let mut kept = 0;
    let mut width = 0;
    for c in s.chars().rev() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w > limit {
            break;
        }
        width += w;
        kept += 1;
    }
    let dropped = total - kept;
    let mut out = String::from("…");
    out.extend(s.chars().skip(dropped));
    (out, dropped)
}

// `/` の位置（先頭自身を除く）を境界候補として、末尾からいちばん多く残せる
// 境界を探す。境界の文字（`/`自身）ごと残すので、`…` の直後は必ず `/` になる
fn truncate_start_at_boundary(s: &str, max: usize) -> Option<(usize, String)> {
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if i == 0 || c != '/' {
            continue;
        }
        let tail: String = chars[i..].iter().collect();
        // 省略記号（幅1）ぶんの余地を残して収まるか
        if UnicodeWidthStr::width(tail.as_str()) < max {
            return Some((i, tail));
        }
    }
    None
}

// 中身がパスかどうか。Claude Code はペイン名に cwd をそのまま入れることがあり、
// その場合は末尾を切ると `/Users/example/develo…` のようにどれも同じ見た目になる
fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("~/")
}

// ホームディレクトリ配下なら `~` へ畳む（設定 show_cwd_tilde、まだ実験段階。
// docs/issues/issue-sidebar-cwd-tilde-home.md）。`home` と完全一致 or
// `home + "/"` で前方一致する場合だけに限る — `/Users/roshi2` のような
// 別ユーザー名を `~2` に誤爆させないための境界判定
pub(crate) fn tildify<'a>(cwd: &'a str, home: Option<&str>) -> Cow<'a, str> {
    let Some(home) = home.filter(|h| !h.is_empty()) else {
        return Cow::Borrowed(cwd);
    };
    if cwd == home {
        Cow::Owned("~".to_string())
    } else if let Some(rest) = cwd.strip_prefix(home).and_then(|r| r.strip_prefix('/')) {
        Cow::Owned(format!("~/{rest}"))
    } else {
        Cow::Borrowed(cwd)
    }
}

// 表示幅 max に畳む（決定202608060053）。パスは先頭省略、それ以外は切り詰め。
// 返り値は (畳んだ文字列, 先頭で落とした文字数)
pub(crate) fn fold_to_width(s: &str, max: usize) -> (String, usize) {
    if looks_like_path(s) {
        truncate_start(s, max)
    } else {
        (truncate(s, max), 0)
    }
}

// 表示幅が cols に達するまで右側を空白で埋める（選択背景を幅いっぱいに
// 伸ばすためのパディング）
pub(crate) fn pad_to_width(mut s: String, cols: usize) -> String {
    let pad = cols.saturating_sub(UnicodeWidthStr::width(s.as_str()));
    s.push_str(&" ".repeat(pad));
    s
}

// 左側に空白を足して表示幅 width に揃える（カウンタ列の右揃え用）
pub(crate) fn pad_left(s: &str, width: usize) -> String {
    let pad = width.saturating_sub(UnicodeWidthStr::width(s));
    format!("{}{}", " ".repeat(pad), s)
}

// 切り詰め後の行で色を乗せてよい文字数の上限。
// 切り詰められた行の末尾は … なので、そこには色を乗せない
pub(crate) fn colorable_char_limit(visible: usize, original_len: usize) -> usize {
    if visible < original_len {
        visible.saturating_sub(1)
    } else {
        visible
    }
}

// フィールド内のマッチ位置（char index）を、畳んだあとの行内の位置へ移す。
// 画面から落ちた位置は捨てる — 見えていない文字にハイライトを置くと、
// 関係ない文字が光ってヒット箇所の提示にならない
pub(crate) fn fold_highlight_indices(
    indices: &[usize],
    folded: &str,
    dropped: usize,
    original_len: usize,
    offset: usize,
) -> Vec<usize> {
    if dropped > 0 {
        // 先頭省略。残った側は省略記号（1文字）ぶん右へずれる
        indices
            .iter()
            .filter(|&&i| i >= dropped)
            .map(|&i| i - dropped + offset + 1)
            .collect()
    } else {
        let limit = colorable_char_limit(folded.chars().count(), original_len);
        indices
            .iter()
            .filter(|&&i| i < limit)
            .map(|&i| i + offset)
            .collect()
    }
}

// マッチ位置（フィールド内の char index）を行ラベル内の位置へずらし、
// truncate() で切られて画面に無い位置を捨てる
pub(crate) fn shift_highlight_indices(
    indices: &[usize],
    offset: usize,
    truncated: &str,
    original_len: usize,
) -> Vec<usize> {
    let limit = colorable_char_limit(truncated.chars().count(), original_len);
    indices
        .iter()
        .map(|i| i + offset)
        .filter(|i| *i < limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- truncate ---

    #[test]
    fn truncate_leaves_short_strings_alone() {
        assert_eq!(truncate("abc", 5), "abc");
        // 境界: ちょうど収まるときは省略記号を付けない
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_appends_ellipsis_within_budget() {
        assert_eq!(truncate("abcdef", 5), "abcd…");
        assert_eq!(truncate("abcdef", 5).chars().count(), 5);
    }

    #[test]
    fn truncate_counts_display_width_not_chars() {
        // 全角文字（CJK）は2セル分として数える。6文字でも表示幅は12あるので、
        // 文字数ベースだった旧実装ではここが誤って「そのまま返す」になっていた
        // （.docs/issues/issue-sidebar-bottom-highlight-glitch.md）
        assert_eq!(truncate("日本語テスト", 12), "日本語テスト");
        assert_eq!(truncate("日本語テスト", 6), "日本…");
    }

    #[test]
    fn truncate_with_zero_width_is_empty() {
        // 省略記号1文字だけがはみ出すとサイドバー幅を壊す
        assert_eq!(truncate("abc", 0), "");
    }

    // --- pad_to_width ---

    #[test]
    fn pad_to_width_fills_with_spaces_up_to_the_column_count() {
        assert_eq!(pad_to_width("abc".to_string(), 5), "abc  ");
    }

    #[test]
    fn pad_to_width_counts_cjk_chars_as_two_cells() {
        // 全角文字混じりのラベルを文字数でパディングすると表示幅が cols を
        // 超えてしまい、選択背景が端末側で折り返されて次の行にはみ出す
        // （.docs/issues/issue-sidebar-bottom-highlight-glitch.md）。
        // 「日本語」は3文字・表示幅6なので、cols=10 なら空白4個で埋まるのが正しい
        let padded = pad_to_width("日本語".to_string(), 10);
        assert_eq!(padded, "日本語    ");
        assert_eq!(unicode_width::UnicodeWidthStr::width(padded.as_str()), 10);
    }

    #[test]
    fn pad_to_width_does_not_underflow_when_already_wide_enough() {
        // 表示幅がすでに cols 以上のときは空白を足さない（saturating_sub）
        assert_eq!(pad_to_width("日本語テスト".to_string(), 3), "日本語テスト");
    }

    // --- shift_highlight_indices（ハイライト位置の変換） ---

    #[test]
    fn highlight_indices_are_shifted_by_the_label_prefix() {
        // "▌ ○ alpha" — タイトルは4文字目から
        assert_eq!(
            shift_highlight_indices(&[0, 2], 4, "▌ ○ alpha", 9),
            vec![4, 6]
        );
    }

    #[test]
    fn highlight_indices_beyond_the_truncation_are_dropped() {
        // 元9文字を6文字に切り詰めると、末尾は … になる（実位置5が省略記号）
        let truncated = truncate("▌ ○ alpha", 6);
        assert_eq!(truncated.chars().count(), 6);
        assert_eq!(
            shift_highlight_indices(&[0, 1, 2, 3, 4], 4, &truncated, 9),
            vec![4],
            "省略記号とその先の位置には色を乗せない"
        );
    }

    // --- 先頭省略とハイライト位置 ---

    #[test]
    fn truncate_start_leaves_short_strings_alone() {
        assert_eq!(truncate_start("/a/b", 5), ("/a/b".to_string(), 0));
        // 境界: ちょうど収まるときは省略記号を付けない
        assert_eq!(truncate_start("/a/bc", 5), ("/a/bc".to_string(), 0));
    }

    #[test]
    fn truncate_start_drops_the_head_within_budget() {
        // "/bb" までは境界候補だが幅に収まらないので、収まる直近の境界 "/cc" へ丸める
        let (folded, dropped) = truncate_start("/aa/bb/cc", 5);
        assert_eq!(folded, "…/cc");
        assert_eq!(dropped, 6);
        // 収まる `/` 境界が無いときだけ、従来どおり文字幅で機械的に末尾を残す
        // （全角は2セルぶん食う）
        let (folded, dropped) = truncate_start("/あ/いう", 5);
        assert_eq!(folded, "…いう");
        assert_eq!(dropped, 3);
    }

    #[test]
    fn truncate_start_rounds_to_a_slash_boundary() {
        // ディレクトリ名の途中で切らず、`…` の直後が必ず `/` になるように
        // 収まる範囲でいちばん手前の区切りへ丸める（中途半端な文字列を避ける調整）
        let (folded, dropped) =
            truncate_start("/Users/example/development/oss/zellij-plugins/fujin", 24);
        assert_eq!(folded, "…/zellij-plugins/fujin");
        assert_eq!(dropped, 30);
    }

    #[test]
    fn truncate_start_falls_back_to_character_width_when_no_boundary_fits() {
        // 区切りが無い（か、区切りまで残しても収まらない）ほど1セグメントが
        // 長いときは、`…/` を諦めて文字幅で機械的に末尾を残す
        let (folded, dropped) = truncate_start("/aaaaaaaaaa", 5);
        assert_eq!(folded, "…aaaa");
        assert_eq!(dropped, 7);
    }

    // --- tildify（設定 show_cwd_tilde） ---

    #[test]
    fn tildify_folds_a_path_under_home() {
        assert_eq!(
            tildify("/Users/example/work/fujin", Some("/Users/example")),
            "~/work/fujin"
        );
    }

    #[test]
    fn tildify_folds_home_itself_to_a_bare_tilde() {
        assert_eq!(tildify("/Users/example", Some("/Users/example")), "~");
    }

    #[test]
    fn tildify_does_not_misfire_on_a_similar_username() {
        // "/Users/example2" は "/Users/example" の前方一致だが別ユーザーなので、
        // 区切り境界で弾いて誤って "~2" にしない
        assert_eq!(
            tildify("/Users/example2/work", Some("/Users/example")),
            "/Users/example2/work"
        );
    }

    #[test]
    fn tildify_leaves_paths_outside_home_alone() {
        assert_eq!(tildify("/tmp/work", Some("/Users/example")), "/tmp/work");
    }

    #[test]
    fn tildify_is_a_no_op_without_a_known_home() {
        assert_eq!(tildify("/Users/example/work", None), "/Users/example/work");
    }

    #[test]
    fn highlight_indices_follow_a_leading_ellipsis() {
        // "/aa/bb/cc" を先頭省略すると "…b/cc"。落ちた側（0..5）のヒットは捨て、
        // 残った側は省略記号1文字ぶん右へずれる
        assert_eq!(
            fold_highlight_indices(&[0, 4, 5, 8], "…b/cc", 5, 9, 6),
            vec![6 + 1, 6 + 1 + 3]
        );
    }

    #[test]
    fn highlight_indices_after_a_trailing_ellipsis_are_dropped() {
        // 末尾切り詰め側は既存の規則のまま。"abcdefghi" を "abcd…" に詰めたので、
        // 見えている 0..4 は offset ぶんずらし、省略記号に重なる 4 以降は捨てる
        assert_eq!(
            fold_highlight_indices(&[0, 3, 4, 8], "abcd…", 0, 9, 4),
            vec![4, 7]
        );
    }
}
