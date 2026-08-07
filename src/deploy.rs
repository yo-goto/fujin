// 配置演出 — 新規エージェント検出をトリガーに、ヘッダーで一度だけ再生する
// 一過性のアニメーション（要件: docs/requirements/header-animation/）。
//
// 兵（検出したエージェント1体につき1体）がブランド行の右側から発進して右へ流れ、
// 着地列（先に発進した兵ほど奥、後続ほど手前）に整列して静止する。少し見せたのち
// 消えて、ヘッダーは通常表示へ**完全に**戻る — 稼働数のような機能的な情報は
// 一切残さない、純粋な遊び心の演出という位置づけ。
//
// 駆動は `set_timeout()` + `Event::Timer`。再生中だけタイマーを繋ぎ直し、
// 終わったら鎖を切る（静かなときにタイマーを回し続けない）。
//
// **再生できるのは可視インスタンスだけ**。トリガーにする `Event::PaneUpdate` が
// 可視インスタンスにしか届かないため（docs/dev/api-reference.md）で、見ていない
// 間の検出は遡って再生しない。「見ている間だけのご褒美演出」として要件側で
// 許容した制約なので、実装で解消しようとしないこと。

use std::collections::BTreeSet;

use zellij_tile::prelude::*;

use crate::State;

// 兵の記号。ヘッダーの本陣 `▲` と対をなす、白抜きの小さい三角
pub(crate) const TROOP: &str = "▵";

// フレーム間隔（秒）。実機での見え方はまだ詰めていない暫定値
pub(crate) const FRAME_INTERVAL: f64 = 0.15;

// 1フレームで兵が進むセル数
const SPEED: usize = 2;
// 兵どうしの発進間隔（フレーム）。時間差を付けて隊列に見せる
const STAGGER: usize = 2;
// 着地列どうしの間隔（セル）。詰めると整列した兵が1つの塊に見える
const LANDING_GAP: usize = 2;
// 全員が着地してから消えるまでの保持フレーム数。横一列に並ぶ瞬間そのものを
// 見せるための間で、これが0だと「布陣」ではなく素通りに見える
const HOLD: usize = 3;

// 再生中の配置演出。数えているのは**出す兵の数と経過フレームだけ**で、
// 兵1体ごとの位置は持たない — 位置は添字と経過フレームから導出できるうえ、
// 幅（着地列）は描画時にしか分からないため
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Deployment {
    // 出す兵の数（＝検出したエージェントの数の合計）
    pub(crate) troops: usize,
    // 発進からの経過フレーム
    pub(crate) frame: usize,
}

impl Deployment {
    pub(crate) fn new(troops: usize) -> Self {
        Deployment { troops, frame: 0 }
    }

    // 再生中に届いた検出を同じ演出へ合流させる（要件: 短時間に複数回検出しても
    // 演出はまとめて1回）。増えたぶんは自分の番で発進するので、隊列の見え方は
    // 最初からその数で始めた場合と変わらない
    pub(crate) fn reinforce(&mut self, troops: usize) {
        self.troops += troops;
    }

    pub(crate) fn advance(&mut self) {
        self.frame += 1;
    }

    // 画面に出ている兵の列を左から順に。ヘッダーの組み立てはこの並びだけを見る。
    // `launch` は発進位置、`width` は兵が使える幅（右マージンを除いた内容幅）
    pub(crate) fn columns(&self, launch: usize, width: usize) -> Vec<usize> {
        let mut columns: Vec<usize> = (0..self.troops)
            .filter_map(|index| self.column(index, launch, width))
            .collect();
        // 添字の順（先着ほど奥）と画面の左右は逆向きなので並べ直す
        columns.sort_unstable();
        columns
    }

    // 全員が着地して保持ぶんも過ぎたか。true になったら演出は終わり、
    // ヘッダーは通常表示へ戻る
    pub(crate) fn is_over(&self, launch: usize, width: usize) -> bool {
        self.frame > self.last_frame(launch, width)
    }

