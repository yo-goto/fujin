// ツリーの行（タブ見出し・ペイン行・cwd行）。
//
// 要件は features/sidebar-tree/ 配下。3種とも同じ行頭（`row_head`）と
// ハイライト（`highlight_row`）を共有するため、1モジュールにまとめてある。

use super::*;

impl State {
    // タブ見出し行1行ぶんの Text を組み立てる
    pub(crate) fn tab_heading(&self, tab: &TabInfo, cols: usize) -> Line {
        let marker = if tab.active { "▾" } else { "▸" };
        let prefix = format!("{} {} ", marker, tab.position + 1);
        let full_heading = format!("{}{}", prefix, tab.name);
        let title = truncate(&full_heading, content_cols(cols));
        let mut text = Line::new(&title);
        if tab.active {
            text = text.color_range(0, 0..title.chars().count());
        }
        // タブ名にヒットしたら見出し側をハイライトする（ヒット箇所の提示）。
        // 配下のどのペインの Hit も同じタブ名を指すので、最初の1つで足りる
        if let Some(search) = &self.search {
            let tab_hit = self
                .selectable
                .iter()
                .filter(|e| e.tab_position == tab.position)
                .find_map(|e| {
                    search
                        .hits
                        .get(&e.pane_id)
                        .filter(|h| h.field == Field::Tab)
                });
            if let Some(hit) = tab_hit {
                let indices = shift_highlight_indices(
                    &hit.indices,
                    prefix.chars().count(),
                    &title,
                    full_heading.chars().count(),
                );
                if !indices.is_empty() {
                    text = text.color_indices(1, indices);
                }
            }
        }
        text
    }

    // ペイン行に出す名前（決定202608070102）。ペイン名が空のときだけ cwd を代わりに出す。
    //
    // claude は終了時に空文字のタイトルを OSC で送り、zellij 側にそれを戻す経路が
    // 無いため、エージェントを落とした瞬間にペイン名が空のまま残る
    // （docs/issues/issue-pane-title-blank-on-exit.md）。前回の名前を保持すると死んだ
    // エージェントが動いているように見えるので、いまそこに何があるかが分かる cwd へ落とす
    pub(crate) fn display_title<'a>(&'a self, entry: &'a Selectable) -> &'a str {
        self.title_fallback(entry).unwrap_or(&entry.title)
    }

    // ペイン名フォールバックが効いているならその cwd。空でないペイン名はそのまま
    // 出すし（決定202608050055）、cwd を持たないペインは空欄のままにする
    pub(super) fn title_fallback(&self, entry: &Selectable) -> Option<&str> {
        if entry.title.trim().is_empty() {
            self.pane_cwds.get(&entry.pane_id).map(String::as_str)
        } else {
            None
        }
    }

