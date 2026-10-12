// 取り込み（IMP-01・VID-01）のダイアログとドラッグ＆ドロップの判定（DOM を使わない純粋な計算）。

import type { ExternalTools } from "../api/types";
import { baseName } from "./dialogFormat";

/**
 * フォルダではないと分かる拡張子（小文字）。ドロップしたものがファイルなら取り込まない（取り込みはフォルダ
 * 単位）。genzo-api の import.rs の RAW_EXTENSIONS・IMAGE_EXTENSIONS・VIDEO_EXTENSIONS に、よく一緒に
 * 扱うファイル（カタログ・サイドカー・ほかの RAW など）を加えたもの。ここにない拡張子の名前（"2024.05.01"
 * のようなフォルダもある）はフォルダとみなし、実際はファイルなら取り込みのジョブが「フォルダではありません」で
 * 失敗する。
 */
const FILE_EXTENSIONS = new Set([
  "arw",
  "dng",
  "jpg",
  "jpeg",
  "tif",
  "tiff",
  "png",
  "mp4",
  "mov",
  "heic",
  "heif",
  "cr2",
  "cr3",
  "nef",
  "raf",
  "orf",
  "rw2",
  "xmp",
  "db",
  "lrcat",
  "zip",
  "txt",
  "pdf",
]);

/** ドロップしたパスを、フォルダとみなすものとファイルに分ける。 */
export function splitDroppedPaths(paths: readonly string[]): { folders: string[]; files: string[] } {
  const folders: string[] = [];
  const files: string[] = [];
  for (const p of paths) {
    const name = baseName(p);
    const i = name.lastIndexOf(".");
    const ext = i > 0 ? name.slice(i + 1).toLowerCase() : "";
    if (ext && FILE_EXTENSIONS.has(ext)) files.push(p);
    else folders.push(p);
  }
  return { folders, files };
}

/** ffmpeg・ffprobe が見つからないときの注意（見つかっていれば null）。決定事項ログ No.62。 */
export function videoToolsWarning(tools: ExternalTools | null | undefined): string | null {
  if (!tools) return null;
  const missing: string[] = [];
  if (!tools.ffprobe) missing.push("ffprobe（動画の長さ・fps・コーデック・作成日時などの情報）");
  if (!tools.ffmpeg) missing.push("ffmpeg（動画のサムネイル）");
  if (missing.length === 0) return null;
  return `次のものが見つかりません: ${missing.join("、")}。動画は登録できますが、これらは取れません。`;
}
