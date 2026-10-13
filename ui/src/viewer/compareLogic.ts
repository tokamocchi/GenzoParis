// 比較表示・選別表示（LIB-03）の計算（DOM を使わない純粋な計算）。
//
// # 比較表示（C）
//
// - 左の面が「選択」、右の面が「候補」（Lightroom と同じ）。始めるときの選択は表示中の写真、候補は選択中の
//   ほかの写真（選んだ順の最初）。ほかに選んでいなければ、検索結果の次の写真（最後なら前の写真）。
// - 作業中の面（評価・フラグ・ラベルの対象。枠で強調する）は、アプリの選択の「表示中の写真」（selection.current）
//   にそろえる。選択の集合は [選択, 候補]。
// - フィルムストリップで別の写真を表示した（普通のクリック）ら、作業中の面の写真をそれにする。Ctrl / ⌘ +
//   クリックで選択に足したら、その写真を候補にする（pairAfterSelection）。
// - ←・→ で候補を前・次の写真へ（選択の写真は飛ばす）、↑ で候補を選択にして次の写真を候補に、↓ で選択と候補を
//   入れ替える（決定事項ログ No.115）。
// - 拡大（等倍）とパンは 2 つの面で同期する（既定）。同期しているときは、クリックした点の割合を両方の面の中心に
//   し、ドラッグの量（画面の画素）を両方の面に同じだけ足す（同期を外して片方だけ動かした後に同期し直すと、その
//   ずれを保ったまま動く。「位置をそろえる」で作業中の面の割合に合わせる）。
//
// # 選別表示（N）
//
// - 選択中の写真（選んだ順。表示中の写真を含める）を、MAX_SURVEY_PHOTOS 枚まで並べる（仮置き）。並べ方は、各枠に
//   写真を収めたときの面積の和が最も大きくなる列の数（surveyLayout）。
// - 作業中の写真（selection.current）は枠で強調し、クリック・←・→ で移す。× と「/」で表示から外す（選択から
//   外す。写真は消えない。1 枚だけになったら外さない）。

import type { VariantId } from "../api/types";
import { type Rect, clampCenter, pannedCenter } from "./viewerMath";

/** 選別表示に並べる写真の数の上限（**仮置き**: 12。同時に読む標準プレビューの数を抑える。決定事項ログ No.116）。 */
export const MAX_SURVEY_PHOTOS = 12;

/** 選別表示の枠の間の幅（CSS の画素。仮置き）。 */
export const SURVEY_GAP_PX = 8;

/** 選別表示の枠の下の、評価などを出す帯の高さ（CSS の画素。仮置き）。 */
export const SURVEY_CAPTION_PX = 22;

/**
 * 選別表示の枠で標準プレビュー（L1）を読む大きさ（写真を出す長辺の物理画素。これ以下ならサムネイル（L0。
 * 長辺 320）で足りる。仮置き）。
 */
export const SURVEY_L1_MIN_PX = 320;

/** 比較表示の 2 つの面の間の幅（CSS の画素。仮置き）。 */
export const COMPARE_GAP_PX = 4;

/** 比較表示の面。 */
export type CompareSide = "select" | "candidate";

/** 比較表示の 2 枚（選択と候補）。 */
export interface ComparePair {
  select: VariantId | null;
  candidate: VariantId | null;
}

/** 面の番号（コアの面。0 が選択、1 が候補）。 */
export function paneOf(side: CompareSide): 0 | 1 {
  return side === "select" ? 0 : 1;
}

/**
 * 比較を始めるときの組: 選択は表示中の写真、候補は選択中のほかの写真（選んだ順の最初）。ほかになければ候補は
 * null（呼び出し側が検索結果の次の写真を候補にする）。
 */
export function initialPair(ids: readonly VariantId[], current: VariantId | null): ComparePair {
  const select = current ?? ids[0] ?? null;
  const candidate = ids.find((id) => id !== select) ?? null;
  return { select, candidate };
}

/**
 * 位置 `index` から `delta` だけ送った位置（`skip`（選択の写真の位置）は飛ばす）。検索結果の範囲（0〜count-1）の
 * 外なら null。`index` が null なら `skip` から送る。
 */
export function stepIndex(index: number | null, delta: 1 | -1, count: number, skip: number | null): number | null {
  let i = index ?? skip;
  if (i === null) return null;
  for (;;) {
    i += delta;
    if (i < 0 || i >= count) return null;
    if (i !== skip) return i;
  }
}

/** 選択と候補を入れ替える。 */
export function swapPair(pair: ComparePair): ComparePair {
  return { select: pair.candidate, candidate: pair.select };
}

