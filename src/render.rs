// サイドバーの描画。
//
// レイアウト（決定202608070119）: 境界線 → ヘッダー(1行) → 境界線 → ツリー(可変) →
// 境界線 → フッター(1行) の5要素からなる固定枠（最下部にもう1行、zellij 本体の
// status-bar と離すための余白が付く）。要件は
// features/sidebar-tree/sidebar-header.feature（ヘッダー・枠構造）と
// sidebar-footer.feature（フッター）。ツリーの中身はタブ見出し行 > 配下の
// ペイン行 をタブ順で縦に並べたもの。
//
// **枠の高さ・位置はモードによらず不変**で、変わるのは中身（色・テキスト）だけ。
// 行数が動くと、モードの入退場のたびにツリー全体が上下にずれる。セッション名は
// 出さない — zellij 本体のトップバーが `Zellij (セッション名)` の形で常時
// 出しており、重複が視認性を下げる。
//
// 行が画面高に収まらないときは表示範囲を選択行へ寄せる（縦スクロール。
// 要件: features/sidebar-tree/sidebar-scroll.feature）。枠は固定で、
// あいだのツリーだけが動く。
//
// **このファイルが持つのは描画の共通語彙だけ**（`Row`・`Ink`・レイアウトの定数・
// `compose`・行頭とハイライトの組み立て）。枠の要素ごとの組み立ては子モジュールが
// 持つ: 行並びと表示範囲は `rows`、描画の入口は `draw`、ヘッダー・フッター・ヘルプは
// 同名のモジュール、ツリーの行は `pane_row`、トリアージ一覧の行は `triage_row`。
// 複数の子から使うものだけをここへ置くこと — 子どうしは兄弟なので、片方の中に
// 置いたものはもう片方から見えない。

use std::ops::Range;

use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::*;

use crate::agent::{AgentInfo, AgentState};
use crate::config::{Kind, SETTINGS};
use crate::deploy::TROOP;
use crate::host;
use crate::mark::MARK_GLYPH;
use crate::search::{Field, Hit, SearchPhase};
use crate::width::{
    colorable_char_limit, fold_highlight_indices, fold_to_width, pad_left, pad_to_width,
    shift_highlight_indices, truncate, truncate_start,
};
use crate::{Selectable, State};

mod draw;
mod footer;
mod header;
mod help;
mod pane_row;
mod rows;
mod triage_row;

// draw が使う行は子モジュールが組み立てる。テストからも `crate::render::` で引けるよう
// ここで再公開する
pub(crate) use pane_row::cwd_row;
pub(crate) use rows::overflow_row;
// 純粋関数なのでテストからは直接呼ぶ（本体からは rows の中でしか使わない）
#[cfg(test)]
pub(crate) use rows::reconcile_scroll;

// 画面に縦に積む1行ぶんの中身。
//
// 描画（draw）とクリック位置の逆引き（pane_at_row、要件:
// docs/requirements/req-click-to-focus.md）が**同じ並びを共有する**ために切り出してある。
// 行の増減を伴うレイアウト変更は必ず visible_rows() 側で行うこと。
// 描画だけ直すとクリックが行ずれする
pub(crate) enum Row<'a> {
    // ヘッダー。`▲ fujin` を常時出し、三角の色でいまのモードを示す。
    // モード中だけ直後にモードラベルが付く
    Header,
    // chrome（ヘッダー・フッター）と content（ツリー）の境目を示す横線。
    // 最上部・ヘッダー下・フッター上の3箇所に同じ見た目で出る（決定202608070119）
    Divider,
    // フッター。いまの状態で使えるコマンドを1行で出す。空にはならない
    Footer,
    // 高さだけを占めて何も描かない行。2箇所で使う:
    //  - ツリーが画面高に届かないときの埋め草（下の枠を最下部へ押し下げる。
    //    要件: sidebar-footer.feature の「フッターの高さと位置はモードによらず常に同じ」）
    //  - フッターと zellij 本体の status-bar のあいだに空ける最下部の1行
    Blank,
    // ヘルプオーバーレイの1行（要件: docs/requirements/req-nav-mode.md）。
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
    // トリアージ一覧の1行（要件: docs/requirements/req-triage-mode.md）。
    // タブ見出し行を持たないフラットな並びなので、所属タブ名を行に併記する
    Triage {
        entry: &'a Selectable,
        tab_name: &'a str,
    },
    // 直前のペイン行の cwd（決定202608060053）。ペイン行と同じペインを指すので、
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
    // 持つのが不揃いに見える（決定202608071906）
    Section(&'static str),
    // キーと、それが何をするか
    Entry(&'static str, &'static str),
    // 状態アイコン凡例の1行（決定202608062201）。アイコン・色・説明はすべて
    // AgentState のテーブルから引くので、ここは状態だけを持つ
    Legend(AgentState),
    // エージェントが乗っていないペインの印（`NO_AGENT_ICON`）の凡例。
    // AgentState のテーブルには無い別概念なので、Legend とは別の行として持つ
    NoAgent,
    Blank,
}

