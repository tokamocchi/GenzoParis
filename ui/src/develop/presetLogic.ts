// スナップショット（DEV-28）・現像プリセット（DEV-29）の画面の計算（DOM を使わない純粋な計算）。
//
// - 名前の規則はコアと同じ（genzo-api の presets.rs: 前後の空白を除いて 1〜100 文字・制御文字なし。フォルダは空でも
//   よい。genzo-catalog の develop.rs: スナップショットの名前は 1〜200 文字・制御文字なし）。画面でも確かめて、
//   コアのエラーはダイアログの中に出す。
// - プリセットの一覧はコアがフォルダ → 名前の順に並べて返す。画面はフォルダごとにまとめる（フォルダなしを先に）。
// - プリセットを当てた結果の知らせは、設定の貼り付けと同じ形（処理バージョンが古い写真の警告。pasteLogic.ts）。
// - 動画には適用しない: コアが動画を除いて写真にだけ適用し、除いた数（PasteReport の skipped_videos）を返す。
//   ダイアログは、読み込み済みで動画と分かっているものの数を先に知らせる（presetApplyTargets）。

import type { PasteReport, PresetInfo, SettingGroups, VariantId } from "../api/types";
import { PASTE_GROUP_ITEMS, type PasteSelection } from "../dialogs/pasteLogic";

/** プリセットの名前・フォルダの名前の文字数の上限（genzo-api の MAX_PRESET_NAME_CHARS）。 */
export const MAX_PRESET_NAME_CHARS = 100;
/** スナップショットの名前の文字数の上限（genzo-catalog の MAX_SNAPSHOT_NAME_CHARS）。 */
export const MAX_SNAPSHOT_NAME_CHARS = 200;

/** 改行・タブなどの制御文字を含むか。 */
function hasControl(s: string): boolean {
  return /[\u0000-\u001f\u007f-\u009f]/.test(s);
}

/** 名前を確かめる（問題がなければ null）。`allowEmpty` ならフォルダの名前（空はフォルダなし）。 */
function checkName(name: string, what: string, max: number, allowEmpty = false): string | null {
  const t = name.trim();
  if (t === "" && !allowEmpty) return `${what}を入れてください`;
  if ([...t].length > max) return `${what}は ${max} 文字までです`;
  if (hasControl(t)) return `${what}に改行・タブなどは使えません`;
  return null;
}

export const validatePresetName = (name: string) => checkName(name, "名前", MAX_PRESET_NAME_CHARS);
export const validateFolderName = (name: string) => checkName(name, "フォルダの名前", MAX_PRESET_NAME_CHARS, true);
export const validateSnapshotName = (name: string) => checkName(name, "名前", MAX_SNAPSHOT_NAME_CHARS);

