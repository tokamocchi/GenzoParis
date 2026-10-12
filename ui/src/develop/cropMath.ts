// 切り抜きの枠の計算（DEV-15。DOM を使わない純粋な計算）。
//
// 座標の約束（genzo-model の CropRect・genzo-pipeline の coords.rs）:
//
// - 切り抜き（CropRect）は G（角度補正の後の、切り抜く前の全体）の外接矩形の **各軸の割合**（0〜1）。
//   横は外接矩形の幅を 1、縦は高さを 1 とする。そのため、割合の縦横比と画素の縦横比は違う
//   （画素の縦横比 = 割合の縦横比 × 外接矩形の幅 / 高さ）。
// - O（向きを反映した元の画像）の大きさ: SourceInfo の幅・高さ（向きを反映する前。RAW は既定の切り抜きの
//   大きさ）を、EXIF の向きが 5〜8 なら入れ替えたもの。
// - G の外接矩形: (ow·|cos θ| + oh·|sin θ|, ow·|sin θ| + oh·|cos θ|)。θ は角度（正で反時計回り）。
// - 切り抜きの幅・高さの下限は genzo-model の MIN_CROP_SIZE（0.001）。画面ではつかみやすさのため
//   MIN_CROP_FRACTION（仮置き）を下限にする。

import type { AspectRatio, CropRect, Orientation } from "../api/types";

/** 全体（切り抜きなし）。 */
export const FULL_CROP: CropRect = { left: 0, top: 0, right: 1, bottom: 1 };

/** genzo-model の MIN_CROP_SIZE（切り抜きの幅・高さの最小値。各軸の割合）。 */
export const MODEL_MIN_CROP_SIZE = 0.001;

/** 画面で枠を動かすときの幅・高さの最小値（各軸の割合。仮置き）。 */
export const MIN_CROP_FRACTION = 0.02;

/** 枠のつまみ（四隅・四辺）と、枠の中（移動）。 */
export type CropHandle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "move";

/** 縦横比の選択肢（画面の表示順）。 */
export const ASPECT_CHOICES: { key: string; label: string; aspect: AspectRatio | null }[] = [
  { key: "original", label: "元の比率", aspect: "original" },
  { key: "free", label: "自由", aspect: null },
  { key: "1:1", label: "1 : 1", aspect: { ratio: { w: 1, h: 1 } } },
  { key: "4:5", label: "4 : 5", aspect: { ratio: { w: 4, h: 5 } } },
  { key: "5:4", label: "5 : 4", aspect: { ratio: { w: 5, h: 4 } } },
  { key: "2:3", label: "2 : 3", aspect: { ratio: { w: 2, h: 3 } } },
  { key: "3:2", label: "3 : 2", aspect: { ratio: { w: 3, h: 2 } } },
  { key: "3:4", label: "3 : 4", aspect: { ratio: { w: 3, h: 4 } } },
  { key: "4:3", label: "4 : 3", aspect: { ratio: { w: 4, h: 3 } } },
  { key: "9:16", label: "9 : 16", aspect: { ratio: { w: 9, h: 16 } } },
  { key: "16:9", label: "16 : 9", aspect: { ratio: { w: 16, h: 9 } } },
];

/** 縦横比の指定の選択肢のキー（"original"・"free"・"3:2" など）。 */
export function aspectKey(aspect: AspectRatio | null | undefined): string {
  if (!aspect) return "free";
  if (aspect === "original") return "original";
  return `${aspect.ratio.w}:${aspect.ratio.h}`;
}

/** EXIF の向きで幅と高さが入れ替わるか（5〜8。genzo-model の Orientation::swaps_dimensions）。 */
export function swapsDimensions(o: Orientation | null | undefined): boolean {
  return o === 5 || o === 6 || o === 7 || o === 8;
}

/** O（向きを反映した元の画像）の大きさ。 */
export function orientedSize(
  width: number,
  height: number,
  orientation: Orientation | null | undefined,
): { width: number; height: number } {
  return swapsDimensions(orientation) ? { width: height, height: width } : { width, height };
}

