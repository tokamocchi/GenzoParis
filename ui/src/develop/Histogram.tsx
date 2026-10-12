// ヒストグラムとクリッピングの印（DEV-26）。
//
// - コアがプレビューから数えた度数（frame.header.histogram。画面への出力の符号化した値の 256 階級）を描く。
// - 白飛び・黒つぶれの三角の印は clipped_high.any・clipped_low.any が 0 でなければ点く。クリックで
//   プレビューのクリッピングの表示（J）を切り替える。
// - 処理した側（GPU / CPU）と品質（簡易 / 最終）を小さく出す。

import { useEffect, useRef } from "react";

import type { HistogramData } from "../api/types";
import { comboLabel } from "../keyboard";
import { useDevelop } from "../state/develop";
import { clipIndicators, formatRatio, histogramHeights, histogramScale } from "./pixels";
import { toggleClipping, useDevelopView } from "./viewState";

/** チャンネルの塗り（重ねると明るくなる合成で描く。状態の表示なので色を使う）。 */
const CHANNEL_FILL: [keyof Pick<HistogramData, "red" | "green" | "blue">, string][] = [
  ["red", "rgba(200, 70, 70, 0.75)"],
  ["green", "rgba(70, 170, 90, 0.75)"],
  ["blue", "rgba(70, 110, 210, 0.75)"],
];

function drawHistogram(canvas: HTMLCanvasElement, h: HistogramData | null): void {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
  const hh = Math.max(1, Math.round(canvas.clientHeight * dpr));
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== hh) canvas.height = hh;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.clearRect(0, 0, w, hh);
  if (!h || h.luma.length === 0) return;
  const scale = histogramScale([h.red, h.green, h.blue, h.luma]);
  const path = (bins: number[]) => {
    const heights = histogramHeights(bins, scale);
    const n = heights.length;
    ctx.beginPath();
    ctx.moveTo(0, hh);
    heights.forEach((v, i) => ctx.lineTo(n > 1 ? (i / (n - 1)) * w : 0, hh - v * (hh - 2)));
    ctx.lineTo(w, hh);
    ctx.closePath();
  };
  ctx.globalCompositeOperation = "lighter";
  for (const [key, fill] of CHANNEL_FILL) {
    path(h[key]);
    ctx.fillStyle = fill;
    ctx.fill();
  }
  ctx.globalCompositeOperation = "source-over";
  path(h.luma);
  ctx.strokeStyle = "rgba(220, 220, 220, 0.85)";
  ctx.lineWidth = Math.max(1, dpr);
  ctx.stroke();
}

export function Histogram() {
  const ref = useRef<HTMLCanvasElement>(null);
  const histogram = useDevelop((s) => s.frame?.header.histogram ?? null);
  const backend = useDevelop((s) => s.frame?.header.backend ?? null);
  const quality = useDevelop((s) => s.frame?.header.quality ?? null);
  const showClipping = useDevelopView((s) => s.showClipping);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    drawHistogram(canvas, histogram);
    // パネルの幅が変わったら描き直す。
    const ro = new ResizeObserver(() => drawHistogram(canvas, histogram));
    ro.observe(canvas);
    return () => ro.disconnect();
  }, [histogram]);

  const marks = clipIndicators(histogram);
  const total = histogram?.pixel_count ?? 0;
  const lowText = `黒つぶれ（いずれかのチャンネルが 0 の画素）: ${formatRatio(histogram?.clipped_low.any ?? 0, total)}`;
  const highText = `白飛び（いずれかのチャンネルが 255 の画素）: ${formatRatio(histogram?.clipped_high.any ?? 0, total)}`;
  const toggleTitle = `クリックでクリッピングの表示を${showClipping ? "やめる" : "出す"}（${comboLabel("j")}）`;
  return (
    <div className="dp-histo">
      <canvas ref={ref} role="img" aria-label="ヒストグラム" />
      <button
        className={`dp-clip-mark low${marks.low ? " on" : ""}${showClipping ? " active" : ""}`}
        title={`${lowText}\n${toggleTitle}`}
        aria-label="黒つぶれの表示を切り替える"
        aria-pressed={showClipping}
        onClick={toggleClipping}
      />
      <button
        className={`dp-clip-mark high${marks.high ? " on" : ""}${showClipping ? " active" : ""}`}
        title={`${highText}\n${toggleTitle}`}
        aria-label="白飛びの表示を切り替える"
        aria-pressed={showClipping}
        onClick={toggleClipping}
      />
      <div className="dp-histo-meta">
        <span>
          {backend ? `処理: ${backend === "gpu" ? "GPU" : "CPU"}` : "—"}
          {quality ? `・${quality === "draft" ? "簡易" : "最終"}` : ""}
        </span>
        <span title="いずれかのチャンネルが 0 / 255 の画素の割合">
          黒 {formatRatio(histogram?.clipped_low.any ?? 0, total)}・白{" "}
          {formatRatio(histogram?.clipped_high.any ?? 0, total)}
        </span>
      </div>
    </div>
  );
}
