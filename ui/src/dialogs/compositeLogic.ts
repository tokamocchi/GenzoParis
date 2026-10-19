// 比較明・比較暗合成（DEV-34。docs/composite_design.md）のダイアログの入力・CompositeSettings への変換・確認の結果と
// ジョブの結果のまとめ（DOM を使わない純粋な計算）。
//
// CompositeSettings の JSON の形は genzo-model の composite.rs（形式・色空間は書き出しと同じ形）。既定は TIFF 16bit・
// Adobe RGB・元の写真とスタックにまとめる（決定事項ログ No.177・178）。

import type {
  CompositeCheck,
  CompositeMode,
  CompositeReport,
  CompositeSettings,
  ExportFormat,
  JobInfo,
  OutputColorSpace,
} from "../api/types";
import { formatBytes } from "../format";
import { type Notice, formatCount } from "./dialogFormat";
import { DEFAULT_JPEG_QUALITY, type FormatKind, JPEG_QUALITY_MAX, JPEG_QUALITY_MIN } from "./exportLogic";

/** 比べ方の選択肢（「比較明」「比較暗」を上に。カラーの 2 つは詳しい設定）。 */
export const MODE_OPTIONS: { value: CompositeMode; label: string; hint: string; advanced: boolean }[] = [
  {
    value: "lighten",
    label: "比較明合成",
    hint: "R・G・B をチャンネルごとに比べ、明るいほうを残します（Photoshop の「比較（明）」）。花火・星の軌跡・光跡に。",
    advanced: false,
  },
  {
    value: "darken",
    label: "比較暗合成",
    hint: "R・G・B をチャンネルごとに比べ、暗いほうを残します（Photoshop の「比較（暗）」）。",
    advanced: false,
  },
  {
    value: "lighter_color",
    label: "カラー比較明合成",
    hint: "画素の明るさ（輝度）で比べ、明るい写真の色をそのまま残します（色を混ぜません）。",
    advanced: true,
  },
  {
    value: "darker_color",
    label: "カラー比較暗合成",
    hint: "画素の明るさ（輝度）で比べ、暗い写真の色をそのまま残します（色を混ぜません）。",
    advanced: true,
  },
];

export function modeLabel(mode: CompositeMode): string {
  return MODE_OPTIONS.find((o) => o.value === mode)?.label ?? mode;
}

/** 画面の入力（数値は入力欄の文字列のまま持つ）。 */
export interface CompositeForm {
  mode: CompositeMode;
  format: FormatKind;
  quality: string;
  colorSpace: OutputColorSpace;
  /** 保存先のフォルダ（空なら 1 枚目の写真と同じフォルダ）。 */
  destDir: string;
  stackWithSources: boolean;
  removeGps: boolean;
}

/** 既定の入力（genzo-model の CompositeSettings::default と同じ）。 */
export function defaultCompositeForm(mode: CompositeMode): CompositeForm {
  return {
    mode,
    format: "tiff16",
    quality: String(DEFAULT_JPEG_QUALITY),
    colorSpace: "adobe_rgb",
    destDir: "",
    stackWithSources: true,
    removeGps: false,
  };
}

/** JPEG の品質（範囲外・数でなければ null）。 */
export function parseQuality(text: string): number | null {
  const t = text.trim().replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0));
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return n >= JPEG_QUALITY_MIN && n <= JPEG_QUALITY_MAX ? n : null;
}

/** 入力を CompositeSettings にする（JPEG の品質が正しくなければ null）。 */
export function toCompositeSettings(f: CompositeForm): CompositeSettings | null {
  let format: ExportFormat;
  if (f.format === "jpeg") {
    const quality = parseQuality(f.quality);
    if (quality === null) return null;
    format = { jpeg: { quality } };
  } else {
    format = f.format;
  }
  return {
    mode: f.mode,
    format,
    color_space: f.colorSpace,
    remove_gps: f.removeGps,
    stack_with_sources: f.stackWithSources,
  };
}

/** 保存先（空なら null = 1 枚目の写真と同じフォルダ）。 */
export function destOf(f: CompositeForm): string | null {
  const d = f.destDir.trim();
  return d === "" ? null : d;
}

/** 確認の結果の 1 行。 */
export interface CheckLine {
  level: "info" | "warning" | "error";
  text: string;
}

