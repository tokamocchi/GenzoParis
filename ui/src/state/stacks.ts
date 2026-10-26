// スタック（LIB-12）・仮想コピー（LIB-13）の画面の操作（docs/04 の 3.10 節）。
//
// - どの操作も、コアがカタログを変えて catalog_changed（stacks・variants・variant_added）を送り、表示中の
//   検索結果を作り直す（search_updated）。選ぶ写真が変わる操作（まとめる・折りたたむ・仮想コピーを作る）は、
//   その操作を反映した世代を待たずに使えるよう runSearch で検索し直してから、選ぶ写真を id で選ぶ
//   （actions.selectById）。
// - 操作の対象は actions.targets()（グリッドでは選択中のすべて、ルーペ・現像では表示中の 1 枚）。折りたたんだ
//   スタックは代表の 1 枚だけが選ばれている（隠れた写真には付かない。library/stackLogic.ts の冒頭）。検索し直す
//   たびに、選択を結果に出ている写真だけにし、表示中の写真が隠れたらその代表を選ぶ（state/app.ts の
//   relocateSelection。折りたたみ・自動スタック・まとめる・絞り込みのどれでも）。
// - ショートカット（App.tsx）: Ctrl / ⌘ + G（まとめる）、Ctrl / ⌘ + Shift + G（解除）、S（折りたたみの切り替え）、
//   Shift + S（一番上にする）、Ctrl / ⌘ + '（仮想コピーを作る。日本語配列は Ctrl / ⌘ + Shift + 7）。

import * as core from "../api/core";
import type { StackInfo, VariantId } from "../api/types";
import type { PhotoMenuAction } from "../library/stackLogic";
import { actions, appStore } from "./app";
import { enqueue } from "./develop";
import { openMoveFiles, openRelink, openRenameFiles } from "./files";
import { openDeleteDialog, shownManualCollection } from "./organize";
import { searchCache } from "./search";

/** 写真が入っているスタック（入っていなければ null。読めなければ知らせて null）。 */
async function stackOf(id: VariantId): Promise<StackInfo | null> {
  try {
    return (await core.variantStacks([id]))[0] ?? null;
  } catch (e) {
    actions.reportError(e, "スタックを読めません");
    return null;
  }
}

/** 操作の対象をまとめて 1 つのスタックにする（Ctrl / ⌘ + G）。一番上は表示中の写真。作ったら一番上を選ぶ。 */
export async function groupIntoStack(): Promise<void> {
  const ids = actions.targets();
  if (ids.length < 2) {
    actions.notify("info", "スタックにするには 2 枚以上の写真を選んでください");
    return;
  }
  const current = appStore.get().selection.current;
  try {
    const s = await core.createStack(ids, current !== null && ids.includes(current) ? current : null);
    await actions.runSearch();
    await actions.selectById(s.top_variant_id);
    actions.notify("success", `${s.members.length.toLocaleString("ja-JP")} 枚をスタックにまとめました（S で展開）`);
  } catch (e) {
    actions.reportError(e, "スタックにまとめられません");
  }
}

/** 操作の対象が入っているスタックを解除する（Ctrl / ⌘ + Shift + G。写真は消えない）。 */
export async function unstackTargets(): Promise<void> {
  const ids = actions.targets();
  if (ids.length === 0) return;
  try {
    const n = await core.unstack(ids);
    if (n > 0) actions.notify("info", `スタックを ${n.toLocaleString("ja-JP")} 個解除しました（写真は削除していません）`);
  } catch (e) {
    actions.reportError(e, "スタックを解除できません");
  }
}

/**
 * 足す先のスタック: 表示中の写真のスタック。表示中の写真がスタックに入っていなければ、選んだ写真が入っている
 * ただ 1 つのスタック（`stacks` は選んだ写真が入っているスタック）。決められなければ null。
 */
export function stackToAddTo(stacks: StackInfo[], current: VariantId | null): StackInfo | null {
  const own = current !== null ? stacks.find((s) => s.members.includes(current)) : undefined;
  return own ?? (stacks.length === 1 ? stacks[0] : null);
}

/**
 * 選んだ写真を、そのうちのスタックの写真のスタックに足す（スタックの最後に撮影日時の順で。一番上の写真と折りたたみ
 * の状態は変えない。ほかのスタックに入っている写真は、そちらから移す）。足す先は stackToAddTo（表示中の写真の
 * スタック、なければ選んだ写真が入っているただ 1 つのスタック）。対象はグリッドでは選択中のすべて、ルーペ・現像
 * ではフィルムストリップで選んだ写真も含める（actions.pasteTargets）。まとめる（Ctrl / ⌘ + G）と違い、中の順・
 * 一番上・折りたたみを作り直さない。
 */
