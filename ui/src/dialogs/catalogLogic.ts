// カタログの選択・開けないときの画面の判断（DOM を使わない純粋な計算）。

import type { CatalogFileState, ErrorInfo } from "../api/types";
import { baseName } from "./dialogFormat";

/** 選んだパスの扱い（新しく作る: 保存のダイアログ、既存を開く: 開くのダイアログ）。 */
export type CatalogIntent = "new" | "existing";

/**
 * 選んだパスの状態から、開き方を決める（アプリの inspect_catalog_path の結果）。
 *
 * - 新しく作る: ないなら作る。GenzoParis のカタログなら、そのまま開く。ほかのファイル・フォルダは上書き
 *   しないので断る。
 * - 既存を開く: カタログなら開く。ないもの・ほかのファイルは断る。
 */
export function decideCatalogPath(
  state: CatalogFileState,
  intent: CatalogIntent,
): { create: boolean } | { error: string } {
  switch (state) {
    case "catalog":
      return { create: false };
    case "missing":
      return intent === "new" ? { create: true } : { error: "ファイルが見つかりません" };
    case "not_catalog":
      return {
        error:
          intent === "new"
            ? "同じ名前のファイル（またはフォルダ）があり、GenzoParis のカタログではありません。上書きしないので、別の名前か場所を選んでください"
            : "GenzoParis のカタログではありません。カタログ（.db）のファイルを選んでください",
      };
  }
}

/**
 * 復元で退避する今のカタログの名前の形（genzo-api の prepare_restore: `<名前>-before-restore-<日時>.db`。
 * 日時は復元した時刻の UTC）。
 */
export function restoreDisplacedName(catalogPath: string): string {
  const name = baseName(catalogPath);
  const i = name.lastIndexOf(".");
  const stem = i > 0 ? name.slice(0, i) : name || "catalog";
  return `${stem}-before-restore-（日時）.db`;
}

/** バックアップからの復元をどう案内するか。 */
export type RestoreGuide = "open" | "collapsed" | "none";

export interface CatalogErrorGuide {
  title: string;
  guidance: string[];
  restore: RestoreGuide;
}

/** カタログを開けなかった理由ごとの見出しと案内。 */
export function catalogErrorGuide(error: ErrorInfo): CatalogErrorGuide {
  switch (error.kind) {
    case "catalog_corrupt":
      return {
        title: "カタログが壊れている可能性があります",
        guidance: ["バックアップから復元できます（下の一覧から選んでください）。"],
        restore: "open",
      };
    case "conflict":
      return {
        title: "カタログをほかのアプリが使っています",
        guidance: ["このカタログを開いている GenzoParis（アプリ・CLI）を終了してから「再試行」を押してください。"],
        restore: "none",
      };
    case "incompatible_version":
      return {
        title: "このカタログの版が、このアプリと合いません",
        guidance: ["カタログは変更していません。下の案内に従ってください（アプリの更新など）。"],
        restore: "none",
      };
    case "not_found":
      return {
        title: "カタログが見つかりません",
        guidance: [
          "カタログを置いたドライブ（外付けのドライブなど）が接続されているか確かめて「再試行」を押すか、別のカタログを開いてください。",
          "カタログのファイルを失った場合は、バックアップから復元できることがあります（下の一覧）。",
        ],
        restore: "open",
      };
    case "invalid_input":
      return {
        title: "カタログとして開けません",
        guidance: ["GenzoParis のカタログ（.db）を選ぶか、新しいカタログを作ってください。"],
        restore: "none",
      };
    case "file_access":
    case "storage":
      return {
        title: "カタログのファイルを読み書きできません",
        guidance: ["ドライブの接続・空き容量・フォルダの権限を確かめてから「再試行」を押してください。"],
        restore: "collapsed",
      };
    default:
      return {
        title: "カタログを開けません",
        guidance: ["「再試行」を押すか、別のカタログを開いてください。"],
        restore: "collapsed",
      };
  }
}
