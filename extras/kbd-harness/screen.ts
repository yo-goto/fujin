#!/usr/bin/env -S deno run --allow-read
// harness.py が落とした out.log を再生して、「最後に画面に出ていた内容」を復元する。
//
// **なぜ要るか。** out.log は pty へ流れたバイト列そのままで、zellij も Claude Code も
// 絶対座標指定（CSI row;col H）で画面を描き直す。エスケープを落とすだけのフィルタでは
// 行の順序も改行も無いテキストが1行に潰れるだけで、サイドバーの状態アイコンのような
// 「画面のどこに何が出ているか」は読めない。座標を解釈してグリッドへ書き戻す必要がある。
//
// 実装するのは**カーソル移動と消去だけ**。色・属性は捨てる（読みたいのは文字であって
// 見た目ではない。色の判断は人に頼む方針 — .docs/dev/build-and-test.md）。
//
//   deno run --allow-read screen.ts <out.log> [--rows 45] [--cols 160]
//                                     [--before-exit] [--upto TEXT]
//
// 既定の 45x160 は harness.py が TIOCSWINSZ で固定している大きさに合わせてある。
//
// **畳んだあとのログを読むときは `--before-exit` を付ける。** zellij は終了時に
// 代替画面バッファから出るので、素直に最後まで再生すると復元されるのは「その下に
// あったシェルの画面」になる（実際の端末と同じ挙動）。`--before-exit` は代替画面を
// 出る指示が来た時点で再生を打ち切るので、畳む直前の画面がそのまま出る。
//
// `--upto TEXT` は「その文字列が最初に現れた瞬間の画面」を出す。中断・エラー表示が
// 出た時点まで巻き戻したいときに使う（例: `--upto Interrupted`）。

const usage = `usage: screen.ts <out.log> [--rows N] [--cols N] [--upto TEXT]

Replays a kbd-harness out.log and prints the final screen contents.
--before-exit stops at the point the app leaves the alternate screen buffer, so
a torn-down session still shows its last real screen.
--upto stops just before the first occurrence of TEXT.`;

// ---------------------------------------------------------------- 引数

const args = [...Deno.args];
let rows = 45;
let cols = 160;
let upto = "";
let beforeExit = false;
let path = "";
while (args.length > 0) {
  const a = args.shift() as string;
  if (a === "--rows") rows = Number(args.shift());
  else if (a === "--cols") cols = Number(args.shift());
  else if (a === "--upto") upto = args.shift() ?? "";
  else if (a === "--before-exit") beforeExit = true;
  else if (a === "-h" || a === "--help") {
    console.log(usage);
    Deno.exit(0);
  } else if (path === "") path = a;
  else {
    console.error(`screen.ts: unexpected argument: ${a}`);
    Deno.exit(2);
  }
}
if (path === "" || !Number.isFinite(rows) || !Number.isFinite(cols)) {
  console.error(usage);
  Deno.exit(2);
}

// ---------------------------------------------------------------- 文字幅

// 全角（2桁）と結合文字（0桁）だけ判別できればよい。East Asian Width の完全な表は
// 持たず、罫線・ボックス描画に使う範囲を巻き込まないよう必要な区間だけ挙げる
const WIDE: [number, number][] = [
  [0x1100, 0x115f], // ハングル字母
  [0x2e80, 0x303e], // CJK 部首・記号
  [0x3041, 0x33ff], // かな・ハングル・CJK 互換
  [0x3400, 0x4dbf],
  [0x4e00, 0x9fff], // CJK 統合漢字
  [0xa000, 0xa4cf],
  [0xac00, 0xd7a3], // ハングル音節
  [0xf900, 0xfaff],
  [0xfe30, 0xfe6f],
  [0xff00, 0xff60], // 全角英数・記号
  [0xffe0, 0xffe6],
  [0x1f300, 0x1f64f], // 絵文字
  [0x1f900, 0x1f9ff],
  [0x20000, 0x3fffd],
];
const ZERO: [number, number][] = [
  [0x0300, 0x036f],
  [0x200b, 0x200f],
  [0xfe00, 0xfe0f], // 異体字セレクタ
  [0xfe20, 0xfe2f],
];
const inRanges = (cp: number, rs: [number, number][]) =>
  rs.some(([lo, hi]) => cp >= lo && cp <= hi);
const charWidth = (ch: string): number => {
  const cp = ch.codePointAt(0) ?? 0;
  if (inRanges(cp, ZERO)) return 0;
  return inRanges(cp, WIDE) ? 2 : 1;
};

// ---------------------------------------------------------------- 画面

