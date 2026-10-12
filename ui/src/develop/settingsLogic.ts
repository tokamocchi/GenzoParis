// 現像設定の組み立て（WB の選択・リセット。DOM を使わない純粋な計算）。

import {
  type AsShotWhiteBalance,
  type DevelopSettings,
  type Geometry,
  RANGES,
  type WbPreset,
  type WhiteBalance,
  WB_PRESETS,
} from "../api/types";
import { FULL_CROP } from "./cropMath";

/** WB の選択（撮影時の設定・プリセット・カスタム）。 */
export type WbMode = "as_shot" | WbPreset | "custom";

/**
 * 撮影時の WB の値が分からないとき（RAW 以外・コアが求められなかったとき）にスライダーに出す値
 * （仮置き: 太陽光のプリセットに近い 5500 K、tint 0）。
 */
export const FALLBACK_WB = { temperature_k: 5500, tint: 0 } as const;

/** 設定の WB の選択。 */
export function wbMode(wb: WhiteBalance): WbMode {
  if (wb === "as_shot") return "as_shot";
  if ("preset" in wb) return wb.preset;
  return "custom";
}

/**
 * スライダーに出す色温度と tint（撮影時の設定はコアが求めた撮影時の値、プリセットはプリセットの値）。
 * `known` は撮影時の値が分からず仮の値を出しているとき false。
 */
export function effectiveWb(
  wb: WhiteBalance,
  asShot: AsShotWhiteBalance | null | undefined,
): { temperature_k: number; tint: number; known: boolean } {
  if (wb === "as_shot") {
    return asShot
      ? { temperature_k: asShot.temperature_k, tint: asShot.tint, known: true }
      : { ...FALLBACK_WB, known: false };
  }
  if ("preset" in wb) {
    const p = WB_PRESETS[wb.preset];
    return { temperature_k: p.temperature_k, tint: p.tint, known: true };
  }
  return { temperature_k: wb.custom.temperature_k, tint: wb.custom.tint, known: true };
}

/** カスタムの WB（値を範囲に収める。コアの検証で拒まれないように）。 */
export function customWb(temperature_k: number, tint: number): WhiteBalance {
  const t = Math.min(RANGES.temperatureK.max, Math.max(RANGES.temperatureK.min, temperature_k));
  const g = Math.min(RANGES.tint.max, Math.max(RANGES.tint.min, tint));
  return { custom: { temperature_k: t, tint: g } };
}

/** 選択から WB を作る（カスタムは今スライダーに出している値から始める）。 */
export function wbFromMode(
  mode: WbMode,
  current: WhiteBalance,
  asShot: AsShotWhiteBalance | null | undefined,
): WhiteBalance {
  if (mode === "as_shot") return "as_shot";
  if (mode === "custom") {
    const e = effectiveWb(current, asShot);
    return customWb(e.temperature_k, e.tint);
  }
  return { preset: mode };
}

/** 既定のジオメトリ（切り抜きなし・角度 0・縦横比は自由。genzo-model の Geometry::default）。 */
export function defaultGeometry(): Geometry {
  return { crop: { ...FULL_CROP }, angle_deg: 0, aspect: null };
}

/**
 * すべてリセット（genzo-model の DevelopSettings::default の値にする）。
 *
 * - schema_version・process_version・render_deps は今の値を保つ（コアが管理する値）。
 * - masks・lens・detail も今の値を保つ（MVP の画面にない項目を、利用者が見えないところで消さない）。
 */
export function resetAll(s: DevelopSettings): DevelopSettings {
  return {
    ...s,
    white_balance: "as_shot",
    exposure_ev: 0,
    contrast: 0,
    tone: { highlights: 0, shadows: 0, whites: 0, blacks: 0 },
    tone_curve: {
      parametric: { highlights: 0, lights: 0, darks: 0, shadows: 0 },
      luma: [],
      red: [],
      green: [],
      blue: [],
    },
    color: { saturation: 0, vibrance: 0 },
    geometry: defaultGeometry(),
  };
}
