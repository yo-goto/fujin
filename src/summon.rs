// フローティングでの臨時召喚（決定16）。
//
// フォーカス中のタブに fujin が1つも居ないと、`refresh_focus()` が真になる
// インスタンスがどこにも存在せず、入場pipeが届いても誰も横取りを始めない
//（無反応に見える）。レイアウト側だけではこの穴は塞ぎきれない。既存セッションの
// 復活、セッションマネージャ経由でのタブ作成、別レイアウトの指定、ユーザーが
// サイドバーを閉じた場合は、いずれも fujin の居ないタブを作る。
// そこで代表1つがフローティングで自分を召喚する。
//
// 常駐をフローティングに変えるわけではない（決定5は維持）。タイルは幅を返す
// 代わりに下のペインを欠けさせないので長時間の常駐に向くが、ここでは一時的に
// 出すだけなので重なっても構わない。

use std::collections::BTreeMap;

use zellij_tile::prelude::*;

use crate::config::SUMMONED_KEY;
use crate::{State, SYNC_STATE_PIPE};

// 召喚するフローティングの幅。レイアウトの常駐サイドバー（size=32）に合わせる。
// プレビュー用フローティングペインの左端もここから決める（決定42）
pub(crate) const SIDEBAR_WIDTH: usize = 32;

impl State {
    // fujin_dismiss の受け口。召喚された本人は自分で退場し、常駐サイドバーは
    // 取り残された召喚インスタンスを代わりに閉じる（決定16）
    pub(crate) fn handle_dismiss_pipe(&mut self) -> bool {
        if self.summoned {
            self.exit_nav_mode();
        } else {
            self.dismiss_stranded_summons();
        }
        false
    }

    pub(crate) fn summon_floating_if_absent(&mut self) {
        let (Some(own_id), Some(own_url)) = (self.own_plugin_id, self.own_plugin_url.clone())
        else {
            eprintln!("fujin: summon skipped (own id/url unknown)");
            return;
        };
        let Ok((focused_tab, _)) = get_focused_pane_info() else {
            eprintln!("fujin: summon skipped (focused pane query failed)");
            return;
        };
        // 一覧はフォーカス中のタブに fujin が既に居るかを見る唯一の材料で、
        // 古いまま進むと常駐が居るタブへ重ねて召喚してしまう（実際に二度壊した:
        // まだ訪れていないタブの常駐が召喚役として暴走した／新規タブに常駐が
        // 居るのに重ねて召喚した）。かといって諦めると今度は召喚できない。
        //
        // **`self.panes` は使わない。** ここへ来る時点で自分はフォーカス中の
        // タブに居ない＝非可視であり、非可視インスタンスには PaneUpdate が
        // 届かない。`self.panes` は良くて「最後に可視だった時点」で凍っており
        //（一度も可視でなければ永久に `None`）、その後に作られたタブの常駐は
        // 載らない。凍った一覧で判定すると新規タブへ重ねて召喚してしまうため、
        // 毎回サーバに問い合わせ直す。重い問い合わせだが、増殖して消せない事故
        //（決定16で一度実際に壊した経路）の方がコストが高い
        // 問い合わせが落ちたときだけ凍った一覧に頼る（それも無ければ諦める）。
        // 完全に諦めると「入場キーが無反応」に逆戻りするので、経路自体は残す
        let Some(manifest) = query_pane_manifest().or_else(|| self.panes.clone()) else {
            eprintln!("fujin: summon skipped (no pane manifest)");
            return;
        };
        // 自分が前に召喚したものがまだ生きていれば、**同じキーで引っ込める**
        //（トグル・決定16）。召喚された本人はこの pipe を受け取れないので、
        // 出した側が始末をつけるしかない。
        //
        // 生死の判定に一覧は使えない。召喚役は多くの場合非可視で PaneUpdate が
        // 届かず、自分が召喚したペインすら一覧に載らない（実測では入場のたびに
        // 積み上がって7枚溜まった）。`get_pane_info()` はサーバへの問い合わせ
        // なので、記録したIDの生存確認に使える
        if let Some(previous) = self.summoned_panes.get(&focused_tab).copied() {
            if get_pane_info(PaneId::Plugin(previous)).is_some() {
                eprintln!("fujin: dismissing summon {previous} (toggled off)");
                close_plugin_pane(previous);
                self.summoned_panes.remove(&focused_tab);
                return;
            }
        }
        // そのタブに兄弟が居るなら、そいつが権威を持つので任せる。
        // 居るのに権威が立たなかった＝ manifest のずれ。実装を疑う手がかりになる
        if Self::tab_has_fujin(&manifest, focused_tab, &own_url) {
            eprintln!(
                "fujin: summon skipped (tab {} already has fujin)",
                focused_tab
            );
            return;
        }
        // 代表でなければ黙って降りる（インスタンス数ぶん出るとノイズになる）
        if !Self::is_summon_delegate(&manifest, own_id, &own_url) {
            return;
        }
        eprintln!("fujin: summoning floating instance into tab {focused_tab}");

        let mut config = BTreeMap::new();
        config.insert(SUMMONED_KEY.to_string(), "true".to_string());
        if self.show_cwd {
            config.insert("show_cwd".to_string(), "true".to_string());
        }
        // 既定が真の設定なので、渡すのは切ってあるときだけ
        if !self.show_deploy_animation.0 {
            config.insert("show_deploy_animation".to_string(), "false".to_string());
        }
        let summoned = open_plugin_pane_floating(
            &own_url,
            config,
            Some(Self::summon_coordinates()),
            BTreeMap::new(),
        );
        // 表示への切り替えはここではやらない。召喚した側は非フォーカスの
        // タブに居るため `show_floating_panes()` が「アクティブなタブ」を
        // 特定できず、`None` でも tab index でも "Tab not found" になる（実測）。
        // ピン留めしてあるので表示状態に関係なく最前面に出る。

        let Some(PaneId::Plugin(new_id)) = summoned else {
            eprintln!("fujin: summon failed (no pane id returned)");
            return;
        };
        self.summoned_panes.insert(focused_tab, new_id);
        // **開くときに渡した座標は効かない。** 実測では pinned だけが通り、
        // x/y/width/height は既定のカスケード配置のままだった。
        // 開いた後に指定し直すと効く
        change_floating_panes_coordinates(vec![(
            PaneId::Plugin(new_id),
            Self::summon_coordinates(),
        )]);

        // 決定13の同期は「可視インスタンスが PaneUpdate で新入りに気づいて配る」
        // 方式だが、このタブには可視インスタンスが居ないため誰も気づけない。
        // 召喚した本人が明示的に配る
        if !self.agents.is_empty() {
            crate::sync::send_to_plugin(new_id, SYNC_STATE_PIPE, self.state_dump());
        }
    }