    // ペイン行1行ぶんの Text を組み立てる（状態アイコン・ペイン名・カウンタ列・
    // ハイライト込み）。cwd は別行なのでここには出てこない（決定202608060053）。
    //
    // レイアウトは `{アイコン} {ペイン名} …余白… {カウンタ列}` で、カウンタ列は
    // 右端に揃える。ペイン名はカウンタ列を除いた残り幅に収め、名前の長さで
    // サブエージェント数・未完了タスク数が消えないようにする（決定202608060052）。
    //
    // `head` はアイコンの手前に挟む列（番号列・マーク列）。出すフレームでは
    // head がそのぶん伸びて、ペイン名の残り幅が縮む
    pub(crate) fn pane_row(
        &self,
        entry: &Selectable,
        is_highlighted: bool,
        hit: Option<&Hit>,
        column: CounterColumn,
        cells: HeadCells<'_>,
        cols: usize,
    ) -> Line {
        let HeadCells { number, mark } = cells;
        let agent = self.agents.get(&entry.pane_id);
        // アイコンはエージェント状態・コマンド状態のどちらからでも来る（決定202608072218）
        let status = self.pane_status(entry.pane_id);
        // 状態を持たないペインでも列は埋める。空白のままだと、エージェントが乗る行と
        // 並べたときに左端が欠けて見える（docs/issues/issue-sidebar-cwd-row-legibility.md）
        let icon = status.map(|s| s.icon()).unwrap_or(NO_AGENT_ICON);
        let head = row_head(is_highlighted, number, mark, icon);
        let head_width = UnicodeWidthStr::width(head.text.as_str());

        let (subagents, open_tasks) = counter_labels(agent);
        let counters = column.render(&subagents, &open_tasks);
        let counters_width = UnicodeWidthStr::width(counters.as_str());
        // フローティングペインの丸括弧もカウンタ列と同じく先に確保する（決定202608060052の
        // 考え方。畳むのはペイン名の側）
        let brackets = if entry.is_floating {
            FLOATING_BRACKETS
        } else {
            0
        };
        // この行自身がカウンタを持つときだけ列ぶんを確保する（決定202608060053、2026-08-08追記）。
        // 他のペインがカウンタを持っていても、この行が持たなければ counters は
        // 空文字列のまま — フレーム内の他行の状態に巻き込まれない
        let reserved = if counters.is_empty() {
            head_width + brackets
        } else {
            head_width + brackets + COLUMN_GAP + counters_width
        };

        // 右マージンぶんは文字を置かない。カウンタ列もそこまでで揃える
        let inner = content_cols(cols);
        let title_budget = inner.saturating_sub(reserved);
        let fallback = self.title_fallback(entry);
        let source = fallback.unwrap_or(&entry.title);
        let title_original_len = source.chars().count();
        let (title, title_dropped) = fold_to_width(source, title_budget);
        let (open, close) = floating_brackets(entry, &title);

        let mut label = format!("{}{}{}{}", head.text, open, title, close);
        if !counters.is_empty() {
            // ペイン名の長さに関わらず、カウンタ列は右端で揃える
            append_right_column(&mut label, &counters, inner);
        }
        if is_highlighted {
            // 選択背景がサイドバー幅いっぱいに伸びるよう空白で埋める。
            // 埋めないと文字列の長さぶんしか色が乗らず、帯に見えない
            label = pad_to_width(label, cols);
        }

        // ハイライトする場所が、そのままヒットしたフィールドの提示になる。
        // ペイン名は行全体とは別の予算で畳んでいるので、可視範囲も
        // ペイン名自身の畳んだ結果から判定する。
        // ペイン名フォールバック中はこの位置に出ているのが cwd なので、
        // 拾うヒットも cwd のものに切り替える（決定202608070102）
        let field = if fallback.is_some() {
            Field::Cwd
        } else {
            Field::Title
        };
        let highlight = hit.filter(|h| h.field == field).map(|hit| {
            fold_highlight_indices(
                &hit.indices,
                &title,
                title_dropped,
                title_original_len,
                // 丸括弧のぶんだけペイン名の開始位置が右へずれる
                head.text.chars().count() + open.chars().count(),
            )
        });

        let mut text = Line::new(&label);
        if !is_highlighted {
            text = unbold_name(text, &head.text, open, &title, close);
        }
        if let Some(status) = status {
            // 状態アイコン部分に状態色（位置は番号列の有無に追従する）
            text = text.color_range(status.color(), head.icon_at..head.icon_at + 1);
        }
        if let Some((digits, matches)) = number {
            // 番号は「そのまま打つ文字」なのでキーの色で出す（ヘルプの
            // キー列と同じ扱い）。番号入力バッファに前方一致しなくなった
            // 番号は落とし、残っている候補だけが目に入るようにする
            //（vimiumのリンクヒントと同じ提示。決定202608070342）
            let span = 2..2 + digits.chars().count();
            text = if matches {
                text.color_range(2, span)
            } else {
                text.dim_range(span)
            };
        }
        if let (Some(at), Some(true)) = (head.mark_at, mark) {
            // マーク印は「ユーザーが自分で指した」印なので、選択バーと同じレベル2。
            // 状態アイコンの色（行ごとに変わる）とは役割が違う
            text = text.color_range(2, at..at + 1);
        }
        if let Some(indices) = highlight.filter(|i| !i.is_empty()) {
            // レベル1で固定（決定202607302303のv1スコープ: 設定項目は増やさない）
            text = text.color_indices(1, indices);
        }
        if is_highlighted {
            text = highlight_row(text);
        }
        text
    }
}

// cwd行1行ぶんの Text（決定202608060053）。ペイン行の続きとして読めるよう字下げして dim で
// 出す。zellij側の実装制約で dim は bold を打ち消さないため、見た目上は bold+dim
// になる（決定202608080027の対象外。docs/issues/issue-sidebar-cwd-bold.md）。
// パスは末尾のディレクトリ名のほうが識別に効くので、先頭省略で畳む
pub(crate) fn cwd_row(cwd: &str, is_highlighted: bool, hit: Option<&Hit>, cols: usize) -> Line {
    // 選択中は左端のバーをこの行まで伸ばし、ペイン行と1つの帯に見せる
    let bar = if is_highlighted { "▌" } else { " " };
    let indent = format!("{}{}", bar, " ".repeat(CWD_INDENT.saturating_sub(1)));
    let inner = content_cols(cols);
    let (path, dropped) = truncate_start(cwd, inner.saturating_sub(CWD_INDENT));

    // 字下げだけで幅を使い切るほど狭いときの保険。はみ出した行は端末側で
    // 折り返り、選択背景が次の行を汚す（docs/issues/issue-sidebar-bottom-highlight-glitch.md）
    let mut label = truncate(&format!("{}{}", indent, path), inner);
    if is_highlighted {
        label = pad_to_width(label, cols);
    }
    let mut text = Line::new(&label);
    let end = label.chars().count();
    if end > CWD_INDENT {
        // cwd は主役ではないので落として出す。選択中も落としたまま — 外すと
        // bold に戻ってカーソル移動のたびにちらつく（2026-08-09 実機指摘）。
        // unbold_range は併用しない — zellij 側が dim/unbold を排他的に処理して
        // おり、足すと dim が無視される（実測・ソース確認済み。
        // zellij-server/src/ui/components/text.rs の color_index_character）
        text = text.dim_range(CWD_INDENT..end);
    }

    if let Some(hit) = hit {
        let indices = fold_highlight_indices(
            &hit.indices,
            &path,
            dropped,
            cwd.chars().count(),
            CWD_INDENT,
        );
        if !indices.is_empty() {
            text = text.color_indices(1, indices);
        }
    }
    if is_highlighted {
        text = highlight_row(text);
    }
    text
}
