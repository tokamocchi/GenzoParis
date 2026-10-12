// 現像の画面の道具の状態（スポイト・切り抜きの編集・等倍表示・クリッピングの表示）と操作。
//
// 中央の DevelopView と右の DevelopPanel が共有する（パネルのボタンとキーのどちらからも操作する）。
// コアへの送信は state/develop.ts の列（applySettings・enqueue）を通す（設定の送信との順序を保つ）。
//
// - スポイト（DEV-03）: 出力の画像の点を中立にする WB をコアに求め（white_balance_at）、カスタムの WB として
//   設定する。RAW だけ。
// - 切り抜きの編集（DEV-15）: コアに切り抜く前の全体を描かせ（set_develop_view(true)）、その上で枠を
//   動かす。完了で枠を画像の内側に制限して（constrain_crop）設定に入れ、取り消しでは保存しない。
//   どちらでも全体の表示を戻す（set_develop_view(false)）。編集中に設定の切り抜きが外から変わったら
//   （Undo など）、編集中の枠をその値で置き換える。
// - 角度（DEV-15）: 縦横比を固定していれば、角度と一緒に枠も合わせ直す（出力の縦横比を保つ）。
// - これらのコマンドはコアに追加中のため、失敗しても画面が壊れないようにする（切り抜きの編集を始められ
//   なければ知らせて元に戻る。制限できなければ、制限しない枠で保存する（コアが描画で同じ制限をかける））。

import * as core from "../api/core";
import type { AspectRatio, CropRect, VariantId } from "../api/types";
import { actions } from "../state/app";
import { applySettings, developStore, endDrag, enqueue, refreshFrame } from "../state/develop";
import { cancelRegionRender } from "../state/region";
import { createStore, useStore } from "../state/store";
import {
  type GeometryInfo,
  aspectKey,
  cropFractionAspect,
  maxAspectCrop,
  refitCropForAngle,
  sameCrop,
  sanitizeCrop,
} from "./cropMath";
import { formatAngle, sameShown } from "./scales";
import { customWb } from "./settingsLogic";
import { clamp01 } from "./zoomMath";

export type DevelopTool = "none" | "wb" | "crop";

/** 編集中の切り抜きの枠（保存前）。 */
export interface CropDraft {
  variantId: VariantId;
  crop: CropRect;
  aspect: AspectRatio | null;
  /**
   * 設定の切り抜きと縦横比（編集を始めたとき・外から変わったときの値）。編集中に設定のほうが変わったか
   * （Undo / Redo・履歴の移動・貼り付けの後の読み直し）を見分ける。
   */
  saved: { crop: CropRect; aspect: AspectRatio | null };
}

export interface DevelopViewState {
  tool: DevelopTool;
  /** 白飛び・黒つぶれの表示（J）。 */
  showClipping: boolean;
  /** 等倍表示の中心（出力の画像のフル解像度の画素）。全体表示なら null。 */
  zoom: { centerX: number; centerY: number } | null;
  /** 切り抜きの編集中の枠。 */
  crop: CropDraft | null;
}

export const viewStore = createStore<DevelopViewState>({
  tool: "none",
  showClipping: false,
  zoom: null,
  crop: null,
});

export function useDevelopView<T>(selector: (s: DevelopViewState) => T, equal?: (a: T, b: T) => boolean): T {
  return useStore(viewStore, selector, equal);
}

// ---------------------------------------------------------------------------
// クリッピングの表示・等倍表示
// ---------------------------------------------------------------------------

export function toggleClipping(): void {
  viewStore.set((s) => ({ showClipping: !s.showClipping }));
}

/** 等倍表示にする（中心は出力の画像のフル解像度の画素）。切り抜きの編集中はしない。 */
export function setZoom(center: { centerX: number; centerY: number } | null): void {
  if (center && viewStore.get().crop) return;
  if (!center && viewStore.get().zoom) cancelRegionRender();
  viewStore.set({ zoom: center });
}

// ---------------------------------------------------------------------------
// スポイト
// ---------------------------------------------------------------------------

