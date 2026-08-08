// サイドバーの描画。
//
// レイアウト（決定27）: 境界線 → ヘッダー(1行) → 境界線 → ツリー(可変) →
// 境界線 → フッター(1行) の5要素からなる固定枠（最下部にもう1行、zellij 本体の
// status-bar と離すための余白が付く）。要件は
// docs/requirements/sidebar-tree/sidebar-header.feature（ヘッダー・枠構造）と
// sidebar-footer.feature（フッター）。ツリーの中身はタブ見出し行 > 配下の
// ペイン行 をタブ順で縦に並べたもの。
//
// **枠の高さ・位置はモードによらず不変**で、変わるのは中身（色・テキスト）だけ。
// 行数が動くと、モードの入退場のたびにツリー全体が上下にずれる。セッション名は
// 出さない — zellij 本体のトップバーが `Zellij (セッション名)` の形で常時
// 出しており、重複が視認性を下げる。
//
// 行が画面高に収まらないときは表示範囲を選択行へ寄せる（縦スクロール。
// 要件: docs/requirements/sidebar-tree/sidebar-scroll.feature）。枠は固定で、
// あいだのツリーだけが動く。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::*;

use crate::agent::{AgentInfo, AgentState};
use crate::config::{Kind, SETTINGS};
use crate::deploy::TROOP;
use crate::mark::MARK_GLYPH;
use crate::search::{Field, Hit};
use crate::{Selectable, State};