export async function addTargetsToStack(): Promise<void> {
  const ids = actions.pasteTargets();
  if (ids.length < 2) {
    actions.notify("info", "足す先のスタックの写真と、足す写真を一緒に選んでください");
    return;
  }
  try {
    const stacks = await core.variantStacks(ids);
    const target = stackToAddTo(stacks, appStore.get().selection.current);
    if (!target) {
      actions.notify(
        "info",
        stacks.length === 0
          ? "選んだ写真にスタックの写真がありません（2 枚以上をまとめるときは Ctrl / ⌘ + G）"
          : "複数のスタックの写真を選んでいます。足す先のスタックの写真を表示中にしてください（クリックで選び直す）",
      );
      return;
    }
    const add = ids.filter((id) => !target.members.includes(id));
    if (add.length === 0) {
      actions.notify("info", "選んだ写真はすべてそのスタックに入っています");
      return;
    }
    const n = await core.addToStack(target.stack_id, add);
    await actions.runSearch();
    actions.notify(
      "success",
      `${n.toLocaleString("ja-JP")} 枚をスタックに足しました${target.collapsed ? "（折りたたみ中。S で展開）" : ""}`,
    );
  } catch (e) {
    actions.reportError(e, "スタックに足せません");
  }
}

/** 操作の対象をスタックから外す（写真は消えない。1 枚になったスタックは消える）。 */
export async function removeTargetsFromStack(): Promise<void> {
  const ids = actions.targets();
  if (ids.length === 0) return;
  try {
    const n = await core.removeFromStack(ids);
    if (n > 0) actions.notify("info", `${n.toLocaleString("ja-JP")} 枚をスタックから外しました`);
  } catch (e) {
    actions.reportError(e, "スタックから外せません");
  }
}

/**
 * 表示中の写真のスタックを折りたたむ・展開する（S・印のクリック）。`id` を省くと表示中の写真。スタックに
 * 入っていなければ何もしない。折りたたんで隠れた写真は、検索し直したときに選択から外れ、表示中の写真が隠れたら
 * そのスタックの代表（ふつうは一番上の写真）を選ぶ（actions.runSearch の選択の求め直し。state/app.ts の
 * relocateSelection）。
 */
export async function toggleStack(id?: VariantId): Promise<void> {
  const target = id ?? appStore.get().selection.current;
  if (target === null) return;
  const s = await stackOf(target);
  if (!s) return;
  try {
    await core.setStacksCollapsed([s.stack_id], !s.collapsed);
    await actions.runSearch();
  } catch (e) {
    actions.reportError(e, "スタックの表示を変えられません");
  }
}

/**
 * すべてのスタックを展開する・折りたたむ。折りたたんで隠れた写真の選択は toggleStack と同じ（選択から外し、
 * 表示中の写真が隠れたらその代表を選ぶ）。
 */
export async function setAllStacksCollapsed(collapsed: boolean): Promise<void> {
  try {
    await core.setStacksCollapsed(null, collapsed);
    await actions.runSearch();
  } catch (e) {
    actions.reportError(e, "スタックの表示を変えられません");
  }
}

/** 表示中の写真をそのスタックの一番上にする（Shift + S）。 */
export async function setStackTop(id?: VariantId): Promise<void> {
  const target = id ?? appStore.get().selection.current;
  if (target === null) return;
  try {
    await core.setStackTop(target);
  } catch (e) {
    actions.reportError(e, "スタックの一番上にできません");
  }
}

/**
 * 仮想コピーを作る（Ctrl / ⌘ + '・メニュー）。作った仮想コピーは元の写真と同じスタックに入り（展開する）、
 * 手動のコレクションを表示していればそのコレクションにも入れる（Lightroom と同じ）。作ったら選ぶ。`name` が
 * null なら「コピー n」。作った仮想コピーを返す（失敗は知らせて null）。
 */
export async function createVirtualCopyOf(source?: VariantId, name: string | null = null): Promise<VariantId | null> {
  const from = source ?? appStore.get().selection.current;
  if (from === null) return null;
  const kind = searchCache.getLoadedById(from)?.kind;
  if (kind === "video") {
    actions.notify("info", "動画の仮想コピーは作れません（動画は現像しません）");
    return null;
  }
  const result: { id: VariantId | null } = { id: null };
  // 現像中の写真の送信待ちの設定を送ってから作る（コアは保存待ちの変更を保存してから写す）。
  await enqueue(async () => {
    try {
      result.id = await core.createVirtualCopy(from, name);
    } catch (e) {
      actions.reportError(e, "仮想コピーを作れません");
    }
  });
  const id = result.id;
  if (id === null) return null;
  const shown = await shownManualCollection();
  if (shown) {
    try {
      await core.addToCollection(shown.collection_id, [id]);
    } catch (e) {
      actions.reportError(e, `仮想コピーを「${shown.name}」に入れられません`);
    }
  }
  await actions.runSearch();
  await actions.selectById(id);
  const visible = appStore.get().selection.current === id && appStore.get().selection.currentIndex !== null;
  actions.notify("success", visible ? "仮想コピーを作りました" : "仮想コピーを作りました（今の絞り込みでは表示されません）");
  return id;
}