    // 自分がフローティングで起動されていたら、召喚インスタンスとして自覚する
    //（要件: docs/requirements/summon/cold-start.feature）。
    //
    // セッションに fujin が1つも居ないと、入場pipe（`MessagePlugin`）の宛先が
    // 存在せず **zellij 自身がプラグインを起動する**。キーバインドに
    // `floating true` を書いておけばフローティングで開くので、それを掴まえて
    // 常駐と同じ左端のサイドバーに整え、決定16の召喚と同じ後始末をさせる。
    // `floating true` が無いと右側にタイルで開くが、タイルの配置と幅を
    // プラグイン側から作り直す手段は無いので、そちらは要件の対象外。
    //
    // **フローティング = 臨時、タイル = 常駐**（決定5）で区別する。常駐サイドバーを
    // 誤って閉じてしまわないよう、判定はこの1点だけに絞る
    pub(crate) fn adopt_floating_as_summoned(&mut self) {
        // 決定16の召喚は configuration で最初から自覚している
        if self.summoned {
            return;
        }
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        // 一覧は当てにできない（この時点では PaneUpdate が来ていない）。
        // `get_pane_info()` はサーバへの問い合わせなので可視性にも依らない
        let Some(info) = get_pane_info(PaneId::Plugin(own_id)) else {
            return;
        };
        if !info.is_floating {
            return;
        }
        eprintln!("fujin: adopting floating instance as a summoned instance");
        self.summoned = true;
        self.pending_nav_entry = true;
        // 開いたときの座標は zellij 既定のカスケード配置なので、常駐サイドバーと
        // 同じ位置・幅に置き直す（召喚経路と同じ手当て。summon_coordinates 参照）
        change_floating_panes_coordinates(vec![(
            PaneId::Plugin(own_id),
            Self::summon_coordinates(),
        )]);
    }

    // 召喚するフローティングの配置。常駐サイドバーと同じ見た目・同じ位置。
    //
    // ピン留めは必須。タブのフローティングは既定で非表示状態のため、普通に
    // 開くとペインは在るのに描画されない。`show_floating_panes()` で表示に
    // 切り替える手も試したが、召喚した側でも召喚された側でも "Tab not found"
    // で失敗する（zellij 0.44.3）。ピン留めしたペインは表示状態に関係なく
    // 最前面に出る
    fn summon_coordinates() -> FloatingPaneCoordinates {
        let mut coordinates = FloatingPaneCoordinates::default()
            .with_x_fixed(0)
            .with_y_fixed(0)
            .with_width_fixed(SIDEBAR_WIDTH)
            .with_height_percent(100);
        coordinates.pinned = Some(true);
        coordinates
    }

