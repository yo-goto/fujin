// 永続化基盤（要件: docs/requirements/persistence/。実装フェーズF3）。
//
// fujin がディスクへ書くのはここが初めて。最初の利用者は Formation（F4）だが、
// Formation 固有ではなく fujin の基盤として切ってある（決定44）。
//
// **プラグインは任意のパスへは書けない。** zellij が wasm へ preopen するのは
// `/host` `/data` `/cache` `/tmp` の4つだけで（zellij-server 0.44.3 の
// `plugin_loader.rs::create_wasi_ctx`）、`/data` はアンロードで消える揮発領域。
// `FullHdAccess` が実際に解禁するのは `change_host_folder()`（`/host` の向け先を
// 変えるホスト関数）であって、任意パスへの直接アクセスではない。したがって
// **`/host` を永続化ディレクトリへ向け直し、その配下だけを読み書きする**。
//
// 向け先は**存在しないと切り替えに失敗する**（zellij-server の
// `wasm_bridge.rs::change_plugin_host_dir`）ので、初回は作れる祖先まで遡って
// 掘ってから向け直す（`Store`）。切り替えは非同期で、結果は `HostFolderChanged` /
// `FailedToChangeHostFolder` で返る。
//
// このフェーズで実際に読み書きするのはセッション索引だけ。フォーメーションの
// 定義・割り当てファイルの読み書きは F4 で足す（フェーズをまたいで先回りしない）。

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zellij_tile::prelude::*;

use crate::State;

// 永続化ディレクトリの名前。`$XDG_CONFIG_HOME/fujin/`（決定44）
const STORE_DIR: &str = "fujin";
// guest 側の起点。`/host` を永続化ディレクトリへ向けてあるので常にここが根
const HOST: &str = "/host";
// 原子的書き込み用の一時ファイル置き場（要件のディレクトリ構成）
const TMP_DIR: &str = "sessions/.tmp";
// セッション索引
const INDEX_FILE: &str = "session_index.toml";

// スキーマのバージョン。読み込み時にこれと一致しなければ内容を採らない
const SCHEMA_VERSION: u32 = 1;

// 向け先を祖先へ遡る上限。`~/.config/fujin` なら2段（`~/.config` → `~`）で足りる
const ANCESTOR_LIMIT: usize = 3;

// 楽観的並行制御の再試行回数（要件では未解決だった値）。索引を書くのは起動時と
// リネーム時しかない。競合相手は同一セッションの兄弟インスタンスと、索引を共有する
// 別セッションのインスタンスだが、いずれも書く機会が少ないため数回で十分に収束する
const WRITE_RETRIES: usize = 3;

// 内部IDの桁数の上限。ディレクトリ名として使うので長さも縛っておく
const ID_MAX_LEN: usize = 12;

// `/host` の向け先の付け替え（モジュール冒頭）。読み書きできるのは Ready のときだけ。
//
// 権限が承認されない・環境変数が取れない・ディレクトリを作れないときは
// Unavailable に落ちる。**そのときも fujin は通常どおり動く**（要件: 永続化に
// 必要な権限が承認されていなくてもfujinは動作を続ける）
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum Store {
    // 権限の承認待ち。まだ何も試していない
    #[default]
    Idle,
    // 向け先を切り替えている最中。`target` が目的地、`attempt` がいま頼んだ先
    Opening {
        target: PathBuf,
        attempt: PathBuf,
    },
    // `/host` が永続化ディレクトリを指している
    Ready,
    // 永続化を諦めた
    Unavailable,
}

