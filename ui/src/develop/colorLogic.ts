// 色（カラーミキサー・カラーグレーディング・白黒。DEV-09・DEV-10・DEV-25）の設定の組み立てと、色相環の座標の計算
// （DOM を使わない純粋な計算）。式はコアの genzo-pipeline の finish::color。

import {
  type BlackWhite,
  type ColorGrading,
  type ColorParams,
  type DevelopSettings,
  type GradingWheel,
  type HslAdjustment,
  HUE_BANDS,
  type HueBand,
  type HueBands,
} from "../api/types";

/** 帯の名前と、画面の帯の色（sRGB の色相環（HSV の色相）の色。コアの帯の中心はこの色の OKLCh の色相）。 */
export const HUE_BAND_INFO: Record<HueBand, { label: string; hue: number }> = {
  red: { label: "レッド", hue: 0 },
  orange: { label: "オレンジ", hue: 30 },
  yellow: { label: "イエロー", hue: 60 },
  green: { label: "グリーン", hue: 120 },
  aqua: { label: "アクア", hue: 180 },
  blue: { label: "ブルー", hue: 240 },
  purple: { label: "パープル", hue: 270 },
  magenta: { label: "マゼンタ", hue: 300 },
};

/** カラーミキサーの調整の種類（タブ）。 */
export type MixerAspect = keyof HslAdjustment;

/** カラーミキサーのタブ（色相・彩度・輝度）。 */
export const MIXER_ASPECTS: { key: MixerAspect; label: string; title: string }[] = [
  { key: "hue", label: "色相", title: "その色の色相を回します（正で色相環の次の色の側へ。例: レッドをオレンジへ）" },
  { key: "saturation", label: "彩度", title: "その色の鮮やかさ（-100 で灰色、+100 で 2 倍）" },
  { key: "luminance", label: "輝度", title: "その色の明るさ（その色の部分の露光量を変えるのと同じ）" },
];

/** カラーグレーディングの範囲（色相環）。 */
export type GradingRange = "shadows" | "midtones" | "highlights" | "global";

/** カラーグレーディングのタブ。 */
export const GRADING_RANGES: { key: GradingRange; label: string; title: string }[] = [
  { key: "shadows", label: "シャドウ", title: "暗い部分の色付け" },
  { key: "midtones", label: "中間調", title: "中間の明るさの部分の色付け" },
  { key: "highlights", label: "ハイライト", title: "明るい部分の色付け" },
  { key: "global", label: "全体", title: "すべての部分に同じように効く色付け" },
];

/** 色の項目を使える最初の処理バージョン（genzo-pipeline の ProcessVersion::V2）。 */
export const COLOR_PROCESS_VERSION = 2;

function bands<T>(f: () => T): HueBands<T> {
  return Object.fromEntries(HUE_BANDS.map((b) => [b, f()])) as HueBands<T>;
}

/** 既定のカラーミキサー（すべて 0）。 */
export function defaultMixer(): HueBands<HslAdjustment> {
  return bands(() => ({ hue: 0, saturation: 0, luminance: 0 }));
}

/** 既定の色相環（強さ 0）。 */
export function defaultWheel(): GradingWheel {
  return { hue: 0, saturation: 0, luminance: 0 };
}

/** 既定のカラーグレーディング（genzo-model の ColorGrading::default。ブレンド 50・バランス 0）。 */
export function defaultGrading(): ColorGrading {
  return {
    shadows: defaultWheel(),
    midtones: defaultWheel(),
    highlights: defaultWheel(),
    global: defaultWheel(),
    blending: 50,
    balance: 0,
  };
}

/** 既定の白黒（カラー。白黒ミキサーはすべて 0）。 */
export function defaultBlackWhite(): BlackWhite {
  return { enabled: false, mix: bands(() => 0) };
}

/** 既定の色（genzo-model の ColorParams::default）。 */
export function defaultColor(): ColorParams {
  return { saturation: 0, vibrance: 0, mixer: defaultMixer(), grading: defaultGrading(), black_white: defaultBlackWhite() };
}

type Loose<T> = { [K in keyof T]?: T[K] extends object ? Loose<T[K]> : T[K] } | null | undefined;

/**
 * 古いコアや古い設定で項目が欠けていても、色のすべての項目をそろえる（欠けた項目は既定値。コアの serde の既定と
 * 同じ）。
 */
