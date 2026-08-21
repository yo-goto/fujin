// 描画の入口。組み上がった行を実際に `print_text_with_coordinates` へ流す。
//
// 枠（境界線・ヘッダー・フッター）の位置決めと、入力欄のテキストカーソルの
// 置き場所を持つ。行そのものの組み立ては header / footer / help / pane_row /
// triage_row 各モジュールの担当。

use super::*;

impl State {
    // 描画の入口。組み立ては `render_ui()` が済ませているので、ここは印字するだけ
    pub(crate) fn draw(&self, rows: usize, cols: usize) {
        for (y, line) in self.render_ui(rows, cols).into_iter().enumerate() {
            // 高さを占めるだけの行は印字しない。位置は詰めない
            let Some(line) = line else { continue };
            print_text_with_coordinates(Text::from(&line), 0, y, None, None);
        }
    }

    // 画面に並ぶ行を組み立てる。**`draw()` と1対1**——プレビュー・権限未承認の
    // early-return もここに含める。印字だけを剥がしたことが自明になり、分岐の
    // 取りこぼしが起きないため（docs/issues/issue-ui-requirements-approach.md）。
    //
    // 高さを占めるだけの行（`Row::Blank`）は `None`。印字しない行のぶんも位置を
    // 残すので、添字はそのまま画面上の y になる
    pub(crate) fn render_ui(&self, rows: usize, cols: usize) -> Vec<Option<Line>> {
        // プレビュー用フローティングペイン（決定202608082045）はサイドバーの枠組みを持たない。
        // 一覧も状態も持っていないので、共通の描画へ落とすと空の枠だけが出る
        if self.is_preview {
            return self.preview_lines(rows, cols);
        }
        if !self.permissions_granted {
            return vec![Some(Line::new("permissions required (press y)"))];
        }
        if rows == 0 {
            return Vec::new();
        }
        self.sidebar_lines(rows, cols)
    }

    // 枠と中身の行。`render_ui()` の本線で、プレビュー・権限未承認・高さ0は
    // 呼び出し側が先に分けている
    fn sidebar_lines(&self, rows: usize, cols: usize) -> Vec<Option<Line>> {
        // 画面高での打ち切りは screen_rows() が済ませている。ここで改めて
        // 打ち切ると、あふれマーカー行の勘定と食い違ってクリックが行ずれする
        let screen = self.screen_rows(rows);
        // カウンタ列の幅はフレーム全体で1つ。行ごとに測ると桁が揃わない（決定202608060053）
        let column = self.counter_column(&screen);
        // トリアージ行のタブ名列も同じ理由でフレーム全体で1つ
        let tab_column = self.triage_tab_column(&screen, cols);
        // マーク列を出すかもフレーム全体で1つ（決定202608080250）。行ごとに決めると
        // マーク済みの行だけアイコンの位置がずれる
        let marks = self.mark_column(&screen);
        // カーソルは一覧から導出されるので、行ごとに引き直さず1度だけ求める
        let triage_cursor = self.triage_cursor();
        screen
            .into_iter()
            .map(|row| match row {
                Row::Header => Some(self.header_line(cols)),
                Row::Footer => Some(self.footer_line(cols)),
                // 高さを占めるだけの行。描くものは無い
                Row::Blank => None,
                Row::Divider => Some(divider_line(cols)),
                Row::Help(row) => Some(self.help_line(row, cols)),
                Row::Notice(notice) => {
                    // 操作の対象ではない通知なので、一覧の行より落として出す
                    let label = format!("  {}", notice);
                    Some(compose(&[(&label, Ink::Muted)], cols))
                }
                Row::Overflow { hidden, above } => Some(overflow_row(hidden, above, cols)),
                Row::Tab(tab) => Some(self.tab_heading(tab, cols)),
                Row::Pane {
                    entry,
                    flat_index,
                    hit,
                } => {
                    let is_highlighted = self.row_is_highlighted(entry, flat_index);
                    // 番号ジャンプサブモード中だけ番号列が付く（決定202608070342）
                    let number = self.jump_number(flat_index);
                    let cells = HeadCells {
                        number: number.as_ref().map(|(n, m)| (n.as_str(), *m)),
                        mark: marks.then(|| self.is_marked(entry.pane_id)),
                    };
                    Some(self.pane_row(entry, is_highlighted, hit, column, cells, cols))
                }
                Row::Triage { entry, tab_name } => {
                    let is_highlighted = triage_cursor == Some(entry.pane_id);
                    let mark = marks.then(|| self.is_marked(entry.pane_id));
                    Some(self.triage_row(entry, tab_name, is_highlighted, tab_column, mark, cols))
                }
                Row::Cwd {
                    entry,
                    flat_index,
                    cwd,
                    hit,
                } => {
                    let is_highlighted = self.row_is_highlighted(entry, flat_index);
                    Some(cwd_row(cwd, is_highlighted, hit, cols))
                }
            })
            .collect()
    }

    // 入力欄のテキストカーソル位置をホストへ伝える。位置が変わったときだけ送る。
    //
    // **`render()` の中からは呼べない。** 描画中の stdout はホストコマンドの
    // 経路と混線し、zellij 側が毎フレーム
    // 「failed to deserialize object from WASI env」で落とす（実測。
    // [`../../docs/dev/implementation-notes.md`]）。呼ぶのはイベント処理の側
    pub(crate) fn sync_input_cursor(&mut self) {
        let next = self.input_cursor_position();
        if next != self.cursor_shown {
            host::show_cursor(next);
            self.cursor_shown = next;
        }
    }

