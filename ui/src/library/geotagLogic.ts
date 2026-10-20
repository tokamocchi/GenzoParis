// 位置情報の後付け（LIB-21。ジオタグ付け）の画面の計算（DOM・地図のライブラリを使わない純粋な計算。docs/04 の
// 3.16 節）。
//
// - 座標の文字列の解釈（parseCoordinates）。genzo-model の location.rs の parse_coordinates と同じ形を同じ規則で
//   受け付ける（規則を変えるときは両方を直し、テストのケースもそろえる。geotagLogic.test.ts は location.rs の
//   tests の写し）。読むのはこのパソコンの中だけで、外部には送らない。
// - 見込み・結果の文（describeLocationPlan）・対象の内訳（describeTargets）・履歴の文（describeLocationEdit）・
//   取り消しの文（describeLocationUndo）・位置の出どころの文（locationSummary・gpsSourceNote）。
//
// 受け付ける形（location.rs のモジュールの doc と同じ。例は東京駅の付近）:
//
// 1. 10 進の度: "35.681236, 139.767125"、"-33.8568 151.2153"。区切りはカンマ・セミコロン・読点「、」（前後の
//    空白は任意）か、空白だけ。符号（+・-）と度の記号（"35.681236°"）は任意。小数の桁は制限しない。
// 2. 方角の記号: "35.681236N 139.767125E"、"N35.681236 E139.767125"、"35.681236° N, 139.767125° E"（小文字も）。
//    S・W は負。経度の記号が先なら入れ替える（"139.767125E 35.681236N"）。
// 3. 日本語: "北緯 35.681236°、東経 139.767125°"（metadataLogic.ts の formatGps の出力）。南緯・西経は負。
// 4. 度分秒: `35°40'52.4"N 139°46'01.7"E`、"35°40′52.4″N 139°46′1.7″E"、度分 "35°40.873'N"。記号のない度分秒
//    （"35 40 52.4 N 139 46 1.7 E"）は方角の記号があるときだけ。度は整数、分は 0 以上 60 未満（秒があれば整数）、
//    秒は 0 以上 60 未満。
// 5. Google マップの URL（http://・https:// で始まるもの）: クエリの q=緯度,経度、ll=緯度,経度、パスの /data= より
//    後ろの !3d緯度!4d経度（場所の URL のピンの位置。"https://www.google.com/maps/place/…/@35.6812362,139.7645445,17z/
//    data=…!8m2!3d35.6812362!4d139.7671248" なら 35.6812362, 139.7671248）、パスの @緯度,経度
//    （"https://www.google.com/maps/@35.681236,139.767125,17z"）の順に探す。q・ll の値は %XX と +（空白）を戻して
//    から読む。!3d は直後の数にすぐ !4d と数が続く組だけを読む（経路の URL の !1d経度!2d緯度 は読まない）。@ の値は
//    地図の中心と考えられ、ピンの位置とずれることがある（Google の資料では確かめていない）。
//
// 読む前に、全角の数字・「，」「．」「－」「＋」・全角の空白を半角にし、前後の空白を除く。細かい規則（区切りの
// 記号がないときの方角の記号の付け方、誤りの順など）は location.rs のモジュールの doc を参照。

import type {
  GpsCoord,
  LocationEditInfo,
  LocationEditKind,
  LocationEditPlan,
  LocationUndo,
  VariantDetails,
} from "../api/types";
import { formatGps } from "./metadataLogic";

/** 位置の変更の履歴を残す数（genzo-catalog の MAX_LOCATION_EDITS と同じ。仮置き）。 */
export const MAX_LOCATION_EDITS = 20;

// ---------------------------------------------------------------------------
// 座標の文字列の解釈（genzo-model の location.rs と同じ規則）
// ---------------------------------------------------------------------------

