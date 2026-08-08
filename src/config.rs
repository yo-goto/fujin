// 設定（configuration）の取り込み口（決定40）。
//
// **ここが設定仕様の正本。** ユーザーが書く設定はこの `SETTINGS` テーブルが
// すべてで、README の設定節はここから生成する（`settings_doc()` と
// tests.rs の README 同期テスト。生成は `make readme`）。手で転記した表は
// 必ずいつか実装とずれるので、転記そのものを無くしてある。
//
// 取り込みを1本の入口（`Config::parse`）に集約しているのは、KDL の書式差で
// 設定が黙って無視される実測バグへの対応でもある
//（docs/issues/config-requirements-review.md の問題1）。zellij はプロパティ
// 書式（`show_cwd="true"`）の値を `KdlValue` の `Display` で文字列化するため
// **引用符が付いたまま**プラグインへ渡り、子ノード書式（`show_cwd "true"`）と
// 違う値になる。値の正規化をここでまとめてかけて差を吸収する。
//
// 正規化しても解釈できない値は、黙って既定値へ倒さずに警告として持ち帰る
//（表示はサイドバーのフッター。決定40）。真偽値の受け口は広げない —
// `"true"` だけが真で、`1` や `yes` は解釈できない値として扱う。

use std::collections::BTreeMap;
use std::str::FromStr;

use zellij_tile::prelude::{BareKey, KeyWithModifier};

// 召喚インスタンスに渡す**内部用**の configuration キー（決定16）。
// 臨時召喚が自分で渡すもので、ユーザーが config.kdl に書くものではない。
// したがって `SETTINGS` には載せず、README の設定表にも出さない
//（決定40「内部用キーは公開仕様に含めない」）。
// "true" で起動したインスタンスは、準備でき次第 navモードへ入る
pub(crate) const SUMMONED_KEY: &str = "summoned";

// 設定値の種類。受け口の広さと、README の「値」列の書き方がこれで決まる
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    // 真偽値。`"true"` のときだけ真（決定40）
    Flag,
    // フッターに出す direct-keys のキー表記（決定28）。
    // `label` はヒントの動作名、`arrow` は幅が足りないときの代替表記
    DirectKey {
        label: &'static str,
        arrow: Option<&'static str>,
    },
}

// ユーザーが書ける設定1項目ぶん
pub(crate) struct Setting {
    pub(crate) key: &'static str,
    pub(crate) kind: Kind,
    // README の KDL 例に出す値。ドキュメント生成専用
    #[allow(dead_code)] // 読むのは README 生成（テスト時のみ）
    pub(crate) example: &'static str,
    #[allow(dead_code)] // 同上
    pub(crate) summary_ja: &'static str,
    #[allow(dead_code)] // 同上
    pub(crate) summary_en: &'static str,
}

// 公開する設定の全部。**並び順がそのまま README の表と、フッターの
// direct-keys ヒントの表示順になる**（決定27・28）
pub(crate) const SETTINGS: [Setting; 4] = [
    Setting {
        key: "show_cwd",
        kind: Kind::Flag,
        example: "true",
        summary_ja: "ペイン行の下に cwd を表示します（フック設定済みのペインのみ）",
        summary_en: "Show cwd under each pane row (only for panes with the hook set up)",
    },
    Setting {
        key: "up_key",
        kind: Kind::DirectKey {
            label: "up",
            // 矢印は「東アジア文字幅が曖昧な記号をキー表記に使わない」という
            // UI規則の例外で、幅が足りないときだけ使う
            arrow: Some("↑"),
        },
        example: "Alt u",
        summary_ja: "`fujin_up` に割り当てたキーの表記（フッターのヒント用・表示専用）",
        summary_en: "Spelling of the key bound to `fujin_up` (footer hint only)",
    },
    Setting {
        key: "down_key",
        kind: Kind::DirectKey {
            label: "down",
            arrow: Some("↓"),
        },
        example: "Alt d",
        summary_ja: "`fujin_down` に割り当てたキーの表記（フッターのヒント用・表示専用）",
        summary_en: "Spelling of the key bound to `fujin_down` (footer hint only)",
    },
    Setting {
        key: "go_key",
        kind: Kind::DirectKey {
            label: "jump",
            // `jump` に対応する矢印記号は無い
            arrow: None,
        },
        example: "Alt g",
        summary_ja: "`fujin_go` に割り当てたキーの表記（フッターのヒント用・表示専用）",
        summary_en: "Spelling of the key bound to `fujin_go` (footer hint only)",
    },
];

