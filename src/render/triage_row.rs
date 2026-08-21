// トリアージ一覧の行（要件: docs/requirements/req-triage-mode.md）。
//
// ツリーの行と違ってタブ名を行内へ持ち、カウンタ列と cwd行を落とす。

use super::*;

impl State {
    // トリアージ行のタブ名列の幅（要件: docs/requirements/req-triage-mode.md）。
    // カウンタ列（決定202608060053）と同じくフレーム内の実測最大で決めて、行をまたいで
    // タブ名の開始位置を揃える
    pub(crate) fn triage_tab_column(&self, rows: &[Row<'_>], cols: usize) -> usize {
        let mut width = 0;
        for row in rows {
            if let Row::Triage { tab_name, .. } = row {
                width = width.max(UnicodeWidthStr::width(*tab_name));
            }
        }
        width.min(content_cols(cols) / TRIAGE_TAB_SHARE)
    }

    // トリアージ一覧の1行ぶんの Text（要件: docs/requirements/req-triage-mode.md）。
    //
    // レイアウトは `{アイコン} {ペイン名} …余白… {タブ名}` で、ペイン行の
    // カウンタ列（決定202608060053）の位置にタブ名を置いた形。状態アイコンは通常表示と
    // 同じものを使う。
    //
    // カウンタ列は出さない — サイドバー幅32（決定202607302256）にタブ名と両方は載らず、
    // トリアージが答えるのは「今どれに手を入れるか」なので、タブの壁を無視した
    // 一覧で迷子にならないためのタブ名を優先する。cwd行も同じ理由で出さない
    pub(crate) fn triage_row(
        &self,
        entry: &Selectable,
        tab_name: &str,
        is_highlighted: bool,
        tab_column: usize,
        mark: Option<bool>,
        cols: usize,
    ) -> Line {
        let status = self.pane_status(entry.pane_id);
        let icon = status.map(|s| s.icon()).unwrap_or(" ");
        // マーク列もペイン行と同じ位置（アイコンの手前）に出す（決定202608080250）。
        // トリアージ一覧の上でもマークできる以上、印が見えないと積み上げられない
        let head = row_head(is_highlighted, None, mark, icon);
        let head_width = UnicodeWidthStr::width(head.text.as_str());

        let tab = truncate(tab_name, tab_column);
        let inner = content_cols(cols);
        // フローティングペインの丸括弧はペイン行と同じ扱い（先に確保する）。
        // トリアージ行はカウンタ列・cwd行を持たないが、括弧はペイン名に直接付く
        // 要素なのでこの制約とは独立に出す（要件: floating-pane-indicator）
        let brackets = if entry.is_floating {
            FLOATING_BRACKETS
        } else {
            0
        };
        // タブ名を持たない行があっても列ぶんは空けておく（桁が行ごとにずれないため）
        let reserved = if tab.is_empty() {
            head_width + brackets
        } else {
            head_width + brackets + COLUMN_GAP + tab_column
        };
        // ペイン行と同じく、ペイン名が空なら cwd を代わりに出す（決定202608070102）
        let (title, _) = fold_to_width(self.display_title(entry), inner.saturating_sub(reserved));
        let (open, close) = floating_brackets(entry, &title);

        let mut label = format!("{}{}{}{}", head.text, open, title, close);
        let mut tab_span = None;
        if !tab.is_empty() {
            // 切り詰められたらタブ名の位置が確定しないので dim は諦める
            //（append_right_column が None を返す）
            tab_span = append_right_column(&mut label, &tab, inner);
        }
        if is_highlighted {
            label = pad_to_width(label, cols);
        }

        let mut text = Line::new(&label);
        if !is_highlighted {
            text = unbold_name(text, &head.text, open, &title, close);
        }
        if let Some(status) = status {
            text = text.color_range(status.color(), head.icon_at..head.icon_at + 1);
        }
        if let (Some(at), Some(true)) = (head.mark_at, mark) {
            text = text.color_range(2, at..at + 1);
        }
        // タブ名は主役（状態アイコン・ペイン名）ではないので落として出す。
        // 選択行では落とさない — 帯の中でさらに沈むと読めなくなる（cwd行と同じ）
        if let (Some((start, end)), false) = (tab_span, is_highlighted) {
            text = text.dim_range(start..end);
        }
        if is_highlighted {
            text = highlight_row(text);
        }
        text
    }
}
