// スライダーの目盛りと表示の書式（DOM を使わない純粋な計算）。

import { RANGES } from "../api/types";

/** Slider の toPosition・fromPosition の位置の範囲（0〜1000。components/Slider.tsx）。 */
export const POSITION_MAX = 1000;

const T_MIN = RANGES.temperatureK.min;
const T_MAX = RANGES.temperatureK.max;
const T_LOG_SPAN = Math.log(T_MAX / T_MIN);

/**
 * 色温度（K）→ スライダーの位置（0〜1000）。対数の目盛り（2000〜50000 K は 25 倍の幅があり、
 * 等間隔の目盛りでは、よく使う 3000〜7000 K の範囲が狭くなりすぎるため）。
 */
export function temperatureToPosition(k: number): number {
  const v = Math.min(T_MAX, Math.max(T_MIN, Number.isFinite(k) ? k : T_MIN));
  return (POSITION_MAX * Math.log(v / T_MIN)) / T_LOG_SPAN;
}

/** スライダーの位置（0〜1000）→ 色温度（K。1 K 単位に丸め、範囲に収める）。 */
export function positionToTemperature(p: number): number {
  const t = Math.min(POSITION_MAX, Math.max(0, Number.isFinite(p) ? p : 0)) / POSITION_MAX;
  const k = Math.round(T_MIN * Math.exp(t * T_LOG_SPAN));
  return Math.min(T_MAX, Math.max(T_MIN, k));
}

/** 負の記号（入力欄の Slider が "−" を "-" に読み替える）。 */
const MINUS = "−";

/** 符号つきの数（"+25"・"−25"・"0"。`digits` は小数の桁数）。 */
export function formatSigned(v: number, digits = 0): string {
  const s = Math.abs(v).toFixed(digits);
  // 丸めて 0 になる値は符号を付けない。
  if (Number(s) === 0) return (0).toFixed(digits);
  return `${v > 0 ? "+" : MINUS}${s}`;
}

/** 露光量（"+0.35 EV"）。 */
export function formatEv(v: number): string {
  return `${formatSigned(v, 2)} EV`;
}

/** 色温度（"5500 K"）。 */
export function formatKelvin(v: number): string {
  return `${Math.round(v)} K`;
}

/** 角度（"+1.25°"）。 */
export function formatAngle(v: number): string {
  return `${formatSigned(v, 2)}°`;
}

/**
 * 受けた値が、今出している値と同じ表示か（components/Slider.tsx は、入力欄を編集せずに離れたときも
 * 表示の値で onChange を呼ぶ。そのときに設定を変えないように使う）。
 */
export function sameShown(format: (v: number) => string, received: number, current: number): boolean {
  return format(received) === format(current);
}

/** 入力欄の文字を数にする（単位を含んでよい。"5500 K" → 5500。読めなければ NaN）。 */
export function parseNumber(s: string): number {
  return Number.parseFloat(s.replace(/[＋+]/g, "").replace(/[−－]/g, "-").replace(/[，,]/g, ""));
}
