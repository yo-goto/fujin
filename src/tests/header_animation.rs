use crate::deploy::TROOP;
use crate::test_support::*;
use crate::*;

// --- 配置演出（要件: docs/requirements/header-animation/） ---

// 兵が使える領域の実測値（幅32セル・右マージン2セル・`▲ fujin` は7セル）。
// 発進位置は本文の右端の1つ先、いちばん奥の着地列は内容幅の右端
const LAUNCH: usize = 8;
const DEEPEST: usize = CONTENT - 1;

// 新規エージェント検出から配置演出の発火までを通す。
//
// 本番では検出（`apply_status` の戻り値）と発火（`begin_deployment`）の間に
// 可視インスタンス判定（`State::is_visible_instance`）が挟まるが、これはホスト関数
// `get_focused_pane_info()` を呼ぶのでテストから通せない
//（docs/dev/build-and-test.md「テストで検証できない範囲」）。ここでは判定を通った
// 後の発火だけを見る
fn deploy_agents(state: &mut State, troops: usize) {
    state.begin_deployment(troops);
}

// いま画面に出ている兵の列
fn troop_columns(state: &State) -> Vec<usize> {
    let (launch, width) = state.troop_field(SIDEBAR);
    state
        .deployment
        .map(|deployment| deployment.columns(launch, width))
        .unwrap_or_default()
}

// 演出が終わるまでフレームを送る。返すのは要したフレーム数
fn play_out(state: &mut State) -> usize {
    for frame in 1.. {
        state.advance_deployment();
        if state.deployment.is_none() {
            return frame;
        }
        assert!(frame < 100, "演出が終わらない");
    }
    unreachable!()
}

#[test]
fn new_panes_alone_do_not_start_a_deployment() {
    // 判定材料はフック通知だけで、ペインが増えたかどうかは見ない。旧実装（増えた
    // ターミナルペインで判定）では `vim` やビルドコマンドでも演出が出ていた
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    observe_panes(&mut state, &[1, 2, 3]);

    assert!(state.deployment.is_none());
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");
}

#[test]
fn a_session_start_is_a_new_agent_detection() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);

    assert!(
        state.apply_status(status(2, "SessionStart")),
        "SessionStart は新規エージェント検出になる"
    );
}

#[test]
fn an_agent_started_in_an_existing_pane_is_detected() {
    // 空のシェルペインを先に開いておき、後から `claude` を打つ使い方（要件:
    // 前から開いてあるペインで後からエージェントを起動しても配置演出が始まる）。
    // ペインの側は何も変わらないまま通知だけが届く
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    // 一覧を何度観測しても増減が無い状態を作ってから通知を受ける
    observe_panes(&mut state, &[1, 2]);

    assert!(state.apply_status(status(2, "SessionStart")));
}

#[test]
fn a_restarted_conversation_is_not_a_new_agent() {
    // `/clear` とコンパクトは稼働中のエージェントの仕切り直しで、着任ではない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for source in ["clear", "compact"] {
        assert!(
            !state.apply_status(session_start(1, source)),
            "source={} は新規エージェント検出にしない",
            source
        );
    }
}

#[test]
fn a_fresh_session_is_a_new_agent() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for source in ["startup", "resume", "fork"] {
        assert!(
            state.apply_status(session_start(1, source)),
            "source={} は新規エージェント検出になる",
            source
        );
    }
}

#[test]
fn a_session_start_without_a_source_still_counts() {
    // `source` を送らない旧フックスクリプトのままでも演出は出る。判定を
    // ホワイトリストではなく除外方式にしてあるのはこのため
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    assert!(state.apply_status(status(1, "SessionStart")));
}

#[test]
fn other_hook_events_are_never_new_agent_detections() {
    // **リロード直後の誤検出を防いでいるのがこの性質。** プラグインをリロードすると
    // `agents` マップは空になる（docs/issues/redeploy-resets-agent-state.md）が、
    // 稼働中のエージェントから次に届くのは SessionStart 以外のイベントなので、
    // 既存エージェントが新規と誤検出されることはない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);

    for event in [
        "UserPromptSubmit",
        "Stop",
        "StopFailure",
        "Notification",
        "SubagentStart",
        "SubagentStop",
        "TaskCreated",
        "TaskCompleted",
        "SessionEnd",
    ] {
        assert!(
            !state.apply_status(status(1, event)),
            "{} は新規エージェント検出にしない",
            event
        );
    }
}

#[test]
fn a_new_agent_starts_the_deployment_animation() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    deploy_agents(&mut state, 1);

    assert_eq!(state.deployment.map(|d| d.troops), Some(1));
    // 兵はブランド行の `fujin` の右側から現れる
    assert_eq!(troop_columns(&state), vec![LAUNCH]);
    let header = state.header_line(SIDEBAR).content().to_string();
    assert!(header.starts_with("▲ fujin "), "{}", header);
    assert_eq!(
        header.chars().position(|c| c.to_string() == TROOP),
        Some(LAUNCH)
    );
}