    // 添字 `index` の兵がいまいる列。まだ発進していない兵と、着地列を確保できない
    // 兵（幅が足りない）は None
    fn column(&self, index: usize, launch: usize, width: usize) -> Option<usize> {
        let landing = landing_column(index, launch, width)?;
        let elapsed = self.frame.checked_sub(index * STAGGER)?;
        Some((launch + SPEED * elapsed).min(landing))
    }

    // 最後の兵が着地するフレーム＋保持ぶん。着地列を確保できない兵は数に
    // 入れない（出ないものを待つと、何も起きていない時間が伸びるだけ）
    fn last_frame(&self, launch: usize, width: usize) -> usize {
        (0..self.troops)
            .filter_map(|index| Some(index * STAGGER + flight_frames(index, launch, width)?))
            .max()
            .unwrap_or(0)
            + HOLD
    }
}

// 添字 `index` の兵の着地列。右マージンの手前から `LANDING_GAP` 間隔で確保し、
// 先に発進した兵ほど奥（右）に並ぶ。発進位置より左になる兵は着地できない
// ＝その演出には出せないので None を返す
fn landing_column(index: usize, launch: usize, width: usize) -> Option<usize> {
    let column = width.checked_sub(1)?.checked_sub(LANDING_GAP * index)?;
    (column >= launch).then_some(column)
}

// 添字 `index` の兵が発進から着地までに要するフレーム数
fn flight_frames(index: usize, launch: usize, width: usize) -> Option<usize> {
    let landing = landing_column(index, launch, width)?;
    Some((landing - launch).div_ceil(SPEED))
}

impl State {
    // 新規エージェント検出（要件: header-animation）。
    //
    // 新しく現れたターミナルペインを「新規にデプロイされたエージェント」とみなす。
    // フックからの状態通知（`fujin_status`）は待たない — 通知は全インスタンスへ
    // 配送されるので可視インスタンス限定という前提が崩れるうえ、フック未設定の
    // ペインでは永久に届かない。
    //
    // 呼ぶのはペイン一覧を取り込んだ直後（`rebuild_selectable()` の後）だけ。
    // `self.visible` は見ない — プラグインをリロードすると `Event::Visible` は
    // 再送されず（main.rs の refresh_focus 参照）、旗を信じると演出が二度と
    // 出なくなる。`PaneUpdate` が届いたこと自体を可視の証拠として使う
    pub(crate) fn detect_new_agents(&mut self) {
        let current: BTreeSet<u32> = self.selectable.iter().map(|e| e.pane_id).collect();
        let detected = match &self.known_panes {
            Some(known) => current.difference(known).count(),
            // 基準をまだ持っていない。見ていない間の増減を遡って演出しないため、
            // ここでは基準を作るだけで発火させない
            None => 0,
        };
        self.known_panes = Some(current);
        if detected > 0 {
            self.begin_deployment(detected);
        }
    }

    // 見ていない間の検出を遡らせないために基準を捨てる。次の観測は基準を
    // 作り直すだけになる（要件: 見逃した検出は後から遡って演出されない）
    pub(crate) fn forget_known_panes(&mut self) {
        self.known_panes = None;
    }

    fn begin_deployment(&mut self, troops: usize) {
        match &mut self.deployment {
            // 再生中の検出は同じ演出へ合流させる。検出のたびに発火させると
            // 演出が重なって騒がしくなる（要件: まとめて1回）
            Some(deployment) => deployment.reinforce(troops),
            None => {
                self.deployment = Some(Deployment::new(troops));
                set_timeout(FRAME_INTERVAL);
            }
        }
    }

    // タイマー1回ぶん進める。再描画が要るかを返す
    pub(crate) fn advance_deployment(&mut self) -> bool {
        let (launch, width) = self.troop_field(self.viewport_cols);
        let Some(deployment) = &mut self.deployment else {
            // 演出が終わった後に取り残されたタイマー。鎖は繋ぎ直さない
            return false;
        };
        deployment.advance();
        if deployment.is_over(launch, width) {
            self.deployment = None;
        } else {
            set_timeout(FRAME_INTERVAL);
        }
        true
    }
}