/** 組を選択の集合にする（[選択, 候補]。null と重なりを除く）と、表示中の写真（作業中の面の写真）。 */
export function selectionOfPair(
  pair: ComparePair,
  active: CompareSide,
): { ids: VariantId[]; current: VariantId | null } {
  const ids: VariantId[] = [];
  for (const id of [pair.select, pair.candidate]) if (id !== null && !ids.includes(id)) ids.push(id);
  const current = (active === "select" ? pair.select : pair.candidate) ?? pair.select ?? pair.candidate;
  return { ids, current };
}

/**
 * アプリの選択が変わった（フィルムストリップのクリックなど）ときの組と作業中の面。変わらなければ null。
 *
 * - 表示中の写真が組にない（普通のクリック）: 作業中の面の写真をそれにする。
 * - 選択に組にない写真が足された（Ctrl / ⌘ + クリック）: 最後に足された写真を候補にする。
 * - 表示中の写真が組の片方なら、作業中の面をそれに合わせる。
 *
 * 選択から組の写真が外れただけ（絞り込みで外れたなど）なら、組は変えない（表示は続ける）。
 */
export function pairAfterSelection(
  pair: ComparePair,
  active: CompareSide,
  sel: { ids: readonly VariantId[]; current: VariantId | null },
): { pair: ComparePair; active: CompareSide } | null {
  const inPair = (id: VariantId) => id === pair.select || id === pair.candidate;
  const cur = sel.current;
  if (cur !== null && !inPair(cur)) {
    // 選択の側に写真がなければ選択に入れる。
    const side: CompareSide = pair.select === null ? "select" : active;
    return { pair: { ...pair, [side]: cur }, active: side };
  }
  const extras = sel.ids.filter((id) => !inPair(id));
  if (extras.length > 0) {
    return { pair: { ...pair, candidate: extras[extras.length - 1] }, active };
  }
  if (cur !== null && pair.select !== pair.candidate) {
    const side: CompareSide = cur === pair.select ? "select" : "candidate";
    if (side !== active) return { pair, active: side };
  }
  return null;
}

// ---------------------------------------------------------------------------
// 選別表示
// ---------------------------------------------------------------------------

/**
 * 選別表示に並べる写真（選んだ順。表示中の写真を含める）と、上限で出さない数。表示中の写真が上限の外なら、
 * 最後の枠に入れる（作業中の写真が見えなくならないように）。
 */
export function surveyPhotos(
  ids: readonly VariantId[],
  current: VariantId | null,
  max = MAX_SURVEY_PHOTOS,
): { shown: VariantId[]; hidden: number } {
  const all: VariantId[] = [];
  for (const id of ids) if (!all.includes(id)) all.push(id);
  if (current !== null && !all.includes(current)) all.unshift(current);
  if (all.length <= max) return { shown: all, hidden: 0 };
  const shown = all.slice(0, max);
  if (current !== null && !shown.includes(current)) shown[max - 1] = current;
  return { shown, hidden: all.length - max };
}

/** 写真を枠（w × h）に収めたときの大きさ（縦横比 `aspect` は幅 / 高さ）。 */
export function fitSize(aspect: number, w: number, h: number): { width: number; height: number } {
  if (!(aspect > 0) || w <= 0 || h <= 0) return { width: 0, height: 0 };
  return w / h > aspect ? { width: h * aspect, height: h } : { width: w, height: w / aspect };
}

/**
 * 選別表示の並べ方: `aspects`（写真ごとの幅 / 高さ）を、幅 `width` × 高さ `height` に格子で並べる。各枠に写真を
 * 収めたときの面積の和が最も大きい列の数を選ぶ（同じなら列の少ない方）。`caption` は枠ごとに下に空ける帯の高さ。
 * 返す枠は左上から行ごとの順（CSS の画素）。
 */
