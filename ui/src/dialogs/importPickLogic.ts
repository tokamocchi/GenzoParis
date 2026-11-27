// 取り込む前の選択（IMP-02 の残り。取り込みのダイアログの「取り込むファイルを選ぶ」）の計算（DOM を使わない）。
//
// コア（scan_import_source）が、取り込み元のフォルダの取り込める写真・動画の一覧を返す（何も登録しない）。画面は
// サムネイル（genzo:// の /import-thumb/{走査の番号}/{一覧の中の番号}）を並べ、取り込むファイルを選ばせる。
// 選んだファイルの相対パスを、取り込みの設定の only に渡す（すべて選んでいれば渡さない＝フォルダの全体）。

import type { ImportSourceFile, ImportSourceScan } from "../api/types";

/** 選んでいるファイル（相対パスの集合）。 */
export type PickSelection = ReadonlySet<string>;

/** すべてを選んだ状態。 */
export function selectAll(scan: ImportSourceScan): Set<string> {
  return new Set(scan.files.map((f) => f.rel_path));
}

/** 1 つを選ぶ・外す。 */
export function togglePick(selection: PickSelection, relPath: string): Set<string> {
  const next = new Set(selection);
  if (next.has(relPath)) next.delete(relPath);
  else next.add(relPath);
  return next;
}

/**
 * 範囲を選ぶ・外す（Shift ＋ クリック）: 一覧の `from` 番目から `to` 番目まで（両端を含む。順は問わない）を、`on` に
 * そろえる。
 */
export function setPickRange(scan: ImportSourceScan, selection: PickSelection, from: number, to: number, on: boolean): Set<string> {
  const next = new Set(selection);
  const lo = Math.max(0, Math.min(from, to));
  const hi = Math.min(scan.files.length - 1, Math.max(from, to));
  for (let i = lo; i <= hi; i++) {
    const p = scan.files[i].rel_path;
    if (on) next.add(p);
    else next.delete(p);
  }
  return next;
}

/** 種類（写真・動画）のファイルだけを選んだ状態。 */
export function selectKind(scan: ImportSourceScan, kind: ImportSourceFile["kind"]): Set<string> {
  return new Set(scan.files.filter((f) => f.kind === kind).map((f) => f.rel_path));
}

/**
 * 取り込みの設定の only に渡す値: 選んだファイルの相対パス（一覧の順）。すべて選んでいる・一覧を切り詰めている
 * （選んで取り込めない）なら undefined（フォルダの全体を取り込む）。
 */
export function onlyOf(scan: ImportSourceScan, selection: PickSelection): string[] | undefined {
  if (scan.truncated) return undefined;
  const picked = scan.files.filter((f) => selection.has(f.rel_path)).map((f) => f.rel_path);
  return picked.length === scan.files.length ? undefined : picked;
}

/** 選んでいる数（一覧にあるものだけを数える）。 */
export function pickedCount(scan: ImportSourceScan, selection: PickSelection): number {
  return scan.files.reduce((n, f) => n + (selection.has(f.rel_path) ? 1 : 0), 0);
}

const n = (v: number) => v.toLocaleString("ja-JP");

/** 一覧の説明（数と、選んでいる数）。 */
export function describePick(scan: ImportSourceScan, selection: PickSelection): string {
  const photos = scan.files.filter((f) => f.kind === "photo").length;
  const videos = scan.files.length - photos;
  const kinds = videos > 0 ? `写真 ${n(photos)}・動画 ${n(videos)}` : `写真 ${n(photos)}`;
  return `${n(pickedCount(scan, selection))} / ${n(scan.files.length)} 件を取り込みます（${kinds}）`;
}

/** 一覧についての注意（切り詰めた・読めないものがあった）。なければ空。 */
export function pickNotes(scan: ImportSourceScan): string[] {
  const notes: string[] = [];
  if (scan.truncated) {
    notes.push(
      `取り込めるファイルが ${n(scan.total)} 件あり、多すぎるため先頭の ${n(scan.files.length)} 件だけを表示しています。この場合は選んで取り込めません（フォルダの全体を取り込みます）。フォルダを分けて選び直してください。`,
    );
  }
  if (scan.unreadable > 0) notes.push(`読めなかったフォルダ・ファイルが ${n(scan.unreadable)} 件あります（取り込みの結果で確かめられます）`);
  return notes;
}

/** ファイルの大きさの表示（"12.3 MB"。分からなければ空）。 */
export function sizeText(bytes: number | null): string {
  if (bytes === null || !(bytes >= 0)) return "";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[u]}`;
}
