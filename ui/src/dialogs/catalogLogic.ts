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
 * OS のダイアログを開く権限がない（Tauri の権限の設定。crates/genzo-app/capabilities/default.json）ための
 * 失敗か。
 *
 * Tauri は、開発版では「dialog.save not allowed. Permissions associated with this command: …」、配布版では
 * 「Command plugin:dialog|save not allowed by ACL」の文で断る（tauri 2.11 の ipc/authority.rs・
 * webview/mod.rs）。
 */
export function isDialogPermissionError(e: unknown): boolean {
  let text: string;
  if (typeof e === "string") text = e;
  else if (e instanceof Error) text = e.message;
  else if (typeof e === "object" && e !== null && typeof (e as { message?: unknown }).message === "string") {
    text = (e as { message: string }).message;
  } else return false;
  return /not allowed/i.test(text) && /dialog/i.test(text);
}

/** 新しいカタログの名前に使えない文字（Windows のファイル名で使えないもの。どの OS でも断る）。 */
const INVALID_NAME_CHARS = /[\\/:*?"<>|\u0000-\u001f]/;

/** Windows で予約されている名前（拡張子を除いた部分。大文字・小文字を問わない）。 */
const RESERVED_NAMES = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i;

/**
 * 画面で入力した新しいカタログの名前を確かめ、ファイル名（拡張子 .db 付き）にする（保存のダイアログを
 * 使えないときの入力）。
 */
export function newCatalogFileName(input: string): { name: string } | { error: string } {
  const name = input.trim();
  if (name === "") return { error: "カタログの名前を入力してください" };
  if (INVALID_NAME_CHARS.test(name)) {
    return { error: '名前に次の文字は使えません: \\ / : * ? " < > |' };
  }
  if (name === "." || name === ".." || name.endsWith(".")) {
    return { error: "名前の最後に「.」は使えません" };
  }
  const withExt = /\.db$/i.test(name) ? name : `${name}.db`;
  const stem = withExt.slice(0, -3);
  if (stem === "" || stem.startsWith(".")) return { error: "名前の先頭に「.」は使えません" };
  if (RESERVED_NAMES.test(stem)) return { error: `「${stem}」は名前に使えません（OS が予約している名前です）` };
  return { name: withExt };
}

/**
 * 「再試行」で、カタログを作るか（`create`）を決める。
 *
 * 開けなかったときの理由（catalogError）には、作るつもりだったかが残らない。作るつもりで失敗した（フォルダを
 * 作れないなど）ときに「開く」で再試行すると、「見つかりません」という別のエラーになるので、作るつもり
 * だったときは、まだない（missing）なら作る。調べられなかった（null）ときも作る側にする（作る処理が同じ
 * 確認をして、読めなければ同じエラーを返す）。既にあるもの（catalog・not_catalog）は作らない。
 */
export function retryShouldCreate(wantedCreate: boolean, state: CatalogFileState | null): boolean {
  if (!wantedCreate) return false;
  return state === "missing" || state === null;
}

/**
 * 復元の確認で、今のカタログをどうするかの文（`state` は復元の前に調べたカタログの状態。分からなければ
 * null）。
 *
 * genzo-api の apply_restore は、カタログの本体があるときだけ「…-before-restore-（日時）.db」に退避する。
 * 本体がなくても、残っている -wal・-shm・-journal は同じ名前を付けて退避する。
 */
export function restoreTargetNote(catalogPath: string, state: CatalogFileState | null, folder: string): string {
  if (state === "missing") {
    return "カタログのファイルが見つからないので、退避するものはありません（同じ場所に、復元したカタログを置きます。付随するファイル（-wal など）が残っていれば、それだけを「" +
      restoreDisplacedName(catalogPath) +
      "」の名前に付けて残します）。";
  }
  return `今のカタログは「${restoreDisplacedName(catalogPath)}」の名前で同じフォルダ（${folder}）に残します（削除しません）。`;
}

/**
 * 復元の前にカタログを閉じようとして、保存待ちの現像設定を保存できずに閉じられなかったエラーか。
 *
 * アプリの restore_catalog は、この場合に閉じられなかった理由（genzo-app の state.rs の CloseError::SaveFailed
 * の info）を返す。種類（kind）では、バックアップの検証の失敗と見分けられないので、案内の文で見分ける
 * （アプリの文を変えたら、ここも直す。要確認: アプリが閉じられなかったことを別の形で返せば置き換える）。
 * 呼び出し側は、カタログが開いたままであることも確かめる。
 */
export function isCloseSaveFailure(error: ErrorInfo): boolean {
  return error.hint !== null && error.hint.includes("保存していない現像設定を捨てて閉じて");
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
