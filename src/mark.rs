// 複数選択（マーク。決定39。要件: docs/requirements/pane-termination-multi-select/）。
//
// 一括操作の対象として選んだペインの集合を持つ。既存の「選択」（`State::selected`、
// 単一のナビゲーションカーソル）とは別概念で、**タブをまたいでよく、navモードを
// 退場しても保持される**。
//
// 専用サブモードは作らない — マークはトグルだけの軽い操作なので、ツリー表示・
// 検索の絞り込み結果・トリアージ一覧のどこからでも同じように積み上げる。
// 対象は「いま光っている行」で、どの一覧に居るかで引き先が変わる（`mark_cursor`）。
//
// 集合は兄弟インスタンスへ配る（決定13の選択ペインID配布と同じ形）。タブをまたぐ
// マークを許した以上、権威インスタンス（決定14）が交代した先でもマークが見えないと
// 「どれを選んだか」を見失う。

use zellij_tile::prelude::*;

use crate::{State, MARK_PIPE};

// マーク済みの行に出す印（決定39）。選択バー `▌` を置き換える案は採らない —
// カーソル位置とマーク済みが同時に成立する行で区別がつかなくなる
pub(crate) const MARK_GLYPH: &str = "✓";

impl State {
    // マークの対象になるペイン ＝ いま光っている行。トリアージ一覧・検索の
    // 絞り込み結果ではそれぞれのカーソル、ツリー表示では選択行を引く
    //（番号ジャンプサブモード中はここへ来ない。数字専用の入力空間の安全弁が
    // 先に効いて navモードごと退場する）
    fn mark_cursor(&self) -> Option<u32> {
        if self.triage.is_some() {
            return self.triage_cursor();
        }
        if let Some(search) = &self.search {
            return search.cursor;
        }
        self.selectable.get(self.selected).map(|e| e.pane_id)
    }

    // マークのトグル。同じキーで付け外しする
    pub(crate) fn toggle_mark(&mut self) {
        let Some(pane_id) = self.mark_cursor() else {
            return;
        };
        if !self.marked.remove(&pane_id) {
            self.marked.insert(pane_id);
        }
        self.broadcast_marks();
    }

    // マーク全解除。誤って積み上げたマークからの回復コストを下げる
    //（決定12の安全弁と同じ思想）
    pub(crate) fn clear_marks(&mut self) {
        if self.marked.is_empty() {
            return;
        }
        self.marked.clear();
        self.broadcast_marks();
    }

    pub(crate) fn is_marked(&self, pane_id: u32) -> bool {
        self.marked.contains(&pane_id)
    }

    // 選択対象から消えたペインのマークを取り除く（決定39。`selected` のクランプと
    // 同じ考え方）。閉じたペインのマークを残すと、件数だけ合わない確認プロンプトが出る
    pub(crate) fn prune_marks(&mut self) {
        if self.marked.is_empty() {
            return;
        }
        // 一覧側は不変借用で先に取り出す（`marked` の可変借用と両立させるため）
        let selectable = &self.selectable;
        self.marked
            .retain(|id| selectable.iter().any(|e| e.pane_id == *id));
    }

    // マーク済みペインをツリー順（`selectable` の並び）に並べたもの。
    // 一括操作の実行順序はこれで固定する（決定39）
    pub(crate) fn marked_in_tree_order(&self) -> Vec<u32> {
        self.selectable
            .iter()
            .map(|e| e.pane_id)
            .filter(|id| self.marked.contains(id))
            .collect()
    }

    // 集合を兄弟インスタンスへ配る（運搬形式はペインIDのカンマ区切り）。
    // 空文字列は全解除を意味する
    fn broadcast_marks(&self) {
        let payload = self.mark_dump();
        for sibling in &self.known_siblings {
            pipe_message_to_plugin(
                MessageToPlugin::new(MARK_PIPE)
                    .with_destination_plugin_id(*sibling)
                    .with_payload(payload.clone()),
            );
        }
    }

    // 新入りインスタンスへの押し付け（決定13）。既存インスタンスが持っている
    // マークは、配らない限り新入りには一生見えない
    pub(crate) fn push_marks_to(&self, plugin_id: u32) {
        pipe_message_to_plugin(
            MessageToPlugin::new(MARK_PIPE)
                .with_destination_plugin_id(plugin_id)
                .with_payload(self.mark_dump()),
        );
    }

    pub(crate) fn mark_dump(&self) -> String {
        self.marked
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    // 配られた集合をそのまま採る。戻り値は再描画するか。
    //
    // 受け手側の `selectable` で絞り込まない — 非可視インスタンスの一覧は古く
    //（`PaneUpdate` が届かない）、知らないタブのペインを弾くと配ったそばから
    // 消えてしまう。掃除は一覧を持っている側の `prune_marks()` に任せる
    pub(crate) fn apply_marks(&mut self, raw: &str) -> bool {
        let marked = raw
            .split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok())
            .collect();
        if self.marked == marked {
            return false;
        }
        self.marked = marked;
        true
    }
}
