// 現像プリセット（DEV-29）・書き出しプリセット（EXP-02）の一覧と操作、スナップショット（DEV-28）の操作。
//
// - プリセットはコアがアプリのデータのフォルダに 1 件 1 ファイルで保存する（カタログをまたいで使う。genzo-api の
//   presets.rs）。一覧はコアの presets で読み、作る・変える・消す・読み込むたびに読み直す。
// - 現像プリセットの適用はコアの apply_develop_preset（設定の貼り付けと同じく、含める項目だけを写し、写真の処理
//   バージョンは変えない。動画は除いて、除いた数を返す）。現像中の写真が含まれていれば、送信の列に入れて順序を保ち（送信待ちの値で上書きしない）、
//   適用した後にコアの状態を読み直す（履歴も読み直る）。
// - スナップショットの作成・適用も送信の列の中で行う（今の状態から作るときは、送信待ちの値を先に送り、コアが保存して
//   から写す。適用は履歴に 1 件記録する）。一覧は snapshotsVersion が増えたら読み直す。

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type {
  HistoryEntryId,
  PasteReport,
  PasteVersion,
  PresetInfo,
  PresetKind,
  SnapshotId,
  SnapshotInfo,
  SnapshotSource,
  VariantId,
} from "../api/types";
import { describePresetApply, describePresetImport } from "../develop/presetLogic";
import { chooseFolder, choosePresetFiles } from "../dialogs/native";
import { actions } from "./app";
import { developStore, enqueue, reloadState } from "./develop";
import { createStore, useStore } from "./store";

export interface PresetsState {
  /** 現像プリセット（まだ読んでいなければ null）。 */
  develop: PresetInfo[] | null;
  /** 書き出しプリセット（まだ読んでいなければ null）。 */
  export: PresetInfo[] | null;
  /** プリセットのフォルダ（コアが分からなければ null）。 */
  dir: string | null;
  /** 読めなかった理由（種類ごと）。 */
  errors: Partial<Record<PresetKind, string>>;
  /** スナップショットの一覧を読み直す合図（作る・名前の変更・削除・適用で増える）。 */
  snapshotsVersion: number;
}

export const presetsStore = createStore<PresetsState>({
  develop: null,
  export: null,
  dir: null,
  errors: {},
  snapshotsVersion: 0,
});

export const usePresets = <T>(selector: (s: PresetsState) => T, equal?: (a: T, b: T) => boolean): T =>
  useStore(presetsStore, selector, equal);

const seq: Record<PresetKind, number> = { develop: 0, export: 0 };

/** 一覧を読み直す（古い要求の結果は捨てる）。読めたら一覧を返す。 */
export async function reloadPresets(kind: PresetKind): Promise<PresetInfo[] | null> {
  const my = ++seq[kind];
  try {
    const r = await core.presets(kind);
    if (my !== seq[kind]) return null;
    presetsStore.set((s) => ({
      [kind]: r.presets,
      dir: r.dir,
      errors: { ...s.errors, [kind]: undefined },
    }));
    return r.presets;
  } catch (e) {
    if (my !== seq[kind]) return null;
    const message = toApiError(e).info.message;
    presetsStore.set((s) => ({ errors: { ...s.errors, [kind]: message } }));
    return null;
  }
}

/**
 * 現像プリセットを写真に適用する（DEV-29）。現像中の写真が含まれていれば送信の列で順序を保ち、適用した後にコアの
 * 状態を読み直す。結果を知らせる（処理バージョンが古いために使われない項目があれば警告）。失敗は投げる
 * （呼び出し側がダイアログ・知らせに出す）。
 */
export async function applyDevelopPreset(
  preset: Pick<PresetInfo, "id" | "name">,
  targets: VariantId[],
  version: PasteVersion = "keep",
): Promise<void> {
  const ids = [...new Set(targets)];
  if (ids.length === 0) return;
  const developing = developStore.get().variantId;
  const includesDeveloping = developing !== null && ids.includes(developing);
  let report: PasteReport = { count: 0, upgraded: 0, needs_upgrade: 0, skipped_videos: 0 };
  let failure: unknown = null;
  // 失敗は列の外で扱う（列の中で投げると、現像の画面のエラーとして出てしまう）。
  const run = async () => {
    try {
      report = await core.applyDevelopPreset(preset.id, ids, version);
    } catch (e) {
      failure = e;
    }
  };
  if (includesDeveloping) await enqueue(run);
  else await run();
  if (failure !== null) throw failure;
  if (includesDeveloping) await reloadState();
  const done = describePresetApply(report, preset.name);
  actions.notify("success", done.message);
  if (done.warning !== null) actions.notify("warning", done.warning);
}