// 取り込んだ設定。解釈できなかった項目は `warnings` に残す
#[derive(Default)]
pub(crate) struct Config {
    pub(crate) show_cwd: bool,
    // configuration キー -> 画面に出すキー表記。書かれていない項目は持たない
    // ＝フッターのヒントからその項目だけが省かれる
    pub(crate) direct_keys: BTreeMap<String, String>,
    // 解釈できなかった設定のキー（`SETTINGS` の並び順）
    pub(crate) warnings: Vec<&'static str>,
}

impl Config {
    pub(crate) fn parse(configuration: &BTreeMap<String, String>) -> Self {
        let mut config = Config::default();
        for setting in &SETTINGS {
            let Some(value) = configuration.get(setting.key).map(|v| normalize_value(v)) else {
                continue;
            };
            // 空文字は未設定と同じ扱い。プロパティ書式の `up_key=""` も
            // 正規化後はここへ落ちる
            if value.is_empty() {
                continue;
            }
            match setting.kind {
                Kind::Flag => match value {
                    "true" => config.show_cwd = true,
                    "false" => {}
                    _ => config.warnings.push(setting.key),
                },
                Kind::DirectKey { .. } => {
                    // 解釈できない値は**そのまま出す**（要件: sidebar-footer）。
                    // 黙って落とすとヒントが1つ消えるだけになり、設定を
                    // 間違えたことに気づけない。警告はそれとは別に立てる
                    let shown = normalize_key(value).unwrap_or_else(|| {
                        config.warnings.push(setting.key);
                        value.to_string()
                    });
                    config.direct_keys.insert(setting.key.to_string(), shown);
                }
            }
        }
        config
    }
}

// 召喚インスタンスとして起動されたか（決定16）。内部用キーなので
// `SETTINGS` は通さないが、値の正規化だけは同じ入口に揃える
pub(crate) fn summoned(configuration: &BTreeMap<String, String>) -> bool {
    configuration
        .get(SUMMONED_KEY)
        .map(|v| normalize_value(v) == "true")
        .unwrap_or(false)
}

// KDL の書式差を吸収する（決定40）。前後の空白を落とし、プロパティ書式で
// 付いてくる引用符を剥がす。**受け口を広げるのはここまで** — `1` を真と
// 見なすような解釈は増やさない
pub(crate) fn normalize_value(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .map(|inner| inner.trim())
        .unwrap_or(trimmed)
}

// 設定で受け取ったキー表記を画面用に整える（決定28）。
// 解釈できなければ `None`（呼び出し側が生の値をそのまま出す）。
//
// 受けるのは zellij のキーバインド表記（`Alt u`）でも fujin の画面表記
// （`alt+u`）でもよい。ユーザーは config.kdl の `bind "Alt u"` からコピーする
// ことになるので、そのまま貼れないと使いにくい
pub(crate) fn normalize_key(raw: &str) -> Option<String> {
    // zellij のパーサは修飾キーを空白区切りで読む。`+` 区切りも受けたいので均す
    let spaced = raw.replace('+', " ");
    KeyWithModifier::from_str(&spaced)
        .ok()
        .map(|key| format_key(&key))
}

// キーの画面表記（docs/concept/ui-design.md の「文言」）。すべて小文字で、
// 修飾キーは `shift+tab` のように `+` でつなぐ。
//
// `KeyWithModifier` の Display は使えない — 修飾キーを空白でつなぐうえ、
// 大文字（`ESC`）や矢印（`↑`）を返す。矢印は東アジア文字幅が曖昧でキー列の
// 位置揃えを崩すので、fujin ではキー表記に使わない
fn format_key(key: &KeyWithModifier) -> String {
    let mut out = String::new();
    for modifier in &key.key_modifiers {
        out.push_str(&modifier.to_string().to_lowercase());
        out.push('+');
    }
    out.push_str(&format_bare_key(&key.bare_key));
    out
}

fn format_bare_key(key: &BareKey) -> String {
    match key {
        // 矢印・空白は Display が記号を返すので、英字表記へ置き換える
        BareKey::Left => "left".to_string(),
        BareKey::Right => "right".to_string(),
        BareKey::Up => "up".to_string(),
        BareKey::Down => "down".to_string(),
        BareKey::Char(' ') => "space".to_string(),
        BareKey::Char(c) => c.to_string(),
        // 残りは Display の綴りをそのまま小文字化すれば zellij の表記に揃う
        other => other.to_string().to_lowercase(),
    }
}