/** 座標の文字列を読めないときの理由（genzo-model の CoordParseError と同じ種類。説明は coordErrorMessage）。 */
export type CoordParseError =
  /** 空（空白だけ）。 */
  | { kind: "empty" }
  /** 座標の形として読めない。 */
  | { kind: "unreadable" }
  /** URL に座標が見つからない。 */
  | { kind: "no_coordinates_in_url" }
  /** 度分秒の値が正しくない（度・分が整数でない、分・秒が 0 以上 60 未満でない）。 */
  | { kind: "invalid_dms" }
  /** 符号と方角の記号を同じ数に付けた。 */
  | { kind: "sign_with_direction" }
  /** 方角の記号が矛盾している（緯度どうし・経度どうし）。 */
  | { kind: "conflicting_directions" }
  /** 緯度・経度が範囲外（緯度は -90〜90、経度は -180〜180）。値は読んだ値。 */
  | { kind: "latitude_out_of_range"; value: number }
  | { kind: "longitude_out_of_range"; value: number };

export type CoordParseResult = { ok: true; coord: GpsCoord } | { ok: false; error: CoordParseError };

/**
 * 誤りの説明（genzo-model の CoordParseError の表示と同じ文。範囲外の値は JavaScript の数の書き方なので、
 * 無限や桁の多い値だけは Rust の表示と違う）。
 */
export function coordErrorMessage(e: CoordParseError): string {
  switch (e.kind) {
    case "empty":
      return "座標が空です";
    case "unreadable":
      return "座標として読めません（例: 35.681236, 139.767125）";
    case "no_coordinates_in_url":
      return (
        "URL から座標を読めません（Google マップの URL の q=緯度,経度・ll=緯度,経度・場所の URL のピン" +
        "（data=…!3d緯度!4d経度）・@緯度,経度 を読みます）"
      );
    case "invalid_dms":
      return "度分秒の値が正しくありません（度は整数、分・秒は 0 以上 60 未満。秒があれば分も整数）";
    case "sign_with_direction":
      return "符号（+・-）と方角の記号（N・S・E・W・北緯など）は一緒に使えません";
    case "conflicting_directions":
      return "方角の記号が矛盾しています（緯度（N・S）と経度（E・W）を 1 つずつ付けてください）";
    case "latitude_out_of_range":
      return `緯度が範囲外です（-90〜90）: ${e.value}`;
    case "longitude_out_of_range":
      return `経度が範囲外です（-180〜180）: ${e.value}`;
  }
}

/**
 * 座標の文字列を解釈する（受け付ける形はファイルの冒頭）。読めた座標は緯度 -90〜90・経度 -180〜180 の有限の
 * 値で、-0 は 0 にする。
 */
export function parseCoordinates(text: string): CoordParseResult {
  try {
    const s = trim(normalize(text));
    if (s === "") return { ok: false, error: { kind: "empty" } };
    const [lat, lon] = isUrl(s) ? parseUrl(s) : parseText(s);
    if (!(Number.isFinite(lat) && lat >= -90 && lat <= 90)) {
      return { ok: false, error: { kind: "latitude_out_of_range", value: lat } };
    }
    if (!(Number.isFinite(lon) && lon >= -180 && lon <= 180)) {
      return { ok: false, error: { kind: "longitude_out_of_range", value: lon } };
    }
    // -0 + 0 は 0。
    return { ok: true, coord: { lat: lat + 0, lon: lon + 0 } };
  } catch (e) {
    if (e instanceof Failure) return { ok: false, error: e.error };
    throw e;
  }
}

/** 読めないとき（Rust の `?` の代わりに投げて、parseCoordinates で受ける）。 */
class Failure {
  constructor(readonly error: CoordParseError) {}
}

function fail(error: CoordParseError): never {
  throw new Failure(error);
}

const UNREADABLE: CoordParseError = { kind: "unreadable" };

/**
 * 空白か（Rust の char::is_whitespace と同じ、Unicode の White_Space）。JavaScript の \s・trim とは U+0085（White_Space
 * だが \s ではない）と U+FEFF（\s だが White_Space ではない）が違うので、自前で判定する。
 */
const WHITE_SPACE = /^[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]$/;

function isWhitespace(c: string | undefined): boolean {
  return c !== undefined && WHITE_SPACE.test(c);
}