// セッション索引（要件: persistence-session-scope）。セッション名と内部IDの対応と、
// 最終確認時刻を持つ
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SessionIndex {
    // スキーマのバージョン
    pub(crate) version: u32,
    // 楽観的並行制御（決定44）に使う書き込みごとの通し番号。**スキーマの
    // `version` とは別物** — スキーマは書き込みでは動かないので、競合の検出に使えない
    #[serde(default)]
    pub(crate) revision: u64,
    #[serde(default)]
    pub(crate) sessions: Vec<SessionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SessionEntry {
    // 内部ID。ディレクトリ名になるので数字だけに限る（→ パスのサニタイズが要らない）
    pub(crate) id: String,
    pub(crate) name: String,
    // 最終確認時刻（unix秒）。孤立ディレクトリの掃除（F4）が猶予期間の判定に使う
    pub(crate) last_seen: u64,
}

impl SessionIndex {
    pub(crate) fn empty() -> Self {
        Self {
            version: SCHEMA_VERSION,
            revision: 0,
            sessions: Vec::new(),
        }
    }
}

// --- 純粋ロジック（ホスト関数を跨がないのでユニットテストで守れる） ---

// 保存先（要件: persistence-storage）。`$XDG_CONFIG_HOME` があればその配下、
// 無ければ `$HOME/.config` の配下。
//
// 相対パスの環境変数は未設定と同じに倒す — `/host` の向け先はホスト側の絶対パスで
// 解決されるので、相対パスを渡しても意図した場所にはならない
pub(crate) fn store_dir(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let absolute = |raw: &String| {
        let path = PathBuf::from(raw);
        path.is_absolute().then_some(path)
    };
    if let Some(xdg) = env.get("XDG_CONFIG_HOME").and_then(absolute) {
        return Some(xdg.join(STORE_DIR));
    }
    let home = env.get("HOME").and_then(absolute)?;
    Some(home.join(".config").join(STORE_DIR))
}

// 切り替えに失敗したときに次へ頼む先。目的地から祖先へ1段ずつ遡る。
// 遡りすぎ（`ANCESTOR_LIMIT`）とルート到達で打ち切る
pub(crate) fn next_ancestor(target: &Path, attempt: &Path) -> Option<PathBuf> {
    let depth = target.strip_prefix(attempt).ok()?.components().count();
    if depth >= ANCESTOR_LIMIT {
        return None;
    }
    let parent = attempt.parent()?;
    (parent != attempt).then(|| parent.to_path_buf())
}

// 祖先まで戻れたときに掘るべき guest パス。`/host` は `reached` を指しているので、
// そこから目的地までの相対部分を作れば目的地が生える
pub(crate) fn descend(target: &Path, reached: &Path) -> Option<PathBuf> {
    let rel = target.strip_prefix(reached).ok()?;
    (!rel.as_os_str().is_empty()).then(|| Path::new(HOST).join(rel))
}

// 永続化ディレクトリ配下の guest パス（要件: persistence-safety の prefix check）。
// 絶対パスと `..` を弾く — 書き込み先が永続化ディレクトリの外へ出る経路を、
// パスを組み立てる一箇所で塞ぐ
pub(crate) fn guest_path(rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        return None;
    }
    let rel = Path::new(rel);
    if !rel
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(Path::new(HOST).join(rel))
}

// 一時ファイルの置き場所。**セッション名のハッシュとプラグインIDの両方を混ぜる** —
// 一時ファイルまで共有すると書きかけ同士が混ざるため。兄弟インスタンス（同一
// セッションへの複数クライアント）はプラグインIDで分かれるが、プラグインIDは
// zellijサーバ（=セッション）ごとに振り直されるので、全セッションが共有する
// 索引の一時ファイルはIDだけではセッションを跨いで衝突する
pub(crate) fn tmp_rel(rel: &str, plugin_id: u32, session: &str) -> String {
    format!(
        "{}/{:016x}-{}-{}",
        TMP_DIR,
        fnv1a(session.as_bytes()),
        plugin_id,
        rel.replace('/', "_")
    )
}

// FNV-1a（64bit）。セッション名はパスに使えない文字を含みうるので、そのまま
// ファイル名へ埋めずハッシュへ畳む
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// 内部IDとして許す形（要件: ディレクトリ名は内部IDで切る）。数字だけに限ることで、
// 読み込んだ索引が壊れていてもパスの外へ出られない
pub(crate) fn valid_internal_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= ID_MAX_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

// 読み込んだ索引のスキーマ検証（要件: スキーマ検証を通らないファイルは読み込まない）。
// 通らなければ呼び出し側が空の索引へフォールバックする
pub(crate) fn validate_index(index: SessionIndex) -> Option<SessionIndex> {
    if index.version != SCHEMA_VERSION {
        return None;
    }
    if !index.sessions.iter().all(|entry| {
        valid_internal_id(&entry.id) && !entry.name.is_empty() && !entry.name.contains('\0')
    }) {
        return None;
    }
    Some(index)
}

// 索引を現在のセッション名に合わせた結果（変更が要らなければ None）と、使う内部ID。
//
// - 内部IDを既に持っていれば、そのエントリの名前だけを書き換える（リネーム追従。
//   ディレクトリは動かさない）
// - まだ持っていなければ、**同名のエントリの内部IDを再利用する**（同じ名前で作り
//   直したセッションが以前のフォーメーションを引き継ぐ）。無ければ新規に発行する
//
// `last_seen` は起動時（`current` が None のとき）にだけ更新する。届くたびに
// 更新すると、リネーム以外の `SessionUpdate` でも索引を書き換えてしまう
pub(crate) fn reconcile(
    index: &SessionIndex,
    name: &str,
    current: Option<&str>,
    now: u64,
) -> (Option<SessionIndex>, String) {
    let mut next = index.clone();
    if let Some(id) = current {
        let id = id.to_string();
        match next.sessions.iter_mut().find(|entry| entry.id == id) {
            Some(entry) if entry.name == name => return (None, id),
            Some(entry) => entry.name = name.to_string(),
            // 索引から消えていた（手で消された等）。同じ内部IDで書き戻す
            None => next.sessions.push(SessionEntry {
                id: id.clone(),
                name: name.to_string(),
                last_seen: now,
            }),
        }
        return (Some(next), id);
    }
    let reused = next
        .sessions
        .iter_mut()
        .find(|entry| entry.name == name)
        .map(|entry| {
            entry.last_seen = now;
            entry.id.clone()
        });
    if let Some(id) = reused {
        return (Some(next), id);
    }
    let id = next_internal_id(&next);
    next.sessions.push(SessionEntry {
        id: id.clone(),
        name: name.to_string(),
        last_seen: now,
    });
    (Some(next), id)
}

