// ヘルプオーバーレイの行（決定202608071906）。
//
// キー列の幅はいま出ているキー一覧の実測で決めるので、モードによって変わる。

use super::*;

// キー列と説明のあいだに空ける幅。キー列の幅自体は固定せず、**そのモードに
// 出るキーの実測最大**で決める（決定202608071906。カウンタ列と同じ考え方）。17セル固定に
// していたころは、いちばん長い `j k up down tab` のために全行が空白を払っていた
const HELP_KEY_GAP: usize = 2;

// キー列に置いた文字の後ろに空ける幅。列幅を超える場合は空白1つだけ空けて
// 続ける（列は崩れるが、説明が切り詰められて消えるよりはよい）
fn help_key_pad(keys: &str, column: usize) -> usize {
    column.saturating_sub(UnicodeWidthStr::width(keys)).max(1)
}

impl State {
    // キー列の幅。いま出ているキー一覧の実測最大で決めるので、モードによって
    // 変わる（決定202608071906）。凡例のアイコンは1文字なので、列幅を押し上げない
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
                // 凡例の目的そのものだから（決定202608062201）
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
            HelpRow::NoAgent => {
                // 状態の行と同じ列割りで置くが、アイコンには状態色を乗せずに
                // dim で出す（ツリー側の見た目と揃える）
                let gap = " ".repeat(help_key_pad(NO_AGENT_ICON, column));
                compose(
                    &[
                        (&indent, Ink::Plain),
                        (NO_AGENT_ICON, Ink::Plain),
                        (&gap, Ink::Plain),
                        (NO_AGENT_LABEL, Ink::Plain),
                    ],
                    cols,
                )
            }
            HelpRow::Blank => Text::new(""),
        }
    }
}