export function fullColor(c: Loose<ColorParams>): ColorParams {
  const d = defaultColor();
  const m = c?.mixer ?? {};
  const g = c?.grading ?? {};
  const bw = c?.black_white ?? {};
  const wheel = (w: Loose<GradingWheel>) => ({ ...defaultWheel(), ...(w ?? {}) });
  return {
    saturation: c?.saturation ?? d.saturation,
    vibrance: c?.vibrance ?? d.vibrance,
    mixer: Object.fromEntries(
      HUE_BANDS.map((b) => [b, { ...d.mixer[b], ...((m as Record<string, Partial<HslAdjustment>>)[b] ?? {}) }]),
    ) as HueBands<HslAdjustment>,
    grading: {
      shadows: wheel(g.shadows),
      midtones: wheel(g.midtones),
      highlights: wheel(g.highlights),
      global: wheel(g.global),
      blending: g.blending ?? d.grading.blending,
      balance: g.balance ?? d.grading.balance,
    },
    black_white: {
      enabled: bw.enabled ?? false,
      mix: { ...d.black_white.mix, ...((bw.mix ?? {}) as Partial<HueBands<number>>) },
    },
  };
}

/** 白黒にする（on）・カラーに戻す（off）。ほかの値（白黒ミキサー・カラーミキサー）は保つ。 */
export function withBlackWhite(s: DevelopSettings, on: boolean): DevelopSettings {
  const c = fullColor(s.color);
  return { ...s, color: { ...c, black_white: { ...c.black_white, enabled: on } } };
}

/** 白黒とカラーを切り替える（V キー）。 */
export function toggleBlackWhite(s: DevelopSettings): DevelopSettings {
  return withBlackWhite(s, !fullColor(s.color).black_white.enabled);
}

/** カラーミキサーの 1 つの帯の 1 つの値を変える。 */
export function withMixer(s: DevelopSettings, band: HueBand, aspect: MixerAspect, v: number): DevelopSettings {
  const c = fullColor(s.color);
  return { ...s, color: { ...c, mixer: { ...c.mixer, [band]: { ...c.mixer[band], [aspect]: v } } } };
}

/** 白黒ミキサーの 1 つの帯を変える。 */
export function withBwMix(s: DevelopSettings, band: HueBand, v: number): DevelopSettings {
  const c = fullColor(s.color);
  return { ...s, color: { ...c, black_white: { ...c.black_white, mix: { ...c.black_white.mix, [band]: v } } } };
}

/** カラーグレーディングの 1 つの範囲を変える。 */
export function withWheel(s: DevelopSettings, range: GradingRange, patch: Partial<GradingWheel>): DevelopSettings {
  const c = fullColor(s.color);
  return { ...s, color: { ...c, grading: { ...c.grading, [range]: { ...c.grading[range], ...patch } } } };
}

/** カラーグレーディングのブレンド・バランスを変える。 */
export function withGrading(s: DevelopSettings, patch: Partial<Pick<ColorGrading, "blending" | "balance">>): DevelopSettings {
  const c = fullColor(s.color);
  return { ...s, color: { ...c, grading: { ...c.grading, ...patch } } };
}

/** カラーミキサーが何もしないか。 */
export function mixerIsNeutral(m: HueBands<HslAdjustment>): boolean {
  return HUE_BANDS.every((b) => m[b].hue === 0 && m[b].saturation === 0 && m[b].luminance === 0);
}

/** 白黒ミキサーがすべて 0 か。 */
export function bwMixIsNeutral(mix: HueBands<number>): boolean {
  return HUE_BANDS.every((b) => mix[b] === 0);
}

/** 色相環が何もしないか（強さと輝度が 0。色相は強さが 0 なら効かない）。 */
export function wheelIsNeutral(w: GradingWheel): boolean {
  return w.saturation === 0 && w.luminance === 0;
}

/** カラーグレーディングが既定のままか（リセットのボタンの可否。ブレンド・バランスも見る）。 */
export function gradingIsDefault(g: ColorGrading): boolean {
  return JSON.stringify(g) === JSON.stringify(defaultGrading());
}