    // 指定タブに自分と同じ fujin が居るか。常駐（タイル）と召喚（フローティング）を
    // 区別しない。既に出ている召喚に重ねて召喚しないためでもある
    pub(crate) fn tab_has_fujin(
        manifest: &PaneManifest,
        tab_position: usize,
        own_url: &str,
    ) -> bool {
        manifest
            .panes
            .get(&tab_position)
            .map(|panes| {
                panes
                    .iter()
                    .any(|p| p.is_plugin && p.plugin_url.as_deref() == Some(own_url))
            })
            .unwrap_or(false)
    }

    // 一覧に載っている兄弟インスタンスのIDを昇順・重複なしで
    pub(crate) fn sibling_plugin_ids(manifest: &PaneManifest, own_url: &str) -> Vec<u32> {
        let mut ids: Vec<u32> = manifest
            .panes
            .values()
            .flatten()
            .filter(|p| p.is_plugin && p.plugin_url.as_deref() == Some(own_url))
            .map(|p| p.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    // 召喚の実行役を1つに絞る。兄弟IDの昇順で、実在する最初のIDが代表。
    //
    // 単純な最小IDだと、閉じられたペインが manifest に残っているかどうかで
    // インスタンスごとに結論が食い違う。`get_pane_info()` はサーバへの
    // 問い合わせなので、生存確認を挟めば鮮度の違いを吸収できる
    fn is_summon_delegate(manifest: &PaneManifest, own_id: u32, own_url: &str) -> bool {
        for id in Self::sibling_plugin_ids(manifest, own_url) {
            if id == own_id {
                return true;
            }
            // 自分より若いインスタンスが生きていればそちらに譲る
            if get_pane_info(PaneId::Plugin(id)).is_some() {
                return false;
            }
        }
        false
    }

    // 召喚インスタンスの入場。入場pipeは自分の起動前に流れているので
    // 受け取れない。権限と一覧が揃ってから入る（空のまま入ると j/k が効かない）
    pub(crate) fn enter_nav_mode_if_pending(&mut self) {
        if !self.pending_nav_entry {
            return;
        }
        if !self.permissions_granted || self.selectable.is_empty() {
            // 入場できないまま取り残された召喚は横取りをしないので Esc が
            // 届かない。無言で詰まると原因が追えないので、待った理由を残す
            eprintln!(
                "fujin: nav entry deferred (permissions={}, selectable={})",
                self.permissions_granted,
                self.selectable.len()
            );
            return;
        }
        self.pending_nav_entry = false;
        if !self.nav_mode {
            self.enter_nav_mode();
        }
    }

    // 取り残された召喚インスタンスを閉じる（決定16の掃除の逃げ道）。判定材料は
    // 「同じURLのプラグイン」かつ「フローティング」だけに絞る（常駐サイドバーは
    // タイル・決定5）。召喚側が発行したIDを覚えておく手もあるが、覚えている
    // 本人がリロードや再起動で記憶を失うと届かなくなる
    pub(crate) fn dismiss_stranded_summons(&mut self) {
        let (Some(own_url), Some(manifest)) = (self.own_plugin_url.as_deref(), self.panes.as_ref())
        else {
            eprintln!("fujin: dismiss skipped (own url or pane list unknown)");
            return;
        };
        for pane in manifest.panes.values().flatten() {
            if pane.is_plugin
                && pane.is_floating
                && pane.plugin_url.as_deref() == Some(own_url)
                && Some(pane.id) != self.own_plugin_id
            {
                eprintln!("fujin: dismissing stranded summon {}", pane.id);
                close_plugin_pane(pane.id);
            }
        }
        self.summoned_panes.clear();
    }
}

// ペイン一覧をサーバから直接引く。`SessionInfo` には `panes` が丸ごと入って
// おり、**可視性に依らず・イベント配送を待たずに**取れる。全セッションぶんの
// 情報が返る重い問い合わせなので、召喚の判定のように `self.panes` の鮮度を
// 信用できない場面に限って使う
fn query_pane_manifest() -> Option<PaneManifest> {
    get_session_list()
        .ok()?
        .live_sessions
        .into_iter()
        .find(|session| session.is_current_session)
        .map(|session| session.panes)
}
