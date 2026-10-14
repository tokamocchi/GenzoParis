// メタデータの表示（LIB-14・VID-03）の書式とまとめ（DOM を使わない純粋な計算）。

import type {
  CaptureTime,
  FileCheckReport,
  FileInfo,
  FileRole,
  FileStatus,
  GpsCoord,
  Orientation,
  VariantSummary,
  VideoMetadata,
} from "../api/types";
import { formatDuration } from "../format";
import { formatShift } from "./captureTimeLogic";

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
      notes.push(`UTC だけが記録されているため、既定のタイムゾーン（${c.tz_assumed ?? "要確認"}）で表示`);
      break;
    case "user_default":
      notes.push(`タイムゾーンは既定の値（${c.tz_assumed ?? "要確認"}）で推定`);
      break;
    case "user_set":
      if (c.offset === "Z" && c.basis) {
        notes.push(`UTC の印付きの時刻を ${c.basis} の現地時刻として読む（設定した値）`);
        if (c.tz_assumed !== c.basis) notes.push(`${c.tz_assumed ?? "要確認"} の時刻で表示`);
      } else if (c.basis && c.basis !== c.tz_assumed) {
        notes.push(`タイムゾーンは設定した値（${c.tz_assumed ?? "要確認"}。撮影した瞬間はそのまま）`);
      } else {
        notes.push(`タイムゾーンは設定した値（${c.tz_assumed ?? "要確認"}）`);
      }
      break;
    case "exif":
      break;
  }
  if (c.correction_s) notes.push(`時刻を ${formatShift(c.correction_s)} 補正`);
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

/** 主のファイル（なければ先頭。ファイルがなければ null）。 */
export function primaryFile(files: FileInfo[]): FileInfo | null {
  return files.find((f) => f.role === "primary") ?? files[0] ?? null;
}

/** ファイルの状態の説明（missing・error は理由を添える）。 */
export function fileStatusText(f: Pick<FileInfo, "status" | "status_reason">): string {
  const label = fileStatusLabel(f.status);
  return f.status !== "ok" && f.status_reason ? `${label}（${f.status_reason}）` : label;
}

/** 動画の行を出すか（どれか 1 つでも値があれば）。 */
export function hasVideoInfo(v: VideoMetadata | null | undefined): v is VideoMetadata {
  if (!v) return false;
  return [v.duration_s, v.fps, v.codec, v.bit_depth, v.color_transfer, v.color_primaries, v.width, v.height].some(
    (x) => x !== null && x !== undefined,
  );
}

/** 仮想コピーの表示名（名前がなければ「仮想コピー」）。マスターなら null。 */
export function virtualCopyLabel(s: Pick<VariantSummary, "is_master" | "variant_name">): string | null {
  if (s.is_master) return null;
  return s.variant_name ? `仮想コピー: ${s.variant_name}` : "仮想コピー";
}

/**
 * RAW と同時記録の JPEG を 1 枚の写真にまとめた写真の印の説明（IMP-06。まとめていなければ null）。グリッドの印・
 * セルとフィルムストリップの説明に使う。
 */
export function pairLabel(s: Pick<VariantSummary, "has_sidecar_jpeg">): string | null {
  return s.has_sidecar_jpeg ? "RAW + JPEG（同時記録の JPEG を 1 枚の写真として扱う）" : null;
}

/** 内容のハッシュ（BLAKE3）の短い表示（先頭 12 文字。まだ計算していなければ「未計算」）。 */
export function shortHash(full: string | null | undefined): string {
  return full ? `${full.slice(0, 12)}…` : "未計算";
}

/** 元ファイルを確かめ直した結果の知らせ。 */
export interface FileCheckNotice {
  level: "success" | "warning";
  message: string;
  detail?: string;
}

/**
 * 元ファイルを確かめ直した結果（core.checkFiles）と、その後の主のファイルの状態から、知らせる内容を決める。
 *
 * genzo-api の library.rs の `check_files` では、場所が分からない（ボリュームが接続されていない）ファイルは
 * `checked` に数えるが `ok` にも `missing` にも入れない。読めない（権限がないなど）ファイルは `ok` に数え、
 * 状態は前のまま残す。そのため `ok` だけでは確かめられたと言えず、その後の状態も見る。
 * `after` は確かめ直した後の主のファイルの状態（読めなければ null）。
 */
export function fileCheckNotice(
  r: Pick<FileCheckReport, "checked" | "ok" | "missing">,
  after: Pick<FileInfo, "status" | "status_reason"> | null,
): FileCheckNotice {
  if (r.missing.length > 0) {
    const more = r.missing.length > 1 ? `ほか ${(r.missing.length - 1).toLocaleString("ja-JP")} 件` : null;
    return {
      level: "warning",
      message: "元ファイルが見つかりません",
      detail: [r.missing[0].path, more].filter(Boolean).join("\n"),
    };
  }
  if (r.ok < r.checked) {
    return {
      level: "warning",
      message: "元ファイルを確かめられませんでした",
      detail: "場所が分からないファイルがあります（ドライブが接続されていない可能性があります）",
    };
  }
  if (after && after.status === "missing") {
    return {
      level: "warning",
      message: "元ファイルを確かめられませんでした",
      detail: "ファイルを読めないため、状態を確かめられません（読む権限がないなどの可能性があります）",
    };
  }
  if (after && after.status === "error") {
    return { level: "warning", message: "元ファイルを読めません", detail: after.status_reason ?? undefined };
  }
  return { level: "success", message: "元ファイルを確かめました" };
}
