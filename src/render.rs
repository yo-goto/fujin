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

use crate::agent::AgentInfo;
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
    // 直前のペイン行の cwd（決定22）。ペイン行と同じペインを指すので、
    // 選択のハイライトも行クリックもペイン行と同じ扱いにする
    Cwd {
        entry: &'a Selectable,
        flat_index: usize,
        cwd: &'a str,
        // cwd に一致したヒットだけを持つ（ペイン名・タブ名のヒットは
        // この行のハイライトには関係しない）
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
// cwd行の字下げ。ペイン名の開始位置（4セル）よりさらに右へ寄せて、
// 隣のペイン行ではなく「上のペイン行の続き」として読ませる
const CWD_INDENT: usize = 6;
// ペイン名とカウンタ列のあいだに最低限空ける幅。
// 名前と数字がくっつくと、どこまでが名前か読めなくなる
const COUNTER_GAP: usize = 1;
// 右端に常に空ける幅。文字がサイドバーの縁に貼り付くと窮屈に見える。
// 左マージン（選択バーぶんの2セル）と揃えてある
const RIGHT_MARGIN: usize = 2;

// 文字を置いてよい幅。選択行の背景は右マージンも含めて塗るので、
// 背景のパディング（pad_to_width）はこれではなく cols を使うこと
fn content_cols(cols: usize) -> usize {
    cols.saturating_sub(RIGHT_MARGIN)
}
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

// カウンタ列の幅（決定22）。サブエージェント数 `+N`・未完了タスク数 `[M]` を
// それぞれ固定幅のフィールドに右揃えで置き、行をまたいで桁を揃える。
//
// 幅は**そのフレームに出るペイン行の実測最大**で決める。誰もカウンタを持っていない
// フレームでは 0 になり、ペイン名が幅をすべて使える。固定幅で常に予約すると、
// 静かなときにも右端が空白のまま失われる
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct CounterColumn {
    subagents: usize,
    open_tasks: usize,
}

impl CounterColumn {
    // 列全体の表示幅。両方あるときだけ、あいだに空白1つを挟む
    fn width(&self) -> usize {
        match (self.subagents, self.open_tasks) {
            (0, 0) => 0,
            (s, 0) | (0, s) => s,
            (s, t) => s + 1 + t,
        }
    }

    // この列に載る1行ぶんの文字列。持っていないカウンタのフィールドは空白で埋め、
    // 桁の位置を行ごとにずらさない
    fn render(&self, subagents: &str, open_tasks: &str) -> String {
        if self.width() == 0 {
            return String::new();
        }
        let mut out = pad_left(subagents, self.subagents);
        if self.subagents > 0 && self.open_tasks > 0 {
            out.push(' ');
        }
        out.push_str(&pad_left(open_tasks, self.open_tasks));
        out
    }
}

// ペイン行に出すカウンタの文字列。0 のときは出さない（決定11: 静かな行は静かに）
fn counter_labels(agent: Option<&AgentInfo>) -> (String, String) {
    let Some(agent) = agent else {
        return (String::new(), String::new());
    };
    let subagents = if agent.subagents > 0 {
        format!("+{}", agent.subagents)
    } else {
        String::new()
    };
    let open_tasks = if agent.open_tasks > 0 {
        format!("[{}]", agent.open_tasks)
    } else {
        String::new()
    };
    (subagents, open_tasks)
}

impl State {
    // このフレームのカウンタ列の幅。visible_rows() の結果から測るので、
    // 絞り込みで消えたペインは勘定に入らない
    pub(crate) fn counter_column(&self, rows: &[Row<'_>]) -> CounterColumn {
        let mut column = CounterColumn::default();
        for row in rows {
            let Row::Pane { entry, .. } = row else {
                continue;
            };
            let (subagents, open_tasks) = counter_labels(self.agents.get(&entry.pane_id));
            column.subagents = column.subagents.max(UnicodeWidthStr::width(&*subagents));
            column.open_tasks = column.open_tasks.max(UnicodeWidthStr::width(&*open_tasks));
        }
        column
    }

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
        // ヘッダ: 検索サブモード中はクエリ入力行、navモード中はモード名、
        // 通常表示ではブランディング文言。**常時1行を確保する** — モードの
        // 入退場でヘッダの有無が切り替わると、ツリー全体が1行分上下にずれる
        rows.push(Row::Header);
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

                // cwd はペイン行に混ぜず、続く1行として出す（決定22）。
                // show_cwd が false でも、cwd に一致した行だけは出す —
                // 画面に無い文字列でヒットしたように見せないため
                let cwd_hit = hit.filter(|h| h.field == Field::Cwd);
                if self.show_cwd || cwd_hit.is_some() {
                    if let Some(cwd) = self.pane_cwds.get(&entry.pane_id) {
                        rows.push(Row::Cwd {
                            entry,
                            flat_index: this_index,
                            cwd,
                            hit: cwd_hit,
                        });
                    }
                }
            }
        }
        rows
    }

    // 画面のこの行に載っているペイン（要件: docs/requirements/click-to-focus/）。
    // ヘッダ・タブ見出し行・一覧の外は None。
    // cwd行はペイン行と同じペインを指すので、そこをクリックしても同じように当たる
    pub(crate) fn pane_at_row(&self, row: usize) -> Option<u32> {
        match self.visible_rows().get(row)? {
            Row::Pane { entry, .. } | Row::Cwd { entry, .. } => Some(entry.pane_id),
            _ => None,
        }
    }

    // その行がハイライトされるか。ペイン行と cwd行で同じ判定を使い、
    // 2行が1つの帯に見えるようにする
    fn row_is_selected(&self, entry: &Selectable, flat_index: usize) -> bool {
        match &self.search {
            // 検索サブモード中にハイライトする行はカーソル（ペインID）で決まる
            Some(search) => search.cursor == Some(entry.pane_id),
            None => flat_index == self.selected,
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
        let all_rows = self.visible_rows();
        // カウンタ列の幅はフレーム全体で1つ。行ごとに測ると桁が揃わない（決定22）
        let column = self.counter_column(&all_rows);
        for (y, row) in all_rows.into_iter().enumerate() {
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
                    let is_selected = self.row_is_selected(entry, flat_index);
                    let row = self.pane_row(entry, is_selected, hit, column, cols);
                    print_text_with_coordinates(row, 0, y, None, None);
                }
                Row::Cwd {
                    entry,
                    flat_index,
                    cwd,
                    hit,
                } => {
                    let is_selected = self.row_is_selected(entry, flat_index);
                    let row = cwd_row(cwd, is_selected, hit, cols);
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
        // ツリーの行と同じく右端は空ける（ヘルプオーバーレイは別の面なので対象外）
        let cols = content_cols(cols);
        // 通常表示はブランディング文言。dim にしてモード名の強調色と区別し、
        // 角括弧でも囲まない — `[NAV]` と同じ見た目だとモードの一種に誤読される
        if self.search.is_none() && !self.nav_mode {
            return compose(&[("> fujin", Ink::Muted)], cols);
        }
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
    pub(crate) fn tab_heading(&self, tab: &TabInfo, cols: usize) -> Text {
        let marker = if tab.active { "▾" } else { "▸" };
        let prefix = format!("{} {} ", marker, tab.position + 1);
        let full_heading = format!("{}{}", prefix, tab.name);
        let title = truncate(&full_heading, content_cols(cols));
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

    // ペイン行1行ぶんの Text を組み立てる（状態アイコン・ペイン名・カウンタ列・
    // ハイライト込み）。cwd は別行なのでここには出てこない（決定22）。
    //
    // レイアウトは `{アイコン} {ペイン名} …余白… {カウンタ列}` で、カウンタ列は
    // 右端に揃える。ペイン名はカウンタ列を除いた残り幅に収め、名前の長さで
    // サブエージェント数・未完了タスク数が消えないようにする（決定21）
    pub(crate) fn pane_row(
        &self,
        entry: &Selectable,
        is_selected: bool,
        hit: Option<&Hit>,
        column: CounterColumn,
        cols: usize,
    ) -> Text {
        let agent = self.agents.get(&entry.pane_id);
        let icon = agent.map(|a| a.state.icon()).unwrap_or(" ");
        // 選択行は左端にバーを立てる。テーマの選択色が沈む配色でも
        // どこが選択中か一目で分かるようにするため（幅は2文字で固定し、
        // 状態アイコンの color_range 2..3 をずらさない）
        let prefix = if is_selected { "▌ " } else { "  " };
        let head = format!("{}{} ", prefix, icon); // "▌ {icon} " / "  {icon} "
        let head_width = UnicodeWidthStr::width(head.as_str());

        let (subagents, open_tasks) = counter_labels(agent);
        let counters = column.render(&subagents, &open_tasks);
        let counters_width = UnicodeWidthStr::width(counters.as_str());
        // カウンタ列を持つフレームでは、この行に数字が無くても列ぶんは空けておく。
        // 空けないと、右端に揃えたはずの桁が行によってずれる
        let reserved = if counters.is_empty() {
            head_width
        } else {
            head_width + COUNTER_GAP + counters_width
        };

        // 右マージンぶんは文字を置かない。カウンタ列もそこまでで揃える
        let inner = content_cols(cols);
        let title_budget = inner.saturating_sub(reserved);
        let title_original_len = entry.title.chars().count();
        let (title, title_dropped) = fold_to_width(&entry.title, title_budget);

        let mut label = format!("{}{}", head, title);
        if !counters.is_empty() {
            // ペイン名の長さに関わらず、カウンタ列は右端で揃える
            let filler =
                inner.saturating_sub(UnicodeWidthStr::width(label.as_str()) + counters_width);
            label.push_str(&" ".repeat(filler));
            label.push_str(&counters);
            // アイコンとカウンタ列だけで幅を使い切るほど狭いときの保険。
            // はみ出すと選択背景が端末側で折り返して次の行を汚す
            label = truncate(&label, inner);
        }
        if is_selected {
            // 選択背景がサイドバー幅いっぱいに伸びるよう空白で埋める。
            // 埋めないと文字列の長さぶんしか色が乗らず、帯に見えない
            label = pad_to_width(label, cols);
        }

        // ハイライトする場所が、そのままヒットしたフィールドの提示になる。
        // ペイン名は行全体とは別の予算で畳んでいるので、可視範囲も
        // ペイン名自身の畳んだ結果から判定する
        let highlight = hit.filter(|h| h.field == Field::Title).map(|hit| {
            fold_highlight_indices(
                &hit.indices,
                &title,
                title_dropped,
                title_original_len,
                head.chars().count(),
            )
        });

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

// cwd行1行ぶんの Text（決定22）。ペイン行の続きとして読めるよう字下げして dim で出す。
//
// パスは末尾のディレクトリ名のほうが識別に効くので、収まらないときは
// 切り詰め（末尾 `…`）ではなく先頭省略で畳む
pub(crate) fn cwd_row(cwd: &str, is_selected: bool, hit: Option<&Hit>, cols: usize) -> Text {
    // 選択中は左端のバーをこの行まで伸ばし、ペイン行と1つの帯に見せる
    let bar = if is_selected { "▌" } else { " " };
    let indent = format!("{}{}", bar, " ".repeat(CWD_INDENT.saturating_sub(1)));
    let inner = content_cols(cols);
    let (path, dropped) = truncate_start(cwd, inner.saturating_sub(CWD_INDENT));

    // 字下げだけで幅を使い切るほど狭いときの保険。はみ出した行は端末側で
    // 折り返り、選択背景が次の行を汚す（docs/issues/sidebar-bottom-highlight-glitch.md）
    let mut label = truncate(&format!("{}{}", indent, path), inner);
    if is_selected {
        label = pad_to_width(label, cols);
    }
    let mut text = Text::new(&label);
    let end = label.chars().count();
    if !is_selected && end > CWD_INDENT {
        // cwd は主役（ペイン名・カウンタ列）ではないので落として出す。
        // 選択行では落とさない — 帯の中でさらに沈むと読めなくなる
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
    if is_selected {
        text = text.selected().opaque().color_range(2, 0..1);
    }
    text
}

// 中身がパスかどうか。Claude Code はペイン名に cwd をそのまま入れることがあり、
// その場合は末尾を切ると `/Users/example/develo…` のようにどれも同じ見た目になる
fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("~/")
}

// 表示幅 max に畳む（決定22）。パスは先頭省略、それ以外は切り詰め。
// 返り値は (畳んだ文字列, 先頭で落とした文字数)
fn fold_to_width(s: &str, max: usize) -> (String, usize) {
    if looks_like_path(s) {
        truncate_start(s, max)
    } else {
        (truncate(s, max), 0)
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

// 左側に空白を足して表示幅 width に揃える（カウンタ列の右揃え用）
fn pad_left(s: &str, width: usize) -> String {
    let pad = width.saturating_sub(UnicodeWidthStr::width(s));
    format!("{}{}", " ".repeat(pad), s)
}

// 表示セル幅ベースの先頭省略。先頭を落として `…` に畳み、末尾を残す（決定22）。
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
    // 省略記号（幅1）ぶんの余地を残しながら、末尾から幅が max-1 を超える手前まで拾う
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
