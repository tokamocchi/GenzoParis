// 撮影日時の一括修正（LIB-16）の画面の計算（DOM を使わない純粋な計算。docs/04 の 3.13 節）。
//
// - ずらす量（日・時間・分・秒）と秒の相互の変換、表示（formatShift。CLI の time.rs の format_shift と同じ文）。
// - 基準の写真の正しい撮影日時の入力（parseLocalDateTime。"2024-05-01 12:34:56" などを
//   "2024-05-01T12:34:56" にする）と、写真の今の現地時刻（captureLocal）。
// - タイムゾーンの選択肢（ZONE_OPTIONS。-12:00〜+14:00 の、実際に使われるオフセット）。
// - 機材の名前（deviceLabel）・見込みの文（describePlan）・履歴の文（describeEditInfo）。

import type {
  CaptureDevice,
  CaptureDeviceKey,
  CaptureSourceCounts,
  CaptureTime,
  CaptureTimeEdit,
  CaptureTimeEditInfo,
  CaptureTimePlan,
  ZoneMode,
} from "../api/types";
import { parseOffset } from "../format";

/** ずらす量の上限（秒。genzo-catalog の MAX_CAPTURE_SHIFT_SECONDS と同じ 36,525 日）。 */
export const MAX_SHIFT_SECONDS = 36_525 * 86_400;

/** 修正の方法。 */
export type CaptureMethod = "sync" | "shift" | "set_zone" | "clear_zone" | "reset";

export const CAPTURE_METHODS: { value: CaptureMethod; label: string; hint: string }[] = [
  {
    value: "sync",
    label: "基準の写真の正しい時刻に合わせる",
    hint: "表示中の写真の正しい撮影日時を入れると、今の撮影日時との差を求め、対象のすべてを同じだけずらします（カメラの時計のずれを直す）。",
  },
  {
    value: "shift",
    label: "時刻をずらす",
    hint: "日・時間・分・秒を指定して、対象のすべてを同じだけずらします（今の補正に足します）。",
  },
  {
    value: "set_zone",
    label: "タイムゾーンを指定する",
    hint: "撮影した地域のタイムゾーンを指定します。読み方を 2 つから選べます。",
  },
  {
    value: "clear_zone",
    label: "タイムゾーンの指定を取り消す",
    hint: "ファイルに記録されたオフセット（なければ既定のタイムゾーン）に戻します。時刻のずらしは保ちます。",
  },
  {
    value: "reset",
    label: "ファイルの値に戻す",
    hint: "時刻のずらしとタイムゾーンの指定をすべて取り消し、ファイルに記録された撮影日時に戻します。",
  },
];

export const ZONE_MODES: { value: ZoneMode; label: string; hint: string }[] = [
  {
    value: "keep_local",
    label: "記録の時刻を、このタイムゾーンの時刻として読む",
    hint: "カメラの時計がその地域の時刻に合っていた場合。撮影した瞬間（並び順）が変わります。UTC の印付きで記録された動画（DJI など、現地時刻が UTC として書かれていた場合）も、その時刻を現地時刻として読みます。",
  },
  {
    value: "keep_instant",
    label: "撮影した瞬間はそのままで、このタイムゾーンの時刻で表示する",
    hint: "カメラの時計は正しく、別の地域の時刻で見たい場合（日本の時刻のまま旅行先で撮ったなど）。並び順は変わりません。",
  },
];

/** ずらす量の入力（符号と、日・時間・分・秒の文字列）。 */
export interface ShiftInput {
  negative: boolean;
  days: string;
  hours: string;
  minutes: string;
  seconds: string;
}

export const EMPTY_SHIFT: ShiftInput = { negative: false, days: "", hours: "", minutes: "", seconds: "" };

/** 0 以上の整数（空は 0。全角の数字も読む）。読めなければ null。 */
function nonNegativeInt(s: string): number | null {
  const t = s.trim().replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0));
  if (t === "") return 0;
  if (!/^\d{1,9}$/.test(t)) return null;
  return Number(t);
}

/** ずらす量の入力を秒にする（読めない・上限を超えるなら null）。 */
export function shiftSeconds(i: ShiftInput): number | null {
  const parts = [nonNegativeInt(i.days), nonNegativeInt(i.hours), nonNegativeInt(i.minutes), nonNegativeInt(i.seconds)];
  if (parts.some((p) => p === null)) return null;
  const [d, h, m, s] = parts as number[];
  const total = ((d * 24 + h) * 60 + m) * 60 + s;
  if (!Number.isSafeInteger(total) || total > MAX_SHIFT_SECONDS) return null;
  return i.negative ? -total : total;
}