/** 色のパネルの各項目を操作できるか。 */
export interface ColorControls {
  /** 処理バージョンが古く、カラーミキサー・カラーグレーディング・白黒を使えない（更新が必要）。 */
  needsUpgrade: boolean;
  /** 白黒にしている。 */
  blackWhite: boolean;
  /** 「カラー / 白黒」の切り替え。 */
  treatment: boolean;
  /** 彩度・自然な彩度（白黒のときは使われない）。 */
  saturation: boolean;
  /** カラーミキサー（白黒でないとき）。 */
  mixer: boolean;
  /** 白黒ミキサー（白黒のとき）。 */
  bwMix: boolean;
  /** カラーグレーディング。 */
  grading: boolean;
}

/**
 * 色のパネルの各項目を操作できるか。カラーミキサー・カラーグレーディング・白黒は処理バージョン 2 から使え、RAW 以外の
 * 写真にも使える（仕上げの段階の処理）。白黒のときは、彩度・自然な彩度・カラーミキサーを使わず（値は保つ）、白黒
 * ミキサーを使う。`disabled` は写真を開いていないなど、パネル全体を使えないとき。
 */
export function colorControls(settings: DevelopSettings, disabled: boolean): ColorControls {
  const needsUpgrade = settings.process_version < COLOR_PROCESS_VERSION;
  const bw = fullColor(settings.color).black_white.enabled;
  const on = !disabled && !needsUpgrade;
  return {
    needsUpgrade,
    blackWhite: bw,
    treatment: on,
    saturation: !disabled && !(bw && !needsUpgrade),
    mixer: on && !bw,
    bwMix: on && bw,
    grading: on,
  };
}

// ---------------------------------------------------------------------------
// 色相環（カラーグレーディング）
// ---------------------------------------------------------------------------

/**
 * sRGB の HSV（色相 h 度・彩度 s・明度 v。s と v は 0〜1）の色を CSS の `rgb(...)` にする（帯の色・色相環の表示）。
 */
export function hsvCss(h: number, s = 1, v = 1): string {
  const hh = (((h % 360) + 360) % 360) / 60;
  const c = v * s;
  const x = c * (1 - Math.abs((hh % 2) - 1));
  const m = v - c;
  const [r, g, b] =
    hh < 1 ? [c, x, 0] : hh < 2 ? [x, c, 0] : hh < 3 ? [0, c, x] : hh < 4 ? [0, x, c] : hh < 5 ? [x, 0, c] : [c, 0, x];
  const to = (u: number) => Math.round((u + m) * 255);
  return `rgb(${to(r)}, ${to(g)}, ${to(b)})`;
}

/**
 * 色相環の上の点（中心からの位置。右が +x、**下が +y**（画面の座標））を、色相（度。0〜360。右が 0、反時計回りに
 * 増える）と強さ（0〜100。縁が 100）にする。中心では色相を `fallbackHue` のままにする。
 */
export function wheelPointToHueSat(
  dx: number,
  dy: number,
  radius: number,
  fallbackHue = 0,
): { hue: number; saturation: number } {
  const r = Math.hypot(dx, dy);
  if (!(radius > 0) || !(r > 0)) return { hue: fallbackHue, saturation: 0 };
  let hue = (Math.atan2(-dy, dx) * 180) / Math.PI;
  if (hue < 0) hue += 360;
  // -0（atan2(-0, x)）は 0 にする。
  hue = Math.round(hue) % 360 || 0;
  const saturation = Math.min(100, Math.round((r / radius) * 100));
  return { hue, saturation };
}

/** 色相（度）と強さ（0〜100）を、色相環の上の点（中心からの位置。下が +y）にする。 */
export function hueSatToWheelPoint(hue: number, saturation: number, radius: number): { x: number; y: number } {
  const a = (hue * Math.PI) / 180;
  const r = (Math.min(100, Math.max(0, saturation)) / 100) * radius;
  return { x: Math.cos(a) * r, y: -Math.sin(a) * r };
}

/**
 * 色相環の背景（CSS）。右が色相 0（レッド）で、反時計回りに色相が増える（CSS の conic-gradient は上から時計回り
 * なので、右から始めて色相の逆の順に並べる）。中心ほど灰色。
 */
export function wheelBackground(): string {
  const stops: string[] = [];
  for (let i = 0; i <= 12; i++) {
    const hue = (360 - i * 30) % 360;
    stops.push(`${hsvCss(hue, 1, 0.9)} ${i * 30}deg`);
  }
  return `radial-gradient(circle closest-side, rgb(128, 128, 128), rgba(128, 128, 128, 0)), conic-gradient(from 90deg, ${stops.join(", ")})`;
}