/** 仮想コピーの名前を変える（ダイアログから）。 */
export async function renameVirtualCopy(id: VariantId, name: string): Promise<void> {
  await core.renameVirtualCopy(id, name.trim());
}

/** 仮想コピーを元の写真（マスター）にする（前のマスターは仮想コピーになり、名前を受け継ぐ）。 */
export async function makeMaster(id?: VariantId): Promise<void> {
  const target = id ?? appStore.get().selection.current;
  if (target === null) return;
  try {
    const previous = await core.setAsMaster(target);
    if (previous !== target) actions.notify("success", "元の写真（マスター）にしました。前のマスターは仮想コピーになりました");
  } catch (e) {
    actions.reportError(e, "元の写真にできません");
  }
}

/**
 * 写真を選ぶ（右のパネルの、同じ写真のマスター・仮想コピーの一覧から）。折りたたんだスタックの中に隠れている
 * 写真なら、スタックを展開してから選ぶ。今の絞り込みで表示されない写真は選べない（知らせる）。
 */
export async function revealAndSelect(id: VariantId): Promise<void> {
  const s = await stackOf(id);
  try {
    if (s && s.collapsed && s.top_variant_id !== id) {
      await core.setStacksCollapsed([s.stack_id], false);
    }
    await actions.runSearch();
    await actions.selectById(id);
    if (appStore.get().selection.current !== id) actions.notify("info", "その写真は今の絞り込みでは表示されていません");
  } catch (e) {
    actions.reportError(e, "写真を選べません");
  }
}

/** 写真の右クリックのメニューの項目を実行する（library/stackLogic.ts の photoMenuEntries）。 */
export async function runPhotoMenu(action: PhotoMenuAction): Promise<void> {
  const current = appStore.get().selection.current;
  switch (action) {
    case "create_copy":
      await createVirtualCopyOf();
      return;
    case "rename_copy":
      if (current !== null) actions.openDialog({ kind: "virtual_copy_name", variantId: current });
      return;
    case "make_master":
      await makeMaster();
      return;
    case "group":
      await groupIntoStack();
      return;
    case "add_to_stack":
      await addTargetsToStack();
      return;
    case "toggle_stack":
      await toggleStack();
      return;
    case "set_top":
      await setStackTop();
      return;
    case "remove_from_stack":
      await removeTargetsFromStack();
      return;
    case "unstack":
      await unstackTargets();
      return;
    case "expand_all":
      await setAllStacksCollapsed(false);
      return;
    case "collapse_all":
      await setAllStacksCollapsed(true);
      return;
    case "auto_stack":
      actions.openDialog({ kind: "auto_stack" });
      return;
    case "edit_capture_time":
      openCaptureTimeDialog();
      return;
    case "edit_location":
      openGeotagDialog();
      return;
    case "upgrade_process": {
      const targets = actions.targets();
      if (targets.length > 0) actions.openDialog({ kind: "upgrade_process", targets });
      return;
    }
    case "apply_preset": {
      // 現像プリセット（DEV-29）を操作の対象の写真に適用する（動画はコアが除いて写真にだけ適用し、ダイアログで知らせる）。
      const targets = actions.targets();
      if (targets.length > 0) actions.openDialog({ kind: "apply_preset", targets });
      return;
    }
    case "move_to_folder":
      openMoveFiles(null);
      return;
    case "rename_files":
      openRenameFiles();
      return;
    case "relink":
      openRelink(actions.targets());
      return;
    case "composite_lighten":
    case "composite_darken": {
      const targets = actions.targets();
      if (targets.length >= 2) {
        actions.openDialog({
          kind: "composite",
          variantIds: targets,
          mode: action === "composite_lighten" ? "lighten" : "darken",
        });
      }
      return;
    }
    case "delete":
      openDeleteDialog(actions.targets());
      return;
  }
}

/** 撮影日時の修正のダイアログを開く（LIB-16。対象は操作の対象の写真、基準は表示中の写真）。 */
export function openCaptureTimeDialog(): void {
  actions.openDialog({ kind: "capture_time", variantIds: actions.targets(), current: appStore.get().selection.current });
}

/** 位置を付けるダイアログを開く（LIB-21。対象は操作の対象の写真、今の位置を出すのは表示中の写真）。 */
export function openGeotagDialog(): void {
  actions.openDialog({ kind: "geotag", variantIds: actions.targets(), current: appStore.get().selection.current });
}