// 全角文字は左のセルへ字を入れ、右のセルに "" を置いて桁を食わせる。
// 出力時に "" は何も足さないので、そのまま桁が揃う
const blank = () => Array<string>(cols).fill(" ");
let grid: string[][] = Array.from({ length: rows }, blank);
let cy = 0, cx = 0;
let savedCursor: [number, number] = [0, 0];
// 代替画面バッファ。入るときに主画面を退避し、出るときに戻す（実際の端末と同じ）
let primary: { grid: string[][]; cy: number; cx: number } | null = null;
// スクロール領域（DECSTBM）。既定は画面全体
let top = 0, bottom = rows - 1;

const clampCursor = () => {
  cy = Math.max(0, Math.min(rows - 1, cy));
  cx = Math.max(0, Math.min(cols - 1, cx));
};
const clearCell = (y: number, x: number) => {
  grid[y][x] = " ";
};
const scrollUp = (n = 1) => {
  for (let i = 0; i < n; i++) {
    grid.splice(top, 1);
    grid.splice(bottom, 0, blank());
  }
};
const scrollDown = (n = 1) => {
  for (let i = 0; i < n; i++) {
    grid.splice(bottom, 1);
    grid.splice(top, 0, blank());
  }
};
const lineFeed = () => {
  if (cy === bottom) scrollUp();
  else if (cy < rows - 1) cy++;
};
const put = (ch: string) => {
  const w = charWidth(ch);
  if (w === 0) return; // 結合文字は落とす（幅に影響しない）
  if (cx + w > cols) {
    cx = 0;
    lineFeed();
  }
  // 上書き先が全角の右半分なら、相方も消して半端な残骸を作らない
  if (grid[cy][cx] === "" && cx > 0) grid[cy][cx - 1] = " ";
  if (w === 2 && grid[cy][cx + 1] === "") grid[cy][cx + 2] = " ";
  grid[cy][cx] = ch;
  if (w === 2) grid[cy][cx + 1] = "";
  cx += w;
};

// ---------------------------------------------------------------- 再生

let s = new TextDecoder("utf-8").decode(Deno.readFileSync(path));
if (upto !== "") {
  const at = s.indexOf(upto);
  if (at < 0) {
    console.error(`screen.ts: --upto text not found: ${upto}`);
    Deno.exit(1);
  }
  s = s.slice(0, at);
}
const chars = [...s]; // コードポイント単位。サロゲートペアを割らない

