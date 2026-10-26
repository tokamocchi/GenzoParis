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
// - ビフォー / アフターの表示（DEV-31。\ でビフォーだけ・Y で左右に並べる・Shift ＋ Y で分割）: 表示の
//   形だけをここで持つ（ビフォーの設定と画素は state/develop.ts）。写真を変えても形は保つ。スポイト・切り抜きの
//   編集を始めたら、ビフォー / アフターの表示をやめる（アフターの画像を操作する道具のため）。逆に、
//   ビフォー / アフターの表示を始めたら、道具を終える。
// - ローカル補正（マスク。DEV-18・DEV-19）: 選んでいるマスク・部品と、重ねて表示（O）・ブラシの道具の設定をここで
//   持つ（マスクそのものは設定の masks）。ブラシで描く（brush）・色域のスポイト（mask_color）は道具。描いた
//   ストロークは離したときに 1 本として設定に足す（履歴に 1 件）。スポイトはコアに点の色を求め（mask_color_at）、
//   見本の色として足す。選んでいる番号が設定の外になったら（Undo など）、選んでいないとみなす（currentMask）。

import * as core from "../api/core";
import type {
  AspectRatio,
  BeforeSource,
  CropRect,
  DevelopSettings,
  LocalAdjustment,
  Transform,
  UprightMode,
  VariantId,
} from "../api/types";
import { actions } from "../state/app";
import { applySettings, developStore, endDrag, enqueue, refreshFrame, setBeforeSource } from "../state/develop";
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
import {
  type BrushOptions,
  DEFAULT_BRUSH,
  addColorSample,
  addComponent,
  addMask,
  appendStroke,
  makeStroke,
  newColorShape,
  newComponent,
  newMask,
  shapeKind,
} from "./maskLogic";
import { formatAngle, sameShown } from "./scales";
import { customWb, describeUpright, fullTransform, sameTransform } from "./settingsLogic";
import { clamp01 } from "./zoomMath";

export type DevelopTool = "none" | "wb" | "crop" | "brush" | "mask_color";

/**
 * 色域のスポイトで選んだ色の入れ先（new_mask: 新しいマスク、new_component: マスク `mask` に新しい部品、
 * add_sample: マスク `mask` の部品 `component` に見本を足す）。
 */
export type MaskPickTarget =
  | { kind: "new_mask" }
  | { kind: "new_component"; mask: number }
  | { kind: "add_sample"; mask: number; component: number };

/** ビフォー / アフターの表示の形（off: アフターだけ、before: ビフォーだけ、side_by_side: 左右、split: 分割）。 */
export type CompareMode = "off" | "before" | "side_by_side" | "split";

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
  /** ビフォー / アフターの表示の形（DEV-31）。 */
  compare: CompareMode;
  /** 選んでいるマスク（設定の masks の番号）。 */
  maskIndex: number | null;
  /** 選んでいるマスクの部品の番号。 */
  componentIndex: number | null;
  /** 選んでいるマスクを重ねて表示する（O。ブラシで描いている間は brushOverlay）。 */
  maskOverlay: boolean;
  /** ブラシで描いている間に重ねて表示する（O。既定は表示する。描いたところが赤くなる）。 */
  brushOverlay: boolean;
  /** ブラシの道具の設定（次に描くストロークに使う）。 */
  brush: BrushOptions;
  /** 色域のスポイトの入れ先（tool が mask_color の間）。 */
  pickTarget: MaskPickTarget | null;
  /**
   * 周辺光量補正の中心の印をプレビューに出す（効果の区切りを開いている間。ドラッグで中心を動かす）。
   * 量が 0 のとき・マスクを選んでいるとき・ほかの道具の間などは、出す側（DevelopView）で出さない。
   */
  vignetteHandle: boolean;
}