/** G の外接矩形の大きさ（O の大きさと角度から。genzo-pipeline の CoordinateFrames::g_size_px）。 */
export function gBoxSize(ow: number, oh: number, angleDeg: number): { width: number; height: number } {
  if (angleDeg === 0) return { width: ow, height: oh };
  const t = (angleDeg * Math.PI) / 180;
  const c = Math.abs(Math.cos(t));
  const s = Math.abs(Math.sin(t));
  return { width: ow * c + oh * s, height: ow * s + oh * c };
}

/** 縦横比の指定の、画素の縦横比（幅 / 高さ）。自由なら null。"original" は O の比。 */
export function pixelAspect(aspect: AspectRatio | null | undefined, ow: number, oh: number): number | null {
  if (!aspect) return null;
  if (aspect === "original") return ow > 0 && oh > 0 ? ow / oh : null;
  const { w, h } = aspect.ratio;
  return w > 0 && h > 0 ? w / h : null;
}

/** 画素の縦横比を、切り抜きの割合の縦横比（割合の幅 / 割合の高さ）にする。 */
export function fractionAspect(pixel: number | null, gw: number, gh: number): number | null {
  if (pixel === null || !(gw > 0) || !(gh > 0) || !(pixel > 0)) return null;
  return (pixel * gh) / gw;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/**
 * コアに送れる形にする（各値を 0〜1 に収め、left < right・top < bottom、幅・高さを MIN_CROP_SIZE 以上に。
 * 浮動小数点の誤差で 1 をわずかに超えると、コアの検証で拒まれるため）。
 */
export function sanitizeCrop(c: CropRect): CropRect {
  const axis = (a: number, b: number): [number, number] => {
    let lo = clamp(Number.isFinite(a) ? a : 0, 0, 1);
    let hi = clamp(Number.isFinite(b) ? b : 1, 0, 1);
    if (lo > hi) [lo, hi] = [hi, lo];
    if (hi - lo < MODEL_MIN_CROP_SIZE * 1.01) {
      // 小さすぎる: 中心を保って下限まで広げる（端では内側へ）。
      const size = MODEL_MIN_CROP_SIZE * 1.01;
      const mid = clamp((lo + hi) / 2, size / 2, 1 - size / 2);
      lo = mid - size / 2;
      hi = mid + size / 2;
    }
    return [lo, hi];
  };
  const [left, right] = axis(c.left, c.right);
  const [top, bottom] = axis(c.top, c.bottom);
  return { left, top, right, bottom };
}

/**
 * 割合の縦横比 `k` の、画像（外接矩形）に収まる最大の枠（縦横比を選んだとき）。中心は今の枠の中心
 * （枠が外接矩形からはみ出すなら内側へずらす）。回転した画像の内側への制限は、コアの constrain_crop で行う。
 */
export function maxAspectCrop(c: CropRect, k: number | null): CropRect {
  if (k === null || !(k > 0)) return c;
  const w = Math.min(1, k);
  const h = w / k;
  const cx = clamp((c.left + c.right) / 2, w / 2, 1 - w / 2);
  const cy = clamp((c.top + c.bottom) / 2, h / 2, 1 - h / 2);
  return { left: cx - w / 2, top: cy - h / 2, right: cx + w / 2, bottom: cy + h / 2 };
}

/**
 * 中心と幅を保ち、高さを割合の縦横比 `k` に合わせる（角度を変えて外接矩形の比が変わったとき）。
 * 画像の外に出るなら、中心を保って縮める。
 */
export function matchAspect(c: CropRect, k: number | null): CropRect {
  if (k === null || !(k > 0)) return c;
  const cx = (c.left + c.right) / 2;
  const cy = (c.top + c.bottom) / 2;
  let w = c.right - c.left;
  let h = w / k;
  const maxW = 2 * Math.min(cx, 1 - cx);
  const maxH = 2 * Math.min(cy, 1 - cy);
  if (h > maxH) {
    h = maxH;
    w = h * k;
  }
  if (w > maxW) {
    w = maxW;
    h = w / k;
  }
  return { left: cx - w / 2, top: cy - h / 2, right: cx + w / 2, bottom: cy + h / 2 };
}

/**
 * 枠をドラッグした結果（`start` はドラッグを始めたときの枠、`dx`・`dy` は動かした量（各軸の割合）、
 * `k` は割合の縦横比（自由なら null））。
 *
 * - 移動: 大きさを保ち、0〜1 に収める。
 * - 辺: その辺だけを動かす。縦横比を固定しているときは、反対の辺を固定し、もう一方の向きは中心を保って
 *   広げる・縮める。
 * - 角: 反対の角を固定する。縦横比を固定しているときは、指した位置を覆う向き（大きく動いた側）に合わせる。
 * - 幅・高さは `min` 以上（画像が小さすぎるときを除く）、枠は 0〜1 の内側。
 */
export function dragCrop(
  start: CropRect,
  handle: CropHandle,
  dx: number,
  dy: number,
  k: number | null,
  min = MIN_CROP_FRACTION,
): CropRect {
  const { left: l, top: t, right: r, bottom: b } = start;
  const w0 = r - l;
  const h0 = b - t;
  if (handle === "move") {
    const nl = clamp(l + dx, 0, Math.max(0, 1 - w0));
    const nt = clamp(t + dy, 0, Math.max(0, 1 - h0));
    return { left: nl, top: nt, right: nl + w0, bottom: nt + h0 };
  }
  const east = handle === "e" || handle === "ne" || handle === "se";
  const west = handle === "w" || handle === "nw" || handle === "sw";
  const south = handle === "s" || handle === "se" || handle === "sw";
  const north = handle === "n" || handle === "ne" || handle === "nw";
  const corner = (east || west) && (north || south);

  if (k === null || !(k > 0)) {
    // 自由: 動かした辺だけを変える。
    let nl = l;
    let nr = r;
    let nt = t;
    let nb = b;
    // 反対の辺までが `min` より狭い枠（保存済み・貼り付けた設定ではありうる）でも 0〜1 の外に出ないよう、
    // 上限・下限を 0〜1 に収めてから比べる。
    if (west) nl = clamp(l + dx, 0, Math.max(0, r - min));
    if (east) nr = clamp(r + dx, Math.min(1, l + min), 1);
    if (north) nt = clamp(t + dy, 0, Math.max(0, b - min));
    if (south) nb = clamp(b + dy, Math.min(1, t + min), 1);
    return { left: Math.min(nl, nr), top: Math.min(nt, nb), right: Math.max(nl, nr), bottom: Math.max(nt, nb) };
  }

  if (corner) {
    // 反対の角を固定する。
    const ax = east ? l : r;
    const ay = south ? t : b;
    const px = (east ? r : l) + dx;
    const py = (south ? b : t) + dy;
    const dirX = east ? 1 : -1;
    const dirY = south ? 1 : -1;
    let w = Math.max(min, (px - ax) * dirX);
    let h = Math.max(min, (py - ay) * dirY);
    // 指した位置を覆う向きに合わせる。
    if (w / h > k) h = w / k;
    else w = h * k;
    const wMax = dirX > 0 ? 1 - ax : ax;
    const hMax = dirY > 0 ? 1 - ay : ay;
    if (w > wMax) {
      w = wMax;
      h = w / k;
    }
    if (h > hMax) {
      h = hMax;
      w = h * k;
    }
    const x1 = ax + dirX * w;
    const y1 = ay + dirY * h;
    return { left: Math.min(ax, x1), top: Math.min(ay, y1), right: Math.max(ax, x1), bottom: Math.max(ay, y1) };
  }

  if (east || west) {
    // 横の辺: 反対の辺を固定し、高さは中心を保って合わせる。
    const ax = east ? l : r;
    const dirX = east ? 1 : -1;
    const cy = (t + b) / 2;
    let w = Math.max(min, ((east ? r : l) + dx - ax) * dirX);
    w = Math.min(w, dirX > 0 ? 1 - ax : ax);
    let h = w / k;
    const hMax = 2 * Math.min(cy, 1 - cy);
    if (h > hMax) {
      h = hMax;
      w = h * k;
    }
    const x1 = ax + dirX * w;
    return { left: Math.min(ax, x1), top: cy - h / 2, right: Math.max(ax, x1), bottom: cy + h / 2 };
  }

  // 縦の辺: 反対の辺を固定し、幅は中心を保って合わせる。
  const ay = south ? t : b;
  const dirY = south ? 1 : -1;
  const cx = (l + r) / 2;
  let h = Math.max(min, ((south ? b : t) + dy - ay) * dirY);
  h = Math.min(h, dirY > 0 ? 1 - ay : ay);
  let w = h * k;
  const wMax = 2 * Math.min(cx, 1 - cx);
  if (w > wMax) {
    w = wMax;
    h = w / k;
  }
  const y1 = ay + dirY * h;
  return { left: cx - w / 2, top: Math.min(ay, y1), right: cx + w / 2, bottom: Math.max(ay, y1) };
}

/**
 * 回転した元の画像（O）の四隅の、G の外接矩形の中の位置（各軸の割合。左上・右上・右下・左下の順）。
 * genzo-pipeline の CoordinateFrames::oriented_to_g と同じ向き（正の角度で画面上の反時計回り）。
 */
export function rotatedImageCorners(ow: number, oh: number, angleDeg: number): [number, number][] {
  const { width: gw, height: gh } = gBoxSize(ow, oh, angleDeg);
  const t = (angleDeg * Math.PI) / 180;
  const c = angleDeg === 0 ? 1 : Math.cos(t);
  const s = angleDeg === 0 ? 0 : Math.sin(t);
  const corners: [number, number][] = [
    [0, 0],
    [ow, 0],
    [ow, oh],
    [0, oh],
  ];
  return corners.map(([u, v]) => {
    const dx = u - ow / 2;
    const dy = v - oh / 2;
    const gx = c * dx + s * dy + gw / 2;
    const gy = -s * dx + c * dy + gh / 2;
    return [gx / gw, gy / gh];
  });
}

/** 2 つの枠がほぼ同じか（各値の差が `eps` 以下）。 */
export function sameCrop(a: CropRect, b: CropRect, eps = 1e-6): boolean {
  return (
    Math.abs(a.left - b.left) <= eps &&
    Math.abs(a.top - b.top) <= eps &&
    Math.abs(a.right - b.right) <= eps &&
    Math.abs(a.bottom - b.bottom) <= eps
  );
}

/** 切り抜きの縦横比の計算に使う大きさ（O と G の外接矩形。単位は問わず、比だけを使う）。 */
export interface GeometryInfo {
  ow: number;
  oh: number;
  gw: number;
  gh: number;
}

/**
 * 切り抜きの縦横比の計算に使う大きさを求める。
 *
 * - 元の画像の大きさ（SourceInfo の幅・高さ）と EXIF の向きが分かれば、O と G の外接矩形を計算する。
 * - 向きが分からなければ、切り抜く前の全体のプレビューの大きさ（`uncropped`。G の外接矩形）から求める
 *   （O は回転を逆にたどって求める。45 度の近くでは求められないので null）。
 */
export function geometryInfo(opts: {
  source: { width: number; height: number } | null;
  orientation: Orientation | null;
  angleDeg: number;
  uncropped: { width: number; height: number } | null;
}): GeometryInfo | null {
  const { source, orientation, angleDeg, uncropped } = opts;
  if (source && orientation !== null && source.width > 0 && source.height > 0) {
    const o = orientedSize(source.width, source.height, orientation);
    const g = gBoxSize(o.width, o.height, angleDeg);
    return { ow: o.width, oh: o.height, gw: g.width, gh: g.height };
  }
  if (uncropped && uncropped.width > 0 && uncropped.height > 0) {
    const gw = uncropped.width;
    const gh = uncropped.height;
    if (angleDeg === 0) return { ow: gw, oh: gh, gw, gh };
    const t = (angleDeg * Math.PI) / 180;
    const c = Math.abs(Math.cos(t));
    const s = Math.abs(Math.sin(t));
    const det = c * c - s * s;
    if (det < 0.05) return null;
    const ow = (gw * c - gh * s) / det;
    const oh = (gh * c - gw * s) / det;
    return ow > 0 && oh > 0 ? { ow, oh, gw, gh } : null;
  }
  return null;
}

/** 縦横比の指定の、割合の縦横比（自由・大きさが分からないなら null）。 */
export function cropFractionAspect(aspect: AspectRatio | null | undefined, info: GeometryInfo | null): number | null {
  if (!info) return null;
  return fractionAspect(pixelAspect(aspect, info.ow, info.oh), info.gw, info.gh);
}

/** 角度を変えたときの大きさ（O は同じで、G の外接矩形だけが変わる）。 */
export function infoAtAngle(info: GeometryInfo | null, angleDeg: number): GeometryInfo | null {
  if (!info) return null;
  const g = gBoxSize(info.ow, info.oh, angleDeg);
  return { ow: info.ow, oh: info.oh, gw: g.width, gh: g.height };
}

/** 切り抜きの画素の縦横比（幅 / 高さ。`gw`・`gh` は G の外接矩形の大きさ）。 */
export function cropPixelAspect(c: CropRect, gw: number, gh: number): number {
  return ((c.right - c.left) * gw) / ((c.bottom - c.top) * gh);
}

/** 端（0・1）とみなす差（浮動小数点の誤差の範囲。仮置き）。 */
const EDGE_SNAP = 1e-9;

/**
 * 角度を変えたときの切り抜き（縦横比を固定しているとき。DEV-15）。
 *
 * 切り抜き（CropRect）は G の外接矩形の各軸の割合なので、割合のまま角度を変えると、外接矩形の比が
 * 変わった分だけ画素の縦横比が選んだ比からずれる（パイプラインは geometry.aspect を読まない）。そこで、
 * 角度 `fromDeg` のときの枠の大きさと、外接矩形の中心からの位置を画素で保ち（画面では枠が止まって画像が
 * その下で回る）、角度 `toDeg` の外接矩形の割合に直す。高さは幅と選んだ比から決め直す（元の枠の比が
 * ずれていても直す）。外接矩形に収まらなければ、比を保って縮め、内側へずらす。
 *
 * 角度 `fromDeg` → `toDeg` → `fromDeg` と変えると、縮めなかった限り元の枠に戻る。回転した画像の内側への
 * 制限はしない（コアの constrain_crop。パイプラインも描画のときに比を保って同じ制限をかける）。
 * 自由・大きさが分からないときは `crop` をそのまま返す。
 */
export function refitCropForAngle(
  crop: CropRect,
  aspect: AspectRatio | null | undefined,
  size: { ow: number; oh: number } | null,
  fromDeg: number,
  toDeg: number,
): CropRect {
  if (!size || !(size.ow > 0) || !(size.oh > 0)) return crop;
  const p = pixelAspect(aspect, size.ow, size.oh);
  if (p === null) return crop;
  const g0 = gBoxSize(size.ow, size.oh, fromDeg);
  const g1 = gBoxSize(size.ow, size.oh, toDeg);
  // 画素（G の外接矩形の単位）での幅・高さと、外接矩形の中心からの位置。
  const wPx = (crop.right - crop.left) * g0.width;
  const hPx = wPx / p;
  const dxPx = ((crop.left + crop.right) / 2 - 0.5) * g0.width;
  const dyPx = ((crop.top + crop.bottom) / 2 - 0.5) * g0.height;
  let w = wPx / g1.width;
  let h = hPx / g1.height;
  if (!(w > 0) || !(h > 0)) return crop;
  // 外接矩形に収まらなければ、比を保って縮める。
  const shrink = Math.min(1, 1 / w, 1 / h);
  w *= shrink;
  h *= shrink;
  const cx = clamp(0.5 + dxPx / g1.width, w / 2, 1 - w / 2);
  const cy = clamp(0.5 + dyPx / g1.height, h / 2, 1 - h / 2);
  // 浮動小数点の誤差で端からわずかにずれた値は端に戻す（全体の枠が「切り抜きあり」にならないように）。
  const snap = (v: number) => (Math.abs(v) < EDGE_SNAP ? 0 : Math.abs(1 - v) < EDGE_SNAP ? 1 : v);
  return sanitizeCrop({
    left: snap(cx - w / 2),
    top: snap(cy - h / 2),
    right: snap(cx + w / 2),
    bottom: snap(cy + h / 2),
  });
}