    // 入力欄（検索・番号ジャンプ）を出している間の、テキストカーソルの位置。
    // 入力欄が無ければ None ＝ カーソルを隠す。サイドバーは読むための面なので、
    // 平常時にカーソルが点いていると入力できるように見えてしまう
    pub(crate) fn input_cursor_position(&self) -> Option<(usize, usize)> {
        let x = self.input_cursor_column()?;
        // 行は描画と同じ組み立てから引く（直近に描いた画面高を使う）。
        // まだ一度も描いていなければ位置が決まらない
        if self.viewport_rows == 0 {
            return None;
        }
        let y = self
            .screen_rows(self.viewport_rows)
            .iter()
            .position(|row| matches!(row, Row::Footer))?;
        // クエリが欄からあふれてもペインの外の列を指さない。zellij は範囲外の
        // 座標を非表示扱いにする（zellij-server `plugin_pane.rs` の
        // `cursor_coordinates`）ので、送ると候補窓がまた左上へ飛ぶ
        let x = x.min(content_cols(self.viewport_cols).saturating_sub(1));
        Some((x, y))
    }

    // 入力欄を出している間、**自分の打鍵以外での再描画を見送るか**。
    //
    // サイドバーを描き直すと、zellij は描画の最後にテキストカーソルを入力欄へ戻す。
    // IMEで変換している最中にこれが起きると、端末が描いていた未確定文字列が
    // 上書きされ、**変換候補ウィンドウが打っている途中で飛ぶ**（実測。
    // docs/issues/issue-ime-input-support.md）。外から届くイベント（他ペインの変化・
    // 状態通知・タイマー）は入力が終わるまで描画を待たせる。
    //
    // 代償: 入力中は一覧が古いまま止まる。**状態そのものは更新し続けている**
    // ので、入力欄を抜けた時点の描画で追いつく。
    //
    // **`input_cursor_column` とは判定基準が異なる**（2026-08-13）。テキストカーソルは
    // 検索サブモードの操作状態でも表示する（位置表示の一本化。決定202608131200）が、
    // 操作状態はIMEの変換が起きないので、ここまで見送りを広げると結果を見ながら
    // 動かしているあいだ一覧が古いまま固まる。打鍵中（編集状態・番号ジャンプ）
    // だけに絞る
    pub(crate) fn defers_render_while_typing(&self) -> bool {
        if self.help_overlay || self.termination.is_some() {
            return false;
        }
        match &self.search {
            Some(search) => search.phase == SearchPhase::Editing,
            None => self.jump.is_some(),
        }
    }

    // 入力欄の中でテキストカーソルを置く列。
    //
    // **IME の変換候補ウィンドウは端末がテキストカーソルの位置に出す**ので、置かないと
    // 画面左上（プラグインペインの原点）に離れて出る。カーソル非表示のままだと
    // 変換の確定そのものが効かない端末もある（docs/issues/issue-ime-input-support.md）。
    //
    // 検索サブモードは**編集状態・操作状態のどちらでも**カーソルを置く（決定202608131200、
    // 2026-08-13 に一本化）。以前は操作状態を隠して地の文の疑似カーソルに位置表示を
    // 譲っていたが、端末のカーソル形状はこちらから指定できず地の文の字も
    // その下に隠れるため、見分けの手がかりとして機能していなかった
    // （docs/issues/issue-search-input-cursor-shape.md）。テキストカーソルへ一本化し、
    // 状態の違いは入力文字列の明暗とフッターのヒント文言で示す
    //
    // **分岐は `footer_line` と同じ順序で見ること。** 入力欄が出ていないのに
    // カーソルだけ残すと、候補窓が見当違いの場所に出る（検索サブモード中に
    // ヘルプを開くとフッターは閉じ方の案内に変わる、など）
    fn input_cursor_column(&self) -> Option<usize> {
        if self.help_overlay || self.termination.is_some() {
            return None;
        }
        let (tag, input) = if let Some(search) = &self.search {
            ("/", search.query.as_str())
        } else if let Some(jump) = &self.jump {
            ("n ", jump.buffer.as_str())
        } else {
            return None;
        };
        Some(HEADER_INDENT + UnicodeWidthStr::width(tag) + UnicodeWidthStr::width(input))
    }

    // プレビュー用フローティングペインの描画（決定202608082045。要件: preview）。
    //
    // 見出し（対象ペイン名）＋境界線＋スナップショット本文だけの簡素な作り。
    // サイドバーの枠組み（ヘッダー・フッター・階段状の字下げ）は持ち込まない —
    // ここに出るのは fujin の一覧ではなく**他のペインの画面そのもの**なので、
    // 装飾を足すほど元の見え方から遠ざかる。
    //
    // 本文は右マージンも取らず幅いっぱいを使う（同じ理由）。取れるのは装飾を
    // 持たないプレーンテキストだけで、色付きの再現はできない（決定202608082045の技術調査）
    // プレビュー用フローティングペインの行。サイドバーの枠は持たず、
    // 見出し・境界線・本文の3段だけ（決定202608082045）
    fn preview_lines(&self, rows: usize, cols: usize) -> Vec<Option<Line>> {
        if !self.permissions_granted || rows == 0 {
            return Vec::new();
        }
        let indent = " ".repeat(HEADER_INDENT);
        let title = if self.preview_content.title.is_empty() {
            PREVIEW_PLACEHOLDER
        } else {
            self.preview_content.title.as_str()
        };
        let mut lines = vec![Some(compose(
            &[(&indent, Ink::Plain), (title, Ink::Muted)],
            content_cols(cols),
        ))];
        if rows < PREVIEW_HEAD {
            return lines;
        }
        lines.push(Some(divider_line(cols)));
        lines.extend(
            self.preview_body(rows.saturating_sub(PREVIEW_HEAD))
                .iter()
                .map(|line| Some(Line::new(truncate(line, cols)))),
        );
        lines
    }
}
