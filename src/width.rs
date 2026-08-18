// 表示幅ユーティリティ。
//
// サイドバーの幅計算は表示セル幅（全角=2セル）で行う。文字数で数えると
// CJK 混在時にセル幅を超え、選択背景が端末側で折り返されて次の行を汚す
//（docs/issues/sidebar-bottom-highlight-glitch.md）。
// ここに置くのは zellij のホスト関数に依存しない純粋関数だけ。

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
// の形。詳細は docs/issues/sidebar-cwd-path-boundary.md）。どのセグメント境界でも
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
