// 終了操作サブモード（決定35。要件: docs/requirements/pane-close-kill/）。
//
// 選択行のペインを閉じる（close）・killする（kill）・確実に閉じる（kill→close）。
// 検索サブモード・番号ジャンプサブモードと同じく、navモードの内側で完結する
// ミニフローとして乗る。入場キー（`d`）でフッターが確認プロンプトに転用され、
// close(`c`) / kill(`k`) / kill→close(`x`) / 取消(`Esc`) を選ぶ。
//
// フッター内は専用のキー空間なので、navモード本体の `k`（select_previous）と
// ここの `k`（kill）は衝突しない。

use zellij_tile::prelude::*;

use crate::nav::has_hard_modifier;
use crate::render::HelpRow;
use crate::State;

// 終了操作の3値（決定35）。対象種別（エージェント状態・コマンド状態・どちらも
// 持たないペイン）で出し分けはしない — 対象プロセスが実質存在しない場合は
// no-op になるだけなので、種別ごとの例外を設けない
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Termination {
    // プロセスには触れず、ペインだけを閉じる
    Close,
    // SIGKILLを送るだけ。多くの場合ペインも自動的に閉じるが、コマンドペインでは
    // 閉じずに残る（決定35の実測）
    Kill,
    // kill送信後、明示的にペインも閉じる確実な閉じ方。特にコマンドペインで意味を持つ
    KillThenClose,
}

impl Termination {
    // 確認プロンプト内のキー割り当て（決定35）
    pub(crate) fn from_key(key: BareKey) -> Option<Self> {
        match key {
            BareKey::Char('c') => Some(Self::Close),
            BareKey::Char('k') => Some(Self::Kill),
            BareKey::Char('x') => Some(Self::KillThenClose),
            _ => None,
        }
    }

    // 対象ペインに与える効果 `(SIGKILLを送るか, ペインを閉じるか)`。
    // 名前どおりの順（kill → close）で実行する
    pub(crate) fn effects(self) -> (bool, bool) {
        match self {
            Self::Close => (false, true),
            Self::Kill => (true, false),
            Self::KillThenClose => (true, true),
        }
    }
}

// 終了操作サブモードのローカルUI状態。検索サブモードと同じく権威インスタンス
//（決定14）にしか発生しないため、兄弟インスタンスへは配らない（決定13の範囲外）
#[derive(Debug)]
pub(crate) struct TerminationState {
    // 入場時の選択行のペインID。確認の途中で `self.selected` が別のペインを
    // 指すようになっても（兄弟インスタンスからの選択配布・ペインの増減による
    // クランプ）、確認したときに見えていたペイン以外を kill しないため、
    // インデックスではなくペインIDで捕まえておく
    pub(crate) target: u32,
}

impl State {
    // 入場（navモードの `d`）。選択行が無ければ入らない — 対象の無い確認
    // プロンプトを出しても、どのキーを押しても no-op にしかならない
    pub(crate) fn enter_termination(&mut self) {
        let Some(entry) = self.selectable.get(self.selected) else {
            return;
        };
        self.termination = Some(TerminationState {
            target: entry.pane_id,
        });
    }

    // 終了操作サブモード中のキー解釈。戻り値は再描画するか
    pub(crate) fn handle_termination_key(&mut self, key: KeyWithModifier) -> bool {
        // Shift 以外の修飾キーは安全弁（決定12）。サブモードだけでなく navモード
        // ごと抜ける — 抜けられなくなるより退場に倒す
        if has_hard_modifier(&key) {
            self.leave_nav_mode();
            return true;
        }
        if let Some(kind) = Termination::from_key(key.bare_key) {
            self.run_termination(kind);
            return true;
        }
        match key.bare_key {
            // Esc は取り消し。サブモードだけ抜けて navモードに留まる（検索・
            // 番号ジャンプと同じパターン）。`exit_nav_mode()` を呼んではいけない —
            // 召喚インスタンスなら取り消しでサイドバーごと閉じてしまう（決定16）
            BareKey::Esc => self.termination = None,
            // 未定義キーは navモードごと退場（安全弁は最上位まで効かせる）。
            // 終了操作は実行されない
            _ => self.leave_nav_mode(),
        }
        true
    }

    // 実行の内訳 `(対象ペインID, SIGKILLを送るか, ペインを閉じるか)`。
    // ホスト関数を呼ぶ手前で畳んでおく — 副作用だけのホスト関数は結果を
    // 観測できないので、ここまでをテストの検証対象にする
    pub(crate) fn termination_plan(&self, kind: Termination) -> Option<(u32, bool, bool)> {
        let target = self.termination.as_ref()?.target;
        // 確認の途中で対象が閉じられていたら何もしない。閉じたペインのIDへ
        // 送っても実害はないが、送らない側に倒す
        if !self.selectable.iter().any(|e| e.pane_id == target) {
            return None;
        }
        let (sigkill, close) = kind.effects();
        Some((target, sigkill, close))
    }

    // 選んだ終了操作を実行し、サブモードを抜けて navモードへ戻る
    fn run_termination(&mut self, kind: Termination) {
        let plan = self.termination_plan(kind);
        // 実行できたかによらずサブモードは畳む。プロンプトを出したまま
        // 残すと、次のキーがまた終了操作として解釈される
        self.termination = None;
        let Some((pane_id, sigkill, close)) = plan else {
            return;
        };
        let pane = PaneId::Terminal(pane_id);
        // 順序は kill → close（決定35）。どちらのホスト関数も送りっぱなしで
        // 応答を待たない（zellij-tile 0.44.3 の shim）
        if sigkill {
            send_sigkill_to_pane_id(pane);
        }
        if close {
            close_pane_with_id(pane);
        }
    }

    // ヘルプオーバーレイに出すキー一覧（要件: nav-mode-hints）。
    // フッターの確認プロンプトには収まらない Esc の行き先もここで補う
    pub(crate) fn termination_help_lines(&self) -> &'static [HelpRow] {
        use HelpRow::{Blank, Entry, Section};
        &[
            Section("keys"),
            Blank,
            Entry("c", "close pane"),
            Entry("k", "kill process"),
            Entry("x", "kill & close"),
            Entry("esc", "cancel"),
            Entry("?", "this help"),
        ]
    }
}
