// トリアージモード（要件: docs/requirements/triage-mode/）。
//
// navモードの内側に、検索サブモード（決定18）と同じ位置づけで乗るサブモード。
// タブの壁を無視して、エージェント状態を持つペインだけを緊急度順にフラットに並べる。
//
// ツリー表示の並び順には手を触れない（決定3）。通常表示を上書きするのではなく、
// `p` で切り替えて使う。

use zellij_tile::prelude::*;

use crate::nav::has_hard_modifier;
use crate::render::HelpRow;
use crate::{Selectable, State};

// トリアージモードのローカルUI状態。検索サブモードと同じく権威インスタンス
// （決定14）にしか発生しないため、兄弟インスタンスへは配らない（決定13の範囲外）
#[derive(Debug, Default)]
pub(crate) struct TriageState {
    // トリアージ一覧の中のカーソル（ペインID）。インデックスで持つと
    // 一覧の作り直しをまたいだときに別の行を指す（検索サブモードと同じ理由）
    pub(crate) cursor: Option<u32>,
    // トリアージモードに入る前の選択（Esc で戻すため）
    pub(crate) saved: Option<u32>,
}

impl State {
    // トリアージ一覧に載るペインを優先度順に並べる。
    //
    // デバウンスは持たない（要件: triage-mode）。状態が変わるたびに描画から
    // 呼び直され、そのフレームの並びがそのまま結果になる。「緊急なものが
    // 上に上がってくる」動きそのものがトリアージUIの価値なので、間引かない
    pub(crate) fn triage_entries(&self) -> Vec<&Selectable> {
        // エージェント状態を持たないペイン（フック通知を一度も受けていないシェル）と
        // `idle`（既読）は載せない。載せると「もう何も待っていないペイン」が
        // 一覧に常駐し、既読モデル（決定10）が消したはずの濁りが戻る
        let mut entries: Vec<(&Selectable, u8, u64)> = self
            .selectable
            .iter()
            .filter_map(|entry| {
                let info = self.agents.get(&entry.pane_id)?;
                let rank = info.state.triage_rank()?;
                Some((entry, rank, info.state_change_seq))
            })
            .collect();
        // 優先度階層（error > blocked > working > done）が先。同一階層内は
        // 直近の状態変化が新しい順＝シーケンス番号の降順。どちらも同じなら
        // 安定ソートによりツリー順のまま残る
        entries.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.2.cmp(&a.2)));
        entries.into_iter().map(|(entry, _, _)| entry).collect()
    }

    // トリアージ一覧の並び順のペインID列（カーソル移動用）
    fn triage_ids(&self) -> Vec<u32> {
        self.triage_entries().iter().map(|e| e.pane_id).collect()
    }

    // 実効カーソル。`TriageState::cursor` が指すペインが一覧から消えていたら
    // 先頭へ寄せる（一覧が空なら None）。
    //
    // フィールドを書き戻して追従させるのではなく参照のたびに畳むのは、一覧が
    // 変わる経路が多いため（ペインの増減・フックからの状態通知・既読クリア・
    // 兄弟インスタンスからの配布）。書き戻し方式だと、どこか1つ呼び忘れた瞬間に
    // カーソルが一覧に無い行を指す
    pub(crate) fn triage_cursor(&self) -> Option<u32> {
        let triage = self.triage.as_ref()?;
        let ids = self.triage_ids();
        triage
            .cursor
            .filter(|id| ids.contains(id))
            .or_else(|| ids.first().copied())
    }

    pub(crate) fn enter_triage(&mut self) {
        // 入場時のカーソルは**一覧の先頭＝いちばん緊急な行**（cursor: None を
        // triage_cursor() が先頭へ寄せる）。検索サブモードのように入る前の選択を
        // 継ぐことはしない — トリアージが答えるのは「今どれに手を入れるか」で、
        // 選択中のペインはたいてい今まさに自分が作業している側だから
        self.triage = Some(TriageState {
            cursor: None,
            saved: self.selectable.get(self.selected).map(|e| e.pane_id),
        });
    }

    // Esc: トリアージモードだけを抜けて navモードへ戻る。
    // `exit_nav_mode()` を呼んではいけない — 召喚インスタンス（決定16）だと
    // サイドバーごと閉じてしまう（検索サブモードの二段階 Esc と同じ理由）
    fn leave_triage(&mut self) {
        let Some(triage) = self.triage.take() else {
            return;
        };
        if let Some(saved) = triage.saved {
            self.select_pane_id(saved);
        }
    }

    // Enter: ジャンプして navモードごと抜ける。
    //
    // **専用の既読クリア処理は書かない。** 通常のジャンプと同じ
    // 「横取り解除 → focus_pane_with_id()」の手順だけを踏めば、既読クリアの検知
    // （PaneUpdate でのフォーカス変化）も兄弟インスタンスへの配布（決定13）も
    // そのまま働く。別経路を作ると決定13が踏んだ罠を再発明することになる
    fn confirm_triage(&mut self) {
        // 一覧が空のときの Enter は何もしない（トリアージモードに留まる）
        let Some(cursor) = self.triage_cursor() else {
            return;
        };
        let Some(index) = self.selectable.iter().position(|e| e.pane_id == cursor) else {
            return;
        };
        self.selected = index;
        // フォーカス移動でタブが変わりうるので、先に横取りを解除する
        //（navモード・検索サブモードの Enter と同じ順序。triage も一緒に破棄される）
        self.exit_nav_mode();
        self.broadcast_selection();
        self.focus_selected();
    }

    // トリアージモード中のキー解釈。戻り値は再描画するか。
    //
    // 一覧の中を動いてジャンプするだけなので、移動キーは navモードと同じものを
    // 揃える。クエリ入力が無いぶん1文字ショートカットを潰す必要がない
    pub(crate) fn handle_triage_key(&mut self, key: KeyWithModifier) -> bool {
        // Shift 以外の修飾キーは安全弁（決定12）。トリアージモードだけでなく
        // navモードごと抜ける — 抜けられなくなるより退場に倒す
        if has_hard_modifier(&key) {
            self.leave_nav_mode();
            return true;
        }
        let shifted = key.key_modifiers.contains(&KeyModifier::Shift);
        match key.bare_key {
            BareKey::Esc => self.leave_triage(),
            BareKey::Enter => self.confirm_triage(),
            BareKey::Down | BareKey::Char('j') => self.move_triage_cursor(true),
            BareKey::Up | BareKey::Char('k') => self.move_triage_cursor(false),
            BareKey::Tab => self.move_triage_cursor(!shifted),
            BareKey::Char('g') => self.jump_triage_cursor(false),
            BareKey::Char('G') => self.jump_triage_cursor(true),
            // 未定義キーは navモードごと退場（安全弁は最上位まで効かせる）
            _ => self.leave_nav_mode(),
        }
        // 検索サブモードと同じく、カーソルの移動は兄弟インスタンスへ配らない。
        // 選択が動くのは Enter で確定したときだけ
        true
    }

    // 一覧内のカーソル移動。端で止まる
    fn move_triage_cursor(&mut self, forward: bool) {
        let ids = self.triage_ids();
        if ids.is_empty() {
            return;
        }
        let current = self
            .triage_cursor()
            .and_then(|c| ids.iter().position(|&id| id == c));
        let next = match current {
            Some(i) if forward => (i + 1).min(ids.len() - 1),
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        if let Some(triage) = &mut self.triage {
            triage.cursor = Some(ids[next]);
        }
    }

    // 一覧の先頭／末尾へ（navモードの g / G と同じ）
    fn jump_triage_cursor(&mut self, to_end: bool) {
        let ids = self.triage_ids();
        let target = if to_end {
            ids.last().copied()
        } else {
            ids.first().copied()
        };
        if let (Some(triage), Some(target)) = (&mut self.triage, target) {
            triage.cursor = Some(target);
        }
    }

    // ヘルプオーバーレイに出すキー一覧（要件: nav-mode-hints）
    pub(crate) fn triage_help_lines(&self) -> &'static [HelpRow] {
        use HelpRow::{Blank, Entry, Note, Title};
        &[
            Title("[TRIAGE]", "keys"),
            Blank,
            Entry("j k up down tab", "move"),
            Entry("g G", "top / bottom"),
            Entry("enter", "jump & exit"),
            Entry("esc", "back to tree"),
            Entry("?", "this help"),
            Blank,
            Note("press any key to close"),
        ]
    }
}
