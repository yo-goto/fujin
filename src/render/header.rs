// ヘッダー行（決定202608070119。要件: features/sidebar-tree/sidebar-header.feature）。
//
// `▲ fujin` とモードラベル、配置演出（要件: header-animation）の兵の並び。
// 状態色（`state_ink`）はフッターとも共有する。

use super::*;

impl State {
    // ヘッダー（1行）。`▲ fujin` をモードによらず常に出し、モード中だけ直後に
    // モードラベルを足す（決定202608070119。要件: sidebar-header）。
    //
    // 色を乗せるのは三角とモードラベルで、ブランド名は dim のまま。名前まで色を
    // 付けるとツリーの状態アイコンの色分けと喧嘩する
    pub(crate) fn header_line(&self, cols: usize) -> Text {
        // 兵の間を埋める空白は借用されるので、断片を組む前に作っておく
        let pads = self.troop_pads(cols);
        let mut segments = self.header_segments();
        // 配置演出中だけ、本文の右に兵が並ぶ（要件: header-animation）
        for pad in &pads {
            segments.push((pad.as_str(), Ink::Plain));
            segments.push((TROOP, Ink::Accent(TROOP_LEVEL)));
        }
        compose(&segments, content_cols(cols))
    }

    // ヘッダーの本文（`▲ fujin` ＋モードラベル）。配置演出は**この右側**に
    // 兵を並べるので、本文の幅を測れるよう断片のまま返す
    fn header_segments(&self) -> Vec<(&str, Ink)> {
        // 三角が x=0、ブランド名が x=2（HEADER_INDENT）に来る
        let ink = self.state_ink();
        let mut segments = vec![("▲", ink), (" fujin", Ink::Muted)];
        if let Some(label) = self.mode_label() {
            segments.push(("  ", Ink::Plain));
            segments.push((label, ink));
        }
        segments
    }

    // ヘッダー本文の表示幅
    fn header_width(&self) -> usize {
        self.header_segments()
            .iter()
            .map(|(fragment, _)| UnicodeWidthStr::width(*fragment))
            .sum()
    }

    // 配置演出で兵が使える領域 `(発進位置, 幅)`（要件: header-animation）。
    //
    // 発進位置はヘッダー本文の右端の1つ先で、モードラベルが出ているぶんだけ
    // 右へずれる — 兵がラベルに重なるとどちらも読めなくなる。幅は他の行と同じく
    // 右マージンを除いた内容幅で、着地列はその手前から確保する
    pub(crate) fn troop_field(&self, cols: usize) -> (usize, usize) {
        (self.header_width() + LAUNCH_GAP, content_cols(cols))
    }

    // 兵と兵のあいだを埋める空白。compose() は断片を順に置くだけなので、
    // 兵の絶対位置はこの空白の幅で作る
    fn troop_pads(&self, cols: usize) -> Vec<String> {
        let Some(deployment) = &self.deployment else {
            return Vec::new();
        };
        let (launch, width) = self.troop_field(cols);
        let mut pads = Vec::new();
        let mut x = self.header_width();
        for column in deployment.columns(launch, width) {
            pads.push(" ".repeat(column.saturating_sub(x)));
            x = column + UnicodeWidthStr::width(TROOP);
        }
        pads
    }

    // いまの状態を語る色。ヘッダーの三角・モードラベルとフッター全体に同じ色を
    // 使い、**テキストを読まなくても色だけでモードが判別できる**ようにする
    //（決定202608070119。要件: sidebar-header / sidebar-footer）
    pub(super) fn state_ink(&self) -> Ink {
        // 終了操作サブモードが最優先。確認プロンプトのあいだは、ヘッダーの三角も
        // 含めて警告色にする（決定202608080140のフッター転用を決定202608070119に沿わせたもの）
        if self.termination.is_some() {
            Ink::Accent(TERMINATION_LEVEL)
        } else if self.showing_config_warning() {
            // 警告も同じ error_color を借りる（決定202608080346。新しい色は増やさない）。
            // 三角ごと警告色にする — 出ているあいだは「いまの状態」が警告
            Ink::Accent(TERMINATION_LEVEL)
        } else if self.triage.is_some() {
            Ink::Accent(TRIAGE_LEVEL)
        } else if self.nav_mode || self.search.is_some() {
            Ink::Accent(NAV_LEVEL)
        } else {
            Ink::Muted
        }
    }

    // ヘッダーのモードラベル。ツリー表示（非フォーカス）では出さない。
    // 検索サブモードは navモードの内側なので `[nav]` のまま変えない
    fn mode_label(&self) -> Option<&'static str> {
        if self.triage.is_some() {
            Some("[tri]")
        } else if self.nav_mode || self.search.is_some() {
            Some("[nav]")
        } else {
            None
        }
    }
}
