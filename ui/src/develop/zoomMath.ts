// 全体表示と等倍表示の配置の計算（PRV-03。DOM を使わない純粋な計算）。

/** 画像を枠の中央に合わせて全体を表示する大きさと位置（CSS の画素。縮小だけで、拡大はしない）。 */
export function fitBox(
  viewWidth: number,
  viewHeight: number,
  imageWidth: number,
  imageHeight: number,
): { left: number; top: number; width: number; height: number; scale: number } {
  if (!(viewWidth > 0) || !(viewHeight > 0) || !(imageWidth > 0) || !(imageHeight > 0)) {
    return { left: 0, top: 0, width: 0, height: 0, scale: 0 };
  }
  const scale = Math.min(1, viewWidth / imageWidth, viewHeight / imageHeight);
  const width = imageWidth * scale;
  const height = imageHeight * scale;
  return { left: (viewWidth - width) / 2, top: (viewHeight - height) / 2, width, height, scale };
}

/**
 * 等倍表示の中心を、見える範囲が画像の内側に収まるように制限する（1 軸。単位は画像の画素）。
 * 画像が見える範囲より小さければ画像の中央。
 */
export function clampCenter(full: number, viewPx: number, center: number): number {
  if (full <= viewPx) return full / 2;
  return Math.min(full - viewPx / 2, Math.max(viewPx / 2, center));
}

/**
 * 等倍表示で、枠の左上に当たる画像の位置（1 軸。画像の画素）。画像が見える範囲より小さければ負
 * （画像を中央に置く）。
 */
export function zoomOrigin(full: number, viewPx: number, center: number): number {
  if (full <= viewPx) return -(viewPx - full) / 2;
  return clampCenter(full, viewPx, center) - viewPx / 2;
}

/** 0〜1 に収める。 */
export function clamp01(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0;
}