// ヘルプオーバーレイのキー一覧に続けて出す状態アイコン凡例（決定202608062201）。
// モードによらず同じものを出す — ツリーにもトリアージ一覧にも状態アイコンが
// 出るので、どのオーバーレイから引いても同じ表が要る。
//
// アイコン・色・説明はもちろん、**並びと状態の数まで** AgentState 側のテーブルから
// 引く。状態が増減しても凡例は自動で追従し、ここを直す必要はない。
// 末尾の1行だけは AgentState に無い「エージェントが乗っていないペイン」の印
const STATUS_LEGEND: [HelpRow; LEGEND_HEAD + AgentState::ALL.len() + 1] = {
    // キー一覧との間の空行・見出し・見出し下の空行
    let mut rows = [HelpRow::Blank; LEGEND_HEAD + AgentState::ALL.len() + 1];
    rows[1] = HelpRow::Section("status");
    // const 文脈では for も iterator も使えないので添字で回す
    let mut i = 0;
    while i < AgentState::ALL.len() {
        rows[LEGEND_HEAD + i] = HelpRow::Legend(AgentState::ALL[i]);
        i += 1;
    }
    // 状態の並びの後ろに置く。同じ `status` 節に入れるのは、状態アイコンと同じ列に
    // 出る記号だから — 節を分けると引くのに探す手数が増えるうえ、オーバーレイが
    // 見出し・空行のぶんさらに縦に伸びる（決定202608062201の凡例は高さに余裕が無い）
    rows[LEGEND_HEAD + AgentState::ALL.len()] = HelpRow::NoAgent;
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
// 要件: docs/requirements/req-floating-pane-indicator.md）
const FLOATING_BRACKETS: usize = 2;
// 右端に常に空ける幅。文字がサイドバーの縁に貼り付くと窮屈に見える。
// 左マージン（選択バーぶんの2セル）と揃えてある
const RIGHT_MARGIN: usize = 2;
// プレビュー用フローティングペイン（決定202608082045）で、本文より上に固定される行数
//（見出し・境界線）
const PREVIEW_HEAD: usize = 2;
// 対象ペイン名がまだ届いていないときに見出しへ出す文言。開いた直後の一瞬だけ
// 通る。UI文言は英語で統一する（ui-design.md）
const PREVIEW_PLACEHOLDER: &str = "preview";
// エージェント状態もコマンド状態も持たないペイン（CLIエージェントが乗っていない
// 作業ペイン）の状態アイコン列に置く印。**AgentState には含めない** — 既存の5状態は
// どれも「エージェントが居る」前提の状態で、「居ない」はその一種ではないため、
// 表示層のプレースホルダーとして持つ（docs/issues/issue-sidebar-cwd-row-legibility.md）。
// **状態色（0/1/2/3/6）は乗せない** — 意味の軸が違うものに状態色を割り当てない
// （原則2）。装飾も持たせず既定色のまま出す
pub(crate) const NO_AGENT_ICON: &str = "›";
// 凡例に出す説明。状態名（AgentState::label）と同じ書き方に揃える
pub(crate) const NO_AGENT_LABEL: &str = "no agent";

// 文字を置いてよい幅。選択行の背景は右マージンも含めて塗るので、
// 背景のパディング（pad_to_width）はこれではなく cols を使うこと
fn content_cols(cols: usize) -> usize {
    cols.saturating_sub(RIGHT_MARGIN)
}
// 文字に与える意味。zellij のテーマ側の色をそのまま借りる（決定202607302303と同じ方針で、
// 色そのものを設定項目にはしない）
#[derive(Clone, Copy)]
enum Ink {
    // 既定の文字色。読ませたい本文
    Plain,
    // 検索クエリの先頭 `/`。ヘルプオーバーレイの見出しも当初はこれだったが、
    // モード名を落として dim の節見出しにした（決定202608071906）
    //（ヘッダーのモードラベルは状態色なのでこちらではない・決定202608070119）
    Tag,
    // キーそのもの。zellij 本体の status-bar がキーを強調するのに倣う
    Key,
    // 添え物（補足行・境界線）。落として主役を目立たせる
    Muted,
    // 強調色をレベル指定で乗せる。ヘッダーの三角のように、色そのものが
    // 情報を持つ断片に使う
    Accent(usize),
}

// 1行の中間表現。**印字の直前まで `Text` にしない。**
//
// `Text` は装飾を「レベルごとの文字位置」へ畳んで文字列に抱えてしまうので、組み立ての
// 途中でも後からでも中身を読み返せない。レイアウトの回帰をテストから検査できるように
// するための中間形（docs/issues/issue-ui-requirements-approach.md の層2）。
//
// ビルダーは `Text` の同名メソッドと1対1のミラーで、**使用中の5種類だけ**を持つ。
// 掛ける順序がそのまま `Text` へ写るように、呼び出しを記録して後から再生する。
// 変換は印字の直前に `Text::from(&line)` で行う
#[derive(Clone, Debug, Default)]
pub(crate) struct Line {
    text: String,
    decorations: Vec<Decoration>,
}

// `Text` のビルダー呼び出しを、掛ける順序のまま覚えておくための記録
#[derive(Clone, Debug)]
enum Decoration {
    Color(usize, Range<usize>),
    ColorIndices(usize, Vec<usize>),
    Dim(Range<usize>),
    Unbold(Range<usize>),
    Opaque,
}

impl Line {
    pub(crate) fn new(text: impl AsRef<str>) -> Self {
        Self {
            text: text.as_ref().to_string(),
            decorations: Vec::new(),
        }
    }

    // 本文（装飾を除いた、実際に画面へ出る文字列）。名前は `Text::content()` の
    // ミラー——ビルダーと同じく、呼ぶ側が `Text` との違いを意識せずに済む。
    // 本番の描画は `Text` へ変換してから印字するので、読み出すのはテストだけ
    #[cfg(test)]
    pub(crate) fn content(&self) -> &str {
        &self.text
    }

    fn color_range(mut self, level: usize, range: Range<usize>) -> Self {
        self.decorations.push(Decoration::Color(level, range));
        self
    }

    fn color_indices(mut self, level: usize, indices: Vec<usize>) -> Self {
        self.decorations
            .push(Decoration::ColorIndices(level, indices));
        self
    }

    fn dim_range(mut self, range: Range<usize>) -> Self {
        self.decorations.push(Decoration::Dim(range));
        self
    }

    fn unbold_range(mut self, range: Range<usize>) -> Self {
        self.decorations.push(Decoration::Unbold(range));
        self
    }

    fn opaque(mut self) -> Self {
        self.decorations.push(Decoration::Opaque);
        self
    }
}

impl From<&Line> for Text {
    fn from(line: &Line) -> Self {
        let mut text = Text::new(&line.text);
        for decoration in &line.decorations {
            text = match decoration {
                Decoration::Color(level, range) => text.color_range(*level, range.clone()),
                Decoration::ColorIndices(level, indices) => {
                    text.color_indices(*level, indices.clone())
                }
                Decoration::Dim(range) => text.dim_range(range.clone()),
                Decoration::Unbold(range) => text.unbold_range(range.clone()),
                Decoration::Opaque => text.opaque(),
            };
        }
        text
    }
}

// ヘッダーの三角・モードラベルとフッターに乗せる色（要件: sidebar-header /
// sidebar-footer）。navモード・検索サブモードはレベル2（green）で、zellij の
// タブバーがアクティブなタブに使う色に対応させる
const NAV_LEVEL: usize = 2;
// トリアージモードはレベル0（orange）。緊急度を連想させる側へ寄せる
const TRIAGE_LEVEL: usize = 0;
// 終了操作サブモードはレベル6（error_color）。確認プロンプトを警告色で出す
//（決定202608080140）。新しい色は増やさず、状態アイコン `error` と同じ色を借りる
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
fn compose(segments: &[(&str, Ink)], cols: usize) -> Line {
    let mut body = String::new();
    let mut spans = Vec::with_capacity(segments.len());
    for (fragment, ink) in segments {
        let start = body.chars().count();
        body.push_str(fragment);
        spans.push((start, body.chars().count(), *ink));
    }
    let full_len = body.chars().count();
    let body = truncate(&body, cols);
    let limit = colorable_char_limit(body.chars().count(), full_len);
    let mut line = Line::new(&body);
    for (start, end, ink) in spans {
        let end = end.min(limit);
        if start >= end {
            continue;
        }
        line = match ink {
            Ink::Plain => line,
            Ink::Tag => line.color_range(3, start..end),
            Ink::Key => line.color_range(2, start..end),
            Ink::Muted => line.dim_range(start..end),
            Ink::Accent(level) => line.color_range(level, start..end),
        };
    }
    line
}

// カウンタ列の幅（決定202608060053、2026-08-08追記）。サブエージェント数 `+N`・未完了タスク数
// `[M]` をそれぞれ固定幅のフィールドに右揃えで置き、**カウンタを持つ行同士**で
// 桁を揃える。
//
// 幅は**そのフレームに出るペイン行の実測最大**で決める。誰もカウンタを持っていない
// フレームでは 0 になる。ただし「列の幅がいくつか」と「その幅ぶんの余白をこの行が
// 実際に負うか」は別の話 — 後者は行自身が `+N`/`[M]` のどちらかを持つかどうかで
// 決まる（`render()`）。持たない行は同じフレームの他行の状態に関わらずペイン名が
// 幅をすべて使う
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct CounterColumn {
    subagents: usize,
    open_tasks: usize,
}

impl CounterColumn {
    // この列に載る1行ぶんの文字列。自分自身が `+N`/`[M]` のどちらも持たない行は
    // 空文字列を返す — 他のペインがカウンタを持っていてもこの行は影響を受けない。
    // 片方だけ持つ行では、もう片方のフィールドは空白で埋めて桁の位置をずらさない
    fn render(&self, subagents: &str, open_tasks: &str) -> String {
        if subagents.is_empty() && open_tasks.is_empty() {
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
// 選択バー → 番号列 → マーク列 → アイコン → ペイン名（決定202608080250）
#[derive(Clone, Copy, Default)]
pub(crate) struct HeadCells<'a> {
    // 番号ジャンプサブモード中だけ Some（通し番号の表示と、番号入力バッファに
    // 前方一致して候補に残っているか。決定202608070342）
    pub(crate) number: Option<(&'a str, bool)>,
    // マーク列を出すフレームだけ Some（その行がマーク済みか。決定202608080250）
    pub(crate) mark: Option<bool>,
}

// ペイン行に出すカウンタの文字列。0 のときは出さない（決定202607302303: 静かな行は静かに）
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

// フローティングペインのペイン名を囲む丸括弧（要件:
// docs/requirements/req-floating-pane-indicator.md）。フローティング層ごと隠れうる
// ペインを、一覧の上で見分けられるようにするための印。
//
// 色・dim は乗せない — ペイン名の色は落とさない（決定202608080027）うえ、色は状態・モードへ
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

// ペイン行・トリアージ行のマーク列1つぶん（決定202608080250）。列を出すフレームでは、
// マークされていない行も空白で同じ幅を占めてアイコンの位置を揃える
fn mark_cell(mark: Option<bool>) -> String {
    match mark {
        Some(true) => format!("{} ", MARK_GLYPH),
        Some(false) => "  ".to_string(),
        None => String::new(),
    }
}

// 行頭（選択バー → 番号列 → マーク列 → 状態アイコン。決定202608080250の並び）。
// ペイン行とトリアージ行で共有する
struct RowHead {
    // "▌ {番号} {✓|空白}{icon} " の形。出さない列はそのまま詰める
    text: String,
    // マーク印の文字位置（列を出すフレームだけ）。アイコンの2文字手前で、
    // 番号列の有無に追従する
    mark_at: Option<usize>,
    // 状態アイコンの文字位置。head の末尾は常に「アイコン(1文字)+空白」なので、
    // 番号列の有無で動いても末尾から数えれば追従できる
    icon_at: usize,
}

fn row_head(
    is_highlighted: bool,
    number: Option<(&str, bool)>,
    mark: Option<bool>,
    icon: &str,
) -> RowHead {
    // 光っている行は左端にバーを立てる。テーマの選択色が沈む配色でもどこに
    // いるか一目で分かるようにするため（幅は2文字で固定し、後続の列の開始位置を
    // ずらさない）
    let prefix = if is_highlighted { "▌ " } else { "  " };
    let mark_cell = mark_cell(mark);
    let text = match number {
        Some((digits, _)) => format!("{}{} {}{} ", prefix, digits, mark_cell, icon),
        None => format!("{}{}{} ", prefix, mark_cell, icon),
    };
    let chars = text.chars().count();
    RowHead {
        mark_at: mark.map(|_| chars.saturating_sub(4)),
        icon_at: chars.saturating_sub(2),
        text,
    }
}

// 選択行の見た目（背景の帯 + 左端のバー）。ペイン行・トリアージ行・cwd行で共有する。
//
// **`selected()` は付けない。** zellij-tile の serialize は selected → opaque の
// 順にプレフィックスを前置して `zx…` にするのに対し、zellij 本体は x → z の順に
// 剥がすため、併用すると `x` が残って**レベル0の位置指定が丸ごと壊れる**
// （idle アイコンがテーマの base 色＝白系に落ちる）。`opaque()` だけなら
// プレフィックスは `z` の1文字で、パースは通る。
//
// 落としているものは無い — 併用時も `selected` は false と解釈されており、
// 帯の背景は元から `opaque` 側が塗っている（docs/issues/issue-idle-icon-color-on-selection.md）
fn highlight_row(text: Line) -> Line {
    text.opaque().color_range(2, 0..1)
}

// ペイン名の右側の列（カウンタ列・タブ名列）を右端揃えで足す。
// アイコンと列だけで幅を使い切るほど狭いときは切り詰める — はみ出すと
// 選択背景が端末側で折り返して次の行を汚す。
// 返り値は、列が切り詰められずに収まったときの列の (開始, 終了) 文字位置
fn append_right_column(label: &mut String, column: &str, inner: usize) -> Option<(usize, usize)> {
    let width = UnicodeWidthStr::width(column);
    let filler = inner.saturating_sub(UnicodeWidthStr::width(label.as_str()) + width);
    label.push_str(&" ".repeat(filler));
    let start = label.chars().count();
    label.push_str(column);
    let end = label.chars().count();
    let fitted = truncate(label, inner);
    let span = (fitted.chars().count() == end).then_some((start, end));
    *label = fitted;
    span
}

// ペイン名を通常ウェイトに落とす（決定202608080027改訂）。全行bold・同色だと状態アイコンの
// 色や選択行の背景が相対的に沈むため。**選択行はboldのまま残す** — 実機確認で
// 選択行まで落とすと「いまどこにいるか」が弱まった。丸括弧もペイン名の一部として
// 同じ太さで出す（要件: floating-pane-indicator）
fn unbold_name(text: Line, head: &str, open: &str, title: &str, close: &str) -> Line {
    let start = head.chars().count();
    let end = start + open.chars().count() + title.chars().count() + close.chars().count();
    text.unbold_range(start..end)
}

// 境界線。chrome（ヘッダー・フッター）と content（ツリー）の境目を示す。
// 最上部・ヘッダー下・フッター上の3本とも同じ見た目で引く（決定202608070119）。
//
// サイドバーは borderless で運用していて自前の枠は引かないが、この3本だけは
// 例外。領域を囲う枠ではなく境目を示す線なので許容する。
// 右マージンは他の行と同じく空ける — 端まで引くと縁に貼り付いて見える
pub(crate) fn divider_line(cols: usize) -> Line {
    let cols = content_cols(cols);
    let line = "─".repeat(cols);
    compose(&[(line.as_str(), Ink::Muted)], cols)
}