let i = 0;
while (i < chars.length) {
  const c = chars[i];

  if (c === "\x1b") {
    const next = chars[i + 1];

    if (next === "[") {
      // CSI: パラメータ（数字・;）と中間バイト（?<>=! 空白）を読み飛ばして終端を見る
      let j = i + 2;
      while (j < chars.length && /[0-9;?<>=! ]/.test(chars[j])) j++;
      const fin = chars[j];
      const raw = chars.slice(i + 2, j).join("");
      const priv = /[?<>=!]/.test(raw);
      const nums = raw.replace(/[?<>=! ]/g, "").split(";").map((p) =>
        p === "" ? 0 : Number(p)
      );
      const n1 = nums[0] || 0;

      if (priv && (fin === "h" || fin === "l")) {
        const alt = nums.includes(1049) || nums.includes(47) ||
          nums.includes(1047);
        if (alt && fin === "h" && primary === null) {
          primary = { grid, cy, cx };
          grid = Array.from({ length: rows }, blank);
          cy = 0;
          cx = 0;
        } else if (alt && fin === "l" && primary !== null) {
          if (beforeExit) break; // 畳む直前の画面がほしいので、ここで再生を止める
          ({ grid, cy, cx } = primary);
          primary = null;
        }
      } else if (fin === "H" || fin === "f") {
        cy = (nums[0] || 1) - 1;
        cx = (nums[1] || 1) - 1;
        clampCursor();
      } else if (fin === "A") {
        cy -= n1 || 1;
        clampCursor();
      } else if (fin === "B") {
        cy += n1 || 1;
        clampCursor();
      } else if (fin === "C") {
        cx += n1 || 1;
        clampCursor();
      } else if (fin === "D") {
        cx -= n1 || 1;
        clampCursor();
      } else if (fin === "E") {
        cy += n1 || 1;
        cx = 0;
        clampCursor();
      } else if (fin === "F") {
        cy -= n1 || 1;
        cx = 0;
        clampCursor();
      } else if (fin === "G" || fin === "`") {
        cx = (n1 || 1) - 1;
        clampCursor();
      } else if (
        fin === "d"
      ) {
        cy = (n1 || 1) - 1;
        clampCursor();
      } else if (fin === "K") {
        if (n1 === 0) { for (let x = cx; x < cols; x++) clearCell(cy, x); }
        else if (n1 === 1) { for (let x = 0; x <= cx; x++) clearCell(cy, x); }
        else for (let x = 0; x < cols; x++) clearCell(cy, x);
      } else if (fin === "J") {
        if (n1 === 0) {
          for (let x = cx; x < cols; x++) clearCell(cy, x);
          for (let y = cy + 1; y < rows; y++) grid[y] = blank();
        } else if (n1 === 1) {
          for (let y = 0; y < cy; y++) grid[y] = blank();
          for (let x = 0; x <= cx; x++) clearCell(cy, x);
        } else {
          grid = Array.from({ length: rows }, blank);
        }
      } else if (fin === "L") { // IL: 行を挿入
        for (let k = 0; k < (n1 || 1); k++) {
          grid.splice(bottom, 1);
          grid.splice(cy, 0, blank());
        }
      } else if (fin === "M") { // DL: 行を削除
        for (let k = 0; k < (n1 || 1); k++) {
          grid.splice(cy, 1);
          grid.splice(bottom, 0, blank());
        }
      } else if (fin === "@") { // ICH: 桁を挿入
        for (let k = 0; k < (n1 || 1); k++) {
          grid[cy].splice(cols - 1, 1);
          grid[cy].splice(cx, 0, " ");
        }
      } else if (fin === "P") { // DCH: 桁を削除
        for (let k = 0; k < (n1 || 1); k++) {
          grid[cy].splice(cx, 1);
          grid[cy].push(" ");
        }
      } else if (fin === "X") { // ECH: 桁を空白で潰す
        for (let x = cx; x < Math.min(cols, cx + (n1 || 1)); x++) {
          clearCell(cy, x);
        }
      } else if (fin === "S") scrollUp(n1 || 1);
      else if (fin === "T") scrollDown(n1 || 1);
      else if (fin === "r") {
        top = (nums[0] || 1) - 1;
        bottom = (nums[1] || rows) - 1;
        if (top < 0) top = 0;
        if (bottom > rows - 1) bottom = rows - 1;
        if (top >= bottom) {
          top = 0;
          bottom = rows - 1;
        }
        cy = top;
        cx = 0;
      }
      // それ以外（SGR 等の見た目）は捨てる
      i = j + 1;
      continue;
    }

    if (next === "]") {
      // OSC: BEL か ST（ESC \）まで読み飛ばす。zellij のタイトル・ハイパーリンク
      let j = i + 2;
      while (
        j < chars.length && chars[j] !== "\x07" &&
        !(chars[j] === "\x1b" && chars[j + 1] === "\\")
      ) j++;
      i = chars[j] === "\x07" ? j + 1 : j + 2;
      continue;
    }

    if (next === "P" || next === "X" || next === "^" || next === "_") {
      // DCS / SOS / PM / APC: ST まで
      let j = i + 2;
      while (
        j < chars.length && !(chars[j] === "\x1b" && chars[j + 1] === "\\")
      ) j++;
      i = j + 2;
      continue;
    }

    if (next === "M") { // RI: 逆改行
      if (cy === top) scrollDown();
      else cy = Math.max(0, cy - 1);
      i += 2;
      continue;
    }
    if (next === "D") {
      lineFeed();
      i += 2;
      continue;
    }
    if (next === "E") {
      cx = 0;
      lineFeed();
      i += 2;
      continue;
    }
    if (next === "7") {
      savedCursor = [cy, cx];
      i += 2;
      continue;
    }
    if (next === "8") {
      [cy, cx] = savedCursor;
      clampCursor();
      i += 2;
      continue;
    }
    if (next === "c") { // RIS: 端末リセット
      grid = Array.from({ length: rows }, blank);
      cy = 0;
      cx = 0;
      top = 0;
      bottom = rows - 1;
      i += 2;
      continue;
    }
    i += 2; // 残り（文字集合指定など2バイトのもの）は読み飛ばす
    continue;
  }

  if (c === "\n") {
    lineFeed();
    i++;
    continue;
  }
  if (c === "\r") {
    cx = 0;
    i++;
    continue;
  }
  if (c === "\b") {
    cx = Math.max(0, cx - 1);
    i++;
    continue;
  }
  if (c === "\t") {
    cx = Math.min(cols - 1, (Math.floor(cx / 8) + 1) * 8);
    i++;
    continue;
  }
  if (c < " " || c === "\x7f") {
    i++;
    continue;
  }

  put(c);
  i++;
}

console.log(
  grid.map((row) => row.join("").replace(/\s+$/, "")).join("\n"),
);
