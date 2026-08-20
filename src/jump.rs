// 番号ジャンプサブモード（navモード内の `n`、決定202608070342。
// 要件: docs/requirements/req-pane-number-jump.md）。
//
// 選択対象の全ペインに全タブ貫通の通し番号を振り、番号の入力でジャンプする。
// 曖昧性解消方式（vimiumのリンクヒントに近い）: 数字を1つ入力するたびに
// 前方一致で候補を絞り込み、1件に確定した時点で即ジャンプする。

use zellij_tile::prelude::*;

use crate::nav::has_hard_modifier;
use crate::State;

// サブモードのローカルUI状態。
// 検索サブモードと同じく権威インスタンスにしか発生しないため、
// 兄弟インスタンスへは配らない（決定202608012141の範囲外）
#[derive(Debug, Default)]
pub(crate) struct JumpState {
    // 番号入力バッファ。数字が入るたびに通し番号へ前方一致で照合し、
    // 候補が1件になった時点でジャンプ、0件になったら空へ戻す（決定202608070342）
    pub(crate) buffer: String,
}

impl State {
    pub(crate) fn enter_jump(&mut self) {
        self.jump = Some(JumpState::default());
    }

    // 通し番号の桁数（番号列の幅）。総数の桁数に固定し、全番号をゼロ埋めで
    // 揃える（決定202608070342）。桁数を固定すると番号どうしが互いの前方一致にならず
    //（prefix-free）、「1 を打ったが 10 があるので確定できない」という
    // 行き止まりが構造的に起きない
    pub(crate) fn pane_number_width(&self) -> usize {
        self.selectable.len().to_string().len()
    }

    // 選択対象 flat_index 番目の行に振る通し番号の表示（ゼロ埋め、1始まり）
    pub(crate) fn pane_number(&self, flat_index: usize) -> String {
        format!(
            "{:0width$}",
            flat_index + 1,
            width = self.pane_number_width()
        )
    }

    // 番号ジャンプサブモード中、この行の番号列に出すセル。
    // 返り値は (通し番号の表示, 番号入力バッファに前方一致して候補に残っているか)。
    // サブモード外は None ＝ 番号列そのものを出さない（決定202608070342: 平常時の幅配分を崩さない）
    pub(crate) fn jump_number(&self, flat_index: usize) -> Option<(String, bool)> {
        let jump = self.jump.as_ref()?;
        let number = self.pane_number(flat_index);
        let matches = number.starts_with(&jump.buffer);
        Some((number, matches))
    }

    // 番号ジャンプサブモード中のキー解釈。数字だけを受け、それ以外は
    // navモード本体と同じ安全弁（決定202607310311）に倒す
    pub(crate) fn handle_jump_key(&mut self, key: KeyWithModifier) -> bool {
        // Shift 以外の修飾キーは安全弁 — サブモードだけでなく navモードごと抜ける
        if has_hard_modifier(&key) {
            self.leave_nav_mode();
            return true;
        }
        match key.bare_key {
            // Esc はサブモードだけ抜けて navモードに留まる（検索・トリアージと
            // 同じパターン）。exit_nav_mode() を呼んではいけない — 召喚
            // インスタンスなら番号入力の取り消しでサイドバーごと閉じてしまう（決定202608011644）
            BareKey::Esc => self.jump = None,
            BareKey::Backspace => {
                if let Some(jump) = &mut self.jump {
                    jump.buffer.pop();
                }
            }
            BareKey::Char(c @ '0'..='9') => self.push_jump_digit(c),
            // 未定義キーは navモードごと退場（安全弁は最上位まで効かせる）
            _ => self.leave_nav_mode(),
        }
        // 番号入力の途中経過は兄弟インスタンスへ配らない（決定202608070342。検索サブモードと
        // 同じ扱い）。確定時のジャンプは push_jump_digit 側で配る
        //
        // プレビューも更新しない（決定202608082045）。候補を絞っているあいだは対象ペインが
        // 定まらないので、オンのまま入ってきた場合は直前の表示を保つ
        //（`refresh_preview` が番号ジャンプサブモード中は何もしない）
        true
    }

    // 数字を1つ足して候補を引き直す。前方一致の候補が1件になったら即ジャンプ、
    // 0件になったらバッファを空に戻して次の数字からやり直す（決定202608070342。
    // Backspace での訂正を強制しないための救済）
    fn push_jump_digit(&mut self, digit: char) {
        let Some(jump) = &mut self.jump else {
            return;
        };
        jump.buffer.push(digit);
        let buffer = jump.buffer.clone();
        let (first, ambiguous) = {
            let mut candidates = (0..self.selectable.len())
                .filter(|&index| self.pane_number(index).starts_with(&buffer));
            let first = candidates.next();
            (first, candidates.next().is_some())
        };
        match first {
            // 0件: 存在しない番号。その場でリセットして打ち直させる
            None => {
                if let Some(jump) = &mut self.jump {
                    jump.buffer.clear();
                }
            }
            // 1件: 確定。既存のジャンプと同じ手順で navモードごと抜ける
            //（検索サブモードの confirm_search と同じ順序）
            Some(index) if !ambiguous => {
                self.selected = index;
                self.exit_nav_mode();
                self.broadcast_selection(); // 確定時だけ配る（決定202608012141）
                self.focus_selected();
            }
            // 2件以上: まだ曖昧。次の数字を待つ
            Some(_) => {}
        }
    }
}