function trimStart(s: string): string {
  let i = 0;
  while (isWhitespace(s[i])) i++;
  return s.slice(i);
}

function trim(s: string): string {
  let end = s.length;
  while (end > 0 && isWhitespace(s[end - 1])) end--;
  return trimStart(s.slice(0, end));
}

function isDigit(c: string | undefined): boolean {
  return c !== undefined && c >= "0" && c <= "9";
}

/** 全角の数字・「，」「．」「－」「＋」・全角の空白を半角にする。 */
function normalize(text: string): string {
  return text.replace(/[\uff10-\uff19\uff0c\uff0e\uff0d\uff0b\u3000]/g, (c) => {
    switch (c) {
      case "\uff0c":
        return ",";
      case "\uff0e":
        return ".";
      case "\uff0d":
        return "-";
      case "\uff0b":
        return "+";
      case "\u3000":
        return " ";
      default:
        return String.fromCharCode(c.charCodeAt(0) - 0xfee0);
    }
  });
}

/** http://・https:// で始まるか（大文字・小文字は問わない）。 */
function isUrl(s: string): boolean {
  return /^https?:\/\//i.test(s);
}

/** URL の q=・ll=・場所のピン（/data= より後ろの !3d緯度!4d経度）・@ から（緯度, 経度）を読む。範囲は検査しない。 */
function parseUrl(full: string): [number, number] {
  const hash = full.indexOf("#");
  const url = hash >= 0 ? full.slice(0, hash) : full;
  const question = url.indexOf("?");
  const path = question >= 0 ? url.slice(0, question) : url;
  const query = question >= 0 ? url.slice(question + 1) : "";
  for (const key of ["q", "ll"]) {
    for (const pair of query.split("&")) {
      const eq = pair.indexOf("=");
      const [k, v] = eq >= 0 ? [pair.slice(0, eq), pair.slice(eq + 1)] : [pair, ""];
      if (k !== key) continue;
      const decoded = percentDecode(v);
      const found = decoded === null ? null : decimalPairPrefix(trim(decoded));
      if (found && trim(found.rest) === "") return [found.lat, found.lon];
    }
  }
  // 場所の URL（/maps/place/…）のピンの位置。@ は地図の中心で、ピンとずれることがあるので、先に見る。
  const data = path.indexOf("/data=");
  const pin = data >= 0 ? pinInData(path.slice(data + "/data=".length)) : null;
  if (pin) return pin;
  // パスの `@緯度,経度`（後ろに `,17z` などが続く）。
  for (let i = path.indexOf("@"); i >= 0; i = path.indexOf("@", i + 1)) {
    const found = decimalPairPrefix(path.slice(i + 1));
    if (found && (found.rest === "" || found.rest.startsWith(",") || found.rest.startsWith("/"))) {
      return [found.lat, found.lon];
    }
  }
  return fail({ kind: "no_coordinates_in_url" });
}

/**
 * Google マップの URL のパスの /data= より後ろ（! で始まる項目の並び）から、場所のピンの（緯度, 経度）を読む
 * （Rust の pin_in_data と同じ）。
 *
 * !3d の直後の数に、すぐ !4d と数が続く組（後ろは !・/ か終わり）だけを読む。そうでない !3d は飛ばして次を探し、
 * 組が 2 つ以上あれば最初の組。経路の URL（/maps/dir/）の地点の !1d経度!2d緯度 は読まない。範囲は検査しない。
 */
function pinInData(data: string): [number, number] | null {
  for (let i = data.indexOf("!3d"); i >= 0; i = data.indexOf("!3d", i + 1)) {
    const lat = data.slice(i + "!3d".length);
    const latEnd = numberEnd(lat, 0);
    if (latEnd === null || !lat.startsWith("!4d", latEnd)) continue;
    const lon = lat.slice(latEnd + "!4d".length);
    const lonEnd = numberEnd(lon, 0);
    if (lonEnd === null) continue;
    const rest = lon.slice(lonEnd);
    if (rest !== "" && !rest.startsWith("!") && !rest.startsWith("/")) continue;
    return [Number(lat.slice(0, latEnd)), Number(lon.slice(0, lonEnd))];
  }
  return null;
}