// 新しい内部ID。既存の最大値の次を使う（連番なのでディレクトリ名として安全）。
// 桁数上限に達したIDは飛ばす — 上限の次（13桁）を発行すると、自分の書いた索引を
// 次回の読み込み（`valid_internal_id`）が丸ごと弾いて空へフォールバックしてしまう
fn next_internal_id(index: &SessionIndex) -> String {
    const MAX: u64 = 10u64.pow(ID_MAX_LEN as u32) - 1;
    let max = index
        .sessions
        .iter()
        .filter_map(|entry| entry.id.parse::<u64>().ok())
        .filter(|id| *id < MAX)
        .max()
        .unwrap_or(0);
    format!("{}", max + 1)
}

// --- ホスト関数を跨ぐ側 ---

impl State {
    // 永続化ディレクトリを開き始める。権限が揃った直後（`PermissionRequestResult`）に
    // 呼ぶ。以降は `HostFolderChanged` / `FailedToChangeHostFolder` で進む
    pub(crate) fn open_store(&mut self) {
        if self.store != Store::Idle {
            return;
        }
        let Some(target) = store_dir(&self.store_env()) else {
            // 保存先を決められない。永続化は諦めて通常どおり動く
            eprintln!("fujin: could not resolve the config directory; persistence is off");
            self.store = Store::Unavailable;
            return;
        };
        self.attempt_host_folder(target.clone(), target);
    }

    // 環境変数（要件: 保存先は環境変数から解決する）。
    //
    // **まず自プロセスの環境を見る** — zellij は wasi ctx に `inherit_env()` を
    // 通しているので（zellij-server `plugin_loader.rs::create_wasi_ctx`）、サーバ
    // プロセスの環境がそのまま見える。足りないときだけホスト関数へ落ちる。
    // そちらは `ReadSessionEnvironmentVariables` が要り、**未承認だと shim 側の
    // unwrap でプラグインごと落ちる**ので、承認済みのときしか呼ばない
    fn store_env(&self) -> BTreeMap<String, String> {
        // `std::env::vars()` は使わない — 非UTF-8の変数が1つでもあると
        // イテレーション中に panic する。読めない変数は黙って捨てる
        let mut env: BTreeMap<String, String> = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        if env.contains_key("XDG_CONFIG_HOME") || env.contains_key("HOME") {
            return env;
        }
        if self.permissions_granted {
            env.extend(get_session_environment_variables());
        }
        env
    }

    fn attempt_host_folder(&mut self, target: PathBuf, attempt: PathBuf) {
        self.store = Store::Opening {
            target,
            attempt: attempt.clone(),
        };
        change_host_folder(attempt);
    }

    // 向け先が切り替わった。目的地なら準備完了、祖先なら掘ってから向け直す。
    // 戻り値は再描画するか
    pub(crate) fn on_host_folder_changed(&mut self, reached: PathBuf) -> bool {
        let Store::Opening { target, .. } = &self.store else {
            return false;
        };
        let target = target.clone();
        if reached == target {
            self.store = Store::Ready;
            return self.sync_session_index();
        }
        let Some(dir) = descend(&target, &reached) else {
            self.store = Store::Unavailable;
            return false;
        };
        if let Err(err) = std::fs::create_dir_all(&dir) {
            eprintln!("fujin: could not create {}: {}", target.display(), err);
            self.store = Store::Unavailable;
            return false;
        }
        self.attempt_host_folder(target.clone(), target);
        false
    }

    // 向け先の切り替えに失敗した。原因は目的地がまだ無いことなので、作れる祖先まで
    // 遡ってから掘り直す（`next_ancestor`）
    pub(crate) fn on_host_folder_failed(&mut self) -> bool {
        let Store::Opening { target, attempt } = &self.store else {
            return false;
        };
        let (target, attempt) = (target.clone(), attempt.clone());
        match next_ancestor(&target, &attempt) {
            Some(next) => self.attempt_host_folder(target, next),
            None => {
                eprintln!(
                    "fujin: could not open {}; persistence is off",
                    target.display()
                );
                self.store = Store::Unavailable;
            }
        }
        false
    }

