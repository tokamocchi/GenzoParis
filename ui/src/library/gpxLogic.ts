// GPX のログからの位置付け（LIB-21 の拡張。写真の右クリックの「GPX のログから位置を付ける…」）の、入力の検証と
// 見込み・結果の文（DOM を使わない）。
//
// コア（plan_gpx_locations・apply_gpx_locations）が、GPX のログと写真の撮影日時（補正後の UTC）を照らし合わせて
// 位置を決める。時刻をはさむ 2 点の間隔が「記録の途切れとみなす間隔」以内なら補間し、そうでなければ近いほうの点が
// その秒数以内にあるときだけ使う。すでに位置のある写真・動画・撮影日時のない写真には付けない。

import type { GpxLocationPlan, GpxOptions } from "../api/types";
import type { Notice } from "../dialogs/dialogFormat";

/** 既定の設定（genzo-api の GpxOptions の既定と同じ）。 */
export const DEFAULT_GPX_OPTIONS: GpxOptions = { max_gap_seconds: 300, offset_seconds: 0, overwrite: false };

/** 記録の途切れとみなす間隔の範囲（秒。genzo-model の MAX_GPX_MAX_GAP_SECONDS と同じ）。 */
export const GPX_MAX_GAP_RANGE = { min: 1, max: 86_400 } as const;
/** 写真の時計のずれの範囲（秒。±14 日。genzo-api の MAX_GPX_OFFSET_SECONDS と同じ）。 */
export const GPX_OFFSET_LIMIT = 14 * 86_400;

/** 入力欄の文字列（間隔・ずれ）。 */
export interface GpxForm {
  maxGap: string;
  offset: string;
  overwrite: boolean;
}

export const DEFAULT_GPX_FORM: GpxForm = {
  maxGap: String(DEFAULT_GPX_OPTIONS.max_gap_seconds),
  offset: "0",
  overwrite: false,
};

const INTEGER = /^[+-−]?\d+$/;

/** 整数の入力を読む（全角の数字・マイナスの記号も受ける。読めなければ null）。 */
function parseInteger(text: string): number | null {
  const t = text
    .trim()
    .replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
    .replace(/[−－]/g, "-")
    .replace(/＋/g, "+");
  if (!INTEGER.test(t)) return null;
  const v = Number(t);
  return Number.isSafeInteger(v) ? v : null;
}

/** 入力を設定にする（誤りがあれば、その説明）。 */
export function gpxOptionsOf(form: GpxForm): { options: GpxOptions } | { error: string } {
  const gap = parseInteger(form.maxGap);
  if (gap === null || gap < GPX_MAX_GAP_RANGE.min || gap > GPX_MAX_GAP_RANGE.max) {
    return { error: `記録の途切れとみなす間隔は ${GPX_MAX_GAP_RANGE.min}〜${GPX_MAX_GAP_RANGE.max.toLocaleString("ja-JP")} 秒の整数で入れてください` };
  }
  const offset = parseInteger(form.offset);
  if (offset === null || Math.abs(offset) > GPX_OFFSET_LIMIT) {
    return { error: `写真の時計のずれは ±${GPX_OFFSET_LIMIT.toLocaleString("ja-JP")} 秒までの整数で入れてください` };
  }
  return { options: { max_gap_seconds: gap, offset_seconds: offset, overwrite: form.overwrite } };
}

const n = (v: number) => v.toLocaleString("ja-JP");

/** 日時（RFC 3339）を、画面の地域の日時の文にする（読めなければそのまま）。 */
export function formatTrackTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const p = (v: number) => String(v).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** ログの説明（点の数と期間）。 */
export function describeTrack(p: GpxLocationPlan): string {
  const skipped = p.track_skipped > 0 ? `。読めなかった点 ${n(p.track_skipped)}` : "";
  return `ログ: ${n(p.track_points)} 点（${formatTrackTime(p.track_start)} 〜 ${formatTrackTime(p.track_end)}${skipped}）`;
}

/** 付けない写真の説明（数が 0 のものは出さない）。 */
export function describeGpxSkips(p: GpxLocationPlan, overwrite: boolean): string[] {
  const lines: string[] = [];
  if (p.unchanged > 0) lines.push(`${n(p.unchanged)} 枚は、同じ位置がすでに付いているため変わりません`);
  if (p.skipped_located > 0 && !overwrite) {
    lines.push(`${n(p.skipped_located)} 枚は、すでに位置があるため付けません（「位置のある写真にも付ける」で付けられます）`);
  }
  if (p.out_of_track > 0) {
    lines.push(`${n(p.out_of_track)} 枚は、撮影日時がログの記録から離れているため付けられません（間隔・時計のずれを確かめてください）`);
  }
  if (p.no_capture_time > 0) lines.push(`${n(p.no_capture_time)} 枚は、撮影日時がないため付けられません`);
  if (p.skipped_videos > 0) lines.push(`動画 ${n(p.skipped_videos)} 本は対象外です（動画には位置を付けません）`);
  if (p.missing > 0) lines.push(`${n(p.missing)} 枚は見つかりませんでした（削除された写真など）`);
  return lines;
}

/** 見込みの文（1 行目）。 */
export function describeGpxPlan(p: GpxLocationPlan): string {
  if (p.changed === 0) return `位置を付けられる写真はありません（対象 ${n(p.photos)} 枚）`;
  const how = p.interpolated > 0 && p.interpolated < p.matched ? `（${n(p.interpolated)} 枚は 2 点の間を補間）` : "";
  return `${n(p.changed)} 枚に位置を付けます（対象 ${n(p.photos)} 枚）${how}`;
}

/** 結果の通知。 */
export function describeGpxResult(p: GpxLocationPlan, overwrite: boolean): Notice {
  const skips = describeGpxSkips(p, overwrite);
  const detail = [describeTrack(p), ...skips].join("\n");
  if (p.changed === 0) return { level: "info", message: "GPX のログから位置を付けられる写真はありませんでした", detail };
  return {
    level: "success",
    message: `GPX のログから ${n(p.changed)} 枚に位置を付けました（位置を付けるダイアログの「元に戻す」で取り消せます）`,
    detail,
  };
}
