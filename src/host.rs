// ホストコマンドの間接層（要件というより試験基盤。経緯:
// .docs/issues/issue-host-command-recorder.md）。
//
// 本番ビルドは zellij-tile の shim へ素通しし、テストビルドでは呼び出しを
// 引数ごと記録して検証できるようにする。従来はリンクスタブ（test_support.rs）が
// 握り潰すため「呼ばれたことも引数も観測できない」領域だった。
//
// **副作用だけのホストコマンドは shim を直接呼ばず、必ずここを経由すること。**
// 対象外は次の2種（従来どおり shim を直接呼ぶ）:
// - 戻り値を stdin から読み返す問い合わせ系（get_pane_info 等）。テストから
//   呼べない制約ごと据え置き（応答の差し込みは別課題）
// - report_panic（entry.rs の panic フック）。引数が非 'static で記録に馴染まない
//
// MessageToPlugin は PartialEq を持たないため、記録は要点フィールドへ蒸留する

use zellij_tile::prelude::{
    EventType, FloatingPaneCoordinates, MessageToPlugin, PaneId, PermissionType, ResizeStrategy,
};

#[cfg(not(test))]
mod passthrough {
    use super::*;

    pub(crate) fn subscribe(event_types: &[EventType]) {
        zellij_tile::shim::subscribe(event_types);
    }

    pub(crate) fn set_selectable(selectable: bool) {
        zellij_tile::shim::set_selectable(selectable);
    }

    pub(crate) fn show_cursor(cursor_position: Option<(usize, usize)>) {
        zellij_tile::shim::show_cursor(cursor_position);
    }

    pub(crate) fn request_permission(permissions: &[PermissionType]) {
        zellij_tile::shim::request_permission(permissions);
    }

    pub(crate) fn set_timeout(secs: f64) {
        zellij_tile::shim::set_timeout(secs);
    }

    pub(crate) fn close_plugin_pane(plugin_pane_id: u32) {
        zellij_tile::shim::close_plugin_pane(plugin_pane_id);
    }

    pub(crate) fn focus_plugin_pane(
        plugin_pane_id: u32,
        should_float_if_hidden: bool,
        should_be_in_place_if_hidden: bool,
    ) {
        zellij_tile::shim::focus_plugin_pane(
            plugin_pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
        );
    }

    pub(crate) fn rename_plugin_pane(plugin_pane_id: u32, new_name: &str) {
        zellij_tile::shim::rename_plugin_pane(plugin_pane_id, new_name);
    }

    pub(crate) fn unblock_cli_pipe_input(pipe_name: &str) {
        zellij_tile::shim::unblock_cli_pipe_input(pipe_name);
    }

    pub(crate) fn pipe_message_to_plugin(message_to_plugin: MessageToPlugin) {
        zellij_tile::shim::pipe_message_to_plugin(message_to_plugin);
    }

    pub(crate) fn close_pane_with_id(pane_id: PaneId) {
        zellij_tile::shim::close_pane_with_id(pane_id);
    }

    pub(crate) fn resize_pane_with_id(resize_strategy: ResizeStrategy, pane_id: PaneId) {
        zellij_tile::shim::resize_pane_with_id(resize_strategy, pane_id);
    }

    pub(crate) fn focus_pane_with_id(
        pane_id: PaneId,
        should_float_if_hidden: bool,
        should_be_in_place_if_hidden: bool,
    ) {
        zellij_tile::shim::focus_pane_with_id(
            pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
        );
    }

    pub(crate) fn send_sigkill_to_pane_id(pane_id: PaneId) {
        zellij_tile::shim::send_sigkill_to_pane_id(pane_id);
    }

    pub(crate) fn change_floating_panes_coordinates(
        pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
    ) {
        zellij_tile::shim::change_floating_panes_coordinates(pane_ids_and_coordinates);
    }

    pub(crate) fn intercept_key_presses() {
        zellij_tile::shim::intercept_key_presses();
    }

    pub(crate) fn clear_key_presses_intercepts() {
        zellij_tile::shim::clear_key_presses_intercepts();
    }
}

#[cfg(not(test))]
pub(crate) use passthrough::*;

#[cfg(test)]
pub(crate) use recorder::*;