/** スポイトを使い始める・やめる（W）。 */
export function toggleWbTool(): void {
  const s = viewStore.get();
  if (s.tool === "wb") {
    viewStore.set({ tool: "none" });
    return;
  }
  if (s.tool === "crop") return;
  const d = developStore.get();
  if (!d.state) return;
  if (!d.state.source.is_raw) {
    actions.notify("info", "RAW 以外の写真ではホワイトバランスを変えられません");
    return;
  }
  viewStore.set({ tool: "wb" });
}

/** スポイトをやめる（Esc）。 */
export function cancelWbTool(): void {
  if (viewStore.get().tool === "wb") viewStore.set({ tool: "none" });
}

/**
 * 表示中のプレビューの点（各軸 0〜1）を中立にする WB にする。
 *
 * クリックした時点で表示しているプレビューが切り抜く前の全体（uncropped）かをコアに伝える（コアは、その表示の
 * 画像に対する割合として点を解釈する。表示を切り替えた直後でも取り違えない）。
 */
export async function pickWhiteBalance(x: number, y: number): Promise<void> {
  const d = developStore.get();
  const id = d.variantId;
  const uncropped = d.frame?.header.uncropped ?? false;
  viewStore.set({ tool: "none" });
  if (id === null || !d.state) return;
  // 送信待ちの設定の後に求める（今の設定での色で中立にする）。
  await enqueue(async () => {
    try {
      const wb = await core.whiteBalanceAt(id, clamp01(x), clamp01(y), uncropped);
      const cur = developStore.get();
      if (cur.variantId !== id || !cur.settings) return;
      applySettings({ ...cur.settings, white_balance: customWb(wb.temperature_k, wb.tint) }, false);
    } catch (e) {
      actions.reportError(e, "スポイトでホワイトバランスを求められません");
    }
  });
}

// ---------------------------------------------------------------------------
// 切り抜きの編集
// ---------------------------------------------------------------------------

/** 編集の世代（取り消し・完了の後に届いた古い結果を捨てる）。 */
let cropSeq = 0;
/** 編集中の枠の版（制限の結果が届く前に枠が動いていたら、その結果を捨てる）。 */
let draftRev = 0;

/** 切り抜く前の全体を描くかをコアに伝える（設定の送信の後に。失敗は返す）。 */
async function setUncropped(uncropped: boolean): Promise<unknown> {
  let error: unknown = null;
  await enqueue(async () => {
    try {
      await core.setDevelopView(uncropped);
    } catch (e) {
      error = e;
    }
  });
  return error;
}

/** 切り抜きの編集を始める（R・パネルのボタン）。 */
export async function startCrop(): Promise<void> {
  const s = viewStore.get();
  if (s.crop) return;
  const d = developStore.get();
  if (!d.state || !d.settings || d.variantId === null) return;
  const seq = ++cropSeq;
  if (s.zoom) cancelRegionRender();
  watchSavedGeometry();
  angleBase = null;
  const g = d.settings.geometry;
  const aspect = g.aspect ?? null;
  viewStore.set({
    tool: "crop",
    zoom: null,
    crop: { variantId: d.variantId, crop: { ...g.crop }, aspect, saved: { crop: g.crop, aspect } },
  });
  const error = await setUncropped(true);
  if (seq !== cropSeq) return;
  if (error) {
    viewStore.set({ tool: "none", crop: null });
    actions.reportError(error, "切り抜きの編集を始められません");
    return;
  }
  // 切り抜く前の全体の描画は preview_updated で届く。念のため今の最新も取りにいく。
  void refreshFrame();
  // 保存済みの切り抜きが回転した画像の外にかかっていれば（編集の外で角度を変えたとき）、描画で使われる
  // 枠（コアの constrain_crop。パイプラインと同じ制限）を出す。角度 0 では制限はかからない。
  if (g.angle_deg !== 0) void constrainDraft();
}

/** 全体の表示に戻す（失敗しても編集は終える。次に写真を開けば戻る）。 */
async function restoreCroppedView(): Promise<void> {
  const error = await setUncropped(false);
  if (error) console.warn("切り抜いた表示に戻せません", error);
}