export const viewStore = createStore<DevelopViewState>({
  tool: "none",
  showClipping: false,
  zoom: null,
  crop: null,
  compare: "off",
  maskIndex: null,
  componentIndex: null,
  maskOverlay: false,
  brushOverlay: true,
  brush: DEFAULT_BRUSH,
  pickTarget: null,
  vignetteHandle: false,
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
// ビフォー / アフター（DEV-31）
// ---------------------------------------------------------------------------

/** ビフォー / アフターの表示の形を変える（off 以外にするときは、スポイト・切り抜きの編集を終える）。 */
export function setCompareMode(mode: CompareMode): void {
  const s = viewStore.get();
  if (mode !== "off") {
    if (s.crop) void cancelCrop();
    if (s.tool === "wb" || s.tool === "brush" || s.tool === "mask_color") viewStore.set({ tool: "none", pickTarget: null });
  }
  viewStore.set({ compare: mode });
}

/** 表示を始めるときに使うビフォーの出どころ（スナップショット・履歴の「ビフォーに設定」。DEV-28。使ったら消す）。 */
let wantedBefore: BeforeSource | null = null;

/**
 * スナップショット・履歴の項目をビフォーにする（DEV-28・DEV-31）。ビフォー / アフターの表示がオフなら、左右に
 * 並べる表示にして、表示を始めるときにこの出どころを使う（DevelopView の useBeforeForCompare が takeWantedBefore で
 * 受け取る。オフのまま設定すると、表示をやめたものとしてビフォーが捨てられるため）。表示中ならすぐに変える。
 */
export function showAsBefore(source: BeforeSource): void {
  if (viewStore.get().compare === "off") {
    wantedBefore = source;
    setCompareMode("side_by_side");
    return;
  }
  void setBeforeSource(source).catch((e) => actions.reportError(e, "ビフォーを変えられません"));
}

/** 表示を始めるときのビフォーの出どころ（showAsBefore で選んだもの。なければ null）を受け取る（1 回だけ）。 */
export function takeWantedBefore(): BeforeSource | null {
  const w = wantedBefore;
  wantedBefore = null;
  return w;
}

/** キーで形を切り替える（同じ形なら off に戻す。\・Y・Shift ＋ Y）。 */
export function toggleCompareMode(mode: Exclude<CompareMode, "off">): void {
  setCompareMode(nextCompareMode(viewStore.get().compare, mode));
}

/** 切り替えの次の形（同じ形なら off）。 */
export function nextCompareMode(current: CompareMode, pressed: Exclude<CompareMode, "off">): CompareMode {
  return current === pressed ? "off" : pressed;
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
  if (s.tool === "brush" || s.tool === "mask_color") viewStore.set({ tool: "none", pickTarget: null });
  if (!d.state.source.is_raw) {
    actions.notify("info", "RAW 以外の写真ではホワイトバランスを変えられません");
    return;
  }
  viewStore.set({ tool: "wb", compare: "off" });
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
  if (s.tool === "brush" || s.tool === "mask_color") viewStore.set({ pickTarget: null });
  if (s.zoom) cancelRegionRender();
  watchSavedGeometry();
  angleBase = null;
  const g = d.settings.geometry;
  const aspect = g.aspect ?? null;
  viewStore.set({
    tool: "crop",
    zoom: null,
    compare: "off",
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
  const seq = ++cropSeq;
  viewStore.set({ tool: "none", crop: null });
  const d = developStore.get();
  if (d.variantId === draft.variantId && d.settings && d.state) {
    const angle = d.settings.geometry.angle_deg;
    let crop = sanitizeCrop(draft.crop);
    try {
      crop = sanitizeCrop(await core.constrainCrop(draft.variantId, crop, angle, fullTransform(d.settings.transform)));
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
  // 切り抜いた表示に戻すのは、確定した写真のままで、その間に新しい編集を始めていないときだけ（表示の切り替え
  // （set_develop_view）は今の写真に効くので、別の写真に移って編集を始めていると、その表示を戻してしまう）。
  // 別の写真に移っていれば、その写真は切り抜いた表示で開いている。
  if (seq === cropSeq && developStore.get().variantId === draft.variantId) await restoreCroppedView();
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
      await core.constrainCrop(
        draft.variantId,
        sanitizeCrop(draft.crop),
        d.settings.geometry.angle_deg,
        fullTransform(d.settings.transform),
      ),
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
      crop = sanitizeCrop(
        await core.constrainCrop(id, crop, d.settings.geometry.angle_deg, fullTransform(d.settings.transform)),
      );
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
// 変形（遠近補正。DEV-16）
// ---------------------------------------------------------------------------

/**
 * 変形の項目を変える（パネルのスライダー）。設定の変形だけを送る（切り抜きは、描画のときにコアが画像の内側に
 * 制限する。編集中の枠はドラッグを終えたときに制限する）。
 */
export function changeTransform(patch: Partial<Transform>, dragging: boolean): void {
  const d = developStore.get();
  if (!d.state || !d.settings || d.variantId === null) return;
  const next = { ...fullTransform(d.settings.transform), ...patch };
  if (sameTransform(next, fullTransform(d.settings.transform))) return;
  applySettings({ ...d.settings, transform: next }, dragging);
}

/** 変形のドラッグを終える（履歴に 1 件。切り抜きの編集中は、枠を画像の内側に制限する）。 */
export function commitTransform(): void {
  endDrag();
  if (viewStore.get().crop) void constrainDraft();
}

/**
 * 自動の遠近補正（Upright 相当）: コアに直線を調べてもらい（analyze_upright）、求めた角度補正と垂直・水平を設定に
 * 入れて送る（履歴に 1 件。元に戻せる）。直線が見つからなければ知らせる。切り抜きの編集中は枠を制限し直す。
 */
export async function applyUpright(mode: UprightMode): Promise<void> {
  const d = developStore.get();
  if (!d.state || !d.settings || d.variantId === null) return;
  const id = d.variantId;
  try {
    const s = await core.analyzeUpright(id, mode);
    const latest = developStore.get();
    if (latest.variantId !== id || !latest.settings || !latest.state) return;
    const transform = fullTransform(s.transform);
    applySettings(
      { ...latest.settings, geometry: { ...latest.settings.geometry, angle_deg: s.angle_deg }, transform },
      false,
    );
    actions.notify("info", describeUpright(mode, s.angle_deg, transform, { vertical: s.vertical_lines, horizontal: s.horizontal_lines }));
    if (viewStore.get().crop) void constrainDraft();
  } catch (e) {
    actions.reportError(e, "自動で補正できません");
  }
}

// ---------------------------------------------------------------------------
// 写真が変わったとき・画面を離れたとき
// ---------------------------------------------------------------------------

/**
 * 道具をすべて終える（切り抜きは保存しない。等倍表示の描画は取り消す。マスクの選択も外す。重ねて表示の切り替えと
 * ブラシの設定は保つ）。
 */
export function endAllTools(): void {
  angleBase = null;
  const s = viewStore.get();
  if (s.crop) void cancelCrop();
  if (s.zoom) cancelRegionRender();
  viewStore.set({ tool: "none", zoom: null, maskIndex: null, componentIndex: null, pickTarget: null });
}

// ---------------------------------------------------------------------------
// ローカル補正（マスク。DEV-18・DEV-19）
// ---------------------------------------------------------------------------

/** 今の設定で有効な選択（番号が設定の外なら null）。 */
export function currentMask(
  settings: DevelopSettings | null,
  s: Pick<DevelopViewState, "maskIndex" | "componentIndex">,
): { index: number; mask: LocalAdjustment; component: number | null } | null {
  const masks = settings?.masks ?? [];
  const i = s.maskIndex;
  if (i === null || i < 0 || i >= masks.length) return null;
  const m = masks[i];
  const j = s.componentIndex;
  return { index: i, mask: m, component: j !== null && j >= 0 && j < m.components.length ? j : null };
}

/** マスクを選ぶ（null で外す。部品は最初のもの）。ブラシ・スポイトの道具は終える。 */
export function selectMask(index: number | null): void {
  const s = viewStore.get();
  const toolOff = s.tool === "brush" || s.tool === "mask_color";
  viewStore.set({
    maskIndex: index,
    componentIndex: index === null ? null : 0,
    ...(toolOff ? { tool: "none" as const, pickTarget: null } : {}),
  });
}

/** 部品を選ぶ（ブラシで描いている間にブラシでない部品を選んだら、描くのをやめる）。 */
export function selectComponent(index: number | null): void {
  const s = viewStore.get();
  const d = developStore.get();
  const sel = currentMask(d.settings, { maskIndex: s.maskIndex, componentIndex: index });
  const isBrush = sel?.component != null && shapeKind(sel.mask.components[sel.component].shape) === "brush";
  viewStore.set({
    componentIndex: index,
    ...(s.tool === "brush" && !isBrush ? { tool: "none" as const } : {}),
    ...(s.tool === "mask_color" ? { tool: "none" as const, pickTarget: null } : {}),
  });
}

/** 重ねて表示をするか（ブラシで描いている間は brushOverlay、それ以外は maskOverlay）。 */
export function overlayShown(s: Pick<DevelopViewState, "tool" | "maskOverlay" | "brushOverlay">): boolean {
  return s.tool === "brush" ? s.brushOverlay : s.maskOverlay;
}

/**
 * 重ねて表示を切り替える（O。マスクを選んでいなければ最初のマスクを選ぶ。マスクがなければ知らせる）。ブラシで描いている
 * 間は、描いている間の表示（brushOverlay）を切り替える。
 */
export function toggleMaskOverlay(): void {
  const s = viewStore.get();
  const d = developStore.get();
  if (!d.state || !d.settings) return;
  if (s.tool === "brush") {
    viewStore.set({ brushOverlay: !s.brushOverlay });
    return;
  }
  const masks = d.settings.masks ?? [];
  if (!s.maskOverlay && masks.length === 0) {
    actions.notify("info", "マスクがありません（右のパネルの「マスク」で追加できます）");
    return;
  }
  if (!s.maskOverlay && currentMask(d.settings, s) === null) {
    viewStore.set({ maskIndex: 0, componentIndex: 0 });
  }
  viewStore.set({ maskOverlay: !s.maskOverlay });
}

/** ブラシの道具の設定を変える。 */
export function setBrushOptions(patch: Partial<BrushOptions>): void {
  viewStore.set((s) => ({ brush: { ...s.brush, ...patch } }));
}

/** マスク・ブラシ・スポイトの道具を使える状態か（写真を開いていて、処理バージョンが 2 以上）。 */
function maskToolReady(): boolean {
  const d = developStore.get();
  return !!d.state && !!d.settings && d.settings.process_version >= 2;
}

/** 選んでいる部品をブラシで描き始める・やめる（選んでいる部品がブラシのときだけ）。 */
export function toggleBrushTool(): void {
  const s = viewStore.get();
  if (s.tool === "brush") {
    viewStore.set({ tool: "none" });
    return;
  }
  if (s.crop || !maskToolReady()) return;
  const sel = currentMask(developStore.get().settings, s);
  if (!sel || sel.component === null || shapeKind(sel.mask.components[sel.component].shape) !== "brush") return;
  viewStore.set({ tool: "brush", compare: "off", pickTarget: null });
}

/** マスク・ブラシ・スポイトの道具をやめる（Esc）。 */
export function cancelMaskTool(): void {
  const t = viewStore.get().tool;
  if (t === "brush" || t === "mask_color") viewStore.set({ tool: "none", pickTarget: null });
}

/** 新しいマスクを足して選ぶ（ブラシなら描き始める）。 */
export function addMaskWith(settings: DevelopSettings, mask: LocalAdjustment, startBrush: boolean): void {
  const next = addMask(settings, mask);
  if (next === settings) {
    actions.notify("info", "これ以上マスクを足せません");
    return;
  }
  applySettings(next, false);
  selectMask(next.masks.length - 1);
  if (startBrush) toggleBrushTool();
}

/** 部品を足した設定 `next` を送り、マスク `index` の最後の部品（足した部品）を選ぶ（ブラシなら描き始める）。 */
export function applyAddedComponent(next: DevelopSettings, index: number, startBrush: boolean): void {
  applySettings(next, false);
  selectMaskComponent(index, next.masks[index].components.length - 1);
  if (startBrush) toggleBrushTool();
}

/** マスク `i` の部品 `j` を選ぶ。 */
export function selectMaskComponent(i: number, j: number): void {
  viewStore.set({ maskIndex: i });
  selectComponent(j);
}

/** 色域のスポイトを使い始める（入れ先を決めて。もう一度押すとやめる）。 */
export function startMaskColorPick(target: MaskPickTarget): void {
  const s = viewStore.get();
  if (s.tool === "mask_color") {
    viewStore.set({ tool: "none", pickTarget: null });
    return;
  }
  if (s.crop || !maskToolReady()) return;
  viewStore.set({ tool: "mask_color", compare: "off", pickTarget: target });
}

/**
 * 色域のスポイト: 表示中のプレビューの点（各軸 0〜1）の色をコアに求め、入れ先に見本の色として足す（新しいマスク・
 * 新しい部品・見本の追加）。道具は終える。点が画像の外（回転で外になった四隅など）ならコアのエラーを知らせる。
 */
export async function pickMaskColor(x: number, y: number): Promise<void> {
  const d = developStore.get();
  const id = d.variantId;
  const uncropped = d.frame?.header.uncropped ?? false;
  const target = viewStore.get().pickTarget;
  viewStore.set({ tool: "none", pickTarget: null });
  if (id === null || !d.state || !target) return;
  await enqueue(async () => {
    try {
      const pick = await core.maskColorAt(id, clamp01(x), clamp01(y), uncropped);
      const cur = developStore.get();
      if (cur.variantId !== id || !cur.settings) return;
      const s = cur.settings;
      if (target.kind === "new_mask") {
        addMaskWith(s, newMask(newColorShape(pick)), false);
      } else if (target.kind === "new_component") {
        const next = addComponent(s, target.mask, newComponent(newColorShape(pick)));
        if (next === s) return;
        applyAddedComponent(next, target.mask, false);
      } else {
        const next = addColorSample(s, target.mask, target.component, pick);
        if (next !== s) applySettings(next, false);
      }
    } catch (e) {
      actions.reportError(e, "スポイトで色を求められません");
    }
  });
}

/**
 * 描いたストローク（G 座標の点の列）を、選んでいるマスクのブラシの部品に足す（離したとき。履歴に 1 件）。足せなければ
 * 知らせる。戻り値は applySettings の通し番号（足せなければ null）。
 */
export function commitBrushStroke(points: [number, number][], erase: boolean): number | null {
  const d = developStore.get();
  const s = viewStore.get();
  if (!d.state || !d.settings) return null;
  const sel = currentMask(d.settings, s);
  if (!sel || sel.component === null) return null;
  const stroke = makeStroke(points, { ...s.brush, erase });
  const r = appendStroke(d.settings, sel.index, sel.component, stroke);
  if ("error" in r) {
    actions.notify("info", r.error);
    return null;
  }
  return applySettings(r.settings, false);
}