/** 新しいスナップショットの既定の名前（今の日時。「2026-10-14 14:05:03」。この PC の時刻。Lightroom と同じく日時）。 */
export function defaultSnapshotName(now: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())} ${p(now.getHours())}:${p(now.getMinutes())}:${p(now.getSeconds())}`;
}

/** フォルダごとのプリセット（フォルダなし（""）を先に、ほかはコアの並びのまま）。 */
export interface PresetFolder {
  folder: string;
  presets: PresetInfo[];
}

export function groupPresets(presets: readonly PresetInfo[]): PresetFolder[] {
  const map = new Map<string, PresetInfo[]>();
  for (const p of presets) {
    const list = map.get(p.folder);
    if (list) list.push(p);
    else map.set(p.folder, [p]);
  }
  const out: PresetFolder[] = [];
  const top = map.get("");
  if (top) out.push({ folder: "", presets: top });
  for (const [folder, list] of map) if (folder !== "") out.push({ folder, presets: list });
  return out;
}

/** 使われているフォルダの名前（入力欄の候補）。 */
export function presetFolders(presets: readonly PresetInfo[]): string[] {
  return [...new Set(presets.map((p) => p.folder).filter((f) => f !== ""))];
}

/** 含める項目の名前の並び（「基本補正・外観」。すべてなら「すべての項目」）。 */
export function describeGroups(groups: SettingGroups | undefined): string {
  if (!groups) return "";
  const on = PASTE_GROUP_ITEMS.filter(({ key }) => groups[key] ?? true);
  if (on.length === PASTE_GROUP_ITEMS.length) return "すべての項目";
  return on.map((i) => i.label).join("・");
}

/** プリセットの項目の選択（ダイアログの初めの値）。プリセットがなければ `fallback`。 */
export function selectionOf(groups: SettingGroups | undefined, fallback: PasteSelection): PasteSelection {
  if (!groups) return { ...fallback };
  const out = { ...fallback };
  for (const { key } of PASTE_GROUP_ITEMS) out[key] = groups[key] ?? true;
  return out;
}

/** 一覧の項目の説明（マウスを乗せたとき）。 */
export function presetTooltip(p: PresetInfo): string {
  if (p.problem) return `使えません: ${p.problem}`;
  const lines = [`含める項目: ${describeGroups(p.groups)}`];
  if (p.process_version !== undefined) lines.push(`処理バージョン ${p.process_version} で作ったプリセット`);
  lines.push("クリックで今の写真に適用（右クリックでほかの操作）");
  return lines.join("\n");
}

/**
 * 現像プリセットの適用先の数（重なりは 1 つ）と、動画の知らせ。`knownVideos` は読み込み済みで動画と分かっている
 * ものの数（読み込んでいないものはコアが除く）。
 */
export function presetApplyTargets(
  targets: readonly VariantId[],
  knownVideos: number,
): { photos: number; videos: number; note: string | null } {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const total = new Set(targets).size;
  const videos = Math.min(Math.max(knownVideos, 0), total);
  const photos = total - videos;
  let note: string | null = null;
  if (videos > 0 && photos === 0) {
    note = `選んだ ${n(videos)} 件はすべて動画です。動画には現像プリセットを適用できません。`;
  } else if (videos > 0) {
    note = `選んだうち動画 ${n(videos)} 件には適用しません（写真 ${n(photos)} 枚に適用します）。`;
  }
  return { photos, videos, note };
}

/** プリセットを当てた結果の知らせ（成功の文と、処理バージョンが古いために使われない項目がある写真の警告）。 */
export function describePresetApply(r: PasteReport, name: string): { message: string; warning: string | null } {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const upgraded = r.upgraded > 0 ? `（${n(r.upgraded)} 枚の処理バージョンを更新）` : "";
  const skipped = (r.skipped_videos ?? 0) > 0 ? `（動画 ${n(r.skipped_videos ?? 0)} 件は除きました）` : "";
  const target = r.count === 1 ? "" : `${n(r.count)} 枚に`;
  return {
    message:
      r.count === 0 && skipped !== ""
        ? `プリセット「${name}」を適用した写真はありません${skipped}`
        : `プリセット「${name}」を${target}適用しました${upgraded}${skipped}`,
    warning:
      r.needs_upgrade > 0
        ? `${n(r.needs_upgrade)} 枚は処理バージョンが古いため、プリセットの項目の一部（レンズ補正・ハイライト復元・ノイズ軽減・` +
          "カメラプロファイル・カラーミキサー・明瞭度・シャープ・効果・マスクなど）は使われません" +
          "（写真を右クリックして「処理バージョンを最新に更新…」で更新できます）"
        : null,
  };
}

/** プリセットのファイルの読み込みの結果の知らせ。 */
export function describePresetImport(r: {
  imported: PresetInfo[];
  failed: { path: string; message: string }[];
}): { level: "success" | "warning" | "error"; message: string; detail?: string } {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const lines = [
    ...r.imported.map((p) => `読み込み: ${p.folder ? `${p.folder} / ` : ""}${p.name}`),
    ...r.failed.map((f) => `読み込めません: ${f.path}（${f.message}）`),
  ];
  const detail = lines.length > 0 ? lines.join("\n") : undefined;
  if (r.failed.length === 0) return { level: "success", message: `${n(r.imported.length)} 件のプリセットを読み込みました`, detail };
  if (r.imported.length === 0) return { level: "error", message: `プリセットを読み込めませんでした（${n(r.failed.length)} 件）`, detail };
  return {
    level: "warning",
    message: `${n(r.imported.length)} 件のプリセットを読み込みました（${n(r.failed.length)} 件は読み込めません）`,
    detail,
  };
}