/** 確認の結果（写真の数・保存先・警告・合成できない理由）を行にする。 */
export function describeCheck(c: CompositeCheck, settings: CompositeSettings): CheckLine[] {
  const lines: CheckLine[] = [];
  lines.push({ level: "info", text: `撮影日時の順に ${formatCount(c.sources.length)} 枚を合成します` });
  if (c.skipped_videos > 0) lines.push({ level: "info", text: `動画 ${formatCount(c.skipped_videos)} 本は除きます` });
  if (c.dest_dir !== null && c.file_name !== null) {
    lines.push({ level: "info", text: `保存するファイル: ${c.file_name}（同じ名前があれば連番を付けます）` });
  }
  if (c.estimated_buffer_bytes > 0) {
    lines.push({
      level: "info",
      text: `合成に使うメモリ: 約 ${formatBytes(c.estimated_buffer_bytes)}（元の写真の寸法からの見込み。現像のメモリは別）`,
    });
  }
  if (c.size_differs.length > 0) {
    lines.push({
      level: "warning",
      text: `元の写真の寸法・向きが 1 枚目と違う写真が ${formatCount(c.size_differs.length)} 枚あります。現像した後の寸法が違うと合成できません`,
    });
  }
  if (c.geometry_differs.length > 0) {
    lines.push({
      level: "warning",
      text: `切り抜き・角度・レンズ補正が 1 枚目と違う写真が ${formatCount(c.geometry_differs.length)} 枚あります。位置がずれて、にじむことがあります`,
    });
  }
  if (c.settings_differ.length > 0) {
    lines.push({
      level: "warning",
      text: `現像の設定が 1 枚目と違う写真が ${formatCount(c.settings_differ.length)} 枚あります（各写真は、それぞれの設定で現像します）`,
    });
  }
  if (settings.stack_with_sources && c.stacks > 0) {
    lines.push({
      level: "info",
      text: `元の写真が入っている ${formatCount(c.stacks)} 個のスタックは、合成の結果と 1 つのスタックにまとまります`,
    });
  }
  for (const p of c.problems) lines.push({ level: "error", text: p });
  return lines;
}

/** 合成の結果の通知。 */
export function describeCompositeReport(r: CompositeReport): Notice {
  const name = modeLabel(r.mode);
  const total = r.sources.length;
  if (r.failure) {
    const which = r.failure.variant_id !== null ? `（${formatCount(r.composited)} / ${formatCount(total)} 枚目まで重ねたところ）` : "";
    return {
      level: "error",
      message: `${name}できませんでした${which}: ${r.failure.error.message}`,
      detail: "何も保存していません。",
    };
  }
  if (r.cancelled) {
    return {
      level: "info",
      message: `${name}を取り消しました（${formatCount(r.composited)} / ${formatCount(total)} 枚。何も保存していません）`,
    };
  }
  const lines: string[] = [];
  if (r.path) lines.push(`保存先: ${r.path}`);
  if (r.size) lines.push(`寸法: ${r.size[0]} × ${r.size[1]}`);
  if (r.registration_error) {
    lines.push(
      r.registered !== null
        ? `カタログ: ${r.registration_error}`
        : `カタログに登録できませんでした（ファイルは残しました。取り込み直せます）: ${r.registration_error}`,
    );
  }
  if (r.stack_error) lines.push(`スタックにまとめられませんでした: ${r.stack_error}`);
  if (r.cpu_renders > 0) lines.push(`CPU 版で現像した写真: ${formatCount(r.cpu_renders)} 枚`);
  if (r.warnings.length > 0) {
    lines.push("警告:", ...r.warnings.slice(0, 5).map((w) => `- ${w.message}`));
    if (r.warnings.length > 5) lines.push(`ほか ${formatCount(r.warnings.length - 5)} 件`);
  }
  const ok = r.registered !== null && r.registration_error === null && r.stack_error === null;
  return {
    level: ok && r.warnings.length === 0 ? "success" : "warning",
    message: `${name}: ${formatCount(r.composited)} 枚を 1 枚にしました${r.stacked ? "（元の写真とスタックにまとめました）" : ""}`,
    detail: lines.join("\n"),
  };
}

/**
 * 合成のジョブの結果の説明。ジョブそのものの失敗（state が failed）は state/app.ts が job_finished のイベントで
 * 通知するので null（二重に知らせない）。
 */
export function describeCompositeJob(info: JobInfo): Notice | null {
  if (info.state === "failed") return null;
  const r = info.result;
  if (r && r.type === "composite") return describeCompositeReport(r);
  if (info.state === "cancelled") return { level: "info", message: "合成を取り消しました（何も保存していません）" };
  return { level: "success", message: "合成が終わりました" };
}
