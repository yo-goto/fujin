// サイドバーの描画。
//
// レイアウト: ヘッダ3行（ブランド行・モード行・境界線。要件:
// docs/requirements/sidebar-tree/sidebar-header.feature）に続けて、
// タブ見出し行 > 配下のペイン行 をタブ順で縦に並べる。
//
// ヘッダは**モードによらず常に3行**を占める。行数が変わると、モードの入退場の
// たびにツリー全体が上下にずれる。セッション名は出さない — zellij 本体の
// トップバーが `Zellij (セッション名)` の形で常時出しており、重複が視認性を下げる。
//
// 行が画面高に収まらないときは表示範囲を選択行へ寄せる（縦スクロール。
// 要件: docs/requirements/sidebar-tree/sidebar-scroll.feature）。ヘッダは
// 固定で、その下の一覧だけが動く。

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
    // ヘッダ1行目。`▲ fujin` を常時出し、三角の色でいまのモードを示す
    Brand,
    // ヘッダ2行目。モード中はモード名と操作ヒント、ツリー表示は待ち件数。
    // どちらも無いときは空のまま（行そのものは消さない）
    Mode,
    // ヘッダ3行目。ヘッダ（chrome）とツリー（content）の境目を示す横線
    Divider,
    // ヘルプオーバーレイの1行（要件: docs/requirements/nav-mode/）。
    // 開いている間はサイドバー全体がこの行だけになる
    Help(&'a HelpRow),
    // 一覧が空であることの通知行（検索の0件・トリアージの対象なし）。
    // 空リストのまま描くと、絞り込みが効いているのか描画が壊れているのか区別できない
    Notice(&'static str),
    // 表示範囲の外に行があることを示す上下端のあふれマーカー行。
    // 出さないと、一覧がそこで終わっているのか隠れているのか区別できない
    Overflow {
        hidden: usize,
        above: bool,
    },
    Tab(&'a TabInfo),
    Pane {
        entry: &'a Selectable,
        // 非検索時の選択判定に使うフラットな通し番号（self.selected と突き合わせる）
        flat_index: usize,
        hit: Option<&'a Hit>,
    },
    // トリアージ一覧の1行（要件: docs/requirements/triage-mode/）。
    // タブ見出し行を持たないフラットな並びなので、所属タブ名を行に併記する
    Triage {
        entry: &'a Selectable,
        tab_name: &'a str,
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
    // モード名とその添え字。例: ("[nav]", "keys")
    Title(&'static str, &'static str),
    // キーと、それが何をするか
    Entry(&'static str, &'static str),
    Blank,
    // 操作の説明ではない補足。例: "press any key to close"
    Note(&'static str),
}

// 行の左に空ける余白。文字を左端に貼り付けると窮屈に見える
const HELP_INDENT: usize = 2;
// ヘッダの文字を置き始める位置（ブランド名・モード行）。ヘッダは chrome なので
// ツリー側の階段（x=0/2/4/6）には乗せず、x=2 に揃えてヘッダだけで1つのブロックに
// 見せる。三角だけがこの左に出る（x=0）
const HEADER_INDENT: usize = 2;
// cwd行の字下げ。ペイン名の開始位置（4セル）よりさらに右へ寄せて、
// 隣のペイン行ではなく「上のペイン行の続き」として読ませる
const CWD_INDENT: usize = 6;
// ペイン名と右寄せの列（カウンタ列・トリアージ行のタブ名列）のあいだに
// 最低限空ける幅。名前と列がくっつくと、どこまでが名前か読めなくなる
const COLUMN_GAP: usize = 1;
// トリアージ行のタブ名列に使ってよい幅の割合（内容幅の 1/N）。
// タブ名が長くてもペイン名を潰さないための上限
const TRIAGE_TAB_SHARE: usize = 3;
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
    // モード名。ヘッダの `[nav]` と同じ強調色に揃える
    Tag,
    // キーそのもの。zellij 本体の status-bar がキーを強調するのに倣う
    Key,
    // 添え物（`:help` の label 部分・補足行）。落として主役を目立たせる
    Muted,
    // 強調色をレベル指定で乗せる。ブランド行の三角のように、色そのものが
    // 情報を持つ断片に使う
    Accent(usize),
    // 強調色を落として乗せる（`color_range` と `dim_range` の併用）。待ち件数の
    // ように「色で意味は伝えたいが主役ではない」断片に使う。
    // 併用が効かない環境では色が落ちて dim だけが残る
    MutedAccent(usize),
}

// ブランド行の三角に乗せる色（要件: sidebar-header）。navモード・検索サブモードは
// レベル2（green）で、zellij のタブバーがアクティブなタブに使う色に対応させる
const NAV_LEVEL: usize = 2;
// トリアージモードはレベル0（orange）。緊急度を連想させる側へ寄せる。
// 待ち件数の数字にも同じ色を薄く乗せ、「トリアージすべき件数」だと読ませる
const TRIAGE_LEVEL: usize = 0;

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
            Ink::Accent(level) => text.color_range(level, start..end),
            Ink::MutedAccent(level) => text.color_range(level, start..end).dim_range(start..end),
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
        // ヘッダは**常時3行を確保する**（要件: sidebar-header）。中身がモードで
        // 変わっても行数は変えない — 入退場でヘッダの高さが動くと、ツリー全体が
        // そのぶん上下にずれる
        rows.push(Row::Brand);
        rows.push(Row::Mode);
        rows.push(Row::Divider);
        // トリアージモード中はツリー表示を隠し、一覧だけを出す（要件: triage-mode）
        if self.triage.is_some() {
            let entries = self.triage_entries();
            if entries.is_empty() {
                rows.push(Row::Notice("対象なし"));
                return rows;
            }
            for entry in entries {
                rows.push(Row::Triage {
                    entry,
                    tab_name: self.tab_name(entry.tab_position),
                });
            }
            return rows;
        }
        if let Some(search) = &self.search {
            // 0件は空リストではなく明示する。絞り込みが効いているのか
            // 描画が壊れているのか区別できないため
            if search.hits.is_empty() {
                rows.push(Row::Notice("一致なし"));
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

    // 画面に実際に載る行。visible_rows() の並びから表示範囲ぶんを切り出し、
    // 隠れた行があれば上下端にあふれマーカー行を足す。
    //
    // `rows` が 0 のときは切り出さない。行クリックの逆引きが最初の描画より前に
    // 来た場合（State::viewport_rows の初期値）で、スクロールは起きていない
    pub(crate) fn screen_rows(&self, rows: usize) -> Vec<Row<'_>> {
        let mut all = self.visible_rows();
        if rows == 0 || all.len() <= rows {
            return all;
        }
        let pinned = pinned_rows(&all);
        // 画面がヘッダ（3行）ぶんの高さも無いときは、入るところまでを出して終わる。
        // 一覧に割ける高さが無いので、スクロールもあふれマーカーも出番がない
        if rows <= pinned {
            all.truncate(rows);
            return all;
        }
        let area = rows - pinned;
        let list_len = all.len() - pinned;
        // 描画とクリックの逆引きで同じ位置を使う。State::scroll は描画時に
        // 寄せた値だが、そのあと一覧が縮んでいることもあるので clamp は掛け直す
        let scroll = reconcile_scroll(list_len, area, self.scroll, None);
        let shown = rows_shown(list_len, area, scroll);

        let list = all.split_off(pinned);
        let mut screen = all;
        if scroll > 0 {
            screen.push(Row::Overflow {
                hidden: scroll,
                above: true,
            });
        }
        screen.extend(list.into_iter().skip(scroll).take(shown));
        let below = list_len - scroll - shown;
        if below > 0 {
            screen.push(Row::Overflow {
                hidden: below,
                above: false,
            });
        }
        screen
    }

    // 選択行が visible_rows() のどこにあるか（先頭行, 末尾行）。
    // ペイン行と cwd行のように複数行が1つの帯になるので範囲で返す
    fn selected_span(&self, all: &[Row<'_>]) -> Option<(usize, usize)> {
        let triage_cursor = self.triage_cursor();
        let mut span: Option<(usize, usize)> = None;
        for (index, row) in all.iter().enumerate() {
            let selected = match row {
                Row::Pane {
                    entry, flat_index, ..
                }
                | Row::Cwd {
                    entry, flat_index, ..
                } => self.row_is_selected(entry, *flat_index),
                Row::Triage { entry, .. } => triage_cursor == Some(entry.pane_id),
                _ => false,
            };
            if selected {
                span = Some(match span {
                    Some((first, _)) => (first, index),
                    None => (index, index),
                });
            }
        }
        span
    }

    // 表示範囲を選択行へ寄せ直す。描画のたびに呼ぶ（画面高は描画時にしか
    // 分からず、行の増減も選択の移動もここで一度に吸収できるため）。
    //
    // スクロール位置は選択（決定13で兄弟インスタンスへ配る）と画面高から
    // 導出されるローカルな表示状態なので、それ自体は配らない
    pub(crate) fn reconcile_viewport(&mut self, rows: usize) {
        self.viewport_rows = rows;
        let (list_len, area, anchor) = {
            let all = self.visible_rows();
            let pinned = pinned_rows(&all);
            let anchor = self
                .selected_span(&all)
                .map(|(first, last)| (first - pinned, last - pinned));
            (all.len() - pinned, rows.saturating_sub(pinned), anchor)
        };
        self.scroll = reconcile_scroll(list_len, area, self.scroll, anchor);
    }

    // 画面のこの行に載っているペイン（要件: docs/requirements/click-to-focus/）。
    // ヘッダ・タブ見出し行・あふれマーカー行・一覧の外は None。
    // cwd行はペイン行と同じペインを指すので、そこをクリックしても同じように当たる
    pub(crate) fn pane_at_row(&self, row: usize) -> Option<u32> {
        match self.screen_rows(self.viewport_rows).get(row)? {
            Row::Pane { entry, .. } | Row::Cwd { entry, .. } | Row::Triage { entry, .. } => {
                Some(entry.pane_id)
            }
            _ => None,
        }
    }

    // タブ位置に対応するタブ名。一覧が古くて引けないときは空文字
    pub(crate) fn tab_name(&self, position: usize) -> &str {
        self.tabs
            .iter()
            .find(|t| t.position == position)
            .map(|t| t.name.as_str())
            .unwrap_or("")
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
        if rows == 0 {
            return;
        }
        // 画面高での打ち切りは screen_rows() が済ませている。ここで改めて
        // 打ち切ると、あふれマーカー行の勘定と食い違ってクリックが行ずれする
        let screen = self.screen_rows(rows);
        // カウンタ列の幅はフレーム全体で1つ。行ごとに測ると桁が揃わない（決定22）
        let column = self.counter_column(&screen);
        // トリアージ行のタブ名列も同じ理由でフレーム全体で1つ
        let tab_column = self.triage_tab_column(&screen, cols);
        // カーソルは一覧から導出されるので、行ごとに引き直さず1度だけ求める
        let triage_cursor = self.triage_cursor();
        for (y, row) in screen.into_iter().enumerate() {
            match row {
                Row::Brand => {
                    print_text_with_coordinates(self.brand_line(cols), 0, y, None, None);
                }
                Row::Mode => {
                    print_text_with_coordinates(self.mode_line(cols), 0, y, None, None);
                }
                Row::Divider => {
                    print_text_with_coordinates(divider_line(cols), 0, y, None, None);
                }
                Row::Help(row) => {
                    print_text_with_coordinates(self.help_line(row, cols), 0, y, None, None);
                }
                Row::Notice(notice) => {
                    // 操作の対象ではない通知なので、一覧の行より落として出す
                    let label = format!("  {}", notice);
                    print_text_with_coordinates(
                        compose(&[(&label, Ink::Muted)], cols),
                        0,
                        y,
                        None,
                        None,
                    );
                }
                Row::Overflow { hidden, above } => {
                    print_text_with_coordinates(
                        overflow_row(hidden, above, cols),
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
                Row::Triage { entry, tab_name } => {
                    let is_selected = triage_cursor == Some(entry.pane_id);
                    let row = self.triage_row(entry, tab_name, is_selected, tab_column, cols);
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

    // ヘッダ1行目（ブランド行）。`▲ fujin` をモードによらず常に出す。
    //
    // 色を乗せるのは三角1文字だけで、ブランド名は dim のまま。名前まで色を付けると
    // ツリーの状態アイコンの色分けと喧嘩する（要件: sidebar-header）
    pub(crate) fn brand_line(&self, cols: usize) -> Text {
        // 三角が x=0、ブランド名が x=2（HEADER_INDENT）に来る
        compose(
            &[("▲", self.brand_ink()), (" fujin", Ink::Muted)],
            content_cols(cols),
        )
    }

    // ブランド行の三角に乗せる色。**テキストを読まなくても色だけでモードが判別
    // できる**ようにするのが狙いで、モード行のモード名はその裏取りという位置づけ
    fn brand_ink(&self) -> Ink {
        if self.triage.is_some() {
            Ink::Accent(TRIAGE_LEVEL)
        } else if self.nav_mode || self.search.is_some() {
            Ink::Accent(NAV_LEVEL)
        } else {
            Ink::Muted
        }
    }

    // ヘッダ2行目（モード行）。モード中は操作ヒント（要件:
    // docs/requirements/nav-mode/）、ツリー表示は待ち件数。
    //
    // サイドバー幅は32文字（決定3）で全キーの説明は載らないので、常時出すのは
    // モード名とヘルプ・退出キーだけに絞り、詳細は `?` のヘルプオーバーレイへ
    // 追い出してある。文言は英語で統一する
    pub(crate) fn mode_line(&self, cols: usize) -> Text {
        // ツリーの行と同じく右端は空ける（ヘルプオーバーレイは別の面なので対象外）
        let cols = content_cols(cols);
        let indent = " ".repeat(HEADER_INDENT);
        // トリアージモード中はモード名を差し替える。Esc の行き先が navモードの
        // ツリー表示（退場ではない）なので、操作ヒントも `exit` ではなく `back`
        if self.triage.is_some() {
            return compose(
                &[
                    (&indent, Ink::Plain),
                    ("[tri]", Ink::Tag),
                    ("  ", Ink::Plain),
                    ("?", Ink::Key),
                    (":help", Ink::Muted),
                    ("  ", Ink::Plain),
                    ("esc", Ink::Key),
                    (":back", Ink::Muted),
                ],
                cols,
            );
        }
        if let Some(search) = &self.search {
            return search_mode_line(&search.query, &indent, cols);
        }
        if self.nav_mode {
            return compose(
                &[
                    (&indent, Ink::Plain),
                    ("[nav]", Ink::Tag),
                    ("  ", Ink::Plain),
                    ("?", Ink::Key),
                    (":help", Ink::Muted),
                    ("  ", Ink::Plain),
                    ("esc", Ink::Key),
                    (":exit", Ink::Muted),
                ],
                cols,
            );
        }
        // ツリー表示は待ち件数。0件なら**空のまま**にする — `all clear` のような
        // 文言は装飾のための装飾で、対応が不要であることは何も無いことで伝わる
        let waiting = self.waiting_count();
        if waiting == 0 {
            return Text::new("");
        }
        let count = waiting.to_string();
        compose(
            &[
                (&indent, Ink::Plain),
                // 数字だけトリアージモードの色を薄く乗せ、何の件数かを色でも示す
                (&count, Ink::MutedAccent(TRIAGE_LEVEL)),
                (" waiting", Ink::Muted),
            ],
            cols,
        )
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
            head_width + COLUMN_GAP + counters_width
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

    // トリアージ行のタブ名列の幅（要件: docs/requirements/triage-mode/）。
    // カウンタ列（決定22）と同じくフレーム内の実測最大で決めて、行をまたいで
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

    // トリアージ一覧の1行ぶんの Text（要件: docs/requirements/triage-mode/）。
    //
    // レイアウトは `{アイコン} {ペイン名} …余白… {タブ名}` で、ペイン行の
    // カウンタ列（決定22）の位置にタブ名を置いた形。状態アイコンは通常表示と
    // 同じものを使う。
    //
    // カウンタ列は出さない — サイドバー幅32（決定3）にタブ名と両方は載らず、
    // トリアージが答えるのは「今どれに手を入れるか」なので、タブの壁を無視した
    // 一覧で迷子にならないためのタブ名を優先する。cwd行も同じ理由で出さない
    pub(crate) fn triage_row(
        &self,
        entry: &Selectable,
        tab_name: &str,
        is_selected: bool,
        tab_column: usize,
        cols: usize,
    ) -> Text {
        let agent = self.agents.get(&entry.pane_id);
        let icon = agent.map(|a| a.state.icon()).unwrap_or(" ");
        // 選択行の左端バーはペイン行と同じ（幅2固定で、状態アイコンの
        // color_range 2..3 をずらさない）
        let prefix = if is_selected { "▌ " } else { "  " };
        let head = format!("{}{} ", prefix, icon);
        let head_width = UnicodeWidthStr::width(head.as_str());

        let tab = truncate(tab_name, tab_column);
        let tab_width = UnicodeWidthStr::width(tab.as_str());
        let inner = content_cols(cols);
        // タブ名を持たない行があっても列ぶんは空けておく（桁が行ごとにずれないため）
        let reserved = if tab.is_empty() {
            head_width
        } else {
            head_width + COLUMN_GAP + tab_column
        };
        let (title, _) = fold_to_width(&entry.title, inner.saturating_sub(reserved));

        let mut label = format!("{}{}", head, title);
        let mut tab_span = None;
        if !tab.is_empty() {
            let filler = inner.saturating_sub(UnicodeWidthStr::width(label.as_str()) + tab_width);
            label.push_str(&" ".repeat(filler));
            let start = label.chars().count();
            label.push_str(&tab);
            let end = label.chars().count();
            // アイコンとタブ名だけで幅を使い切るほど狭いときの保険。はみ出すと
            // 選択背景が端末側で折り返して次の行を汚す
            let fitted = truncate(&label, inner);
            // 切り詰められたらタブ名の位置が確定しないので dim は諦める
            if fitted.chars().count() == end {
                tab_span = Some((start, end));
            }
            label = fitted;
        }
        if is_selected {
            label = pad_to_width(label, cols);
        }

        let mut text = Text::new(&label);
        if let Some(a) = agent {
            text = text.color_range(a.state.color(), 2..3);
        }
        // タブ名は主役（状態アイコン・ペイン名）ではないので落として出す。
        // 選択行では落とさない — 帯の中でさらに沈むと読めなくなる（cwd行と同じ）
        if let (Some((start, end)), false) = (tab_span, is_selected) {
            text = text.dim_range(start..end);
        }
        if is_selected {
            text = text.selected().opaque().color_range(2, 0..1);
        }
        text
    }
}

// 検索サブモード中のモード行。クエリ入力が主役なので左に置き、操作ヒントは
// 右端へ寄せる。クエリが伸びてぶつかるところまで来たら、入力中の文字列のほうを
// 優先してヒント側を落とす（右寄せを使うのはヘッダでここ1箇所だけ）
fn search_mode_line(query: &str, indent: &str, cols: usize) -> Text {
    let query = format!("{}▏", query);
    // 余白は表示セル幅で数える。クエリに全角文字が入ると文字数とセル数が
    // ずれ、操作ヒントが右端からはみ出す
    let hint_width = UnicodeWidthStr::width("?:help");
    let pad = cols
        .saturating_sub(UnicodeWidthStr::width(indent))
        .saturating_sub(UnicodeWidthStr::width(query.as_str()) + 1) // 先頭の `/` のぶん
        .saturating_sub(hint_width);
    let mut segments = vec![
        (indent, Ink::Plain),
        ("/", Ink::Tag),
        (query.as_str(), Ink::Plain),
    ];
    let spacer = " ".repeat(pad);
    if pad > 0 {
        segments.push((spacer.as_str(), Ink::Plain));
        segments.push(("?", Ink::Key));
        segments.push((":help", Ink::Muted));
    }
    compose(&segments, cols)
}

// ヘッダ3行目（境界線）。ヘッダ（chrome）とツリー（content）の境目を示す。
//
// サイドバーは borderless で運用していて自前の枠は引かないが、この1本だけは
// 例外（2026-08-06）。領域を囲う枠ではなく境目を示す線なので許容する。
// 右マージンは他の行と同じく空ける — 端まで引くと縁に貼り付いて見える
pub(crate) fn divider_line(cols: usize) -> Text {
    let cols = content_cols(cols);
    let line = "─".repeat(cols);
    compose(&[(line.as_str(), Ink::Muted)], cols)
}

// 表示範囲の外に隠れている行があることを示す1行。文言は英語で統一する。
// 記号はタブ見出し行と同じ三角の系列で、上下どちら側が隠れているかを向きで示す
fn overflow_row(hidden: usize, above: bool, cols: usize) -> Text {
    let marker = if above { "▴" } else { "▾" };
    let label = format!("  {} {} more", marker, hidden);
    // 一覧の行そのものではないので、通知行と同じく落として出す
    compose(&[(&label, Ink::Muted)], content_cols(cols))
}

// ヘッダのように固定して常に先頭へ出す行数。この下だけがスクロールする。
// ヘッダを一緒に流すと、検索サブモードでクエリ入力行が画面から消える。
//
// ヘッダは3行まとまって先頭に来る（ヘルプオーバーレイ中と権限未許可時は 0 行）。
// 定数で 3 と書かずに数えるのは、visible_rows() 側で行を足したときに
// ここの追従漏れでスクロールが行ずれするのを防ぐため
fn pinned_rows(all: &[Row<'_>]) -> usize {
    all.iter()
        .take_while(|row| matches!(row, Row::Brand | Row::Mode | Row::Divider))
        .count()
}

// スクロール位置 `scroll` のとき、一覧を何行ぶん画面に出せるか。
// あふれマーカー行も画面の行を消費するので、その分を引く
fn rows_shown(list_len: usize, area: usize, scroll: usize) -> usize {
    let mut shown = area;
    if scroll > 0 {
        shown = shown.saturating_sub(1);
    }
    if scroll + shown < list_len {
        shown = shown.saturating_sub(1);
    }
    shown.min(list_len.saturating_sub(scroll))
}

// 選択行が画面に入るようスクロール位置を寄せ直す
//（docs/issues/sidebar-vertical-overflow.md）。行番号はいずれも
// 一覧（固定行を除いた部分）の中で数える。
//
// `anchor` は選択行の範囲（ペイン行 + cwd行のように2行にまたがる）。
// None のときは寄せずに範囲外への行き過ぎだけを直す
pub(crate) fn reconcile_scroll(
    list_len: usize,
    area: usize,
    scroll: usize,
    anchor: Option<(usize, usize)>,
) -> usize {
    // 全部載るならスクロールしない。ここを通さないと、一覧が減ったときに
    // 上へ寄ったままの表示が残る
    if area == 0 || list_len <= area {
        return 0;
    }
    let mut scroll = scroll.min(list_len - 1);
    // 末尾に余白を作らない位置まで戻す（一覧が縮んだあと）
    while scroll > 0 && scroll - 1 + rows_shown(list_len, area, scroll - 1) >= list_len {
        scroll -= 1;
    }
    if let Some((first, last)) = anchor {
        if first < scroll {
            // 上へ外れているなら選択行を先頭に置く
            scroll = first;
        } else {
            // 下へ外れているぶんだけ送る。マーカー行の有無で収容量が1行変わるので、
            // 1行ずつ送って入ったかを確かめる
            while scroll < list_len - 1 && last >= scroll + rows_shown(list_len, area, scroll) {
                scroll += 1;
            }
        }
    }
    // 上端マーカーが1行しか隠さないなら、マーカーではなくその行そのものを出す。
    // どちらも画面の1行を使うので、隠すほうが損（先頭タブの見出し行がこれに当たる）。
    // 収まる範囲の下端は変わらないので、選択行が押し出されることもない
    if scroll == 1 {
        return 0;
    }
    scroll
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