/** プリセットのファイルを選んで読み込む（取り消したら何もしない）。読み込んだら一覧を読み直す。 */
export async function importPresetFiles(kind: PresetKind): Promise<void> {
  let paths: string[];
  try {
    paths = await choosePresetFiles(kind === "develop" ? "現像プリセットを読み込む" : "書き出しプリセットを読み込む");
  } catch (e) {
    actions.reportError(e, "ファイルを選べません");
    return;
  }
  if (paths.length === 0) return;
  try {
    const r = await core.importPresets(paths, kind);
    const d = describePresetImport(r);
    actions.notify(d.level, d.message, d.detail);
  } catch (e) {
    actions.reportError(e, "プリセットを読み込めません");
  }
  await reloadPresets(kind);
}

/** プリセットをファイルに書き出す（フォルダを選ぶ。同じ名前があれば -1 などを付け、上書きしない）。 */
export async function exportPresetFile(kind: PresetKind, preset: Pick<PresetInfo, "id" | "name">): Promise<void> {
  let dir: string | null;
  try {
    dir = await chooseFolder(`プリセット「${preset.name}」を書き出すフォルダを選ぶ`);
  } catch (e) {
    actions.reportError(e, "フォルダを選べません");
    return;
  }
  if (!dir) return;
  try {
    const paths = await core.exportPresetFiles(kind, [preset.id], dir);
    actions.notify("success", `プリセット「${preset.name}」を書き出しました`, paths.join("\n"));
  } catch (e) {
    actions.reportError(e, "プリセットを書き出せません");
  }
}

// ---------------------------------------------------------------------------
// スナップショット（DEV-28）
// ---------------------------------------------------------------------------

function bumpSnapshots(): void {
  presetsStore.set((s) => ({ snapshotsVersion: s.snapshotsVersion + 1 }));
}

/**
 * スナップショットを作る（送信の列の中で。今の状態から作るときは、送信待ちの値を先に送り、コアが保存してから写す）。
 * 保存待ちの変更が履歴に入ることがあるので、作った後にコアの状態（履歴）も読み直す。失敗は投げる。
 */
export async function createSnapshot(
  variantId: VariantId,
  name: string,
  entryId: HistoryEntryId | null = null,
): Promise<SnapshotInfo> {
  const source: SnapshotSource = entryId === null ? { kind: "current" } : { kind: "history", entry_id: entryId };
  const out: { created: SnapshotInfo | null; failure: unknown } = { created: null, failure: null };
  await enqueue(async () => {
    try {
      out.created = await core.createSnapshot(variantId, name, source);
    } catch (e) {
      out.failure = e;
    }
  });
  if (out.created === null) throw out.failure ?? new Error("スナップショットを作れません");
  bumpSnapshots();
  if (developStore.get().variantId === variantId) await reloadState();
  return out.created;
}

/** スナップショットの名前を変える。失敗は投げる。 */
export async function renameSnapshot(snapshotId: SnapshotId, name: string): Promise<void> {
  await core.renameSnapshot(snapshotId, name);
  bumpSnapshots();
}

/** スナップショットを削除する。 */
export async function deleteSnapshot(snapshotId: SnapshotId): Promise<void> {
  try {
    await core.deleteSnapshot(snapshotId);
  } catch (e) {
    actions.reportError(e, "スナップショットを削除できません");
  }
  bumpSnapshots();
}

/** スナップショットを適用する（送信の列の中で。履歴に 1 件記録する）。終わったらコアの状態を読み直す。 */
export async function applySnapshot(snapshot: Pick<SnapshotInfo, "id" | "name">): Promise<void> {
  let failure: unknown = null;
  await enqueue(async () => {
    try {
      await core.applySnapshot(snapshot.id);
    } catch (e) {
      failure = e;
    }
  });
  if (failure !== null) {
    actions.reportError(failure, "スナップショットを適用できません");
  } else {
    actions.notify("info", `スナップショット「${snapshot.name}」を適用しました（元に戻せます）`);
  }
  await reloadState();
}