/**
 * URL のクエリの値の %XX と +（空白）を戻す。% の後ろが 16 進の 2 桁でない・UTF-8 でなければ null（Rust の
 * percent_decode と同じ。decodeURIComponent はどちらも URIError にする）。
 */
function percentDecode(s: string): string | null {
  try {
    return decodeURIComponent(s.replace(/\+/g, " "));
  } catch {
    return null;
  }
}

/** 先頭の `緯度,経度`（10 進の度。符号は任意、カンマの前後の空白は任意）を読み、値と残りを返す。 */
function decimalPairPrefix(s: string): { lat: number; lon: number; rest: string } | null {
  const aEnd = numberEnd(s, 0);
  if (aEnd === null) return null;
  const afterA = trimStart(s.slice(aEnd));
  if (!afterA.startsWith(",")) return null;
  const rest = trimStart(afterA.slice(1));
  const bEnd = numberEnd(rest, 0);
  if (bEnd === null) return null;
  return { lat: Number(s.slice(0, aEnd)), lon: Number(rest.slice(0, bEnd)), rest: rest.slice(bEnd) };
}

/** `start` から始まる数（`[+-]?[0-9]+(\.[0-9]+)?`）の終わりの位置。数でなければ null（"35."・".5" は読まない）。 */
function numberEnd(s: string, start: number): number | null {
  const digits = (from: number) => {
    let n = 0;
    while (isDigit(s[from + n])) n++;
    return n;
  };
  let i = start;
  if (s[i] === "+" || s[i] === "-") i++;
  const n = digits(i);
  if (n === 0) return null;
  i += n;
  if (s[i] === ".") {
    const m = digits(i + 1);
    if (m === 0) return null;
    i += 1 + m;
  }
  return i;
}

/** 緯度か経度か。 */
type Axis = "lat" | "lon";

/** 方角の記号（N・S・E・W・北緯・南緯・東経・西経）。`negative` は S・W・南緯・西経（負の側）。 */
interface Direction {
  axis: Axis;
  negative: boolean;
}

/** 字句の種類（num は符号を含む数の文字列、deg は「°」、min は「'」「′」、sec は「"」「″」、sep は区切りの記号）。 */
type Kind =
  | { t: "num"; text: string }
  | { t: "deg" }
  | { t: "min" }
  | { t: "sec" }
  | { t: "dir"; dir: Direction }
  | { t: "sep" };

/** 字句（`spaceBefore` は直前に空白があったか）。 */
interface Token {
  kind: Kind;
  spaceBefore: boolean;
}

/** 角度（符号を含む最初の数 d と、分 m・秒 s。どれも数の文字列）。 */
type Angle =
  | { d: string; m?: undefined; s?: undefined }
  | { d: string; m: string; s?: undefined }
  | { d: string; m: string; s: string };

/** 緯度か経度の片方（方角の記号と角度）。 */
interface Part {
  dir: Direction | null;
  angle: Angle;
}

const signed = (num: string) => num.startsWith("+") || num.startsWith("-");
const isInteger = (num: string) => !num.includes(".");

/** 符号を除いた値。桁が多すぎれば無限（範囲の検査で落とす）。 */
function magnitude(num: string): number {
  const v = Number(num.replace(/^[+-]+/, ""));
  return Number.isNaN(v) ? Infinity : v;
}

