// サイドバー幅のタブ間追従（用語: .docs/terms/width-sync.md。
// 経緯: .docs/issues/issue-sidebar-width-persist-across-tabs.md）。
//
// zellij の `new_tab_template` はタブ生成時に複製されるだけの静的な雛形で、
// 実行時のリサイズは書き戻されない。そこで「誰かがリサイズしたら、その桁数を
// 兄弟インスタンスへ配り、各自が自分のペインを寄せる」で揃える。
//
// 絶対値で幅を指定するAPIは無い（`Resize` は Increase/Decrease だけ）ので、
// 寄せるのは「自分の幅を見て1段階撃つ」を描画のたびに繰り返す帰還制御になる。
// 非可視インスタンスにも pipe の受け口が true を返せば描画は回る（実測5ms。
// `.docs/dev/api-reference.md`）ので、背面のタブもその場で寄る。
//
// 状態ダンプの配布（`sync`）とは運ぶものも寄せ方も別の関心なので、モジュールを分ける。
// セル幅の計算（`width`）とも別 — あちらは表示幅の純粋関数で、ここは幅を**寄せる**操作。

use zellij_tile::prelude::*;

use crate::host;
use crate::sync::send_to_plugin;
use crate::{State, WIDTH_PIPE};

// 目標の幅へ寄せるために撃つ相対リサイズの上限回数
//（.docs/issues/issue-sidebar-width-persist-across-tabs.md）。zellij のリサイズは
// 端末幅の一定割合ずつ動く量子化された操作で、目標にぴったり乗る保証が無い。
// 乗らないまま撃ち続けると幅が振動するので回数で打ち切る
pub(crate) const WIDTH_MAX_ATTEMPTS: usize = 6;

// 目標との差がこの桁数以内なら到達として扱う。端末ウィンドウのリサイズでは
// 全タブが割合から桁数へ丸め直されるため、タブ間で1桁の食い違いが出うる。
// これを律儀に追うと1段階（端末幅の5%）まるごと動いて、かえって目標から離れる
const WIDTH_TOLERANCE: usize = 1;

// 幅の目標を教えてほしいという問い合わせ。桁数と同じ pipe に相乗りさせる
//（桁数は数値なので、数値にならない文字列とは取り違えない）
pub(crate) const WIDTH_REQUEST: &str = "?";

impl State {
    // 描画のたびに呼ぶ。前回の幅との差から「利用者にリサイズされたか」を見て、
    // 必要なら目標へ1段階寄せる。`viewport_cols` を更新する前に呼ぶこと
    pub(crate) fn reconcile_width(&mut self, cols: usize) {
        // 常駐サイドバー以外（プレビュー・召喚フローティング）は幅の権威にも
        // 寄せ先にもしない。どちらも自前の幅で開かれる別物
        if self.is_preview || self.summoned || cols == 0 {
            return;
        }
        let previous = self.viewport_cols;
        if let Some(fired) = self.width_adjusting {
            if previous == cols {
                // 撃った結果がまだ届いていない（別の理由の描画が挟まった。
                // 新規タブのロード直後は着地の 5ms の間にも描画が何度も来る）。
                // ここで撃ち足すと多重に飛び、あとから届く着地を利用者の操作と
                // 取り違えて配り直してしまう（実測でこの経路を踏んだ）。
                // 幅が動くまで何もせず待つ — 着地しないままなら（最小幅に
                // 当たっている等）、撃てない以上これ以上できることも無い
                return;
            }
            self.width_adjusting = None;
            let landed_as_fired = match fired {
                Resize::Increase => cols > previous,
                Resize::Decrease => cols < previous,
            };
            if landed_as_fired {
                // 自分が撃ったリサイズの着地。利用者の操作ではないので配り直さない
                self.settle_width_after_step(previous, cols);
            } else {
                // 撃った向きと逆に動いた ＝ 自分の着地ではありえない。
                // 着地を待つ間に利用者がリサイズした
                self.claim_width_authority(previous, cols);
            }
        } else if previous != 0 && previous != cols && self.width_target != Some(cols) {
            // 利用者のリサイズ。目標と同じ幅になっただけのときを除いてあるのは、
            // 自分が撃ったリサイズの結果が `width_adjusting` を倒したあとの
            // 描画で届く可能性があるため。それを利用者の操作と取り違えると、
            // 寄せ終えた側が権威を名乗って幅を配り直してしまう
            self.claim_width_authority(previous, cols);
        }
        self.apply_width_target(cols);
    }

