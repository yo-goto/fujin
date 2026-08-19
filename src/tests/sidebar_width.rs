use crate::*;

// --- サイドバー幅のタブ間追従（docs/issues/sidebar-width-persist-across-tabs.md） ---
//
// 寄せる側（`apply_width_target`）の実際の一手はホスト関数（resize_pane_with_id）を
// 呼ぶが、`own_plugin_id` が None なら撃つ手前で抜けるので、状態機械の判定
// （権威の判定・打ち切り・着地待ち）はここで確かめられる。実際に寄るかは実機で確認した

#[test]
fn first_render_does_not_claim_authority_over_the_width() {
    let mut state = State::default();
    // 起動直後の1回目。前回の幅が無いので、これは利用者のリサイズではない
    state.reconcile_width(32);
    assert_eq!(state.width_target, None);
}

#[test]
fn a_changed_width_becomes_the_target() {
    let mut state = State {
        viewport_cols: 32,
        ..Default::default()
    };
    state.reconcile_width(40);
    assert_eq!(state.width_target, Some(40));
}

#[test]
fn reaching_the_target_width_does_not_claim_authority() {
    // 自分が撃ったリサイズの結果が遅れて届いた場合。目標と同じ幅になっただけ
    // なので、権威を名乗って配り直してはいけない
    let mut state = State {
        viewport_cols: 32,
        width_target: Some(40),
        ..Default::default()
    };
    state.reconcile_width(40);
    assert_eq!(state.width_target, Some(40));
    assert!(state.width_adjusting.is_none());
}

#[test]
fn a_landing_short_of_the_target_does_not_claim_authority() {
    // 自分が撃ったリサイズが目標の途中の幅で着地した場合。これを利用者の
    // 操作と取り違えると、中間の幅を権威として全タブへ配ってしまう
    let mut state = State {
        viewport_cols: 32,
        width_target: Some(56),
        width_adjusting: Some(Resize::Increase),
        ..Default::default()
    };
    state.reconcile_width(40);
    assert_eq!(state.width_target, Some(56));
    assert!(!state.width_settled);
    assert!(state.width_adjusting.is_none());
}

#[test]
fn a_render_while_the_resize_is_in_flight_freezes_the_width_machine() {
    // 着地前に別の理由の描画が挟まった場合（新規タブのロード直後に実測で頻発）。
    // ここで撃ち足すと多重に飛んで着地を取り違えるので、幅が動くまで何もしない
    let mut state = State {
        viewport_cols: 32,
        width_target: Some(56),
        width_adjusting: Some(Resize::Increase),
        ..Default::default()
    };
    for _ in 0..10 {
        state.reconcile_width(32);
        assert_eq!(state.width_adjusting, Some(Resize::Increase));
        assert_eq!(state.width_target, Some(56));
        assert_eq!(state.width_attempts, 0);
    }
}

#[test]
fn a_width_change_against_the_fired_direction_is_the_user() {
    // 広げる向きに撃った着地は幅が広がるはず。逆に縮んだのなら、それは
    // 着地待ちの間に利用者がリサイズしたということなので、権威を移す
    let mut state = State {
        viewport_cols: 32,
        width_target: Some(56),
        width_adjusting: Some(Resize::Increase),
        ..Default::default()
    };
    state.reconcile_width(24);
    assert_eq!(state.width_target, Some(24));
    assert!(state.width_adjusting.is_none());
}

#[test]
fn overshooting_the_target_settles_the_width() {
    // 目標36へ32から寄せたら刻みの都合で40に着地した場合。両端が同距離なので
    // 着地点で止める。これ以上撃っても目標をまたいで行き来するだけ
    let mut state = State {
        viewport_cols: 32,
        width_target: Some(36),
        width_adjusting: Some(Resize::Increase),
        ..Default::default()
    };
    state.reconcile_width(40);
    assert!(state.width_settled);
    assert!(state.width_adjusting.is_none());
    // 権威も名乗らない（目標は36のまま）
    assert_eq!(state.width_target, Some(36));
}

#[test]
fn a_far_overshoot_steps_back_to_the_nearer_side() {
    // 目標34へ32から寄せたら40に着地した場合（ドラッグリサイズの1セル単位の
    // 目標は5%刻みに乗らない）。跨ぐ前の32のほうが近いので、止めずに戻す
    let mut state = State {
        viewport_cols: 32,
        width_target: Some(34),
        width_adjusting: Some(Resize::Increase),
        ..Default::default()
    };
    state.reconcile_width(40);
    assert!(!state.width_settled);
    assert!(state.width_adjusting.is_none());

    // 戻した着地（40→32）でもう一度跨ぐが、今度は着地側が近いので止まる
    state.viewport_cols = 40;
    state.width_adjusting = Some(Resize::Decrease);
    state.reconcile_width(32);
    assert!(state.width_settled);
    assert_eq!(state.width_target, Some(34));
}

#[test]
fn running_out_of_attempts_settles_the_width() {
    let mut state = State {
        viewport_cols: 40,
        width_target: Some(56),
        width_attempts: crate::sync::WIDTH_MAX_ATTEMPTS,
        ..Default::default()
    };
    state.reconcile_width(40);
    assert!(state.width_settled);
}

#[test]
fn a_width_within_tolerance_counts_as_reached() {
    // 端末ウィンドウのリサイズでは割合からの丸め直しでタブ間に1桁の差が出うる。
    // 1桁を追って1段階（端末幅の5%）動くとかえって離れるので、到達として扱う
    let mut state = State {
        viewport_cols: 40,
        width_target: Some(41),
        width_settled: true,
        width_attempts: 2,
        ..Default::default()
    };
    state.reconcile_width(40);
    assert!(!state.width_settled);
    assert_eq!(state.width_attempts, 0);
    assert!(state.width_adjusting.is_none());
}

#[test]
fn preview_and_summoned_instances_stay_out_of_the_width_sync() {
    // どちらも常駐サイドバーではなく自前の幅で開かれる（決定202608011644・決定202608082045）
    for state in [
        State {
            viewport_cols: 32,
            is_preview: true,
            ..Default::default()
        },
        State {
            viewport_cols: 32,
            summoned: true,
            ..Default::default()
        },
    ] {
        let mut state = state;
        state.reconcile_width(40);
        assert_eq!(state.width_target, None);
    }
}

#[test]
fn width_pipe_takes_a_column_count() {
    let mut state = State {
        width_settled: true,
        width_attempts: 3,
        ..Default::default()
    };
    assert!(state.handle_width_pipe(Some("40"), &PipeSource::Keybind));
    assert_eq!(state.width_target, Some(40));
    // 新しい目標が来たら、前の目標で使い切った回数と打ち切りは無かったことにする
    assert!(!state.width_settled);
    assert_eq!(state.width_attempts, 0);

    // 同じ値の配布では描き直さない
    assert!(!state.handle_width_pipe(Some("40"), &PipeSource::Keybind));
}

#[test]
fn width_pipe_ignores_payloads_that_are_not_column_counts() {
    let mut state = State::default();
    for payload in [None, Some(""), Some("0"), Some("wide")] {
        assert!(!state.handle_width_pipe(payload, &PipeSource::Keybind));
        assert_eq!(state.width_target, None);
    }
}