// 画面に縦に積む1行ぶんの中身。
//
// 描画（draw）とクリック位置の逆引き（pane_at_row、要件:
// docs/requirements/click-to-focus/）が**同じ並びを共有する**ために切り出してある。
// 行の増減を伴うレイアウト変更は必ず visible_rows() 側で行うこと。
// 描画だけ直すとクリックが行ずれする
pub(crate) enum Row<'a> {
    // ヘッダー。`▲ fujin` を常時出し、三角の色でいまのモードを示す。
    // モード中だけ直後にモードラベルが付く
    Header,
    // chrome（ヘッダー・フッター）と content（ツリー）の境目を示す横線。
    // 最上部・ヘッダー下・フッター上の3箇所に同じ見た目で出る（決定27）
    Divider,
    // フッター。いまの状態で使えるコマンドを1行で出す。空にはならない
    Footer,
    // 高さだけを占めて何も描かない行。2箇所で使う:
    //  - ツリーが画面高に届かないときの埋め草（下の枠を最下部へ押し下げる。
    //    要件: sidebar-footer.feature の「フッターの高さと位置はモードによらず常に同じ」）
    //  - フッターと zellij 本体の status-bar のあいだに空ける最下部の1行
    Blank,
    // ヘルプオーバーレイの1行（要件: docs/requirements/nav-mode/）。
    // 開いている間は content（ツリー）がこの行に置き換わる。枠は出したまま
    Help(&'a HelpRow),
    // 一覧が空であることの通知行（検索の0件・トリアージの対象なし）。
    // 空リストのまま描くと、絞り込みが効いているのか描画が壊れているのか区別できない。
    // 文言は他のUI文言と同じく英語（ui-design.md の「文言」）
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
#[derive(Clone, Copy)]
pub(crate) enum HelpRow {
    // 節の見出し（`keys` / `status`）。**モード名は入れない** — ヘッダーの
    // モードラベルと重複するうえ、節が2つに増えた今は片方だけモード名を
    // 持つのが不揃いに見える（決定30）
    Section(&'static str),
    // キーと、それが何をするか
    Entry(&'static str, &'static str),
    // 状態アイコン凡例の1行（決定25）。アイコン・色・説明はすべて
    // AgentState のテーブルから引くので、ここは状態だけを持つ
    Legend(AgentState),
    Blank,
}

// ヘルプオーバーレイのキー一覧に続けて出す状態アイコン凡例（決定25）。
// モードによらず同じものを出す — ツリーにもトリアージ一覧にも状態アイコンが
// 出るので、どのオーバーレイから引いても同じ表が要る。
//
// アイコン・色・説明はもちろん、**並びと状態の数まで** AgentState 側のテーブルから
// 引く。状態が増減しても凡例は自動で追従し、ここを直す必要はない
const STATUS_LEGEND: [HelpRow; LEGEND_HEAD + AgentState::ALL.len()] = {
    // キー一覧との間の空行・見出し・見出し下の空行
    let mut rows = [HelpRow::Blank; LEGEND_HEAD + AgentState::ALL.len()];
    rows[1] = HelpRow::Section("status");
    // const 文脈では for も iterator も使えないので添字で回す
    let mut i = 0;
    while i < AgentState::ALL.len() {
        rows[LEGEND_HEAD + i] = HelpRow::Legend(AgentState::ALL[i]);
        i += 1;
    }
    rows
};

// 凡例の先頭に置く3行（空行・見出し・空行）
const LEGEND_HEAD: usize = 3;

// 行の左に空ける余白。文字を左端に貼り付けると窮屈に見える
const HELP_INDENT: usize = 2;
// ヘッダー・フッターの文字を置き始める位置。どちらも chrome なのでツリー側の
// 階段（x=0/2/4/6）には乗せず、x=2 に揃えて1つのブロックに見せる。
// ヘッダーの三角だけがこの左に出る（x=0）
const HEADER_INDENT: usize = 2;
// 枠のうち、ツリーの上に固定される行数（境界線・ヘッダー・境界線）
const FRAME_TOP: usize = 3;
// 枠のうち、ツリーの下に固定される行数（境界線・フッター・余白）。
//
// 最下部に空行を1つ噛ませるのは、サイドバーのすぐ下が zellij 本体の
// status-bar だから。詰めて置くとフッターの文字が status-bar に貼り付いて
// 読みにくい（右端に2セル空けるのと同じ理由を、下端にも効かせる）
const FRAME_BOTTOM: usize = 3;
// cwd行の字下げ。ペイン名の開始位置（4セル）よりさらに右へ寄せて、
// 隣のペイン行ではなく「上のペイン行の続き」として読ませる
const CWD_INDENT: usize = 6;
// ペイン名と右寄せの列（カウンタ列・トリアージ行のタブ名列）のあいだに
// 最低限空ける幅。名前と列がくっつくと、どこまでが名前か読めなくなる
const COLUMN_GAP: usize = 1;
// トリアージ行のタブ名列に使ってよい幅の割合（内容幅の 1/N）。
// タブ名が長くてもペイン名を潰さないための上限
const TRIAGE_TAB_SHARE: usize = 3;
// フローティングペインのペイン名を囲む丸括弧が占める幅（前後で2セル。
// 要件: docs/requirements/floating-pane-indicator/）
const FLOATING_BRACKETS: usize = 2;
// 右端に常に空ける幅。文字がサイドバーの縁に貼り付くと窮屈に見える。
// 左マージン（選択バーぶんの2セル）と揃えてある
const RIGHT_MARGIN: usize = 2;

// 文字を置いてよい幅。選択行の背景は右マージンも含めて塗るので、
// 背景のパディング（pad_to_width）はこれではなく cols を使うこと
fn content_cols(cols: usize) -> usize {
    cols.saturating_sub(RIGHT_MARGIN)
}
// キー列と説明のあいだに空ける幅。キー列の幅自体は固定せず、**そのモードに
// 出るキーの実測最大**で決める（決定30。カウンタ列と同じ考え方）。17セル固定に
// していたころは、いちばん長い `j k up down tab` のために全行が空白を払っていた
const HELP_KEY_GAP: usize = 2;

// キー列に置いた文字の後ろに空ける幅。列幅を超える場合は空白1つだけ空けて
// 続ける（列は崩れるが、説明が切り詰められて消えるよりはよい）
fn help_key_pad(keys: &str, column: usize) -> usize {
    column.saturating_sub(UnicodeWidthStr::width(keys)).max(1)
}

// 文字に与える意味。zellij のテーマ側の色をそのまま借りる（決定11と同じ方針で、
// 色そのものを設定項目にはしない）
#[derive(Clone, Copy)]
enum Ink {
    // 既定の文字色。読ませたい本文
    Plain,
    // 検索クエリの先頭 `/`。ヘルプオーバーレイの見出しも当初はこれだったが、
    // モード名を落として dim の節見出しにした（決定30）
    //（ヘッダーのモードラベルは状態色なのでこちらではない・決定27）
    Tag,
    // キーそのもの。zellij 本体の status-bar がキーを強調するのに倣う
    Key,
    // 添え物（補足行・境界線）。落として主役を目立たせる
    Muted,
    // 強調色をレベル指定で乗せる。ヘッダーの三角のように、色そのものが
    // 情報を持つ断片に使う
    Accent(usize),
}

// ヘッダーの三角・モードラベルとフッターに乗せる色（要件: sidebar-header /
// sidebar-footer）。navモード・検索サブモードはレベル2（green）で、zellij の
// タブバーがアクティブなタブに使う色に対応させる
const NAV_LEVEL: usize = 2;
// トリアージモードはレベル0（orange）。緊急度を連想させる側へ寄せる
const TRIAGE_LEVEL: usize = 0;
// 終了操作サブモードはレベル6（error_color）。確認プロンプトを警告色で出す
//（決定35）。新しい色は増やさず、状態アイコン `error` と同じ色を借りる
const TERMINATION_LEVEL: usize = 6;
// 配置演出の兵の色（要件: header-animation）。レベル2（green）で、状態アイコンの
// 色分けではなくブランド側の色として読ませる。一過性の演出なので、ヘッダーの
// 状態色（navモードも同じレベル2）と一瞬並んでも意味の取り違えは起きない。
// 実機での見え方はまだ詰めていない暫定値
const TROOP_LEVEL: usize = 2;
// ヘッダー本文の右端と兵の発進位置のあいだに空けるセル数。詰めると `fujin` の
// 語尾と兵がくっついて、文字の一部に見える
const LAUNCH_GAP: usize = 1;

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

// ペイン行の先頭（状態アイコンの手前）に挟む列。どちらも**出すか出さないかは
// フレーム単位**で決まり、中身だけが行ごとに変わる。並びは
// 選択バー → 番号列 → マーク列 → アイコン → ペイン名（決定39）
#[derive(Clone, Copy, Default)]
pub(crate) struct HeadCells<'a> {
    // 番号ジャンプサブモード中だけ Some（通し番号の表示と、番号入力バッファに
    // 前方一致して候補に残っているか。決定29）
    pub(crate) number: Option<(&'a str, bool)>,
    // マーク列を出すフレームだけ Some（その行がマーク済みか。決定39）
    pub(crate) mark: Option<bool>,
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
    // このフレームにマーク列を出すか（決定39）。カウンタ列と同じく**そのフレームに
    // 出る行の実測**で決める — 誰もマークしていないフレームでは列そのものが消え、
    // ペイン名が幅をすべて使う。マーク済みの行が1つでもあれば、同じフレームの
    // マークされていない行も空白で列ぶんを空けてアイコンの位置を揃える
    pub(crate) fn mark_column(&self, rows: &[Row<'_>]) -> bool {
        rows.iter().any(|row| match row {
            Row::Pane { entry, .. } | Row::Triage { entry, .. } => self.is_marked(entry.pane_id),
            _ => false,
        })
    }

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
    // 呼び出し側の責務 — 行の並び自体は高さに依らないため。
    //
    // 枠（境界線・ヘッダー・フッター）は**常時 FRAME_TOP + FRAME_BOTTOM 行を
    // 確保する**（要件: sidebar-header / sidebar-footer）。中身がモードで
    // 変わっても行数は変えない — 入退場で枠の高さが動くと、ツリー全体が
    // そのぶん上下にずれる
    pub(crate) fn visible_rows(&self) -> Vec<Row<'_>> {
        if !self.permissions_granted {
            return Vec::new();
        }
        let content = self.content_rows();
        let mut rows = Vec::with_capacity(content.len() + FRAME_TOP + FRAME_BOTTOM);
        rows.push(Row::Divider);
        rows.push(Row::Header);
        rows.push(Row::Divider);
        rows.extend(content);
        rows.push(Row::Divider);
        rows.push(Row::Footer);
        // status-bar との間に空ける1行（FRAME_BOTTOM のコメント参照）
        rows.push(Row::Blank);
        rows
    }

    // 枠の内側（content）に並ぶ行。ヘルプオーバーレイ表示中はツリーの代わりに
    // ヘルプ内容が入る — 覆うのは content だけで、枠は出したままにする（決定27）
    fn content_rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        if self.help_overlay {
            // キー一覧（モードごとに違う）＋ 状態アイコン凡例（共通）。
            // 収まらないぶんはツリーと同じあふれマーカーで示す — オーバーレイは
            // 「任意のキーで閉じる」ので、スクロール用のキーを持てない
            return self
                .help_lines()
                .iter()
                .chain(STATUS_LEGEND.iter())
                .map(Row::Help)
                .collect();
        }
        // トリアージモード中はツリー表示を隠し、一覧だけを出す（要件: triage-mode）
        if self.triage.is_some() {
            let entries = self.triage_entries();
            if entries.is_empty() {
                rows.push(Row::Notice("nothing to triage"));
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
                rows.push(Row::Notice("no matches"));
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
                // 画面に無い文字列でヒットしたように見せないため。
                // ただしペイン名フォールバック中の行では出さない。ペイン名の位置に
                // 既に同じパスが出ており、2行並べても情報が増えない（決定26）
                let cwd_hit = hit.filter(|h| h.field == Field::Cwd);
                if (self.show_cwd || cwd_hit.is_some()) && self.title_fallback(entry).is_none() {
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
    // **返す行数は常に画面高ぴったり**（画面が枠より低いときを除く）。ツリーが
    // 短いぶんは空行で埋め、下の枠を最下部へ押し下げる — 埋めないとフッターが
    // ツリーの直後に浮き、画面高で位置が動いてしまう。
    //
    // `rows` が 0 のときは切り出さない。行クリックの逆引きが最初の描画より前に
    // 来た場合（State::viewport_rows の初期値）で、スクロールは起きていない
    pub(crate) fn screen_rows(&self, rows: usize) -> Vec<Row<'_>> {
        let mut all = self.visible_rows();
        if rows == 0 || all.is_empty() {
            return all;
        }
        let frame = FRAME_TOP + FRAME_BOTTOM;
        // 画面が枠ぶんの高さも無いときは、入るところまでを出して終わる。
        // 一覧に割ける高さが無いので、スクロールもあふれマーカーも出番がない
        if rows <= frame {
            all.truncate(rows);
            return all;
        }
        let area = rows - frame;
        let list_len = all.len() - frame;
        // 描画とクリックの逆引きで同じ位置を使う。State::scroll は描画時に
        // 寄せた値だが、そのあと一覧が縮んでいることもあるので clamp は掛け直す
        let scroll = reconcile_scroll(list_len, area, self.scroll, None);
        let shown = rows_shown(list_len, area, scroll);

        // 上の枠 / 一覧 / 下の枠 の3つに割る（並びは visible_rows() が決めている）
        let bottom = all.split_off(FRAME_TOP + list_len);
        let list = all.split_off(FRAME_TOP);
        let mut screen = all;
        if scroll > 0 {
            screen.push(Row::Overflow {
                hidden: scroll,
                above: true,
            });
        }
        screen.extend(list.into_iter().skip(scroll).take(shown));
        let below = list_len.saturating_sub(scroll + shown);
        if below > 0 {
            screen.push(Row::Overflow {
                hidden: below,
                above: false,
            });
        }
        // 余った高さを空行で埋めてから下の枠を置く
        screen.resize_with(FRAME_TOP + area, || Row::Blank);
        screen.extend(bottom);
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
        // ヘルプオーバーレイはツリーとは別の面（決定27）。ツリーのスクロール位置を
        // 持ち込むと、開いた瞬間に途中の行から表示される
        if self.help_overlay {
            self.scroll = 0;
            return;
        }
        let frame = FRAME_TOP + FRAME_BOTTOM;
        let (list_len, area, anchor) = {
            let all = self.visible_rows();
            let anchor = self.selected_span(&all).map(|(first, last)| {
                (
                    first.saturating_sub(FRAME_TOP),
                    last.saturating_sub(FRAME_TOP),
                )
            });
            (
                all.len().saturating_sub(frame),
                rows.saturating_sub(frame),
                anchor,
            )
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
        // マーク列を出すかもフレーム全体で1つ（決定39）。行ごとに決めると
        // マーク済みの行だけアイコンの位置がずれる
        let marks = self.mark_column(&screen);
        // カーソルは一覧から導出されるので、行ごとに引き直さず1度だけ求める
        let triage_cursor = self.triage_cursor();
        for (y, row) in screen.into_iter().enumerate() {
            match row {
                Row::Header => {
                    print_text_with_coordinates(self.header_line(cols), 0, y, None, None);
                }
                Row::Footer => {
                    print_text_with_coordinates(self.footer_line(cols), 0, y, None, None);
                }
                // 高さを占めるだけの行。描くものは無い
                Row::Blank => {}
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
                    // 番号ジャンプサブモード中だけ番号列が付く（決定29）
                    let number = self.jump_number(flat_index);
                    let cells = HeadCells {
                        number: number.as_ref().map(|(n, m)| (n.as_str(), *m)),
                        mark: marks.then(|| self.is_marked(entry.pane_id)),
                    };
                    let row = self.pane_row(entry, is_selected, hit, column, cells, cols);
                    print_text_with_coordinates(row, 0, y, None, None);
                }
                Row::Triage { entry, tab_name } => {
                    let is_selected = triage_cursor == Some(entry.pane_id);
                    let mark = marks.then(|| self.is_marked(entry.pane_id));
                    let row = self.triage_row(entry, tab_name, is_selected, tab_column, mark, cols);
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

    // ヘッダー（1行）。`▲ fujin` をモードによらず常に出し、モード中だけ直後に
    // モードラベルを足す（決定27。要件: sidebar-header）。
    //
    // 色を乗せるのは三角とモードラベルで、ブランド名は dim のまま。名前まで色を
    // 付けるとツリーの状態アイコンの色分けと喧嘩する
    pub(crate) fn header_line(&self, cols: usize) -> Text {
        // 兵の間を埋める空白は借用されるので、断片を組む前に作っておく
        let pads = self.troop_pads(cols);
        let mut segments = self.header_segments();
        // 配置演出中だけ、本文の右に兵が並ぶ（要件: header-animation）
        for pad in &pads {
            segments.push((pad.as_str(), Ink::Plain));
            segments.push((TROOP, Ink::Accent(TROOP_LEVEL)));
        }
        compose(&segments, content_cols(cols))
    }

    // ヘッダーの本文（`▲ fujin` ＋モードラベル）。配置演出は**この右側**に
    // 兵を並べるので、本文の幅を測れるよう断片のまま返す
    fn header_segments(&self) -> Vec<(&str, Ink)> {
        // 三角が x=0、ブランド名が x=2（HEADER_INDENT）に来る
        let ink = self.state_ink();
        let mut segments = vec![("▲", ink), (" fujin", Ink::Muted)];
        if let Some(label) = self.mode_label() {
            segments.push(("  ", Ink::Plain));
            segments.push((label, ink));
        }
        segments
    }

    // ヘッダー本文の表示幅
    fn header_width(&self) -> usize {
        self.header_segments()
            .iter()
            .map(|(fragment, _)| UnicodeWidthStr::width(*fragment))
            .sum()
    }

    // 配置演出で兵が使える領域 `(発進位置, 幅)`（要件: header-animation）。
    //
    // 発進位置はヘッダー本文の右端の1つ先で、モードラベルが出ているぶんだけ
    // 右へずれる — 兵がラベルに重なるとどちらも読めなくなる。幅は他の行と同じく
    // 右マージンを除いた内容幅で、着地列はその手前から確保する
    pub(crate) fn troop_field(&self, cols: usize) -> (usize, usize) {
        (self.header_width() + LAUNCH_GAP, content_cols(cols))
    }

    // 兵と兵のあいだを埋める空白。compose() は断片を順に置くだけなので、
    // 兵の絶対位置はこの空白の幅で作る
    fn troop_pads(&self, cols: usize) -> Vec<String> {
        let Some(deployment) = &self.deployment else {
            return Vec::new();
        };
        let (launch, width) = self.troop_field(cols);
        let mut pads = Vec::new();
        let mut x = self.header_width();
        for column in deployment.columns(launch, width) {
            pads.push(" ".repeat(column.saturating_sub(x)));
            x = column + UnicodeWidthStr::width(TROOP);
        }
        pads
    }

    // いまの状態を語る色。ヘッダーの三角・モードラベルとフッター全体に同じ色を
    // 使い、**テキストを読まなくても色だけでモードが判別できる**ようにする
    //（決定27。要件: sidebar-header / sidebar-footer）
    fn state_ink(&self) -> Ink {
        // 終了操作サブモードが最優先。確認プロンプトのあいだは、ヘッダーの三角も
        // 含めて警告色にする（決定35のフッター転用を、決定27の「ヘッダーとフッターは
        // 同じ状態色」に沿わせたもの）
        if self.termination.is_some() {
            Ink::Accent(TERMINATION_LEVEL)
        } else if self.showing_config_warning() {
            // 警告も終了操作と同じ error_color を借りる（決定40）。新しい色は
            // 増やさない。フッターだけ色を変えるとヘッダーの三角と食い違うので、
            // 三角ごと警告色にする — 出ているあいだは「いまの状態」が警告
            Ink::Accent(TERMINATION_LEVEL)
        } else if self.triage.is_some() {
            Ink::Accent(TRIAGE_LEVEL)
        } else if self.nav_mode || self.search.is_some() {
            Ink::Accent(NAV_LEVEL)
        } else {
            Ink::Muted
        }
    }

    // ヘッダーのモードラベル。ツリー表示（非フォーカス）では出さない。
    // 検索サブモードは navモードの内側なので `[nav]` のまま変えない
    fn mode_label(&self) -> Option<&'static str> {
        if self.triage.is_some() {
            Some("[tri]")
        } else if self.nav_mode || self.search.is_some() {
            Some("[nav]")
        } else {
            None
        }
    }

    // フッター（1行）。**いまその状態で使えるコマンドを常に出し、空にはしない**
    //（決定27。要件: sidebar-footer）。
    //
    // サイドバー幅は32文字（決定3）で全キーの説明は載らないので、常時出すのは
    // ヘルプ・退出キーだけに絞り、詳細は `?` のヘルプオーバーレイへ追い出して
    // ある。文言は英語で統一する。
    //
    // 文字色は状態色で統一する — 「キーは常にレベル2固定」という色役割の原則は
    // フッターに限り例外（決定27）。ヘッダーの三角とトーンを揃えるほうを取る
    pub(crate) fn footer_line(&self, cols: usize) -> Text {
        // ツリーの行と同じく右端は空ける
        let inner = content_cols(cols);
        let indent = " ".repeat(HEADER_INDENT);
        let ink = self.state_ink();
        // ヘルプオーバーレイ中は閉じ方だけを出す。ヘルプ本文の末尾にあった
        // 同じ文言はこちらへ移してある（本文側からは削除済み）
        if self.help_overlay {
            return compose(
                &[(&indent, Ink::Plain), ("press any key to close", ink)],
                inner,
            );
        }
        // 終了操作サブモード中は確認プロンプトに転用する（決定35。要件:
        // pane-close-kill）。**ペイン名は出さない** — 対象は選択行のハイライトで
        // 分かっており、この幅では名前の大半が切り詰められて識別の役に立たない
        if self.termination.is_some() {
            // マークが1件以上あれば件数を前置する（決定39）
            let prompt = termination_prompt(
                self.termination_marked_count(),
                inner.saturating_sub(HEADER_INDENT),
            );
            return compose(&[(&indent, Ink::Plain), (&prompt, ink)], inner);
        }
        // 検索サブモード中はクエリ入力欄に転用する
        if let Some(search) = &self.search {
            return input_footer("/", &search.query, &indent, ink, inner);
        }
        // 番号ジャンプサブモード中は番号入力バッファの表示に転用する（決定29）。
        // 先頭の `n` は検索サブモードの `/` と同じく入場キーの提示
        if let Some(jump) = &self.jump {
            return input_footer("n ", &jump.buffer, &indent, ink, inner);
        }
        // 解釈できなかった設定の警告（決定40）。**入力欄・確認プロンプト・
        // ヘルプより後、静的なヒントより先**に見る（`showing_config_warning`）。
        // 起動直後の一定時間だけで、期限が切れれば通常の表示へ戻る
        if self.showing_config_warning() {
            let warning = self.config_warning_line(inner.saturating_sub(HEADER_INDENT));
            return compose(&[(&indent, Ink::Plain), (&warning, ink)], inner);
        }
        // トリアージモードの Esc の行き先は navモードのツリー表示（退場ではない）
        // なので、ヒントも `exit` ではなく `back`
        if self.triage.is_some() {
            return compose(&[(&indent, Ink::Plain), ("?:help  esc:back", ink)], inner);
        }
        if self.nav_mode {
            return compose(&[(&indent, Ink::Plain), ("?:help  esc:exit", ink)], inner);
        }
        // 非フォーカス時は direct-keys方式（決定6）のヒント
        compose(
            &[
                (&indent, Ink::Plain),
                (
                    &self.direct_keys_hint(inner.saturating_sub(HEADER_INDENT)),
                    ink,
                ),
            ],
            inner,
        )
    }

    // 非フォーカス時のフッターに出す direct-keys のヒント（要件: sidebar-footer）。
    //
    // キーは決め打ちせず、`Event::InitialKeybinds` から解決した実際の割り当てを
    // 出す（決定27）。未バインドの項目はその項目だけ省く。
    //
    // 実キーの長さはユーザー依存で伸び縮みするので、幅に収まらないときは
    //  1. `up`/`down` を矢印へ落とす（`jump` に対応する矢印記号は無い）
    //  2. それでも溢れるなら末尾の項目ごと省く
    // の順で削る。**末尾を `…` で切り詰めない** — `キー:動作` の形が壊れた
    // ヒントは読めず、項目ごと省いたほうが残りは正しく読める。
    // 項目数を優先し、同じ項目数で選べるなら既定の英字表記を採る
    fn direct_keys_hint(&self, budget: usize) -> String {
        let full = self.direct_key_parts(false).len();
        for count in (1..=full).rev() {
            for arrows in [false, true] {
                let mut parts = self.direct_key_parts(arrows);
                parts.truncate(count);
                let line = parts.join("  ");
                if UnicodeWidthStr::width(line.as_str()) <= budget {
                    return line;
                }
            }
        }
        String::new()
    }

    // 割り当てのある項目だけを `キー:動作` の形に組んだもの（表示順）。
    // 並び順も動作名も設定テーブル（config.rs）から引く（決定40）
    fn direct_key_parts(&self, arrows: bool) -> Vec<String> {
        let mut parts = Vec::new();
        for setting in &SETTINGS {
            let Kind::DirectKey { label, arrow } = setting.kind else {
                continue;
            };
            let Some(key) = self.direct_keys.get(setting.key) else {
                continue;
            };
            let label = match (arrows, arrow) {
                (true, Some(arrow)) => arrow,
                _ => label,
            };
            parts.push(format!("{}:{}", key, label));
        }
        parts
    }

    // 解釈できなかった設定を伝える1行（決定40。要件: configuration）。
    //
    // 幅32のフッターに理由まで書く余地は無いので、**どのキーが効いていないか**
    // だけを出して、直す場所を指させる形に絞る。詳細は README の設定表と
    // stderr のログ側にある。項目が幅に収まらないときは、direct-keys のヒントと
    // 同じ考え方で末尾を `…` で切らず、残り件数（`+N`）に畳む
    fn config_warning_line(&self, budget: usize) -> String {
        let keys = &self.config_warnings;
        let head = if keys.len() == 1 {
            "!bad value: "
        } else {
            "!bad values: "
        };
        for shown in (1..=keys.len()).rev() {
            let mut line = format!("{}{}", head, keys[..shown].join(" "));
            if shown < keys.len() {
                line.push_str(&format!(" +{}", keys.len() - shown));
            }
            if UnicodeWidthStr::width(line.as_str()) <= budget {
                return line;
            }
        }
        // キー名が1つも置けない幅。せめて「設定を見ろ」だけは残す
        "!bad config".to_string()
    }

    // キー列の幅。いま出ているキー一覧の実測最大で決めるので、モードによって
    // 変わる（決定30）。凡例のアイコンは1文字なので、列幅を押し上げない
    fn help_key_column(&self) -> usize {
        self.help_lines()
            .iter()
            .filter_map(|row| match row {
                HelpRow::Entry(keys, _) => Some(UnicodeWidthStr::width(*keys)),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + HELP_KEY_GAP
    }

    // ヘルプオーバーレイの1行。左マージンは描画位置（x）ではなく行の中に
    // 持たせる — 画面座標を行ごとに変えると、行の並びと描画がずれやすい
    pub(crate) fn help_line(&self, row: &HelpRow, cols: usize) -> Text {
        let indent = " ".repeat(HELP_INDENT);
        let column = self.help_key_column();
        match row {
            HelpRow::Section(label) => compose(&[(&indent, Ink::Plain), (label, Ink::Muted)], cols),
            HelpRow::Entry(keys, description) => {
                let gap = " ".repeat(help_key_pad(keys, column));
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
            HelpRow::Legend(state) => {
                // アイコンをキー列の位置に置き、説明はキー一覧と同じ開始位置から
                // 出す。キーが常にレベル2固定なのに対し、凡例のアイコンだけは
                // 状態色をそのまま乗せる — 意味と色を結びつけて見せるのが
                // 凡例の目的そのものだから（決定25）
                let icon = state.icon();
                let gap = " ".repeat(help_key_pad(icon, column));
                compose(
                    &[
                        (&indent, Ink::Plain),
                        (icon, Ink::Accent(state.color())),
                        (&gap, Ink::Plain),
                        (state.label(), Ink::Plain),
                    ],
                    cols,
                )
            }
            HelpRow::Blank => Text::new(""),
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

    // ペイン行に出す名前（決定26）。ペイン名が空のときだけ cwd を代わりに出す。
    //
    // claude は終了時に空文字のタイトルを OSC で送り、zellij 側にそれを戻す経路が
    // 無いため、エージェントを落とした瞬間にペイン名が空のまま残る
    // （docs/issues/pane-title-blank-on-exit.md）。前回の名前を保持すると死んだ
    // エージェントが動いているように見えるので、いまそこに何があるかが分かる cwd へ落とす
    pub(crate) fn display_title<'a>(&'a self, entry: &'a Selectable) -> &'a str {
        self.title_fallback(entry).unwrap_or(&entry.title)
    }

    // ペイン名フォールバックが効いているならその cwd。空でないペイン名はそのまま
    // 出すし（決定19）、cwd を持たないペインは空欄のままにする
    fn title_fallback(&self, entry: &Selectable) -> Option<&str> {
        if entry.title.trim().is_empty() {
            self.pane_cwds.get(&entry.pane_id).map(String::as_str)
        } else {
            None
        }
    }

    // ペイン行1行ぶんの Text を組み立てる（状態アイコン・ペイン名・カウンタ列・
    // ハイライト込み）。cwd は別行なのでここには出てこない（決定22）。
    //
    // レイアウトは `{アイコン} {ペイン名} …余白… {カウンタ列}` で、カウンタ列は
    // 右端に揃える。ペイン名はカウンタ列を除いた残り幅に収め、名前の長さで
    // サブエージェント数・未完了タスク数が消えないようにする（決定21）。
    //
    // `head` はアイコンの手前に挟む列（番号列・マーク列）。出すフレームでは
    // head がそのぶん伸びて、ペイン名の残り幅が縮む
    pub(crate) fn pane_row(
        &self,
        entry: &Selectable,
        is_selected: bool,
        hit: Option<&Hit>,
        column: CounterColumn,
        cells: HeadCells<'_>,
        cols: usize,
    ) -> Text {
        let HeadCells { number, mark } = cells;
        let agent = self.agents.get(&entry.pane_id);
        // アイコンはエージェント状態・コマンド状態のどちらからでも来る（決定32）
        let status = self.pane_status(entry.pane_id);
        let icon = status.map(|s| s.icon()).unwrap_or(" ");
        // 選択行は左端にバーを立てる。テーマの選択色が沈む配色でも
        // どこが選択中か一目で分かるようにするため（幅は2文字で固定し、
        // 番号列・状態アイコンの開始位置をずらさない）
        let prefix = if is_selected { "▌ " } else { "  " };
        // マーク列は列を出すフレームでだけ1文字＋空白を占める（決定39）
        let mark_cell = mark_cell(mark);
        // "▌ {icon} " / "  {icon} "、番号ジャンプサブモード中は "▌ {番号} {icon} "。
        // マーク列を出すフレームでは番号列とアイコンのあいだに "{✓|空白} " が入る
        let head = match number {
            Some((digits, _)) => format!("{}{} {}{} ", prefix, digits, mark_cell, icon),
            None => format!("{}{}{} ", prefix, mark_cell, icon),
        };
        let head_width = UnicodeWidthStr::width(head.as_str());
        // マーク印の文字位置（列を出すフレームだけ）。アイコンの2文字手前で、
        // 番号列の有無に追従する
        let mark_at = mark.map(|_| head.chars().count().saturating_sub(4));
        // 状態アイコンの文字位置。head の末尾は常に「アイコン(1文字)+空白」なので、
        // 番号列の有無で動いても末尾から数えれば追従できる
        let icon_at = head.chars().count().saturating_sub(2);

        let (subagents, open_tasks) = counter_labels(agent);
        let counters = column.render(&subagents, &open_tasks);
        let counters_width = UnicodeWidthStr::width(counters.as_str());
        // フローティングペインの丸括弧もカウンタ列と同じく先に確保する（決定21の
        // 考え方。畳むのはペイン名の側）
        let brackets = if entry.is_floating {
            FLOATING_BRACKETS
        } else {
            0
        };
        // カウンタ列を持つフレームでは、この行に数字が無くても列ぶんは空けておく。
        // 空けないと、右端に揃えたはずの桁が行によってずれる
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

        let mut label = format!("{}{}{}{}", head, open, title, close);
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
        // ペイン名自身の畳んだ結果から判定する。
        // ペイン名フォールバック中はこの位置に出ているのが cwd なので、
        // 拾うヒットも cwd のものに切り替える（決定26）
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
                head.chars().count() + open.chars().count(),
            )
        });

        let mut text = Text::new(&label);
        // ペイン名は通常ウェイトに落とす（決定36）。全行bold・同色だと状態アイコンの
        // 色や選択行の背景が相対的に沈み、一覧のメリハリが弱くなるため。
        // **選択行（実フォーカスのペイン）はboldのまま残す** — 実機で確認したところ
        // 選択行まで落とすと「いまどこにいるか」が弱まった（決定36改訂）
        if !is_selected {
            let name_start = head.chars().count();
            // 丸括弧もペイン名の一部として同じ太さで出す（要件:
            // floating-pane-indicator。括弧だけ別扱いにはしない）
            let name_end =
                name_start + open.chars().count() + title.chars().count() + close.chars().count();
            text = text.unbold_range(name_start..name_end);
        }
        if let Some(status) = status {
            // 状態アイコン部分に状態色（位置は番号列の有無に追従する）
            text = text.color_range(status.color(), icon_at..icon_at + 1);
        }
        if let Some((digits, matches)) = number {
            // 番号は「そのまま打つ文字」なのでキーの色で出す（ヘルプの
            // キー列と同じ扱い）。番号入力バッファに前方一致しなくなった
            // 番号は落とし、残っている候補だけが目に入るようにする
            //（vimiumのリンクヒントと同じ提示。決定29）
            let span = 2..2 + digits.chars().count();
            text = if matches {
                text.color_range(2, span)
            } else {
                text.dim_range(span)
            };
        }
        if let (Some(at), Some(true)) = (mark_at, mark) {
            // マーク印は「ユーザーが自分で指した」印なので、選択バーと同じレベル2。
            // 状態アイコンの色（行ごとに変わる）とは役割が違う
            text = text.color_range(2, at..at + 1);
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
        mark: Option<bool>,
        cols: usize,
    ) -> Text {
        let status = self.pane_status(entry.pane_id);
        let icon = status.map(|s| s.icon()).unwrap_or(" ");
        // 選択行の左端バーはペイン行と同じ（幅2固定）
        let prefix = if is_selected { "▌ " } else { "  " };
        // マーク列もペイン行と同じ位置（アイコンの手前）に出す（決定39）。
        // トリアージ一覧の上でもマークできる以上、印が見えないと積み上げられない
        let mark_cell = mark_cell(mark);
        let head = format!("{}{}{} ", prefix, mark_cell, icon);
        let head_width = UnicodeWidthStr::width(head.as_str());
        // 状態アイコンの文字位置はマーク列の有無で動く（ペイン行と同じ数え方）
        let icon_at = head.chars().count().saturating_sub(2);
        let mark_at = mark.map(|_| head.chars().count().saturating_sub(4));

        let tab = truncate(tab_name, tab_column);
        let tab_width = UnicodeWidthStr::width(tab.as_str());
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
        // ペイン行と同じく、ペイン名が空なら cwd を代わりに出す（決定26）
        let (title, _) = fold_to_width(self.display_title(entry), inner.saturating_sub(reserved));
        let (open, close) = floating_brackets(entry, &title);

        let mut label = format!("{}{}{}{}", head, open, title, close);
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
        // ペイン名は通常ウェイトに落とす（決定36改訂。ペイン行と同じ扱いで、選択行は
        // boldのまま残す）
        if !is_selected {
            let name_start = head.chars().count();
            let name_end =
                name_start + open.chars().count() + title.chars().count() + close.chars().count();
            text = text.unbold_range(name_start..name_end);
        }
        if let Some(status) = status {
            text = text.color_range(status.color(), icon_at..icon_at + 1);
        }
        if let (Some(at), Some(true)) = (mark_at, mark) {
            text = text.color_range(2, at..at + 1);
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

// 入力欄を兼ねるフッター（検索サブモードのクエリ・番号ジャンプサブモードの
// 番号入力バッファ）。入力が主役なので左に置き、操作ヒントは右端へ寄せる。
// 入力が伸びてぶつかるところまで来たら、入力中の文字列のほうを優先して
// ヒント側を落とす（右寄せを使うのは枠でここ1箇所だけ）。
//
// 入力本体は本文なので既定色のまま、先頭の `tag`（`/` や `n`）はモード名と
// 同じ扱いでレベル3。状態色で統一するのはヒント側（要件: sidebar-footer）
fn input_footer(tag: &str, input: &str, indent: &str, ink: Ink, cols: usize) -> Text {
    let input = format!("{}▏", input);
    // 余白は表示セル幅で数える。入力に全角文字が入ると文字数とセル数が
    // ずれ、操作ヒントが右端からはみ出す
    let hint = "?:help";
    let pad = cols
        .saturating_sub(UnicodeWidthStr::width(indent))
        .saturating_sub(UnicodeWidthStr::width(tag))
        .saturating_sub(UnicodeWidthStr::width(input.as_str()))
        .saturating_sub(UnicodeWidthStr::width(hint));
    let mut segments = vec![
        (indent, Ink::Plain),
        (tag, Ink::Tag),
        (input.as_str(), Ink::Plain),
    ];
    let spacer = " ".repeat(pad);
    if pad > 0 {
        segments.push((spacer.as_str(), Ink::Plain));
        segments.push((hint, ink));
    }
    compose(&segments, cols)
}

// フローティングペインのペイン名を囲む丸括弧（要件:
// docs/requirements/floating-pane-indicator/）。フローティング層ごと隠れうる
// ペインを、一覧の上で見分けられるようにするための印。
//
// 色・dim は乗せない — ペイン名の色は落とさない（決定36）うえ、色は状態・モードへ
// 割り当ててある（ui-design.md 原則1・2）ので、この軸には記号だけを使う。
// 角括弧はモードラベル、山括弧は zellij のキーバインド表記と衝突するため丸括弧。
//
// 囲むものが無い行（ペイン名も cwd も空）では括弧も出さない
fn floating_brackets(entry: &Selectable, title: &str) -> (&'static str, &'static str) {
    if entry.is_floating && !title.is_empty() {
        ("(", ")")
    } else {
        ("", "")
    }
}

// ペイン行・トリアージ行のマーク列1つぶん（決定39）。列を出すフレームでは、
// マークされていない行も空白で同じ幅を占めてアイコンの位置を揃える
fn mark_cell(mark: Option<bool>) -> String {
    match mark {
        Some(true) => format!("{} ", MARK_GLYPH),
        Some(false) => "  ".to_string(),
        None => String::new(),
    }
}

// 終了操作サブモードの確認プロンプト（決定35。要件: pane-close-kill）。
//
// 幅28セル（決定27）に3項目とも収める都合で、項目のあいだは他のヒントの半分の
// 空白1つ。それでも収まらない幅ではキーだけに落とす — 項目の途中で切り詰めると
// `キー:動作` の形が壊れて読めなくなる（direct-keys のヒントと同じ考え方）。
//
// `marked` が1件以上なら件数を前置する（決定39）。**幅が足りないときも件数だけは
// 残す** — 何件消えるかは対象の数が1件のときと違って行のハイライトから読めず、
// キーの意味（`c k x` の3択）より先に知りたい情報だから。マーク0件（選択行への
// フォールバック）では単一版と同じ文言のままにする。
//
// Esc（取り消し）はここには出ない。3項目で幅を使い切っているので、行き先の説明は
// ヘルプオーバーレイ側（termination_help_lines）が担う
fn termination_prompt(marked: usize, budget: usize) -> String {
    let count = match marked {
        0 => String::new(),
        1 => "1 pane  ".to_string(),
        n => format!("{} panes  ", n),
    };
    let budget = budget.saturating_sub(UnicodeWidthStr::width(count.as_str()));
    let full = "c:close k:kill x:kill+close";
    if UnicodeWidthStr::width(full) <= budget {
        format!("{}{}", count, full)
    } else {
        format!("{}c k x", count)
    }
}

// 境界線。chrome（ヘッダー・フッター）と content（ツリー）の境目を示す。
// 最上部・ヘッダー下・フッター上の3本とも同じ見た目で引く（決定27）。
//
// サイドバーは borderless で運用していて自前の枠は引かないが、この3本だけは
// 例外。領域を囲う枠ではなく境目を示す線なので許容する。
// 右マージンは他の行と同じく空ける — 端まで引くと縁に貼り付いて見える
pub(crate) fn divider_line(cols: usize) -> Text {
    let cols = content_cols(cols);
    let line = "─".repeat(cols);
    compose(&[(line.as_str(), Ink::Muted)], cols)
}

// 表示範囲の外に隠れている行があることを示す1行。文言は英語で統一する。
// 記号はタブ見出し行と同じ三角の系列で、上下どちら側が隠れているかを向きで示す。
// タブ見出しと同列（x=0）に置く — 一覧の1項目ではなく、一覧そのものが
// そこで打ち切られていることを表す行なので、階層の外側に出す
pub(crate) fn overflow_row(hidden: usize, above: bool, cols: usize) -> Text {
    let marker = if above { "▴" } else { "▾" };
    // `…` はペインの cwd（truncate_start）と同じ省略記号。マーカーと数字の
    // あいだに挟み、一覧がそこで途切れていることを添える。
    // **消さないこと** — 下端の `▾` はアクティブなタブ見出しと記号も列も同じなので、
    // この `…` だけが「タブ見出しではない」ことを示している
    let label = format!("{} … {} more", marker, hidden);
    // 一覧の行そのものではないので、通知行と同じく落として出す
    compose(&[(&label, Ink::Muted)], content_cols(cols))
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
// 非選択時は通常ウェイトにも落とす（決定36を拡張）。
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
        // ペイン名と同じく通常ウェイトに落とす（決定36を拡張。選択行はboldのまま残す）
        text = text.unbold_range(CWD_INDENT..end);
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