    // `Event::SessionUpdate` の受け口（要件: リネームの検知は SessionUpdate の
    // 差分で行う）。**キャッシュ済みの名前と違うときだけ**索引へ触る — この
    // イベントはリネーム以外でも高頻度に飛ぶ
    pub(crate) fn handle_session_update(&mut self, sessions: &[SessionInfo]) -> bool {
        let Some(name) = sessions
            .iter()
            .find(|session| session.is_current_session)
            .map(|session| session.name.clone())
        else {
            return false;
        };
        if self.session_name.as_deref() == Some(name.as_str()) {
            return false;
        }
        self.session_name = Some(name);
        self.sync_session_index()
    }

    // セッション索引を現在のセッション名へ合わせる。永続化ディレクトリが開くのと
    // セッション名が届くのは非同期なので、**揃った側から**呼んで両方揃うのを待つ
    fn sync_session_index(&mut self) -> bool {
        if self.store != Store::Ready {
            return false;
        }
        let Some(name) = self.session_name.clone() else {
            return false;
        };
        let now = unix_now();
        let current = self.session_id.clone();
        let mut assigned = None;
        self.update_index(|index| {
            let (next, id) = reconcile(index, &name, current.as_deref(), now);
            assigned = Some(id);
            next
        });
        // 書き込みに失敗しても内部IDは決まる（次の機会に書き戻せる）。
        // 内部IDは描画に出ないので、再描画は要らない
        self.session_id = assigned.or(self.session_id.take());
        false
    }

    // 索引を読んで編集して書き戻す。**書く直前にもう一度読み、読んだときから
    // `revision` が動いていたらやり直す**（決定44の楽観的並行制御。WASI の
    // ファイルロックは zellij のランタイムで通るか未検証なので依存しない）
    fn update_index<F>(&self, mut edit: F) -> bool
    where
        F: FnMut(&SessionIndex) -> Option<SessionIndex>,
    {
        for _ in 0..WRITE_RETRIES {
            let before = self.read_index();
            let Some(mut next) = edit(&before) else {
                return true;
            };
            if self.read_index().revision != before.revision {
                continue;
            }
            next.revision = before.revision.saturating_add(1);
            if self.write_document(INDEX_FILE, &next) {
                return true;
            }
        }
        false
    }

    // 索引を読む。スキーマ検証（要件）を通らなければ**空の索引として扱う** —
    // 壊れたデータを鵜呑みにしない
    fn read_index(&self) -> SessionIndex {
        self.read_document::<SessionIndex>(INDEX_FILE)
            .and_then(validate_index)
            .unwrap_or_else(SessionIndex::empty)
    }

    // TOML を読む。読めない・壊れているは同じ「無い」に倒す（要件: TOMLとして
    // 壊れている・型が一致しない値・必須フィールドの欠落）
    fn read_document<T: serde::de::DeserializeOwned>(&self, rel: &str) -> Option<T> {
        if self.store != Store::Ready {
            return None;
        }
        let raw = std::fs::read_to_string(guest_path(rel)?).ok()?;
        toml::from_str(&raw).ok()
    }

    // TOML を原子的に書く（要件: 一時ファイルへ書いてから rename）。
    // 途中で落ちても既存ファイルは書き込み前の内容のまま残る。
    //
    // 文字列は手で組み立てず必ずシリアライザを通す（要件: persistence-safety）
    fn write_document<T: Serialize>(&self, rel: &str, value: &T) -> bool {
        if self.store != Store::Ready {
            return false;
        }
        let Ok(contents) = toml::to_string(value) else {
            return false;
        };
        let plugin_id = self.own_plugin_id.unwrap_or(0);
        let session = self.session_name.as_deref().unwrap_or("");
        let (Some(dest), Some(tmp)) = (
            guest_path(rel),
            guest_path(&tmp_rel(rel, plugin_id, session)),
        ) else {
            return false;
        };
        for dir in [tmp.parent(), dest.parent()].into_iter().flatten() {
            if std::fs::create_dir_all(dir).is_err() {
                return false;
            }
        }
        if std::fs::write(&tmp, contents).is_err() {
            return false;
        }
        if std::fs::rename(&tmp, &dest).is_err() {
            // 置き換えに失敗したら書きかけを残さない
            let _ = std::fs::remove_file(&tmp);
            return false;
        }
        true
    }
}

// 現在時刻（unix秒）。時計が引けなければ 0 — 最終確認時刻が古いままになるだけで、
// 掃除（F4）は生存セッション一覧との突き合わせも見るので誤って消えることはない
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}
