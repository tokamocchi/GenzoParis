// アプリの中でのファイル操作（FILE-02）・ドライブのつながり（FILE-03）・見つからない写真（FILE-04）の、画面からの
// 操作（ダイアログを開く・ジョブを始めて結果を知らせる）。
//
// - 写真の移動はジョブ（別のドライブへはコピーと照合をするので時間がかかる）。始めたらダイアログを閉じ、進捗は
//   上の帯のジョブの表示に出す。終わったら結果を知らせる（詳細は上の帯の「結果」で見直せる）。
// - 名前の変更・フォルダの操作・再リンク・場所の指定し直しはすぐ終わるので、ダイアログの中で待つ
//   （dialogs/FileDialogs.tsx）。
// - 見つからない写真の確認: 利用者が始めたものは結果をいつも知らせる。カタログを開いたときなどにコアが自動で
//   始めたものは、変わりがあったときだけ知らせる（state/app.ts の job_finished）。
// - ドライブのつながりは、ウィンドウに戻ったときに確かめ直す（VOLUME_REFRESH_MS に 1 回まで）。つながりが
//   変わると、コアがフォルダの変更（catalog_changed の folders）を知らせ、フォルダの一覧を読み直す。

import * as core from "../api/core";
import type { FolderId, VariantId, VolumeInfo } from "../api/types";
import { waitJobUntilClosed } from "../dialogs/catalogActions";
import { formatCount } from "../dialogs/dialogFormat";
import { notifyResult } from "../dialogs/resultLog";
import { describeCheckJob, describeMoveJob, missingOnlyPatch } from "../library/fileOpsLogic";
import { actions, appStore } from "./app";

/** ウィンドウに戻ったときにドライブのつながりを確かめ直す間隔の下限（ms。**仮置き**: 10 秒）。 */
export const VOLUME_REFRESH_MS = 10_000;

/** 写真の移動のダイアログを開く（`destFolder` は落とした先のフォルダ。null ならダイアログで選ぶ）。 */
export function openMoveFiles(destFolder: FolderId | null, variantIds: VariantId[] = actions.targets()): void {
  if (variantIds.length === 0) {
    actions.notify("info", "移す写真を選んでください");
    return;
  }
  actions.openDialog({ kind: "move_files", variantIds, destFolder });
}

/** 写真の名前の変更のダイアログを開く（F2。操作の対象の写真）。 */
export function openRenameFiles(): void {
  const variantIds = actions.targets();
  if (variantIds.length === 0) {
    actions.notify("info", "名前を変える写真を選んでください");
    return;
  }
  actions.openDialog({ kind: "rename_files", variantIds });
}

/** 見つからない写真を探して結び付け直すダイアログを開く（null ならすべての見つからない写真）。 */
export function openRelink(variantIds: VariantId[] | null): void {
  actions.openDialog({ kind: "relink", variantIds });
}

/** 写真の移動を始める（失敗はそのまま投げる。ダイアログが出す）。始めたらダイアログを閉じる。 */
export async function startMoveFiles(variantIds: VariantId[], destFolder: FolderId, destLabel: string): Promise<void> {
  const jobId = await core.moveFiles(variantIds, destFolder);
  actions.trackJob(jobId, "move_files", `写真の移動: ${formatCount(variantIds.length)} 枚 → ${destLabel}`);
  actions.closeDialog();
  void followMoveJob(jobId);
}

async function followMoveJob(jobId: number): Promise<void> {
  try {
    const info = await waitJobUntilClosed(jobId);
    if (info === "closed") return;
    const notice = describeMoveJob(info);
    if (notice) notifyResult(notice);
  } catch (e) {
    actions.reportError(e, "写真の移動の結果を受け取れません");
  }
}

/** 見つからない写真を確かめる（フォルダを与えれば、その配下だけ）。終わったら結果を知らせる。 */
export async function startCheckMissing(folderId: FolderId | null, label?: string): Promise<void> {
  let jobId: number;
  try {
    jobId = await core.checkMissing(folderId);
  } catch (e) {
    actions.reportError(e, "見つからない写真を確かめられません");
    return;
  }
  actions.trackJob(jobId, "check_missing", label ? `見つからない写真の確認: ${label}` : undefined);
  try {
    const info = await waitJobUntilClosed(jobId);
    if (info === "closed") return;
    const notice = describeCheckJob(info, true);
    if (notice) notifyResult(notice);
  } catch (e) {
    actions.reportError(e, "見つからない写真の確認の結果を受け取れません");
  }
}

/** 見つからない写真だけを表示する（フォルダを与えれば、そのフォルダ（サブフォルダを含む）の中だけ）。 */
export function showMissingOnly(folderId?: FolderId): void {
  if (folderId !== undefined) actions.showFolder(folderId, true);
  actions.setFilter(missingOnlyPatch(true));
}

let lastVolumeRefresh = Number.NEGATIVE_INFINITY;
let volumeRefreshing: Promise<VolumeInfo[] | null> | null = null;

/**
 * ドライブのつながりを確かめ直す。`force` でなければ、前に確かめてから VOLUME_REFRESH_MS が経つまでは
 * 確かめない（null を返す）。カタログを開いていなければ何もしない。
 */
export function refreshVolumes(force = false, now = Date.now()): Promise<VolumeInfo[] | null> {
  if (appStore.get().catalog === null) return Promise.resolve(null);
  if (volumeRefreshing) return volumeRefreshing;
  if (!force && now - lastVolumeRefresh < VOLUME_REFRESH_MS) return Promise.resolve(null);
  lastVolumeRefresh = now;
  volumeRefreshing = core
    .volumes(true)
    .catch((e: unknown) => {
      // 確かめられなくても、前の状態のまま使える（次に戻ったときに確かめ直す）。
      if (force) actions.reportError(e, "ドライブのつながりを確かめられません");
      else console.warn("ドライブのつながりを確かめられません", e);
      return null;
    })
    .finally(() => {
      volumeRefreshing = null;
    });
  return volumeRefreshing;
}

/** テストのため: 確かめ直した時刻を忘れる。 */
export function resetVolumeRefresh(): void {
  lastVolumeRefresh = Number.NEGATIVE_INFINITY;
  volumeRefreshing = null;
}