/** 字句に分ける。知らない文字があれば読めない。 */
function tokenize(s: string): Token[] {
  const dir = (axis: Axis, negative: boolean): Kind => ({ t: "dir", dir: { axis, negative } });
  const tokens: Token[] = [];
  let spaceBefore = false;
  let pos = 0;
  while (pos < s.length) {
    const start = pos;
    const c = String.fromCodePoint(s.codePointAt(pos)!);
    pos += c.length;
    if (isWhitespace(c)) {
      spaceBefore = true;
      continue;
    }
    let kind: Kind;
    switch (c) {
      case ",":
      case ";":
      case "、":
        kind = { t: "sep" };
        break;
      case "°":
        kind = { t: "deg" };
        break;
      case "'":
      case "′":
        kind = { t: "min" };
        break;
      case '"':
      case "″":
        kind = { t: "sec" };
        break;
      case "N":
      case "n":
        kind = dir("lat", false);
        break;
      case "S":
      case "s":
        kind = dir("lat", true);
        break;
      case "E":
      case "e":
        kind = dir("lon", false);
        break;
      case "W":
      case "w":
        kind = dir("lon", true);
        break;
      case "北":
      case "南":
      case "東":
      case "西": {
        const lat = c === "北" || c === "南";
        if (s[pos] !== (lat ? "緯" : "経")) return fail(UNREADABLE);
        pos += 1;
        kind = dir(lat ? "lat" : "lon", c === "南" || c === "西");
        break;
      }
      default: {
        if (!(c === "+" || c === "-" || isDigit(c))) return fail(UNREADABLE);
        const end = numberEnd(s, start);
        if (end === null) return fail(UNREADABLE);
        pos = end;
        kind = { t: "num", text: s.slice(start, end) };
      }
    }
    tokens.push({ kind, spaceBefore });
    spaceBefore = false;
  }
  return tokens;
}

/** 字句の数の文字列（数でなければ null）。 */
function numAt(tokens: readonly Token[], i: number): string | null {
  const k = tokens[i]?.kind;
  return k?.t === "num" ? k.text : null;
}

function isAt(tokens: readonly Token[], i: number, t: Kind["t"]): boolean {
  return tokens[i]?.kind.t === t;
}

/**
 * 角度の字句の並び（全体）を読む: [数]・[数 °]・[数 ° 数 ']・[数 ° 数 ' 数 "]。記号のない度分 [数 数]・度分秒
 * [数 数 数] は `hasDir` のときだけ。2 つ目以降の数に符号があれば読まない。
 */
function angle(tokens: readonly Token[], hasDir: boolean): Angle | null {
  const d = numAt(tokens, 0);
  if (d === null) return null;
  const n = tokens.length;
  const [n1, n2, n4] = [numAt(tokens, 1), numAt(tokens, 2), numAt(tokens, 4)];
  let a: Angle;
  if (n === 1 || (n === 2 && isAt(tokens, 1, "deg"))) {
    a = { d };
  } else if (n === 4 && isAt(tokens, 1, "deg") && n2 !== null && isAt(tokens, 3, "min")) {
    a = { d, m: n2 };
  } else if (n === 6 && isAt(tokens, 1, "deg") && n2 !== null && isAt(tokens, 3, "min") && n4 !== null && isAt(tokens, 5, "sec")) {
    a = { d, m: n2, s: n4 };
  } else if (n === 2 && hasDir && n1 !== null) {
    a = { d, m: n1 };
  } else if (n === 3 && hasDir && n1 !== null && n2 !== null) {
    a = { d, m: n1, s: n2 };
  } else {
    return null;
  }
  const laterSigned = (a.m !== undefined && signed(a.m)) || (a.s !== undefined && signed(a.s));
  return laterSigned ? null : a;
}

/** 緯度か経度の片方の字句の並び（全体）を読む。方角の記号は数の前か後に 1 つまで。 */
function part(tokens: readonly Token[]): Part {
  let dir: Direction | null = null;
  let rest = tokens;
  const first = tokens[0]?.kind;
  const last = tokens[tokens.length - 1]?.kind;
  if (first?.t === "dir") {
    dir = first.dir;
    rest = tokens.slice(1);
  } else if (last?.t === "dir") {
    dir = last.dir;
    rest = tokens.slice(0, -1);
  }
  const a = angle(rest, dir !== null);
  if (a === null) return fail(UNREADABLE);
  return { dir, angle: a };
}

/** 記号の形の角度（"35"・"35°"・"35°40'"・`35°40'52"`）を先頭から最も長く取ったときの字句の数。 */
function symbolicLen(tokens: readonly Token[]): number {
  if (!isAt(tokens, 0, "num")) return 0;
  if (!isAt(tokens, 1, "deg")) return 1;
  if (!(isAt(tokens, 2, "num") && isAt(tokens, 3, "min"))) return 2;
  if (!(isAt(tokens, 4, "num") && isAt(tokens, 5, "sec"))) return 4;
  return 6;
}

