// プラグインのエントリポイント（wasm のエクスポート関数）。
//
// zellij-tile の `register_plugin!` を展開して自前で持っている。理由は1つだけ:
// プラグインAPIは `BareKey::Char` を `char_index as u8 as char` でデコードする
// ため、非ASCII のコードポイントが下位1バイトへ潰れる（zellij-utils 0.44.3
// `plugin_api/key.rs:143`。0.44.3 時点で上流 main も未修正）。マクロの中では
// protobuf の生の値に手が届かないので、`update()` だけ自前にして復元する。
// 調査と実測は docs/issues/ime-input-support.md、判断は決定47。
//
// ここ以外はマクロの写しで、挙動を変えているのは
//   - char の復元（`decoded_char`）
//   - unwrap をやめて畳んだこと（プラグイン内の panic はサイドバーごと落とす）
// の2点だけ。

// `#[no_mangle]` は unsafe 属性扱いなので、クレート全体の `unsafe_code = deny`
// に引っかかる。ここで許可するのはシンボル名の固定だけで（zellij はこの名前で
// エクスポートを探す）、unsafe なコードは書いていない。マクロを使っていた頃は
// 外部クレート側の展開だったので lint の対象外だった
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::convert::TryFrom;

use zellij_tile::prelude::*;
use zellij_tile::shim::object_from_stdin;
use zellij_tile::shim::plugin_api::action::ProtobufPluginConfiguration;
use zellij_tile::shim::plugin_api::event::ProtobufEvent;
use zellij_tile::shim::plugin_api::generated_api::api::event::event::Payload as ProtobufEventPayload;
use zellij_tile::shim::plugin_api::generated_api::api::key::key::MainKey as ProtobufMainKey;
use zellij_tile::shim::plugin_api::pipe_message::ProtobufPipeMessage;
use zellij_tile::shim::prost::Message;

use crate::State;

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

// panic をホストへ報告するフックの登録。`fn main()` はバイナリクレートの
// ルート（main.rs）にしか置けないので、中身だけこちらに置く
pub(crate) fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        report_panic(info);
    }));
}

#[no_mangle]
fn load() {
    STATE.with(|state| {
        // 設定を読み損ねても既定値で起動する。ここで諦めるとサイドバーが
        // 何も描かないまま無反応になり、利用者からは原因が分からない
        let configuration = plugin_configuration().unwrap_or_default();
        state.borrow_mut().load(configuration);
    });
}

#[no_mangle]
pub fn update() -> bool {
    STATE.with(|state| {
        let Some(protobuf) = decode_from_stdin::<ProtobufEvent>() else {
            return false;
        };
        // Event へ畳む前に生の値を控える。畳んだ後では復元に要る情報が消えている
        let char_key = decoded_char(&protobuf);
        let mut event = match Event::try_from(protobuf) {
            Ok(event) => event,
            Err(e) => {
                eprintln!("fujin: failed to convert an event: {e}");
                return false;
            }
        };
        restore_char_key(&mut event, char_key);
        state.borrow_mut().update(event)
    })
}

#[no_mangle]
pub fn pipe() -> bool {
    STATE.with(|state| {
        let Some(protobuf) = decode_from_stdin::<ProtobufPipeMessage>() else {
            return false;
        };
        let message = match PipeMessage::try_from(protobuf) {
            Ok(message) => message,
            Err(e) => {
                eprintln!("fujin: failed to convert a pipe message: {e}");
                return false;
            }
        };
        state.borrow_mut().pipe(message)
    })
}

#[no_mangle]
pub fn render(rows: i32, cols: i32) {
    STATE.with(|state| {
        state
            .borrow_mut()
            .render(rows.max(0) as usize, cols.max(0) as usize);
    });
}

#[no_mangle]
pub fn plugin_version() {
    println!("{}", VERSION);
}

// stdin に載ってくる protobuf のデコード。ホスト関数の戻り値は
// バイト列として1行の JSON で渡ってくる（zellij-tile の shim と同じ経路）。
//
// 失敗はログに残して畳む — マクロ版なら unwrap の panic で気づけた
// バージョン不整合が、黙って捨てると「無反応なサイドバー」にしか見えない
fn decode_from_stdin<T: Message + Default>() -> Option<T> {
    let bytes: Vec<u8> = object_from_stdin()
        .map_err(|e| eprintln!("fujin: failed to read a host payload: {e}"))
        .ok()?;
    T::decode(bytes.as_slice())
        .map_err(|e| eprintln!("fujin: failed to decode a host payload: {e}"))
        .ok()
}

fn plugin_configuration() -> Option<BTreeMap<String, String>> {
    let protobuf = decode_from_stdin::<ProtobufPluginConfiguration>()?;
    BTreeMap::try_from(&protobuf).ok()
}

// キーイベントの `Char` を生の値から組み立て直す。
//
// サーバーは `character as i32` で正しいコードポイントを送っているのに、
// 受け側のデコードが `as u8` で畳んでしまう（このファイル冒頭）。payload の
// 生の値を読めば、潰れる前の char をこちらで復元できる。
// ASCII 範囲は畳んでも同じ値なので、実質的に効くのは非ASCII だけ
pub(crate) fn decoded_char(protobuf: &ProtobufEvent) -> Option<char> {
    let Some(ProtobufEventPayload::KeyPayload(key)) = &protobuf.payload else {
        return None;
    };
    let Some(ProtobufMainKey::Char(raw)) = key.main_key else {
        return None;
    };
    char::from_u32(u32::try_from(raw).ok()?)
}

// 潰れた `Char` を復元した値で差し替える。購読しているのは
// InterceptedKeyPress だけだが、同じ潰れ方をする Event::Key も一緒に直しておく
//（後で購読したときの罠を残さない）
pub(crate) fn restore_char_key(event: &mut Event, decoded: Option<char>) {
    let Some(decoded) = decoded else {
        return;
    };
    match event {
        Event::InterceptedKeyPress(key) | Event::Key(key) => {
            key.bare_key = BareKey::Char(decoded);
        }
        _ => {}
    }
}