/** 切り抜きの編集を終えて、枠を保存する（Enter・パネルのボタン）。 */
export async function commitCrop(): Promise<void> {
  const draft = viewStore.get().crop;
  if (!draft) return;
  ++cropSeq;
  viewStore.set({ tool: "none", crop: null });
  const d = developStore.get();
  if (d.variantId === draft.variantId && d.settings && d.state) {
    const angle = d.settings.geometry.angle_deg;
    let crop = sanitizeCrop(draft.crop);
    try {
      crop = sanitizeCrop(await core.constrainCrop(draft.variantId, crop, angle));
    } catch (e) {
      // 制限できなくても、そのまま保存する（コアは描画のときに同じ制限をかける。genzo-pipeline の
      // constrain_crop）。
      console.warn("切り抜きを画像の内側に制限できません", e);
    }
    const latest = developStore.get();
    if (latest.variantId === draft.variantId && latest.settings) {
      const g = latest.settings.geometry;
      const changed = !sameCrop(crop, g.crop) || aspectKey(draft.aspect) !== aspectKey(g.aspect ?? null);
      if (changed) applySettings({ ...latest.settings, geometry: { ...g, crop, aspect: draft.aspect } }, false);
    }
  }
  await restoreCroppedView();
}

/** 切り抜きの編集を取り消す（Esc。枠は保存しない）。 */
export async function cancelCrop(): Promise<void> {
  if (!viewStore.get().crop) return;
  ++cropSeq;
  viewStore.set({ tool: "none", crop: null });
  await restoreCroppedView();
}

/** 編集中の枠を変える。 */
export function setDraftCrop(crop: CropRect): void {
  draftRev++;
  viewStore.set((s) => (s.crop ? { crop: { ...s.crop, crop } } : {}));
}

/** 編集中の枠を、画像の内側に制限する（角度を変え終えたとき。失敗したら何もしない）。 */
export async function constrainDraft(): Promise<void> {
  const draft = viewStore.get().crop;
  const d = developStore.get();
  if (!draft || !d.settings || d.variantId !== draft.variantId) return;
  const seq = cropSeq;
  const rev = draftRev;
  try {
    const c = sanitizeCrop(
      await core.constrainCrop(draft.variantId, sanitizeCrop(draft.crop), d.settings.geometry.angle_deg),
    );
    if (seq === cropSeq && rev === draftRev) setDraftCrop(c);
  } catch (e) {
    console.warn("切り抜きを画像の内側に制限できません", e);
  }
}

/**
 * 編集中に、設定の切り抜き・縦横比が外から変わったら（Undo / Redo・履歴の移動・貼り付けの後の読み直し）、
 * 編集中の枠をその値で置き換える（前の枠を完了で保存して、戻した切り抜きを上書きしないように）。
 *
 * 編集中にこの画面が設定に送るのは角度だけで（枠は完了まで送らない。完了では先に編集を終える）、
 * 切り抜き・縦横比の変化は外からの変更とみなせる。
 */
function onSavedGeometryChange(): void {
  const draft = viewStore.get().crop;
  if (!draft) return;
  const d = developStore.get();
  if (d.variantId !== draft.variantId || !d.settings) return;
  const g = d.settings.geometry;
  const aspect = g.aspect ?? null;
  if (g.crop === draft.saved.crop && aspect === draft.saved.aspect) return;
  if (sameCrop(g.crop, draft.saved.crop, 0) && aspectKey(aspect) === aspectKey(draft.saved.aspect)) return;
  draftRev++;
  angleBase = null;
  viewStore.set({ crop: { ...draft, crop: { ...g.crop }, aspect, saved: { crop: g.crop, aspect } } });
}

let watching = false;

/** 設定の切り抜き・縦横比の変化を見張る（最初に切り抜きの編集を始めたときから。編集中でなければ何もしない）。 */
function watchSavedGeometry(): void {
  if (watching) return;
  watching = true;
  developStore.subscribe(onSavedGeometryChange);
}

/**
 * 縦横比を選ぶ（パネル）。編集中なら枠を合わせ、そうでなければ設定の切り抜きを合わせて保存する
 * （その比で画像に収まる最大の枠にし、回転した画像の内側に制限する）。
 */
