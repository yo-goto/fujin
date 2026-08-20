use crate::entry::decoded_char;
use crate::entry::restore_char_key;
use crate::render::Row;
use crate::test_support::*;
use crate::*;
use zellij_tile::shim::plugin_api::event::ProtobufEvent;

// --- IME経由の非ASCII入力（決定202608111836 / docs/issues/issue-ime-input-support.md） ---
//
// プラグインAPIのデコードが `Char` をコードポイントの下位1バイトへ畳むため、
// 素通しでは日本語がASCIIに化ける（navモードでは別のキーとして誤発火する）。
// entry.rs が protobuf の生の値から復元しているので、その往復を守る

fn intercepted_key_protobuf(c: char) -> ProtobufEvent {
    ProtobufEvent::try_from(Event::InterceptedKeyPress(KeyWithModifier::new(
        BareKey::Char(c),
    )))
    .expect("キーイベントは protobuf へ畳める")
}

fn intercepted_bare_key(event: &Event) -> Option<BareKey> {
    match event {
        Event::InterceptedKeyPress(key) => Some(key.bare_key),
        _ => None,
    }
}

fn decode_intercepted(protobuf: ProtobufEvent) -> Option<BareKey> {
    let decoded = decoded_char(&protobuf);
    let mut event = Event::try_from(protobuf).ok()?;
    restore_char_key(&mut event, decoded);
    intercepted_bare_key(&event)
}

#[test]
fn non_ascii_keys_collapse_without_the_restore() {
    // 復元しないと「日」(U+65E5) は下位バイトだけになり 'å'(U+00E5) に化ける。
    // 修正の前提そのものなので、上流が直ったらこのテストが落ちて気づける
    let event = Event::try_from(intercepted_key_protobuf('日')).expect("デコードできる");
    assert_eq!(intercepted_bare_key(&event), Some(BareKey::Char('å')));
}

#[test]
fn non_ascii_keys_survive_the_restore() {
    for c in ['日', '本', '語', 'ぁ', 'ー', '漢', 'é', '🐎'] {
        assert_eq!(
            decode_intercepted(intercepted_key_protobuf(c)),
            Some(BareKey::Char(c)),
            "{c} が復元されない"
        );
    }
}

#[test]
fn ascii_keys_are_untouched_by_the_restore() {
    for c in ['a', 'Z', '/', ' ', '9'] {
        assert_eq!(
            decode_intercepted(intercepted_key_protobuf(c)),
            Some(BareKey::Char(c))
        );
    }
}

#[test]
fn named_keys_are_untouched_by_the_restore() {
    for bare in [BareKey::Enter, BareKey::Esc, BareKey::Tab, BareKey::Up] {
        let protobuf =
            ProtobufEvent::try_from(Event::InterceptedKeyPress(KeyWithModifier::new(bare)))
                .expect("キーイベントは protobuf へ畳める");
        assert_eq!(decoded_char(&protobuf), None);
        assert_eq!(decode_intercepted(protobuf), Some(bare));
    }
}

#[test]
fn search_query_accepts_japanese() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('/')));
    for c in "日本語".chars() {
        state.handle_nav_key(key(BareKey::Char(c)));
    }
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("日本語")
    );
    // Backspace は char 単位で消える（バイト単位に落ちていない）
    state.handle_nav_key(key(BareKey::Backspace));
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("日本")
    );
}

#[test]
fn the_input_cursor_follows_the_query_end() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    state.nav_mode = true;
    // 画面高は描画で決まる（まだ描いていなければ位置は決まらない）
    assert_eq!(state.input_cursor_position(), None);
    state.render(20, SIDEBAR);
    // 入力欄が無い間はカーソルを隠す
    assert_eq!(state.input_cursor_position(), None);

    state.handle_nav_key(key(BareKey::Char('/')));
    let (x, y) = state
        .input_cursor_position()
        .expect("検索サブモード中はカーソルが出る");
    // 字下げ2 + タグ `/` の1
    assert_eq!(x, 3);
    assert!(matches!(state.screen_rows(20)[y], Row::Footer));

    // 全角は表示セル幅で数える（文字数で数えるとずれる）
    for c in "日本".chars() {
        state.handle_nav_key(key(BareKey::Char(c)));
    }
    assert_eq!(state.input_cursor_position().map(|(x, _)| x), Some(7));

    // 操作状態もテキストは受け付けないが、位置表示はテキストカーソルに一本化した
    // ので出したままにする（決定202608131200、2026-08-13。経緯:
    // docs/issues/issue-search-input-cursor-shape.md）
    state.handle_nav_key(key(BareKey::Esc));
    assert!(state.input_cursor_position().is_some());
    state.handle_nav_key(key(BareKey::Char('i')));
    assert!(state.input_cursor_position().is_some());

    // 抜ければ隠れる
    state.handle_nav_key(key(BareKey::Esc));
    state.handle_nav_key(key(BareKey::Esc));
    assert_eq!(state.input_cursor_position(), None);
}

#[test]
fn pasted_text_lands_in_the_search_query() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    state.nav_mode = true;
    // 入力欄の外に落ちたテキストは捨てる（navモードは抜けない）
    assert!(!state.handle_pasted_text("日本語"));
    assert!(state.nav_mode);

    state.handle_nav_key(key(BareKey::Char('/')));
    assert!(state.handle_pasted_text("日本語"));
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("日本語")
    );
    // 打鍵と混ぜても積み上がる（IMEの確定とキー入力は別経路で届く）
    state.handle_nav_key(key(BareKey::Char('x')));
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("日本語x")
    );
    // 改行・タブは欄を壊すので落とす
    state.handle_pasted_text("\n\tあ");
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("日本語xあ")
    );
    // 制御文字だけのペーストは何も変えない
    assert!(!state.handle_pasted_text("\n"));
}

