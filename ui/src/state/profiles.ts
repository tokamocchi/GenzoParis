// カメラプロファイル・ルック（DEV-17）の一覧と、取り込み・除く操作。
//
// - 一覧はコアの list_profiles（機種を渡すと、カメラプロファイルがその機種用かが分かる）。現像のパネルが表示中の
//   写真の機種で読み、取り込み・除くの後に読み直す。
// - 取り込みは OS のファイルの選択（.dcp・.cube。複数可）→ import_profiles。元のファイルは変えない（コアがカタログの
//   データのフォルダに写す）。同じ ID で内容の違うものは、置き換えを選んだときだけ置き換える。
// - ファイルの中身はコアが解析する（画面は名前・説明・注意を出すだけ）。Adobe のプロファイルなどは同梱しない。

import * as core from "../api/core";
import type { ProfileEntry, ProfileKind } from "../api/types";
import { chooseProfileFiles } from "../dialogs/native";
import { describeProfileImport, hasReplaceableFailures } from "../develop/profileLogic";
import { actions } from "./app";
import { createStore, useStore } from "./store";

export interface ProfilesState {
  /** 一覧（まだ読んでいなければ null）。 */
  entries: ProfileEntry[] | null;
  /** 一覧を読んだときの機種（matches_camera の基準）。 */
  camera: string | null;
  /** 読んでいる・取り込んでいる途中。 */
  busy: boolean;
  /** 読めなかった理由。 */
  error: string | null;
}

export const profilesStore = createStore<ProfilesState>({ entries: null, camera: null, busy: false, error: null });

export const useProfiles = <T>(selector: (s: ProfilesState) => T, equal?: (a: T, b: T) => boolean): T =>
  useStore(profilesStore, selector, equal);

let seq = 0;

/** 一覧を読み直す（`camera` はその機種用かの基準。古い要求の結果は捨てる）。 */
export async function refreshProfiles(camera: string | null): Promise<void> {
  const my = ++seq;
  profilesStore.set({ busy: true, camera });
  try {
    const entries = await core.listProfiles(camera);
    if (my !== seq) return;
    profilesStore.set({ entries, busy: false, error: null });
  } catch (e) {
    if (my !== seq) return;
    profilesStore.set({ busy: false, error: e instanceof Error ? e.message : String(e) });
  }
}

/** 一覧を、前に読んだときの機種のまま読み直す。 */
export function reloadProfiles(): Promise<void> {
  return refreshProfiles(profilesStore.get().camera);
}

/**
 * ファイルを選んで取り込む（取り消したら何もしない）。`replace` なら同じ ID の違う内容を置き換える。置き換えれば
 * 取り込めるファイルがあれば、知らせの中で案内する。取り込んだものの一覧を返す。
 */
export async function importProfileFiles(replace: boolean): Promise<ProfileEntry[]> {
  const paths = await chooseProfileFiles("カメラプロファイル（.dcp）・ルック（.cube）を取り込む");
  if (paths.length === 0) return [];
  return importProfilePaths(paths, replace);
}

/** パスを取り込む（importProfileFiles の本体。テストからも呼ぶ）。 */
export async function importProfilePaths(paths: string[], replace: boolean): Promise<ProfileEntry[]> {
  profilesStore.set({ busy: true });
  try {
    const r = await core.importProfiles(paths, replace);
    const d = describeProfileImport(r);
    const hint =
      !replace && hasReplaceableFailures(r)
        ? "同じ名前で内容の違うものは、「同じ名前のものを置き換える」を選んで取り込み直すと置き換えられます。"
        : null;
    actions.notify(d.level, d.message, [d.detail, hint].filter(Boolean).join("\n") || undefined);
    await reloadProfiles();
    return [...r.imported, ...r.replaced];
  } catch (e) {
    actions.reportError(e, "取り込めません");
    return [];
  } finally {
    profilesStore.set({ busy: false });
  }
}

/** 取り込んだものを一覧から除く（使っている写真は、標準の色・ルックなしで描いて「見つからない」と知らせる）。 */
export async function removeProfileEntry(kind: ProfileKind, id: string): Promise<boolean> {
  try {
    await core.removeProfile(kind, id);
    actions.notify("info", `「${id}」を一覧から除きました（使っている写真は、標準の色・ルックなしで表示します）`);
    await reloadProfiles();
    return true;
  } catch (e) {
    actions.reportError(e, "除けません");
    return false;
  }
}