/** 秒を、ずらす量の入力にする（日・時間・分・秒に分ける）。 */
export function shiftInputOf(seconds: number): ShiftInput {
  const a = Math.abs(Math.trunc(seconds));
  return {
    negative: seconds < 0,
    days: String(Math.floor(a / 86_400)),
    hours: String(Math.floor((a % 86_400) / 3_600)),
    minutes: String(Math.floor((a % 3_600) / 60)),
    seconds: String(a % 60),
  };
}

/** 秒の量の表示（"+1 日 2 時間 3 分 4 秒"。CLI の format_shift と同じ）。 */
export function formatShift(seconds: number): string {
  if (seconds === 0) return "±0 秒";
  const sign = seconds < 0 ? "-" : "+";
  let rest = Math.abs(Math.trunc(seconds));
  const parts: string[] = [];
  for (const [unit, name] of [
    [86_400, "日"],
    [3_600, "時間"],
    [60, "分"],
    [1, "秒"],
  ] as const) {
    const n = Math.floor(rest / unit);
    rest %= unit;
    if (n > 0) parts.push(`${n.toLocaleString("ja-JP")} ${name}`);
  }
  return `${sign}${parts.join(" ")}`;
}

/**
 * 正しい撮影日時の入力を "YYYY-MM-DDTHH:MM:SS" にする（区切りは "-"・"/"・":"、日付と時刻の間は空白か "T"。
 * 秒は省略できる。全角の数字も読む）。読めない・実在しない日時なら null。
 */