#[test]
fn every_detected_agent_gets_a_troop() {
    for detected in [1usize, 3, 6] {
        let mut state = sidebar_state();
        observe_panes(&mut state, &[1]);
        deploy_agents(&mut state, detected);

        assert_eq!(
            state.deployment.map(|d| d.troops),
            Some(detected),
            "{}体の検出",
            detected
        );
    }
}

#[test]
fn show_deploy_animation_can_switch_the_animation_off() {
    // 演出は情報を運ばないので、切っても見える情報は変わらない（決定202608080346）
    let mut state = sidebar_state();
    state.apply_config(&plugin_config(&[("show_deploy_animation", "false")]));
    observe_panes(&mut state, &[1, 2]);

    // 新規エージェントの検出そのものは、切っている間も動く
    //（要件: But 新規エージェントの検出そのものは行われる）
    assert!(state.apply_status(status(2, "SessionStart")));
    deploy_agents(&mut state, 1);
    assert!(state.deployment.is_none(), "配置演出は再生されない");
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");

    // 戻せば次の検出から再生される
    state.apply_config(&plugin_config(&[("show_deploy_animation", "true")]));
    deploy_agents(&mut state, 1);
    assert_eq!(state.deployment.map(|d| d.troops), Some(1));
}

#[test]
fn detections_in_the_same_window_join_one_deployment() {
    // 新規タブ作成のように一括で着任するときは、検出が複数回に割れて届く。
    // 検出のたびに発火させると演出が重なって騒がしくなる
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 1);
    state.advance_deployment();
    deploy_agents(&mut state, 2);

    let deployment = state.deployment.expect("演出は続いている");
    assert_eq!(deployment.troops, 3, "検出した数の合計ぶんの兵が出る");
    assert_eq!(deployment.frame, 1, "演出は最初から巻き直さない");
}

#[test]
fn the_troops_line_up_with_the_first_launched_deepest() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 3);

    // 先に発進した兵ほど奥へ着く。着地列は右端から2セル間隔
    let mut landed = Vec::new();
    for _ in 0..20 {
        state.advance_deployment();
        let columns = troop_columns(&state);
        if columns.len() == 3 && columns.iter().all(|c| *c >= DEEPEST - 4) {
            landed = columns;
            break;
        }
    }
    assert_eq!(landed, vec![DEEPEST - 4, DEEPEST - 2, DEEPEST]);

    // 静止したあとは動かない
    state.advance_deployment();
    assert_eq!(troop_columns(&state), landed);
}

#[test]
fn the_troops_stay_clear_of_the_brand() {
    // 幅が足りないぶんの兵は着地列を確保できない。ブランド行に重ねるくらいなら
    // 出さない（着地列は発進位置より左には作らない）
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 39);

    // 出せるだけ出た瞬間（＝いちばん多く並んだフレーム）を見る
    let mut columns = Vec::new();
    while state.deployment.is_some() {
        state.advance_deployment();
        let frame = troop_columns(&state);
        if frame.len() > columns.len() {
            columns = frame;
        }
    }
    assert!(columns.iter().all(|c| *c >= LAUNCH), "{:?}", columns);
    assert_eq!(columns.first(), Some(&LAUNCH));
    assert_eq!(columns.last(), Some(&DEEPEST));
}

#[test]
fn the_header_returns_to_normal_when_the_deployment_ends() {
    // 着地点は完全に元へ戻る。稼働数のような情報は残さない
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 2);
    assert!(state.header_line(SIDEBAR).content().contains(TROOP));

    let frames = play_out(&mut state);
    assert!(
        frames > 3,
        "整列した状態を見せる間もなく畳んでいる: {}",
        frames
    );
    assert_eq!(state.header_line(SIDEBAR).content(), "▲ fujin");
    // 取り残されたタイマーが来ても何も起きない
    assert!(!state.advance_deployment());
}

// 「サイドバーが表示されていない間の検出では演出は再生されない」「見逃した検出は
// 後から遡って演出されない」の2要件は、可視インスタンス判定（`is_visible_instance`）
// が担っている。ホスト関数 `get_focused_pane_info()` を呼ぶためユニットテストからは
// 通せない（docs/dev/build-and-test.md「テストで検証できない範囲」）ので、実機での
// 手動確認に頼る。

#[test]
fn the_deployment_leaves_the_mode_label_readable() {
    // navモード中は `[nav]` のぶんだけ発進位置が右へずれる。兵がラベルに
    // 重なるとどちらも読めなくなる
    let mut state = sidebar_state();
    state.nav_mode = true;
    observe_panes(&mut state, &[1]);
    deploy_agents(&mut state, 1);

    let header = state.header_line(SIDEBAR).content().to_string();
    assert!(header.starts_with("▲ fujin  [nav] "), "{}", header);
    assert_eq!(
        troop_columns(&state),
        vec!["▲ fujin  [nav]".chars().count() + 1]
    );
}

#[test]
fn render_survives_the_deployment() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2, 3]);
    deploy_agents(&mut state, 2);
    state.render(40, SIDEBAR);
    state.render(40, 12);
    state.render(3, 2);
    state.render(0, 0);
    // 幅0で描いたあともフレーム送りは止まらない
    state.advance_deployment();
}
