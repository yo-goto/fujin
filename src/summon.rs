// フローティングでの臨時召喚（決定16）。
//
// フォーカス中のタブに fujin が1つも居ないと、`is_authoritative()` が真になる
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

use crate::{State, SUMMONED_CONFIG_KEY, SYNC_STATE_PIPE};

// 召喚するフローティングの幅。レイアウトの常駐サイドバー（size=32）に合わせる
const SIDEBAR_WIDTH: usize = 32;

impl State {
    pub(crate) fn summon_floating_if_absent(&mut self) {
        let (Some(own_id), Some(own_url)) = (self.own_plugin_id, self.own_plugin_url.clone())
        else {
            eprintln!("fujin: summon skipped (own id/url unknown)");
            return;
        };
        // 一覧はフォーカス中のタブに fujin が既に居るかを見る唯一の材料で、
        // 無いまま進むと常駐が居るタブへ重ねて召喚してしまう（実際に一度壊した:
        // まだ訪れていないタブの常駐が召喚役として暴走した）。かといって諦めると
        // 今度は召喚できない。非可視インスタンスには PaneUpdate が届かず、一度も
        // 可視になっていない常駐は一覧を永久に持たないため、サーバに問い合わせて埋める
        let manifest = match self.panes.clone().or_else(query_pane_manifest) {
            Some(manifest) => manifest,
            None => {
                eprintln!("fujin: summon skipped (no pane manifest)");
                return;
            }
        };
        let Ok((focused_tab, _)) = get_focused_pane_info() else {
            eprintln!("fujin: summon skipped (focused pane query failed)");
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
        // manifest が古くて取りこぼしても、二重に出るだけで操作不能にはならない。
        // 居るのに権威が立たなかった＝ manifest のずれ。実装を疑う手がかりになる
        let already_present = manifest
            .panes
            .get(&focused_tab)
            .map(|panes| {
                panes
                    .iter()
                    .any(|p| p.is_plugin && p.plugin_url.as_deref() == Some(own_url.as_str()))
            })
            .unwrap_or(false);
        if already_present {
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
        config.insert(SUMMONED_CONFIG_KEY.to_string(), "true".to_string());
        if self.show_cwd {
            config.insert("show_cwd".to_string(), "true".to_string());
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
            pipe_message_to_plugin(
                MessageToPlugin::new(SYNC_STATE_PIPE)
                    .with_destination_plugin_id(new_id)
                    .with_payload(self.state_dump()),
            );
        }
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

    // 召喚の実行役を1つに絞る。兄弟IDの昇順で、実在する最初のIDが代表。
    //
    // 単純な最小IDだと、閉じられたペインが manifest に残っているかどうかで
    // インスタンスごとに結論が食い違う。`get_pane_info()` はサーバへの
    // 問い合わせなので、生存確認を挟めば鮮度の違いを吸収できる
    fn is_summon_delegate(manifest: &PaneManifest, own_id: u32, own_url: &str) -> bool {
        let mut ids: Vec<u32> = manifest
            .panes
            .values()
            .flatten()
            .filter(|p| p.is_plugin && p.plugin_url.as_deref() == Some(own_url))
            .map(|p| p.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
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

    // 召喚されたインスタンスの入場。入場pipeは自分の起動前に流れているので
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

    // 取り残された臨時召喚を閉じる（決定16の掃除の逃げ道）。判定材料は
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
// おり、可視性に依らず取れる。全セッションぶんの情報が返る重い問い合わせなので、
// 一覧が無いときの穴埋めに限って使う
fn query_pane_manifest() -> Option<PaneManifest> {
    get_session_list()
        .ok()?
        .live_sessions
        .into_iter()
        .find(|session| session.is_current_session)
        .map(|session| session.panes)
}