#[test]
fn pasted_text_is_ignored_while_the_help_overlay_is_open() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Esc)); // 操作状態へ（ここから `?` が効く）
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(state.help_overlay);
    // オーバーレイ中のキーは「閉じる」にしか使われない。ペーストも同じ扱いで、
    // 画面に出ていないクエリへは流さない
    assert!(!state.handle_pasted_text("日本語"));
    assert_eq!(state.search.as_ref().map(|s| s.query.as_str()), Some(""));
    // 閉じて編集状態へ戻せばまた入る
    state.handle_nav_key(key(BareKey::Esc));
    state.handle_nav_key(key(BareKey::Char('i')));
    assert!(state.handle_pasted_text("日本語"));
    assert_eq!(
        state.search.as_ref().map(|s| s.query.as_str()),
        Some("日本語")
    );
}

#[test]
fn the_input_cursor_stays_inside_the_pane_when_the_query_overflows() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    state.nav_mode = true;
    state.render(20, SIDEBAR);
    state.handle_nav_key(key(BareKey::Char('/')));
    // 幅32の欄に収まらないクエリ。範囲外の列を伝えると zellij 側がカーソルを
    // 非表示扱いにし、候補窓が左上へ飛ぶ（input_cursor_position のクランプ）
    state.handle_pasted_text(&"あ".repeat(20));
    let (x, _) = state
        .input_cursor_position()
        .expect("検索サブモード中はカーソルが出る");
    // 文字を置ける最終列 = 幅32 − 右マージン2 − 1
    assert_eq!(x, 29);
}

#[test]
fn the_input_cursor_hides_when_the_footer_is_not_an_input() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    state.nav_mode = true;
    state.render(20, SIDEBAR);
    state.handle_nav_key(key(BareKey::Char('/')));
    assert!(state.input_cursor_position().is_some());

    // ヘルプオーバーレイ中のフッターは閉じ方の案内（footer_line）。
    // カーソルを入力欄の位置に残すと、候補窓だけがそこに出てしまう
    state.handle_nav_key(key(BareKey::Esc)); // 操作状態へ（ここから `?` が効く）
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(state.help_overlay);
    assert_eq!(state.input_cursor_position(), None);
    // 閉じて編集状態へ戻せば戻る
    state.handle_nav_key(key(BareKey::Esc));
    state.handle_nav_key(key(BareKey::Char('i')));
    assert!(state.input_cursor_position().is_some());

    // 終了操作サブモードのフッターは確認プロンプト
    state.handle_nav_key(key(BareKey::Esc)); // 操作状態へ
    state.handle_nav_key(key(BareKey::Esc)); // 検索を抜けて navモードへ
    state.handle_nav_key(key(BareKey::Char('d')));
    assert!(state.termination.is_some());
    assert_eq!(state.input_cursor_position(), None);
}

#[test]
fn typing_defers_renders_that_come_from_outside() {
    let mut state = sidebar_state();
    observe_panes(&mut state, &[1, 2]);
    state.nav_mode = true;
    state.render(20, SIDEBAR);
    // 入力欄を出していない間は、外から来たイベントでも普通に描き直す
    assert!(!state.defers_render_while_typing());

    // 入力中は描き直しを見送る（描くとテキストカーソルが入力欄へ戻され、IMEの
    // 変換候補ウィンドウが打っている途中で飛ぶ）
    state.handle_nav_key(key(BareKey::Char('/')));
    assert!(state.defers_render_while_typing());
    // 操作状態は打っていないので見送らない（決定202608131200）。ここで止め続けると
    // 結果を見ながら動かしているあいだ一覧が古いまま固まる。
    // **テキストカーソルは操作状態でも出ている**ので、カーソルの有無で判定していた
    // 頃の実装（input_cursor_column への委譲）ではここが固まる
    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.defers_render_while_typing());
    assert!(state.input_cursor_position().is_some());
    // `i` で編集状態へ戻ればまた見送る
    state.handle_nav_key(key(BareKey::Char('i')));
    assert!(state.defers_render_while_typing());
    state.handle_nav_key(key(BareKey::Esc));
    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.defers_render_while_typing());

    // 番号ジャンプの入力欄も同じ扱い
    state.handle_nav_key(key(BareKey::Char('n')));
    assert!(state.defers_render_while_typing());
    // 番号ジャンプ中の `?` はヘルプを開く。入力欄は画面から消えるのにバッファは
    // 残るので、`jump.is_some()` だけで見ると見送ったまま一覧が固まる
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(state.help_overlay && state.jump.is_some());
    assert!(!state.defers_render_while_typing());
    // ヘルプを閉じれば番号ジャンプの入力欄へ戻る
    state.handle_nav_key(key(BareKey::Esc));
    assert!(state.defers_render_while_typing());
    state.handle_nav_key(key(BareKey::Esc));
    assert!(!state.defers_render_while_typing());

    // フッターが入力欄でなくなる場面（ヘルプ）では見送らない
    state.handle_nav_key(key(BareKey::Char('/')));
    state.handle_nav_key(key(BareKey::Esc));
    state.handle_nav_key(key(BareKey::Char('?')));
    assert!(!state.defers_render_while_typing());
}
