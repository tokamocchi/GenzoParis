// メタデータの表示（LIB-14・VID-03）の書式とまとめ（DOM を使わない純粋な計算）。

import type {
  CaptureTime,
  FileRole,
  FileStatus,
  GpsCoord,
  Orientation,
  VariantSummary,
  VideoMetadata,
} from "../api/types";
import { formatDuration } from "../format";

/** 寸法の表示（向きを反映した幅 × 高さ。EXIF の向き 5〜8 は 90 度回すので幅と高さを入れ替える）。 */
export function formatDimensions(
  width: number | null | undefined,
  height: number | null | undefined,
  orientation: Orientation = 1,
): string {
  if (!width || !height) return "—";
  const [w, h] = orientation >= 5 ? [height, width] : [width, height];
  return `${w.toLocaleString("ja-JP")} × ${h.toLocaleString("ja-JP")}`;
}

/** パスのフォルダの部分（"/a/b/c.ARW" → "/a/b"。Windows の "\" も区切りとみなす）。 */
export function dirName(path: string | null | undefined): string | null {
  if (!path) return null;
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (i < 0) return null;
  if (i === 0) return path.slice(0, 1);
  // "C:\a.jpg" のルートは "C:\" のまま。
  if (i === 2 && /^[A-Za-z]:/.test(path)) return path.slice(0, 3);
  return path.slice(0, i);
}

/** ファイル名の拡張子（大文字。種類の印。なければ ""）。 */
export function extensionMark(fileName: string): string {
  const i = fileName.lastIndexOf(".");
  if (i <= 0 || i === fileName.length - 1) return "";
  return fileName.slice(i + 1).toUpperCase().slice(0, 5);
}

export function fileRoleLabel(role: FileRole): string {
  return role === "primary" ? "主のファイル" : "同時記録の JPEG";
}

export function fileStatusLabel(status: FileStatus): string {
  switch (status) {
    case "ok":
      return "あり";
    case "missing":
      return "見つからない";
    case "error":
      return "読めない";
  }
}

/** セルの印の説明（file_status が ok 以外）。 */
export function fileStatusTitle(status: FileStatus): string | null {
  if (status === "missing") return "元ファイルが見つかりません";
  if (status === "error") return "元ファイルを読めません";
  return null;
}

/** 撮影日時の補足（タイムゾーンの推定・補正。なければ null）。 */
export function captureNote(c: CaptureTime | null | undefined): string | null {
  if (!c || !c.utc) return null;
  const notes: string[] = [];
  switch (c.tz_source) {
    case "utc_only":
      notes.push("UTC だけが記録されています");
      break;
    case "user_default":
      notes.push(`タイムゾーンは既定の値（${c.tz_assumed ?? "要確認"}）で推定`);
      break;
    case "user_set":
      notes.push(`タイムゾーンは設定した値（${c.tz_assumed ?? "要確認"}）`);
      break;
    case "exif":
      break;
  }
  if (c.correction_s) {
    const sign = c.correction_s > 0 ? "+" : "−";
    notes.push(`時刻を ${sign}${Math.abs(c.correction_s).toLocaleString("ja-JP")} 秒補正`);
  }
  return notes.length > 0 ? notes.join("、") : null;
}

/** GPS の座標の表示（10 進の度。小数 6 桁は約 0.1 m）。 */
export function formatGps(g: GpsCoord | null | undefined): string | null {
  if (!g || !Number.isFinite(g.lat) || !Number.isFinite(g.lon)) return null;
  const lat = `${g.lat >= 0 ? "北緯" : "南緯"} ${Math.abs(g.lat).toFixed(6)}°`;
  const lon = `${g.lon >= 0 ? "東経" : "西経"} ${Math.abs(g.lon).toFixed(6)}°`;
  return `${lat}、${lon}`;
}

/** GPS の座標の数値（コピー用。"35.681236, 139.767125"）。 */
export function gpsDecimal(g: GpsCoord): string {
  return `${g.lat.toFixed(6)}, ${g.lon.toFixed(6)}`;
}

/** 動画の情報の行（ラベルと値。値のないものは「—」）。 */
export function videoRows(v: VideoMetadata): [string, string][] {
  const fps = v.fps !== null && Number.isFinite(v.fps) ? `${Number(v.fps.toFixed(3))} fps` : "—";
  return [
    ["長さ", formatDuration(v.duration_s)],
    ["フレームレート", fps],
    ["コーデック", v.codec ?? "—"],
    ["ビット深度", v.bit_depth ? `${v.bit_depth} bit` : "—"],
    ["解像度", formatDimensions(v.width, v.height)],
    ["色の伝達特性", v.color_transfer ?? "—"],
    ["色域", v.color_primaries ?? "—"],
  ];
}

/** ISO 感度の表示。 */
export function formatIso(iso: number | null | undefined): string {
  return iso ? `ISO ${iso.toLocaleString("ja-JP")}` : "—";
}

/** 複数の値が同じならその値、違えば "mixed"、空なら null。 */
export function commonValue<T>(values: T[]): T | "mixed" | null {
  if (values.length === 0) return null;
  const first = values[0];
  return values.every((v) => Object.is(v, first)) ? first : "mixed";
}

/** 仮想コピーの表示名（名前がなければ「仮想コピー」）。マスターなら null。 */
export function virtualCopyLabel(s: Pick<VariantSummary, "is_master" | "variant_name">): string | null {
  if (s.is_master) return null;
  return s.variant_name ? `仮想コピー: ${s.variant_name}` : "仮想コピー";
}