// --- README の設定節の生成（決定40「設定一覧の正本はコードにする」） ---
//
// 生成物を使うのは README 同期テストだけなので、本体（wasm）には積まない。
// 実行の入口は `make readme`（内部で `UPDATE_README=1` を立ててテストを回す）

#[cfg(test)]
pub(crate) mod doc {
    use super::{Kind, Setting, SETTINGS};

    // README は日英2つあり、原本は日本語（.claude/rules/readme.md）。
    // 表の中身だけが言語で変わる
    #[derive(Clone, Copy)]
    pub(crate) enum Lang {
        Ja,
        En,
    }

    // 生成範囲の目印。この2行に挟まれた部分をまるごと差し替える
    pub(crate) const BEGIN: &str = "<!-- settings:begin -->";
    pub(crate) const END: &str = "<!-- settings:end -->";

    // 設定節（KDL の例 + 一覧表）。前後の目印を含む
    pub(crate) fn settings_doc(lang: Lang) -> String {
        let mut out = String::from(BEGIN);
        out.push('\n');
        out.push_str(match lang {
            Lang::Ja => {
                "<!-- ここは repos/main/src/config.rs の SETTINGS から生成しています。\
                 手で直さず `make readme` を実行してください -->\n\n"
            }
            Lang::En => {
                "<!-- Generated from SETTINGS in repos/main/src/config.rs. \
                 Don't edit by hand; run `make readme` -->\n\n"
            }
        });
        out.push_str(&kdl_example());
        out.push('\n');
        out.push_str(&table(lang));
        out.push_str(END);
        out
    }

    // エイリアス定義に貼る KDL の例。設定キーと例示値はテーブルから引く
    fn kdl_example() -> String {
        let width = SETTINGS
            .iter()
            .map(|setting| setting.key.len())
            .max()
            .unwrap_or(0);
        let mut out = String::from("```kdl\nplugins {\n");
        out.push_str("    fujin location=\"file:~/.config/zellij/plugins/fujin.wasm\" {\n");
        for setting in &SETTINGS {
            out.push_str(&format!(
                "        {:width$} \"{}\"\n",
                setting.key,
                setting.example,
                width = width
            ));
        }
        out.push_str("    }\n}\n```\n");
        out
    }

    fn table(lang: Lang) -> String {
        let head = match lang {
            Lang::Ja => "| キー | 値 | 既定 | 説明 |\n| --- | --- | --- | --- |\n",
            Lang::En => "| Key | Value | Default | What it does |\n| --- | --- | --- | --- |\n",
        };
        let mut out = String::from(head);
        for setting in &SETTINGS {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                setting.key,
                value_column(setting, lang),
                default_column(setting, lang),
                match lang {
                    Lang::Ja => setting.summary_ja,
                    Lang::En => setting.summary_en,
                }
            ));
        }
        out.push('\n');
        out
    }

    fn value_column(setting: &Setting, lang: Lang) -> &'static str {
        match (setting.kind, lang) {
            (Kind::Flag, _) => "`\"true\"` / `\"false\"`",
            (Kind::DirectKey { .. }, Lang::Ja) => "キー表記（`\"Alt u\"` / `\"alt+u\"`）",
            (Kind::DirectKey { .. }, Lang::En) => "Key spelling (`\"Alt u\"` / `\"alt+u\"`)",
        }
    }

    fn default_column(setting: &Setting, lang: Lang) -> &'static str {
        match (setting.kind, lang) {
            (Kind::Flag, _) => "`false`",
            (Kind::DirectKey { .. }, Lang::Ja) => "未設定（そのヒントを出さない）",
            (Kind::DirectKey { .. }, Lang::En) => "unset (the hint is omitted)",
        }
    }

    // README の目印に挟まれた範囲を差し替える。目印が無ければ `None`
    pub(crate) fn splice(readme: &str, section: &str) -> Option<String> {
        let begin = readme.find(BEGIN)?;
        let end = readme.find(END)? + END.len();
        let mut out = String::with_capacity(readme.len());
        out.push_str(&readme[..begin]);
        out.push_str(section);
        out.push_str(&readme[end..]);
        Some(out)
    }
}