// テスト側の実装: 発行されたホストコマンドをスレッドローカルに積む。
// テストはスレッド単位で走るので、テスト間の混線はスレッドローカルで防げる
#[cfg(test)]
mod recorder {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum HostCall {
        Subscribe {
            event_types: Vec<EventType>,
        },
        SetSelectable {
            selectable: bool,
        },
        ShowCursor {
            cursor_position: Option<(usize, usize)>,
        },
        RequestPermission {
            permissions: Vec<PermissionType>,
        },
        SetTimeout {
            secs: f64,
        },
        ClosePluginPane {
            plugin_pane_id: u32,
        },
        FocusPluginPane {
            plugin_pane_id: u32,
            should_float_if_hidden: bool,
            should_be_in_place_if_hidden: bool,
        },
        RenamePluginPane {
            plugin_pane_id: u32,
            new_name: String,
        },
        UnblockCliPipeInput {
            pipe_name: String,
        },
        // MessageToPlugin の蒸留（PartialEq が無いため。ファイル冒頭参照）
        PipeMessageToPlugin {
            message_name: String,
            destination_plugin_id: Option<u32>,
            message_payload: Option<String>,
        },
        ClosePaneWithId {
            pane_id: PaneId,
        },
        ResizePaneWithId {
            resize_strategy: ResizeStrategy,
            pane_id: PaneId,
        },
        FocusPaneWithId {
            pane_id: PaneId,
            should_float_if_hidden: bool,
            should_be_in_place_if_hidden: bool,
        },
        SendSigkillToPaneId {
            pane_id: PaneId,
        },
        ChangeFloatingPanesCoordinates {
            pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
        },
        InterceptKeyPresses,
        ClearKeyPressesIntercepts,
    }

    thread_local! {
        static CALLS: RefCell<Vec<HostCall>> = const { RefCell::new(Vec::new()) };
    }

    // 記録を発行順のまま取り出して空にする
    pub(crate) fn take_host_calls() -> Vec<HostCall> {
        CALLS.with(|calls| calls.take())
    }

    fn record(call: HostCall) {
        CALLS.with(|calls| calls.borrow_mut().push(call));
    }

    pub(crate) fn subscribe(event_types: &[EventType]) {
        record(HostCall::Subscribe {
            event_types: event_types.to_vec(),
        });
    }

    pub(crate) fn set_selectable(selectable: bool) {
        record(HostCall::SetSelectable { selectable });
    }

    pub(crate) fn show_cursor(cursor_position: Option<(usize, usize)>) {
        record(HostCall::ShowCursor { cursor_position });
    }

    pub(crate) fn request_permission(permissions: &[PermissionType]) {
        record(HostCall::RequestPermission {
            permissions: permissions.to_vec(),
        });
    }

    pub(crate) fn set_timeout(secs: f64) {
        record(HostCall::SetTimeout { secs });
    }

    pub(crate) fn close_plugin_pane(plugin_pane_id: u32) {
        record(HostCall::ClosePluginPane { plugin_pane_id });
    }

    pub(crate) fn focus_plugin_pane(
        plugin_pane_id: u32,
        should_float_if_hidden: bool,
        should_be_in_place_if_hidden: bool,
    ) {
        record(HostCall::FocusPluginPane {
            plugin_pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
        });
    }

    pub(crate) fn rename_plugin_pane(plugin_pane_id: u32, new_name: &str) {
        record(HostCall::RenamePluginPane {
            plugin_pane_id,
            new_name: new_name.to_string(),
        });
    }

    pub(crate) fn unblock_cli_pipe_input(pipe_name: &str) {
        record(HostCall::UnblockCliPipeInput {
            pipe_name: pipe_name.to_string(),
        });
    }

    pub(crate) fn pipe_message_to_plugin(message_to_plugin: MessageToPlugin) {
        record(HostCall::PipeMessageToPlugin {
            message_name: message_to_plugin.message_name,
            destination_plugin_id: message_to_plugin.destination_plugin_id,
            message_payload: message_to_plugin.message_payload,
        });
    }

    pub(crate) fn close_pane_with_id(pane_id: PaneId) {
        record(HostCall::ClosePaneWithId { pane_id });
    }

    pub(crate) fn resize_pane_with_id(resize_strategy: ResizeStrategy, pane_id: PaneId) {
        record(HostCall::ResizePaneWithId {
            resize_strategy,
            pane_id,
        });
    }

    pub(crate) fn focus_pane_with_id(
        pane_id: PaneId,
        should_float_if_hidden: bool,
        should_be_in_place_if_hidden: bool,
    ) {
        record(HostCall::FocusPaneWithId {
            pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
        });
    }

    pub(crate) fn send_sigkill_to_pane_id(pane_id: PaneId) {
        record(HostCall::SendSigkillToPaneId { pane_id });
    }

    pub(crate) fn change_floating_panes_coordinates(
        pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
    ) {
        record(HostCall::ChangeFloatingPanesCoordinates {
            pane_ids_and_coordinates,
        });
    }

    pub(crate) fn intercept_key_presses() {
        record(HostCall::InterceptKeyPresses);
    }

    pub(crate) fn clear_key_presses_intercepts() {
        record(HostCall::ClearKeyPressesIntercepts);
    }
}
