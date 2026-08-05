// サイドバーの描画。
//
// レイアウト: navモード名・検索クエリのヘッダ1行（navモード外のときは無し）に
// 続けて、タブ見出し行 > 配下のペイン行 をタブ順で縦に並べる。
//
// navモード外でヘッダを出さないのは、セッション名を zellij 本体のトップバーが
// `Zellij (セッション名)` の形で常時出しており、重複が視認性を下げるため
//（要件: docs/requirements/sidebar-tree/）。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::*;

use crate::search::{Field, Hit};
use crate::{Selectable, State};

// 画面に縦に積む1行ぶんの中身。
//
// 描画（draw）とクリック位置の逆引き（pane_at_row、要件:
// docs/requirements/click-to-focus/）が**同じ並びを共有する**ために切り出してある。
// 行の増減を伴うレイアウト変更は必ず visible_rows() 側で行うこと。
// 描画だけ直すとクリックが行ずれする
pub(crate) enum Row<'a> {
    // navモード名 / 検索クエリの入力行
    Header,
    // ヘルプオーバーレイの1行（要件: docs/requirements/nav-mode/）。
    // 開いている間はサイドバー全体がこの行だけになる
    Help(&'a HelpRow),
    // 検索が0件のときの通知行
    NoMatch,
    Tab(&'a TabInfo),
    Pane {
        entry: &'a Selectable,
        // 非検索時の選択判定に使うフラットな通し番号（self.selected と突き合わせる）
        flat_index: usize,
        hit: Option<&'a Hit>,
    },
}

// ヘルプオーバーレイの行の種類（中身は nav::help_lines() が持つ）。
// 整形（キー列の幅・左マージン）と配色はこちら側で決める
pub(crate) enum HelpRow {
    // モード名とその添え字。例: ("[NAV]", "keys")
    Title(&'static str, &'static str),
    // キーと、それが何をするか
    Entry(&'static str, &'static str),
    Blank,
    // 操作の説明ではない補足。例: "press any key to close"
    Note(&'static str),
}

// 行の左に空ける余白。文字を左端に貼り付けると窮屈に見える
const HELP_INDENT: usize = 2;
// キー列の幅（説明との間の空白を含む）。説明の開始位置をここで揃える。
// いちばん長いキー（`j k up down tab`）と、いちばん長い説明（`cancel search`）が
// 左マージン込みで幅32（決定3）にちょうど収まる値
const HELP_KEY_COLUMN: usize = 17;

// 文字に与える意味。zellij のテーマ側の色をそのまま借りる（決定11と同じ方針で、
// 色そのものを設定項目にはしない）
#[derive(Clone, Copy)]
enum Ink {
    // 既定の文字色。読ませたい本文
    Plain,
    // モード名。ヘッダの `[NAV]` と同じ強調色に揃える
    Tag,
    // キーそのもの。zellij 本体の status-bar がキーを強調するのに倣う
    Key,
    // 添え物（`:help` の label 部分・補足行）。落として主役を目立たせる
    Muted,
}

// 意味付きの断片を1行に組み立てる。
//
// 色の指定は文字位置で行うため、文言を直すたびに位置を数え直すことになる。
// 断片の並びから位置を計算させて、その手間と数え間違いを無くす
fn compose(segments: &[(&str, Ink)], cols: usize) -> Text {
    let mut line = String::new();
    let mut spans = Vec::with_capacity(segments.len());
    for (fragment, ink) in segments {
        let start = line.chars().count();
        line.push_str(fragment);
        spans.push((start, line.chars().count(), *ink));
    }
    let full_len = line.chars().count();
    let line = truncate(&line, cols);
    let limit = colorable_char_limit(line.chars().count(), full_len);
    let mut text = Text::new(&line);
    for (start, end, ink) in spans {
        let end = end.min(limit);
        if start >= end {
            continue;
        }
        text = match ink {
            Ink::Plain => text,
            Ink::Tag => text.color_range(3, start..end),
            Ink::Key => text.color_range(2, start..end),
            Ink::Muted => text.dim_range(start..end),
        };
    }
    text
}

impl State {
    // 画面に並ぶ行を上から順に組み立てる。`rows`（画面高）での打ち切りは
    // 呼び出し側の責務 — 行の並び自体は高さに依らないため
    pub(crate) fn visible_rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        if !self.permissions_granted {
            return rows;
        }
        // ヘルプオーバーレイはサイドバー全体を覆う。ツリーも一緒には出さない
        if self.help_overlay {
            return self.help_lines().iter().map(Row::Help).collect();
        }
        // ヘッダ: 検索サブモード中はクエリ入力行、navモード中はモード名。
        // navモード外では出さない
        if self.search.is_some() || self.nav_mode {
            rows.push(Row::Header);
        }
        if let Some(search) = &self.search {
            // 0件は空リストではなく明示する。絞り込みが効いているのか
            // 描画が壊れているのか区別できないため
            if search.hits.is_empty() {
                rows.push(Row::NoMatch);
                return rows;
            }
        }

        let mut flat_index = 0;
        let mut sorted_tabs: Vec<&TabInfo> = self.tabs.iter().collect();
        sorted_tabs.sort_by_key(|t| t.position);
        for tab in sorted_tabs {
            // 絞り込み中、配下に一致ペインを持たないタブは見出しごと消す。
            // flat_index は非検索時の選択にしか使わないので、間引いてもずれない
            if let Some(search) = &self.search {
                let has_hit = self.selectable.iter().any(|e| {
                    e.tab_position == tab.position && search.hits.contains_key(&e.pane_id)
                });
                if !has_hit {
                    continue;
                }
            }
            rows.push(Row::Tab(tab));

            for entry in &self.selectable {
                if entry.tab_position != tab.position {
                    continue;
                }
                let this_index = flat_index;
                flat_index += 1;
                // 絞り込みで外れたペインは描かない
                let hit = self
                    .search
                    .as_ref()
                    .and_then(|s| s.hits.get(&entry.pane_id));
                if self.search.is_some() && hit.is_none() {
                    continue;
                }
                rows.push(Row::Pane {
                    entry,
                    flat_index: this_index,
                    hit,
                });
            }
        }
        rows
    }

    // 画面のこの行に載っているペイン（要件: docs/requirements/click-to-focus/）。
    // ヘッダ・タブ見出し行・一覧の外は None
    pub(crate) fn pane_at_row(&self, row: usize) -> Option<u32> {
        match self.visible_rows().get(row)? {
            Row::Pane { entry, .. } => Some(entry.pane_id),
            _ => None,
        }
    }

    pub(crate) fn draw(&self, rows: usize, cols: usize) {
        if !self.permissions_granted {
            print_text_with_coordinates(
                Text::new("permissions required (press y)"),
                0,
                0,
                None,
                None,
            );
            return;
        }
        for (y, row) in self.visible_rows().into_iter().enumerate() {
            if y >= rows {
                break;
            }
            match row {
                Row::Header => {
                    print_text_with_coordinates(self.header_line(cols), 0, y, None, None);
                }
                Row::Help(row) => {
                    print_text_with_coordinates(self.help_line(row, cols), 0, y, None, None);
                }
                Row::NoMatch => {
                    // 操作の対象ではない通知なので、一覧の行より落として出す
                    print_text_with_coordinates(
                        compose(&[("  一致なし", Ink::Muted)], cols),
                        0,
                        y,
                        None,
                        None,
                    );
                }
                Row::Tab(tab) => {
                    print_text_with_coordinates(self.tab_heading(tab, cols), 0, y, None, None);
                }
                Row::Pane {
                    entry,
                    flat_index,
                    hit,
                } => {
                    // 検索サブモード中にハイライトする行はカーソル（ペインID）で決まる
                    let is_selected = match &self.search {
                        Some(search) => search.cursor == Some(entry.pane_id),
                        None => flat_index == self.selected,
                    };
                    let row = self.pane_row(entry, is_selected, hit, cols);
                    print_text_with_coordinates(row, 0, y, None, None);
                }
            }
        }
    }

    // ヘッダ1行（要件: docs/requirements/nav-mode/ の操作ヒント）。
    //
    // サイドバー幅は32文字（決定3）で全キーの説明は載らないので、常時出すのは
    // モード名とヘルプ・退出キーだけに絞り、詳細は `?` のヘルプオーバーレイへ
    // 追い出してある。文言は英語で統一する
    pub(crate) fn header_line(&self, cols: usize) -> Text {
        let Some(search) = &self.search else {
            return compose(
                &[
                    ("[NAV]", Ink::Tag),
                    ("  ", Ink::Plain),
                    ("?", Ink::Key),
                    (":help", Ink::Muted),
                    ("  ", Ink::Plain),
                    ("esc", Ink::Key),
                    (":exit", Ink::Muted),
                ],
                cols,
            );
        };
        // 検索サブモード中はクエリ入力行が主役。操作ヒントは右端へ寄せ、クエリが
        // 伸びてぶつかるところまで来たら入力中の文字列のほうを優先して落とす
        let query = format!("{}▏", search.query);
        // 余白は表示セル幅で数える。クエリに全角文字が入ると文字数とセル数が
        // ずれ、操作ヒントが右端からはみ出す
        let hint_width = UnicodeWidthStr::width("?:help");
        let pad = cols
            .saturating_sub(UnicodeWidthStr::width(query.as_str()) + 1) // 先頭の `/` のぶん
            .saturating_sub(hint_width);
        let mut segments = vec![("/", Ink::Tag), (query.as_str(), Ink::Plain)];
        let spacer = " ".repeat(pad);
        if pad > 0 {
            segments.push((spacer.as_str(), Ink::Plain));
            segments.push(("?", Ink::Key));
            segments.push((":help", Ink::Muted));
        }
        compose(&segments, cols)
    }

    // ヘルプオーバーレイの1行。左マージンは描画位置（x）ではなく行の中に
    // 持たせる — 画面座標を行ごとに変えると、行の並びと描画がずれやすい
    pub(crate) fn help_line(&self, row: &HelpRow, cols: usize) -> Text {
        let indent = " ".repeat(HELP_INDENT);
        match row {
            HelpRow::Title(tag, rest) => compose(
                &[
                    (&indent, Ink::Plain),
                    (tag, Ink::Tag),
                    (" ", Ink::Plain),
                    (rest, Ink::Muted),
                ],
                cols,
            ),
            HelpRow::Entry(keys, description) => {
                // キー列は幅を固定して説明の開始位置を揃える。キーが長すぎて
                // はみ出す場合は空白1文字だけ空けて続ける（列は崩れるが、
                // 説明が消えるよりはよい）
                let pad = HELP_KEY_COLUMN
                    .saturating_sub(UnicodeWidthStr::width(*keys))
                    .max(1);
                let gap = " ".repeat(pad);
                compose(
                    &[
                        (&indent, Ink::Plain),
                        (keys, Ink::Key),
                        (&gap, Ink::Plain),
                        (description, Ink::Plain),
                    ],
                    cols,
                )
            }
            HelpRow::Blank => Text::new(""),
            HelpRow::Note(note) => compose(&[(&indent, Ink::Plain), (note, Ink::Muted)], cols),
        }
    }

    // タブ見出し行1行ぶんの Text を組み立てる
    fn tab_heading(&self, tab: &TabInfo, cols: usize) -> Text {
        let marker = if tab.active { "▾" } else { "▸" };
        let prefix = format!("{} {} ", marker, tab.position + 1);
        let full_heading = format!("{}{}", prefix, tab.name);
        let title = truncate(&full_heading, cols);
        let mut text = Text::new(&title);
        if tab.active {
            text = text.color_range(0, ..title.chars().count());
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

    // ペイン行1行ぶんの Text を組み立てる（状態アイコン・カウンタ・cwd・ハイライト込み）。
    //
    // 幅が足りないときに削る優先順位は cwd → ペイン名（決定21）。サブエージェント数
    // `+N`・未完了タスク数 `[M]` は幅を先に確保し、最後まで削らない
    pub(crate) fn pane_row(
        &self,
        entry: &Selectable,
        is_selected: bool,
        hit: Option<&Hit>,
        cols: usize,
    ) -> Text {
        let agent = self.agents.get(&entry.pane_id);
        let icon = agent.map(|a| a.state.icon()).unwrap_or(" ");
        // 選択行は左端にバーを立てる。テーマの選択色が沈む配色でも
        // どこが選択中か一目で分かるようにするため（幅は2文字で固定し、
        // 状態アイコンの color_range 2..3 をずらさない）
        let prefix = if is_selected { "▌ " } else { "  " };
        let head = format!("{}{} ", prefix, icon); // "▌ {icon} " / "  {icon} "

        let mut counters = String::new();
        if let Some(a) = agent {
            if a.subagents > 0 {
                counters.push_str(&format!(" +{}", a.subagents));
            }
            if a.open_tasks > 0 {
                counters.push_str(&format!(" [{}]", a.open_tasks));
            }
        }

        // ペイン名はアイコンとカウンタぶんを引いた残り幅に収める。カウンタは
        // 切り詰めの対象にしない — 名前の長さでサブエージェント数・未完了
        // タスク数が消えるのを防ぐ（決定21）
        let reserved =
            UnicodeWidthStr::width(head.as_str()) + UnicodeWidthStr::width(counters.as_str());
        let title_budget = cols.saturating_sub(reserved);
        let title_original_len = entry.title.chars().count();
        let title = truncate(&entry.title, title_budget);
        let title_visible_len = title.chars().count();

        let mut label = format!("{}{}{}", head, title, counters);

        // cwd にヒットしたペインは show_cwd が false でも cwd を出す。画面に
        // 無い文字列でヒットしたように見せないための例外で、この場合だけは
        // 幅が足りなくても出す。それ以外（show_cwd 設定によるもの）は削る
        // 優先順位の最下位で、幅が無ければ丸ごと出さない（決定21）
        let cwd_hit = matches!(hit, Some(h) if h.field == Field::Cwd);
        let show_cwd_here = self.show_cwd || cwd_hit;
        let mut cwd_offset = None;
        if show_cwd_here {
            if let Some(cwd) = self.pane_cwds.get(&entry.pane_id) {
                let fits = UnicodeWidthStr::width(label.as_str())
                    + 2
                    + UnicodeWidthStr::width(cwd.as_str())
                    <= cols;
                if cwd_hit || fits {
                    cwd_offset = Some(label.chars().count() + 2);
                    label.push_str(&format!("  {}", cwd));
                }
            }
        }
        let full_len = label.chars().count();
        // 通常経路（ペイン名・カウンタ）は既に予算内。cwd をヒット表示のため
        // 強制的に足した場合だけ、ここでまだ幅を超えていることがある
        let mut label = truncate(&label, cols);
        // ハイライトする場所が、そのままヒットしたフィールドの提示になる
        let highlight = hit.and_then(|hit| match hit.field {
            Field::Title => {
                // ペイン名は行全体とは別に独自の予算で切り詰め済みなので、
                // 可視範囲もペイン名自身の切り詰め結果から判定する
                let limit = colorable_char_limit(title_visible_len, title_original_len);
                let indices: Vec<usize> = hit
                    .indices
                    .iter()
                    .filter(|&&i| i < limit)
                    .map(|&i| i + 4) // "▌ {icon} " の4文字ぶん
                    .collect();
                Some(indices)
            }
            Field::Cwd => {
                cwd_offset.map(|o| shift_highlight_indices(&hit.indices, o, &label, full_len))
            }
            Field::Tab => None, // タブ見出し側で描いている
        });
        if is_selected {
            // 選択背景がサイドバー幅いっぱいに伸びるよう空白で埋める。
            // 埋めないと文字列の長さぶんしか色が乗らず、帯に見えない
            label = pad_to_width(label, cols);
        }
        let mut text = Text::new(&label);
        if let Some(a) = agent {
            // 状態アイコン部分（先頭2..3文字目）に状態色
            text = text.color_range(a.state.color(), 2..3);
        }
        if let Some(indices) = highlight.filter(|i| !i.is_empty()) {
            // レベル1で固定（決定11のv1スコープ: 設定項目は増やさない）
            text = text.color_indices(1, indices);
        }
        if is_selected {
            // opaque を付けないと背景が透けて選択色が沈む
            text = text.selected().opaque().color_range(2, 0..1);
        }
        text
    }
}

// 切り詰め後の行で色を乗せてよい文字数の上限。
// 切り詰められた行の末尾は … なので、そこには色を乗せない
fn colorable_char_limit(visible: usize, original_len: usize) -> usize {
    if visible < original_len {
        visible.saturating_sub(1)
    } else {
        visible
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

// 表示幅が cols に達するまで右側を空白で埋める。
// 全角文字（2セル）が混ざるので表示幅で数える — 文字数で数えると実際の
// セル幅を超えてパディングしてしまい、選択背景が端末側で折り返されて
// 次の行にはみ出す（docs/issues/sidebar-bottom-highlight-glitch.md）
pub(crate) fn pad_to_width(mut s: String, cols: usize) -> String {
    let pad = cols.saturating_sub(UnicodeWidthStr::width(s.as_str()));
    s.push_str(&" ".repeat(pad));
    s
}

// 表示セル幅ベースの切り詰め。全角文字（CJK）は2セル分として数える
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