/** 字句の並びを緯度・経度の 2 つ（並びは入力の順）に分けて読む。 */
function split(tokens: readonly Token[]): [Part, Part] {
  const positions = (t: Kind["t"]) => tokens.flatMap((tok, i) => (tok.kind.t === t ? [i] : []));
  const seps = positions("sep");
  // 区切りの記号の前と後。
  if (seps.length === 1) return [part(tokens.slice(0, seps[0])), part(tokens.slice(seps[0] + 1))];
  if (seps.length > 1) return fail(UNREADABLE);
  const dirs = positions("dir");
  let at: number;
  if (dirs.length === 2 && dirs[0] === 0) {
    // 方角の記号が両方とも数の前（"N35 E139"）: 2 つ目の記号の前で分ける。
    at = dirs[1];
  } else if (dirs.length === 2 && dirs[1] + 1 === tokens.length) {
    // 両方とも数の後（"35N 139E"）: 1 つ目の記号の後で分ける。
    at = dirs[0] + 1;
  } else if (dirs.length === 0) {
    // 方角の記号なし: 1 つ目の角度を記号の形で最も長く取る。2 つ目との間に空白が要る。
    at = symbolicLen(tokens);
    if (!tokens[at]?.spaceBefore) return fail(UNREADABLE);
  } else {
    return fail(UNREADABLE);
  }
  return [part(tokens.slice(0, at)), part(tokens.slice(at))];
}

/** 符号付きの値（度）。度分・度分秒の値が正しくなければ invalid_dms。 */
function partValue(p: Part): number {
  const inMinute = (num: string) => {
    const v = magnitude(num);
    return v >= 0 && v < 60;
  };
  const { d, m, s } = p.angle;
  let value: number;
  if (m === undefined) {
    value = magnitude(d);
  } else if (s === undefined) {
    if (!(isInteger(d) && inMinute(m))) return fail({ kind: "invalid_dms" });
    value = magnitude(d) + magnitude(m) / 60;
  } else {
    if (!(isInteger(d) && isInteger(m) && inMinute(m) && inMinute(s))) return fail({ kind: "invalid_dms" });
    value = magnitude(d) + magnitude(m) / 60 + magnitude(s) / 3600;
  }
  const negative = d.startsWith("-") || (p.dir?.negative ?? false);
  return negative ? -value : value;
}

/** URL でない文字列から（緯度, 経度）を読む。範囲は検査しない。 */
function parseText(s: string): [number, number] {
  const [first, second] = split(tokenize(s));
  if ([first, second].some((p) => p.dir !== null && signed(p.angle.d))) return fail({ kind: "sign_with_direction" });
  const a1 = first.dir?.axis ?? null;
  const a2 = second.dir?.axis ?? null;
  if (a1 !== null && a1 === a2) return fail({ kind: "conflicting_directions" });
  const swap = a1 === "lon" || a2 === "lat";
  const a = partValue(first);
  const b = partValue(second);
  return swap ? [b, a] : [a, b];
}

// ---------------------------------------------------------------------------
// 文
// ---------------------------------------------------------------------------

const count = (v: number) => v.toLocaleString("ja-JP");

/** 除いた動画・見つからない写真の文（見込みと結果の共通）。 */
function excludedLines(p: LocationEditPlan, done: boolean): string[] {
  const lines: string[] = [];
  if (p.skipped_videos > 0) {
    lines.push(done ? `動画 ${count(p.skipped_videos)} 本は除きました。` : `動画 ${count(p.skipped_videos)} 本は除きます（動画には位置を付けません）。`);
  }
  if (p.missing > 0) lines.push(`見つからない ${count(p.missing)} 枚は除き${done ? "ました" : "ます"}（削除された写真など）。`);
  return lines;
}

