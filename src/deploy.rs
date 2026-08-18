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
// **再生できるのは可視インスタンスだけ**。トリガーであるフック通知（pipe）は
// 全インスタンスへ配送されるので、`State::is_visible_instance()`（決定202608012142の権威判定）で
// 絞る。見ていない間の検出は遡って再生しない — 「見ている間だけのご褒美演出」として
// 要件側で許容した制約なので、実装で解消しようとしないこと。

use crate::State;

// 兵の記号。ヘッダーの本陣 `▲` と対をなす、白抜きの小さい三角
pub(crate) const TROOP: &str = "▵";

// フレーム間隔（秒）。実機での見え方はまだ詰めていない暫定値。
// タイマーの鎖の刻みでもある（`State::arm_timer`）ので、滞在猶予の判定粒度も
// これで決まる
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

// 新規エージェント検出（要件: header-animation）。フック通知の `SessionStart` が
// **エージェントの着任**を意味するかを、`source` から判定する。
//
// 判定材料をフック通知に一本化してあるのが要点（2026-08-09 に差し替え。
// docs/issues/deploy-animation-trigger-scope.md）。増えたターミナルペインで判定して
// いた旧実装は、そのペインで何が動くかを一切見ていなかったため、`vim` やビルド
// コマンドでも演出が出るうえ、前から開いてあるペインで後からエージェントを起動しても
// 出なかった。`SessionStart` はエージェント側のイベントでしか飛ばないので、
// **ペインの新旧を問わず着任だけを拾える**。
//
// `clear`（`/clear`）と `compact`（コンパクト）は、すでに走っているエージェントの
// 途中で飛ぶので着任ではない。**除外方式**にしてあるのは意図的で、ホワイトリストに
// すると `source` を送らない旧フックスクリプトのままの環境で演出が出なくなる。
pub(crate) fn detect_new_agent(source: Option<&str>) -> bool {
    !matches!(source, Some("clear") | Some("compact"))
}

impl State {
    // 配置演出を始める。呼ぶのは**可視インスタンス判定を通した後**だけ
    //（`main.rs` の状態通知ハンドラ）— ここで判定しないのは、可視性の問い合わせが
    // ホスト関数でテストから呼べず、発火のロジックまで巻き添えにテスト不能に
    // なるため（docs/dev/build-and-test.md）
    pub(crate) fn begin_deployment(&mut self, troops: usize) {
        // 無効化されていても新規エージェント検出そのものは動かす。ここで再生だけを
        // 落とす（要件: show_deploy_animation）
        if !self.show_deploy_animation.0 {
            return;
        }
        match &mut self.deployment {
            // 再生中の検出は同じ演出へ合流させる。検出のたびに発火させると
            // 演出が重なって騒がしくなる（要件: まとめて1回）
            Some(deployment) => deployment.reinforce(troops),
            None => {
                self.deployment = Some(Deployment::new(troops));
                self.arm_timer();
            }
        }
    }

    // タイマー1回ぶん進める。再描画が要るかを返す。
    //
    // 鎖を繋ぎ直すのは呼び出し元（`State::on_timer`）の役目 — 滞在猶予も同じ
    // タイマーに相乗りするので、次を張るかはここだけでは決められない
    pub(crate) fn advance_deployment(&mut self) -> bool {
        let (launch, width) = self.troop_field(self.viewport_cols);
        let Some(deployment) = &mut self.deployment else {
            // 演出はもう終わっている。滞在猶予のほうのタイマーが来ただけ
            return false;
        };
        deployment.advance();
        if deployment.is_over(launch, width) {
            self.deployment = None;
        }
        true
    }
}
