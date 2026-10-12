// 取り込み（IMP-01・VID-01）のダイアログとドラッグ＆ドロップの判定（DOM を使わない純粋な計算）。

import type { ExternalTools } from "../api/types";
import { baseName } from "./dialogFormat";

/** パスの種類（アプリの path_kinds。api/app.ts の pathKinds）。 */
export type PathKind = "dir" | "file" | "missing";

/**
 * フォルダではないと分かる拡張子（小文字）。ドロップしたものがファイルなら取り込まない（取り込みはフォルダ
 * 単位）。genzo-api の import.rs の RAW_EXTENSIONS・IMAGE_EXTENSIONS・VIDEO_EXTENSIONS に、よく一緒に
 * 扱うファイル（カタログ・サイドカー・ほかの RAW・画像・動画・文書など）を加えたもの。
 *
 * ドロップしたものの種類は、アプリの path_kinds（api/app.ts の pathKinds）で確かめる（Dialogs.tsx）。これは
 * それを確かめられなかったときの、名前からの推定（splitDroppedPaths）にだけ使う。ここにない拡張子の名前
 * （"2024.05.01"・"2024.Kyoto" のようなフォルダもある）はフォルダとみなす。実際はファイルなら、取り込みの
 * ジョブが「フォルダではありません」で失敗する。
 */
const FILE_EXTENSIONS = new Set([
  // GenzoParis が取り込むもの
  "arw",
  "dng",
  "jpg",
  "jpeg",
  "tif",
  "tiff",
  "png",
  "mp4",
  "mov",
  // ほかの RAW
  "cr2",
  "cr3",
  "crw",
  "nef",
  "nrw",
  "raf",
  "orf",
  "rw2",
  "rwl",
  "pef",
  "srw",
  "sr2",
  "srf",
  "3fr",
  "iiq",
  "x3f",
  "erf",
  "mrw",
  "dcr",
  "kdc",
  "mos",
  // ほかの画像
  "heic",
  "heif",
  "hif",
  "gif",
  "webp",
  "avif",
  "jxl",
  "bmp",
  "psd",
  "psb",
  "svg",
  // ほかの動画・音声
  "mts",
  "m2ts",
  "mkv",
  "avi",
  "wmv",
  "m4v",
  "mpg",
  "mpeg",
  "3gp",
  "lrv",
  "thm",
  "mp3",
  "wav",
  "aac",
  "m4a",
  // カタログ・サイドカー・文書・圧縮ファイル
  "xmp",
  "db",
  "lrcat",
  "dop",
  "pp3",
  "json",
  "xml",
  "csv",
  "txt",
  "pdf",
  "doc",
  "docx",
  "xls",
  "xlsx",
  "ppt",
  "pptx",
  "zip",
  "7z",
  "rar",
  "gz",
  "tar",
  "dmg",
  "exe",
  "lnk",
]);

/** 名前の拡張子（小文字。なければ ""）。 */
function extensionOf(path: string): string {
  const name = baseName(path);
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(i + 1).toLowerCase() : "";
}

/** ドロップしたパスを、フォルダとみなすものとファイルに分ける。 */
export function splitDroppedPaths(paths: readonly string[]): { folders: string[]; files: string[] } {
  const folders: string[] = [];
  const files: string[] = [];
  for (const p of paths) {
    const ext = extensionOf(p);
    if (ext && FILE_EXTENSIONS.has(ext)) files.push(p);
    else folders.push(p);
  }
  return { folders, files };
}

/**
 * 取り込みのダイアログで注意を出すもの: アプリの path_kinds で確かめて、フォルダではなかったもの（`files`）と
 * 見つからなかったもの（`missing`）。`kinds` にないもの（「参照…」で OS のフォルダの選択から選んだもの・
 * 確かめている途中のもの・確かめられなかったもの）には出さない（名前からは推定しない。"2024.Kyoto" のような
 * フォルダもあるため）。
 */
export function nonFolderPaths(
  folders: readonly string[],
  kinds: ReadonlyMap<string, PathKind>,
): { files: string[]; missing: string[] } {
  return {
    files: folders.filter((p) => kinds.get(p) === "file"),
    missing: folders.filter((p) => kinds.get(p) === "missing"),
  };
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