/**
 * 見込み・結果の主な文。`kind` は付ける（set）か、付けた位置を消す（clear）か。結果（`done`）は知らせに出すので
 * 短くする。
 */
export function describeLocationPlan(p: LocationEditPlan, kind: LocationEditKind, done: boolean): string[] {
  const lines: string[] = [];
  if (p.photos === 0) {
    lines.push("対象の写真がありません。");
  } else if (kind === "set") {
    if (p.changed === 0) lines.push(`位置の変わる写真はありません（対象 ${count(p.photos)} 枚。すでに同じ位置が付いています）。`);
    else lines.push(`${count(p.changed)} 枚に位置を付け${done ? "ました" : "ます"}（対象 ${count(p.photos)} 枚）。`);
    if (!done && p.changed > 0) {
      if (p.unchanged > 0) lines.push(`${count(p.unchanged)} 枚には同じ位置が付いているので変えません。`);
      // 同じ位置が付いているもの（unchanged）も、付けた位置があるもの（with_assigned）に数えられている。
      const replaced = Math.max(0, p.with_assigned - p.unchanged);
      if (replaced > 0) lines.push(`${count(replaced)} 枚は、前に付けた位置を置き換えます。`);
      if (p.with_camera_gps > 0) {
        lines.push(`カメラの位置がある ${count(p.with_camera_gps)} 枚は、付けた位置を優先します（カメラの位置は残ります）。`);
      }
    }
  } else if (p.changed === 0) {
    lines.push(`付けた位置のある写真はありません（対象 ${count(p.photos)} 枚）。`);
  } else if (done) {
    lines.push(`${count(p.changed)} 枚の付けた位置を消しました（対象 ${count(p.photos)} 枚）。`);
  } else {
    lines.push(
      `${count(p.changed)} 枚の付けた位置を消します（対象 ${count(p.photos)} 枚）。カメラの位置がある写真はカメラの位置に、ない写真は位置なしに戻ります。`,
    );
  }
  return [...lines, ...excludedLines(p, done)];
}

/** 対象の内訳の文（座標を入れる前。付けた位置を消す見込み（location が null）の数を使う）。 */
export function describeTargets(p: LocationEditPlan): string[] {
  const lines =
    p.photos === 0
      ? ["対象の写真がありません。"]
      : [
          `写真 ${count(p.photos)} 枚（カメラの位置がある ${count(p.with_camera_gps)} 枚・位置を付けた ${count(p.with_assigned)} 枚）。`,
        ];
  return [...lines, ...excludedLines(p, false)];
}

/** 位置の変更の履歴の 1 件の文。 */
export function describeLocationEdit(e: LocationEditInfo): string {
  return e.kind === "set" ? `位置を付ける（${formatGps(e.location) ?? "座標なし"}）` : "付けた位置を消す";
}

/** 取り消しの知らせの文。 */
export function describeLocationUndo(u: LocationUndo): string {
  const removed = u.removed > 0 ? `。その後に削除された ${count(u.removed)} 枚は除きました` : "";
  return `位置の変更（${describeLocationEdit(u.edit)}）を取り消しました（${count(u.restored)} 枚${removed}）`;
}

/** 写真の使う位置と出どころの文（"北緯 …、東経 …（後から付けた位置）"。位置がなければ「位置なし」）。 */
export function locationSummary(d: Pick<VariantDetails, "gps" | "gps_source">): string {
  const g = formatGps(d.gps);
  if (!g) return "位置なし";
  if (d.gps_source === "assigned") return `${g}（後から付けた位置）`;
  if (d.gps_source === "camera") return `${g}（カメラの位置）`;
  return g;
}

/**
 * 右パネルの GPS の行の注記（後から付けた位置のときだけ。カメラの位置も併記する）。カメラの位置のとき・古いコア
 * （gps_source が無い）では null。
 */
export function gpsSourceNote(d: Pick<VariantDetails, "gps_source" | "camera_gps">): string | null {
  if (d.gps_source !== "assigned") return null;
  return `後から付けた位置（カメラの位置: ${formatGps(d.camera_gps) ?? "なし"}）`;
}