export function surveyLayout(
  aspects: readonly number[],
  width: number,
  height: number,
  gap = SURVEY_GAP_PX,
  caption = 0,
): { cols: number; rows: number; cells: Rect[] } {
  const n = aspects.length;
  if (n === 0 || width <= 0 || height <= 0) return { cols: 0, rows: 0, cells: [] };
  let best = { cols: 1, rows: n, area: -1 };
  for (let cols = 1; cols <= n; cols++) {
    const rows = Math.ceil(n / cols);
    const cw = (width - gap * (cols + 1)) / cols;
    const ch = (height - gap * (rows + 1)) / rows - caption;
    if (cw <= 0 || ch <= 0) continue;
    let area = 0;
    for (const a of aspects) {
      const f = fitSize(a, cw, ch);
      area += f.width * f.height;
    }
    if (area > best.area + 1e-6) best = { cols, rows, area };
  }
  const { cols, rows } = best;
  const cw = Math.max(0, (width - gap * (cols + 1)) / cols);
  const ch = Math.max(0, (height - gap * (rows + 1)) / rows);
  const cells: Rect[] = [];
  for (let i = 0; i < n; i++) {
    const r = Math.floor(i / cols);
    const c = i % cols;
    // 最後の行の枠が少なければ、中央に寄せる。
    const inRow = r === rows - 1 ? n - r * cols : cols;
    const offset = ((cols - inRow) * (cw + gap)) / 2;
    cells.push({ x: gap + offset + c * (cw + gap), y: gap + r * (ch + gap), width: cw, height: ch });
  }
  return { cols, rows, cells };
}

/** 並びの中で `id` から `delta` だけ動かした写真（端では動かない。並びになければ先頭）。 */
export function stepInList(list: readonly VariantId[], id: VariantId | null, delta: 1 | -1): VariantId | null {
  if (list.length === 0) return null;
  const i = id === null ? -1 : list.indexOf(id);
  if (i < 0) return list[0];
  const j = Math.max(0, Math.min(list.length - 1, i + delta));
  return list[j];
}

/**
 * 選別表示から `id` を外した後の選択（選択から外す）。外した写真が表示中なら、並びの次（なければ前）の写真を
 * 表示中にする。並びが 1 枚だけ・並びにない写真なら null（外さない）。
 */
export function removeFromSurvey(
  shown: readonly VariantId[],
  ids: readonly VariantId[],
  current: VariantId | null,
  id: VariantId,
): { ids: VariantId[]; current: VariantId } | null {
  const at = shown.indexOf(id);
  if (at < 0 || shown.length <= 1) return null;
  const rest = ids.filter((x) => x !== id);
  let next = current;
  if (current === id || current === null) next = shown[at + 1] ?? shown[at - 1];
  if (next === null || next === undefined) return null;
  return { ids: rest.includes(next) ? rest : [next, ...rest], current: next };
}

// ---------------------------------------------------------------------------
// 比較表示の拡大（等倍）とパン
// ---------------------------------------------------------------------------

/** 等倍の面（写真・出力の画像のフル解像度の大きさ・表示の中心。中心は出力の画素で、表示の大きさに合わせて収める前の値）。 */
export interface PaneZoom {
  id: VariantId;
  fullW: number;
  fullH: number;
  cx: number;
  cy: number;
}

/** 面の中心の割合（出力の画像の大きさに対する 0〜1）。 */
export function relativeCenter(p: PaneZoom): { u: number; v: number } {
  return { u: p.fullW > 0 ? p.cx / p.fullW : 0.5, v: p.fullH > 0 ? p.cy / p.fullH : 0.5 };
}

/** 割合 `rel` を中心にした面。 */
export function paneAt(
  id: VariantId,
  size: { width: number; height: number },
  rel: { u: number; v: number },
): PaneZoom {
  const u = Math.min(1, Math.max(0, rel.u));
  const v = Math.min(1, Math.max(0, rel.v));
  return { id, fullW: size.width, fullH: size.height, cx: u * size.width, cy: v * size.height };
}

/** 面の中心を、表示（その面の大きさ。CSS の画素）の外が見えないように収める（等倍）。 */
export function clampPane(p: PaneZoom, view: { width: number; height: number }, dpr: number): PaneZoom {
  return {
    ...p,
    cx: clampCenter(p.cx, p.fullW, view.width, dpr),
    cy: clampCenter(p.cy, p.fullH, view.height, dpr),
  };
}

/**
 * ドラッグで動かした後の面。`start` はドラッグを始めたときの面（clampPane で収めたもの）、`dragged` はドラッグした
 * 面。同期していれば両方の面に同じ量（等倍なので、出力の画素で画面の物理画素と同じ量）を足し、そうでなければ
 * ドラッグした面だけを動かす。
 */
export function pannedPanes(
  start: readonly (PaneZoom | null)[],
  dragged: number,
  linked: boolean,
  drag: { dx: number; dy: number },
  view: { width: number; height: number },
  dpr: number,
): (PaneZoom | null)[] {
  return start.map((p, i) => {
    if (!p || (!linked && i !== dragged)) return p;
    return clampPane(
      { ...p, cx: pannedCenter(p.cx, drag.dx, dpr), cy: pannedCenter(p.cy, drag.dy, dpr) },
      view,
      dpr,
    );
  });
}