    // 利用者のリサイズを観測した。ここが新しい権威になる
    fn claim_width_authority(&mut self, previous: usize, cols: usize) {
        self.width_target = Some(cols);
        self.width_attempts = 0;
        self.width_settled = false;
        eprintln!(
            "fujin: sidebar width {} -> {}, broadcasting",
            previous, cols
        );
        self.broadcast_to_siblings(WIDTH_PIPE, &cols.to_string());
    }

    // 1段階撃って動いたあとの後始末。目標を跨いだら、跨いだ両端のうち
    // 目標に近いほうで止める。刻みが目標に乗らないのは、ドラッグリサイズが
    // 1セル単位で動く（実測: mouse_handler が delta/viewport で percent を出す）
    // 一方、プラグインの resize は 5% 固定刻みしか撃てないため
    //（.docs/issues/issue-sidebar-width-persist-across-tabs.md）
    fn settle_width_after_step(&mut self, previous: usize, cols: usize) {
        let Some(target) = self.width_target else {
            return;
        };
        let before = target as isize - previous as isize;
        let after = target as isize - cols as isize;
        if after == 0 || before.signum() == after.signum() {
            return;
        }
        if after.abs() <= before.abs() {
            // 跨いだ着地点のほうが目標に近い（か同距離）。ここが最寄り
            eprintln!(
                "fujin: sidebar width overshot {} -> {} (target {}), settling",
                previous, cols, target
            );
            self.width_settled = true;
        } else {
            // 跨ぐ前の幅のほうが近かった。settled を立てずに戻ると、apply が
            // 目標へ向けて撃つ ＝ 1段階戻る。戻した着地でまた跨ぐが、その
            // ときは目標との差が今より小さいので上の分岐で止まる
            eprintln!(
                "fujin: sidebar width overshot {} -> {} (target {}), stepping back",
                previous, cols, target
            );
        }
    }

    // 目標と食い違っていたら1段階だけ撃つ。結果は後続の描画で受け取る
    fn apply_width_target(&mut self, cols: usize) {
        let Some(target) = self.width_target else {
            return;
        };
        if target.abs_diff(cols) <= WIDTH_TOLERANCE {
            self.width_attempts = 0;
            self.width_settled = false;
            return;
        }
        if self.width_settled {
            return;
        }
        if self.width_attempts >= WIDTH_MAX_ATTEMPTS {
            eprintln!(
                "fujin: sidebar width gave up at {} (target {})",
                cols, target
            );
            self.width_settled = true;
            return;
        }
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        let resize = if cols < target {
            Resize::Increase
        } else {
            Resize::Decrease
        };
        // サイドバーは左端に置かれる（決定202607302257）ので、自分の幅を動かす境界は右側
        host::resize_pane_with_id(
            ResizeStrategy::new(resize, Some(Direction::Right)),
            PaneId::Plugin(own_id),
        );
        self.width_attempts += 1;
        self.width_adjusting = Some(resize);
    }

    // fujin_width の受け口。桁数をそのまま運ぶ。payload が `?` のときは
    // 「今の目標を教えてほしい」という新入りからの問い合わせ
    pub(crate) fn handle_width_pipe(&mut self, payload: Option<&str>, source: &PipeSource) -> bool {
        // CLI からの送信は payload 付きの1通目の後に payload 無しの2通目
        //（EOFマーカー）が届く。意味は無いので黙って捨てる
        //（.docs/issues/issue-cli-pipe-testing-pitfalls.md）
        let Some(raw) = payload else {
            return false;
        };
        if raw.trim() == WIDTH_REQUEST {
            if let PipeSource::Plugin(asker) = source {
                self.push_width_to(*asker);
            }
            return false;
        }
        let Some(target) = raw.trim().parse::<usize>().ok().filter(|w| *w > 0) else {
            eprintln!("fujin: unparsable width payload: {:?}", raw);
            return false;
        };
        if self.width_target == Some(target) {
            return false;
        }
        self.width_target = Some(target);
        self.width_attempts = 0;
        self.width_settled = false;
        // 寄せるのは描画のとき（自分の幅が分かるのがそこだけ）なので、
        // 描き直させることが即ち寄せ直しの合図になる
        true
    }

    // 新入りへ現在の目標幅を伝える。cwd表示と同じく無条件に送る — 新入りは
    // レイアウトの雛形の幅で開くので、目標と一致しているかを自分では知らない
    pub(crate) fn push_width_to(&self, plugin_id: u32) {
        if let Some(target) = self.width_target {
            send_to_plugin(plugin_id, WIDTH_PIPE, target.to_string());
        }
    }
}
