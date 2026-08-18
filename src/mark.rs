// 複数選択（マーク。決定202608080250。要件: docs/requirements/pane-termination-multi-select/）。
//
// 一括操作の対象として選んだペインの集合を持つ。既存の「選択」（`State::selected`、
// 単一のナビゲーションカーソル）とは別概念で、**タブをまたいでよく、navモードを
// 退場しても保持される**。
//
// 専用サブモードは作らない — マークはトグルだけの軽い操作なので、ツリー表示・
// 検索の絞り込み結果・トリアージ一覧のどこからでも同じように積み上げる。
// 対象は「いま光っている行」で、どの一覧に居るかで引き先が変わる（`mark_cursor`）。
//
// 集合は兄弟インスタンスへ配る（決定202608012141の選択ペインID配布と同じ形）。タブをまたぐ
// マークを許した以上、権威インスタンス（決定202608012142）が交代した先でもマークが見えないと
// 「どれを選んだか」を見失う。

use crate::{State, MARK_PIPE};

// マーク済みの行に出す印（決定202608080250）。選択バー `▌` を置き換える案は採らない —
// カーソル位置とマーク済みが同時に成立する行で区別がつかなくなる
pub(crate) const MARK_GLYPH: &str = "✓";

impl State {
    // いま光っている行のペイン（注目ペイン）。トリアージ一覧・検索の絞り込み
    // 結果ではそれぞれのカーソル、ツリー表示では選択行を引く。
    //
    // マーク（決定202608080250）とプレビュー（決定202608082045）という**一覧を問わず同じキーで働く
    // 横断的操作**が、どの一覧に居るかを気にせず対象を引くための1本。
    // 番号ジャンプサブモード中のマークはここへ来ない（数字専用の入力空間の
    // 安全弁が先に効いて navモードごと退場する）が、プレビューはオンのまま
    // 番号ジャンプへ入れるので、呼び出し側が更新を止める（決定202608082045）
    pub(crate) fn highlighted_pane(&self) -> Option<u32> {
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
        let Some(pane_id) = self.highlighted_pane() else {
            return;
        };
        if !self.marked.remove(&pane_id) {
            self.marked.insert(pane_id);
        }
        self.broadcast_marks();
    }

    // マーク全解除。誤って積み上げたマークからの回復コストを下げる
    //（決定202607310311の安全弁と同じ思想）
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

    // 選択対象から消えたペインのマークを取り除く（決定202608080250。`selected` のクランプと
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
    // 一括操作の実行順序はこれで固定する（決定202608080250）
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
        self.broadcast_to_siblings(MARK_PIPE, &self.mark_dump());
    }

    // 新入りインスタンスへの押し付け（決定202608012141）。既存インスタンスが持っている
    // マークは、配らない限り新入りには一生見えない
    pub(crate) fn push_marks_to(&self, plugin_id: u32) {
        crate::sync::send_to_plugin(plugin_id, MARK_PIPE, self.mark_dump());
    }

    pub(crate) fn mark_dump(&self) -> String {
        self.marked
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    // fujin_mark の受け口。マークも選択と同じくペインIDで運ぶ（決定202608080250）。
    // 集合まるごとを受け取って置き換えるので、空ペイロードは全解除を意味する
    pub(crate) fn handle_mark_pipe(&mut self, payload: Option<&str>) -> bool {
        payload.map(|raw| self.apply_marks(raw)).unwrap_or(false)
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
