// OS のファイル・フォルダの選択（Tauri の dialog プラグイン）と、ウィンドウへのドラッグ＆ドロップ（webview の
// イベント）。
//
// Tauri の外（ブラウザでの確認）では、選択は window.prompt でパスを入力させ、ドラッグ＆ドロップは受け取らない。
// 保存のダイアログ（save）は、アプリの権限（crates/genzo-app/capabilities/default.json）に dialog:allow-save が
// 要る。ないときは権限のエラーになるので、新しいカタログの場所はフォルダと名前で選ばせる（catalogPick.ts）。

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";

import { inTauri } from "../api/bridge";

const CATALOG_FILTERS = [{ name: "GenzoParis のカタログ", extensions: ["db"] }];

function promptPath(message: string, initial: string): string | null {
  if (typeof window === "undefined" || typeof window.prompt !== "function") return null;
  const v = window.prompt(message, initial);
  return v && v.trim() ? v.trim() : null;
}

function single(r: string | string[] | null): string | null {
  if (Array.isArray(r)) return r[0] ?? null;
  return r;
}

/** フォルダを選ぶ（取り消したら null）。 */
export async function chooseFolder(title: string, defaultPath?: string | null): Promise<string | null> {
  if (!inTauri()) return promptPath(`${title}（フォルダのパス）`, defaultPath ?? "");
  const r = await open({ title, directory: true, multiple: false, defaultPath: defaultPath || undefined });
  return single(r);
}

/** 既存のカタログ（.db）を選ぶ。 */
export async function chooseCatalogFile(title: string, defaultPath?: string | null): Promise<string | null> {
  if (!inTauri()) return promptPath(`${title}（カタログのパス）`, defaultPath ?? "");
  const r = await open({
    title,
    directory: false,
    multiple: false,
    filters: CATALOG_FILTERS,
    defaultPath: defaultPath || undefined,
  });
  return single(r);
}

const PROFILE_FILTERS = [
  { name: "カメラプロファイル・ルック", extensions: ["dcp", "DCP", "cube", "CUBE"] },
  { name: "カメラプロファイル（DCP）", extensions: ["dcp", "DCP"] },
  { name: "ルック（3D LUT）", extensions: ["cube", "CUBE"] },
];

/**
 * 取り込むカメラプロファイル（.dcp）・ルック（.cube）のファイルを選ぶ（複数可。取り消したら空）。Tauri の外では
 * パスを「;」で区切って入力させる。
 */
export async function chooseProfileFiles(title: string): Promise<string[]> {
  if (!inTauri()) {
    const v = promptPath(`${title}（ファイルのパス。複数は「;」で区切る）`, "");
    return v
      ? v
          .split(";")
          .map((p) => p.trim())
          .filter((p) => p.length > 0)
      : [];
  }
  const r = await open({ title, directory: false, multiple: true, filters: PROFILE_FILTERS });
  if (r === null) return [];
  return Array.isArray(r) ? r : [r];
}

const PRESET_FILTERS = [{ name: "GenzoParis のプリセット", extensions: ["json", "JSON"] }];

/**
 * 読み込むプリセットのファイル（.json。複数可。取り消したら空）を選ぶ（DEV-29・EXP-02）。Tauri の外ではパスを「;」で
 * 区切って入力させる。
 */
export async function choosePresetFiles(title: string): Promise<string[]> {
  if (!inTauri()) {
    const v = promptPath(`${title}（ファイルのパス。複数は「;」で区切る）`, "");
    return v
      ? v
          .split(";")
          .map((p) => p.trim())
          .filter((p) => p.length > 0)
      : [];
  }
  const r = await open({ title, directory: false, multiple: true, filters: PRESET_FILTERS });
  if (r === null) return [];
  return Array.isArray(r) ? r : [r];
}

const FONT_FILTERS = [{ name: "フォント（TrueType・OpenType）", extensions: ["ttf", "otf", "ttc", "otc", "TTF", "OTF", "TTC", "OTC"] }];
const WATERMARK_IMAGE_FILTERS = [
  { name: "画像（PNG・JPEG・TIFF）", extensions: ["png", "jpg", "jpeg", "tif", "tiff", "PNG", "JPG", "JPEG", "TIF", "TIFF"] },
];

/**
 * 透かしの文字のフォントのファイルを選ぶ（EXP-03。OS のフォントのフォルダから。アプリはフォントを同梱しない）。取り消したら
 * null。
 */
export async function chooseFontFile(title: string, defaultPath?: string | null): Promise<string | null> {
  if (!inTauri()) return promptPath(`${title}（フォントのファイルのパス）`, defaultPath ?? "");
  const r = await open({ title, directory: false, multiple: false, filters: FONT_FILTERS, defaultPath: defaultPath || undefined });
  return single(r);
}

/** 透かしの画像のファイルを選ぶ（EXP-03）。取り消したら null。 */
export async function chooseWatermarkImage(title: string, defaultPath?: string | null): Promise<string | null> {
  if (!inTauri()) return promptPath(`${title}（画像のファイルのパス）`, defaultPath ?? "");
  const r = await open({
    title,
    directory: false,
    multiple: false,
    filters: WATERMARK_IMAGE_FILTERS,
    defaultPath: defaultPath || undefined,
  });
  return single(r);
}

/** 新しいカタログの場所を選ぶ（保存のダイアログ）。 */
export async function chooseNewCatalogPath(title: string, defaultPath?: string | null): Promise<string | null> {
  if (!inTauri()) return promptPath(`${title}（新しいカタログのパス）`, defaultPath ?? "");
  return save({ title, filters: CATALOG_FILTERS, defaultPath: defaultPath || undefined });
}

/** ドラッグ＆ドロップの知らせ（位置は使わない）。 */
export type DragDropNotice = { type: "enter"; paths: string[] } | { type: "leave" } | { type: "drop"; paths: string[] };

/** ウィンドウへのドラッグ＆ドロップを受け取る（戻り値で解除。Tauri の外では何もしない）。 */
export async function listenDragDrop(handler: (e: DragDropNotice) => void): Promise<() => void> {
  if (!inTauri()) return () => undefined;
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "enter") handler({ type: "enter", paths: p.paths });
    else if (p.type === "drop") handler({ type: "drop", paths: p.paths });
    else if (p.type === "leave") handler({ type: "leave" });
  });
}