export function parseLocalDateTime(text: string): string | null {
  const t = text
    .trim()
    .replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
    .replace(/：/g, ":")
    .replace(/[／]/g, "/");
  const m = /^(\d{4})[-/:](\d{1,2})[-/:](\d{1,2})[ T]+(\d{1,2}):(\d{2})(?::(\d{2}))?$/.exec(t);
  if (!m) return null;
  const [y, mo, d, h, mi, s] = [m[1], m[2], m[3], m[4], m[5], m[6] ?? "0"].map(Number);
  if (mo < 1 || mo > 12 || d < 1 || h > 23 || mi > 59 || s > 59) return null;
  const date = new Date(Date.UTC(y, mo - 1, d, h, mi, s));
  if (date.getUTCMonth() !== mo - 1 || date.getUTCDate() !== d) return null;
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${p(y, 4)}-${p(mo)}-${p(d)}T${p(h)}:${p(mi)}:${p(s)}`;
}

/** 写真の今の撮影日時の現地時刻（"YYYY-MM-DDTHH:MM:SS"。表示のオフセットで表す）。分からなければ null。 */
export function captureLocal(c: CaptureTime | null | undefined): string | null {
  if (!c?.utc) return null;
  const minutes = parseOffset(c.tz_assumed) ?? 0;
  const ms = Date.parse(c.utc);
  if (!Number.isFinite(ms)) return null;
  return new Date(ms + minutes * 60_000).toISOString().slice(0, 19);
}

/** オフセット（分）の表示（"+09:00"）。 */
export function formatOffsetMinutes(minutes: number): string {
  const sign = minutes < 0 ? "-" : "+";
  const a = Math.abs(minutes);
  return `${sign}${String(Math.floor(a / 60)).padStart(2, "0")}:${String(a % 60).padStart(2, "0")}`;
}

/** タイムゾーンの選択肢（分。-12:00〜+14:00 の整時と、使われている 30 分・45 分のオフセット）。 */
export const ZONE_OPTIONS: number[] = (() => {
  const extra = [-570, -210, 210, 270, 330, 345, 390, 525, 570, 630, 765];
  const hours = Array.from({ length: 27 }, (_, i) => (i - 12) * 60);
  return [...new Set([...hours, ...extra])].sort((a, b) => a - b);
})();

/** タイムゾーンの選択肢の文（"+09:00（日本・韓国）" のように、代表の地域を添える。地域は目安）。 */
export function zoneLabel(minutes: number): string {
  const names: Record<number, string> = {
    [-600]: "ハワイ",
    [-480]: "米国の太平洋（冬）",
    [-420]: "米国の太平洋（夏）",
    [-300]: "米国の東部（冬）",
    [-240]: "米国の東部（夏）",
    0: "UTC・英国（冬）",
    60: "英国（夏）・中欧（冬）",
    120: "中欧（夏）・東欧（冬）",
    180: "モスクワ",
    330: "インド",
    420: "タイ・ベトナム",
    480: "中国・台湾・シンガポール",
    540: "日本・韓国",
    600: "オーストラリアの東部（冬）",
    660: "オーストラリアの東部（夏）",
    720: "ニュージーランド（冬）",
    780: "ニュージーランド（夏）",
  };
  const name = names[minutes];
  return name ? `${formatOffsetMinutes(minutes)}（${name}）` : formatOffsetMinutes(minutes);
}

/** 機材の名前（"SONY ILCE-7M4（シリアル 1234）の写真"・"機材の情報のない動画"）。 */
export function deviceLabel(d: CaptureDeviceKey): string {
  const what = d.kind === "video" ? "動画" : "写真";
  if (d.camera === null) return `機材の情報のない${what}`;
  const serial = d.serial ? `（シリアル ${d.serial}）` : "";
  return `${d.camera}${serial}の${what}`;
}

/** 機材を区別する文字列（選択の状態の鍵）。 */
export function deviceId(d: CaptureDeviceKey): string {
  return JSON.stringify([d.camera, d.serial, d.kind]);
}

/** 同じ機材か。 */
export function sameDevice(a: CaptureDeviceKey, b: CaptureDeviceKey): boolean {
  return a.camera === b.camera && a.serial === b.serial && a.kind === b.kind;
}

/** 機材の数と期間の文（"1,234 枚・2024-05-01〜2024-05-03"）。 */
export function deviceSummary(d: CaptureDevice): string {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const unit = d.kind === "video" ? "本" : "枚";
  const parts = [`${n(d.assets)} ${unit}`];
  const first = d.first?.slice(0, 10);
  const last = d.last?.slice(0, 10);
  if (first && last) parts.push(first === last ? first : `${first}〜${last}`);
  if (d.adjusted > 0) parts.push(`修正済み ${n(d.adjusted)}`);
  if (d.with_time < d.assets) parts.push(`撮影日時なし ${n(d.assets - d.with_time)}`);
  return parts.join("・");
}

/** 撮影日時の記録の内訳の文（タイムゾーン情報のないデータの扱いを示す）。 */
export function sourceLines(s: CaptureSourceCounts, defaultOffset: string | null): string[] {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const lines: string[] = [];
  if (s.exif > 0) lines.push(`オフセットが記録されている: ${n(s.exif)}`);
  if (s.user_default > 0)
    lines.push(`オフセットの記録がない（既定のタイムゾーン ${defaultOffset ?? ""} で推定）: ${n(s.user_default)}`);
  if (s.utc_only > 0) lines.push(`UTC の印だけが記録されている（動画など。既定のタイムゾーンで表示）: ${n(s.utc_only)}`);
  if (s.user_set > 0) lines.push(`タイムゾーンを指定済み: ${n(s.user_set)}`);
  return lines;
}

/** 見込み・結果の主な文。 */
export function describePlan(p: CaptureTimePlan, done: boolean): string[] {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const lines: string[] = [];
  if (p.shift_seconds !== null) lines.push(`ずらす量: ${formatShift(p.shift_seconds)}`);
  if (p.assets === 0) {
    lines.push("対象の写真がありません。");
    return lines;
  }
  if (p.changed === 0) lines.push(`撮影日時の変わる写真はありません（対象 ${n(p.assets)} 枚）。`);
  else if (done) lines.push(`${n(p.changed)} 枚の撮影日時を修正しました（対象 ${n(p.assets)} 枚）。`);
  else lines.push(`${n(p.changed)} 枚の撮影日時が変わります（対象 ${n(p.assets)} 枚）。`);
  if (p.without_time > 0) lines.push(`撮影日時の分からない ${n(p.without_time)} 枚は変えません。`);
  return lines;
}

/** 修正の内容（コアの CaptureTimeEdit）を作る。入力が足りない・誤っていれば null。 */
export function buildEdit(
  method: CaptureMethod,
  v: {
    reference: number | null;
    correct: string;
    correctZone: number | null;
    shift: ShiftInput;
    zone: number;
    zoneMode: ZoneMode;
  },
): CaptureTimeEdit | null {
  switch (method) {
    case "sync": {
      const correct = parseLocalDateTime(v.correct);
      if (v.reference === null || correct === null) return null;
      return { kind: "sync", reference: v.reference, correct, offset_minutes: v.correctZone };
    }
    case "shift": {
      const s = shiftSeconds(v.shift);
      return s === null || s === 0 ? null : { kind: "shift", seconds: s };
    }
    case "set_zone":
      return { kind: "set_zone", offset_minutes: v.zone, mode: v.zoneMode };
    case "clear_zone":
      return { kind: "clear_zone" };
    case "reset":
      return { kind: "reset" };
  }
}

/** 修正の履歴の 1 件の文（CLI の time.rs の describe_edit と同じ）。 */
export function describeEditInfo(e: CaptureTimeEditInfo): string {
  switch (e.kind) {
    case "shift":
      return `ずらす ${formatShift(e.shift_seconds ?? 0)}`;
    case "sync":
      return `基準の写真に合わせる ${formatShift(e.shift_seconds ?? 0)}`;
    case "set_zone":
      return `タイムゾーン ${e.zone ?? "?"}（${
        e.zone_mode === "keep_instant" ? "撮影した瞬間はそのまま" : "記録の時刻をその地域の時刻として読む"
      }）`;
    case "clear_zone":
      return "タイムゾーンの指定を取り消す";
    case "reset":
      return "ファイルの値に戻す";
  }
}