export async function chooseAspect(aspect: AspectRatio | null, info: GeometryInfo | null): Promise<void> {
  const k = cropFractionAspect(aspect, info);
  const draft = viewStore.get().crop;
  if (draft) {
    draftRev++;
    viewStore.set({
      crop: { ...draft, aspect, crop: k === null ? draft.crop : sanitizeCrop(maxAspectCrop(draft.crop, k)) },
    });
    if (k !== null) await constrainDraft();
    return;
  }
  const d = developStore.get();
  const id = d.variantId;
  if (id === null || !d.settings || !d.state) return;
  let crop = d.settings.geometry.crop;
  if (k !== null) {
    crop = sanitizeCrop(maxAspectCrop(crop, k));
    try {
      crop = sanitizeCrop(await core.constrainCrop(id, crop, d.settings.geometry.angle_deg));
    } catch (e) {
      console.warn("切り抜きを画像の内側に制限できません", e);
    }
  }
  const latest = developStore.get();
  if (latest.variantId !== id || !latest.settings) return;
  applySettings({ ...latest.settings, geometry: { ...latest.settings.geometry, crop, aspect } }, false);
}

// ---------------------------------------------------------------------------
// 角度
// ---------------------------------------------------------------------------

/**
 * 角度を変え始めたときの枠と角度（ドラッグの間は、毎回ここから枠を合わせ直す。途中で縮めた分が
 * たまらないように）。ドラッグでない変更・ドラッグの終了・写真が変わったときに捨てる。
 */
let angleBase: { variantId: VariantId; editing: boolean; crop: CropRect; angle: number } | null = null;

/**
 * 角度を変える（パネルのスライダー。DEV-15）。
 *
 * 縦横比を固定していれば、切り抜きの枠も合わせ直す（cropMath の refitCropForAngle。割合のままだと
 * 外接矩形の比が変わった分だけ、出力の画像の縦横比が選んだ比からずれるため）。切り抜きの編集中は
 * 編集中の枠を合わせ直し（設定には角度だけを送る）、そうでなければ設定の切り抜きを角度と一緒に送る。
 * `size` は元の画像の大きさ（useGeometryInfo。分からなければ枠は合わせ直さない）。
 */
export function changeAngle(angleDeg: number, dragging: boolean, size: GeometryInfo | null): void {
  const d = developStore.get();
  if (!d.state || !d.settings || d.variantId === null) return;
  const g = d.settings.geometry;
  const draft = viewStore.get().crop;
  const editing = draft !== null && draft.variantId === d.variantId;
  // 値を変えずに入力欄を離れたとき（Slider は編集しなくても表示の値で onChange を呼ぶ）は何もしない。
  if (!dragging && angleBase === null && sameShown(formatAngle, angleDeg, g.angle_deg)) return;
  if (angleBase === null || angleBase.variantId !== d.variantId || angleBase.editing !== editing) {
    angleBase = { variantId: d.variantId, editing, crop: editing ? draft.crop : g.crop, angle: g.angle_deg };
  }
  const base = angleBase;
  if (!dragging) angleBase = null;
  if (editing) {
    applySettings({ ...d.settings, geometry: { ...g, angle_deg: angleDeg } }, dragging);
    if (draft.aspect) {
      const crop = refitCropForAngle(base.crop, draft.aspect, size, base.angle, angleDeg);
      if (!sameCrop(crop, draft.crop, 0)) setDraftCrop(crop);
    }
    if (!dragging) void constrainDraft();
    return;
  }
  const crop = g.aspect ? refitCropForAngle(base.crop, g.aspect, size, base.angle, angleDeg) : g.crop;
  applySettings({ ...d.settings, geometry: { ...g, angle_deg: angleDeg, crop } }, dragging);
}

/** 角度のドラッグを終える（履歴に 1 件。切り抜きの編集中は、枠を回転した画像の内側に制限する）。 */
export function commitAngle(): void {
  angleBase = null;
  endDrag();
  if (viewStore.get().crop) void constrainDraft();
}

// ---------------------------------------------------------------------------
// 写真が変わったとき・画面を離れたとき
// ---------------------------------------------------------------------------

/** 道具をすべて終える（切り抜きは保存しない。等倍表示の描画は取り消す）。 */
export function endAllTools(): void {
  angleBase = null;
  const s = viewStore.get();
  if (s.crop) void cancelCrop();
  if (s.zoom) cancelRegionRender();
  viewStore.set({ tool: "none", zoom: null });
}
