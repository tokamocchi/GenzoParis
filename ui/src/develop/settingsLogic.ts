// 現像設定の組み立て（WB の選択・レンズ補正・変形（遠近補正）・ハイライト復元・ノイズ軽減の操作の可否・リセット。DOM を
// 使わない純粋な計算）。色（カラーミキサー・カラーグレーディング・白黒）は colorLogic.ts、テクスチャ・明瞭度・かすみの除去・シャープ・
// 効果は detailLogic.ts、カメラプロファイル・ルックは profileLogic.ts。

import {
  type AsShotWhiteBalance,
  type DevelopSettings,
  type Geometry,
  type HighlightRecovery,
  type LensCorrection,
  type NoiseReduction,
  RANGES,
  type SourceInfo,
  type Transform,
  type UprightMode,
  type WbPreset,
  type WhiteBalance,
  WB_PRESETS,
} from "../api/types";
import { defaultColor } from "./colorLogic";
import { FULL_CROP } from "./cropMath";
import { defaultEffects, defaultPresence, defaultSharpening } from "./detailLogic";
import { defaultProfile } from "./profileLogic";

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

/** 既定のレンズ補正（補正なし。genzo-model の LensCorrection::default）。 */
export function defaultLens(): LensCorrection {
  return {
    profile_enabled: false,
    distortion_scale: 100,
    vignetting_scale: 100,
    remove_chromatic_aberration: false,
    distortion: 0,
    vignetting_amount: 0,
    vignetting_midpoint: 50,
    ca_red_cyan: 0,
    ca_blue_yellow: 0,
  };
}

/**
 * 古いコアや古い設定で項目が欠けていても、レンズ補正のすべての項目をそろえる（欠けた項目は既定値。コアの
 * serde の既定と同じ）。
 */
export function fullLens(lens: Partial<LensCorrection> | null | undefined): LensCorrection {
  return { ...defaultLens(), ...(lens ?? {}) };
}

/** レンズ補正を使える最初の処理バージョン（genzo-pipeline の ProcessVersion::V2）。 */
export const LENS_PROCESS_VERSION = 2;

/** 処理バージョンを更新できるか（コアが最新の版を知らせていて、設定の版がそれより古い）。 */
export function canUpgradeProcess(processVersion: number, latest: number | undefined): boolean {
  return latest !== undefined && processVersion < latest;
}

/** レンズ補正のパネルの各項目を操作できるか（DEV-14）。 */
export interface LensControls {
  /** 処理バージョンが古く、レンズ補正を使えない（更新が必要）。 */
  needsUpgrade: boolean;
  /** 「プロファイルによる補正を有効」（RAW だけ。補正の情報は RAW に埋め込まれている）。 */
  profile: boolean;
  /** 歪曲補正の適用量（プロファイルの歪曲の情報があり、プロファイルの補正が有効）。 */
  distortionScale: boolean;
  /** 周辺減光補正の適用量（RAW で、プロファイルの周辺減光の情報があり、プロファイルの補正が有効）。 */
  vignettingScale: boolean;
  /** 「色収差を除去」（RAW だけ）。 */
  removeCa: boolean;
  /** 手動の歪曲（RAW 以外の写真にも使える）。 */
  manualDistortion: boolean;
  /** 手動の周辺減光・色収差（RAW だけ。センサーの色空間で補正する）。 */
  manualSensor: boolean;
}

/**
 * レンズ補正のパネルの各項目を操作できるか。周辺減光と倍率色収差はセンサーの色空間の段階（ステージ 7）で
 * 補正するので RAW だけ、歪曲は仕上げの座標変換（ステージ 9）なので、どの写真にも使える。`disabled` は
 * 写真を開いていないなど、パネル全体を使えないとき。
 */
export function lensControls(
  settings: DevelopSettings,
  source: SourceInfo | null,
  disabled: boolean,
): LensControls {
  const lens = fullLens(settings.lens);
  const needsUpgrade = settings.process_version < LENS_PROCESS_VERSION;
  const on = !disabled && !needsUpgrade && source !== null;
  const raw = !!source?.is_raw;
  const info = source?.lens_correction ?? null;
  return {
    needsUpgrade,
    profile: on && raw,
    distortionScale: on && raw && lens.profile_enabled && !!info?.distortion,
    vignettingScale: on && raw && lens.profile_enabled && !!info?.vignetting,
    removeCa: on && raw,
    manualDistortion: on,
    manualSensor: on && raw,
  };
}

/**
 * レンズ補正のパネルの、埋め込みの補正の情報の説明（RAW だけ。RAW 以外・写真を開いていなければ null）。情報が
 * 壊れていて使えなかったとき（コアの lens_correction_note）は、もともとない場合と区別して理由を出す。
 */
export function lensInfoNote(source: SourceInfo | null): string | null {
  if (!source?.is_raw) return null;
  const info = source.lens_correction ?? null;
  if (info) return `補正の情報: ${info.description}`;
  const note = source.lens_correction_note ?? null;
  if (note) return `埋め込みの補正の情報が壊れているため使えません（${note}）。手動で補正できます。`;
  return "この写真には埋め込みの補正の情報がありません（手動で補正できます）。";
}

/** 既定の変形（何もしない。genzo-model の Transform::default）。 */
export function defaultTransform(): Transform {
  return { vertical: 0, horizontal: 0, aspect: 0, scale: 100, offset_x: 0, offset_y: 0 };
}

/** 古いコアや古い設定で項目が欠けていても、変形のすべての項目をそろえる（欠けた項目は既定値）。 */
export function fullTransform(t: Partial<Transform> | null | undefined): Transform {
  return { ...defaultTransform(), ...(t ?? {}) };
}

/** 2 つの変形が同じか（すべての項目が一致）。 */
export function sameTransform(a: Transform, b: Transform): boolean {
  return (Object.keys(defaultTransform()) as (keyof Transform)[]).every((k) => a[k] === b[k]);
}

/** 何も変形しない設定か。 */
export function transformIsIdentity(t: Partial<Transform> | null | undefined): boolean {
  return sameTransform(fullTransform(t), defaultTransform());
}

/** 変形（遠近補正）を使える最初の処理バージョン（genzo-pipeline の ProcessVersion::V2）。 */
export const TRANSFORM_PROCESS_VERSION = 2;

/** 変形のパネルの各項目を操作できるか（DEV-16）。 */
export interface TransformControls {
  /** 処理バージョンが古く、変形を使えない（更新が必要）。 */
  needsUpgrade: boolean;
  /** 垂直・水平・縦横比・拡大縮小・ずらしのスライダーとリセット。 */
  sliders: boolean;
  /** 自動の「水平」（角度補正だけ。どの処理バージョンでも使える）。 */
  uprightLevel: boolean;
  /** 自動の「垂直」「完全」（処理バージョン 2 から）。 */
  uprightPerspective: boolean;
}

/**
 * 変形のパネルの各項目を操作できるか。遠近補正は仕上げの座標変換（ステージ 9）なので、RAW 以外の写真にも使える。
 * `disabled` は写真を開いていないなど、パネル全体を使えないとき。
 */
export function transformControls(settings: DevelopSettings, source: SourceInfo | null, disabled: boolean): TransformControls {
  const needsUpgrade = settings.process_version < TRANSFORM_PROCESS_VERSION;
  const open = !disabled && source !== null;
  return {
    needsUpgrade,
    sliders: open && !needsUpgrade,
    uprightLevel: open,
    uprightPerspective: open && !needsUpgrade,
  };
}

/** 自動の遠近補正の種類の名前（ボタンと知らせ）。 */
export const UPRIGHT_LABELS: Record<UprightMode, string> = {
  level: "水平",
  vertical: "垂直",
  full: "完全",
};

/** 自動の遠近補正の結果の知らせの文（適用した後に出す）。 */
export function describeUpright(mode: UprightMode, angle_deg: number, t: Transform, lines: { vertical: number; horizontal: number }): string {
  const parts = [`角度 ${angle_deg >= 0 ? "+" : ""}${angle_deg.toFixed(2)}°`];
  if (mode !== "level") parts.push(`垂直 ${t.vertical >= 0 ? "+" : ""}${t.vertical.toFixed(1)}`);
  if (mode === "full") parts.push(`水平 ${t.horizontal >= 0 ? "+" : ""}${t.horizontal.toFixed(1)}`);
  return `自動の補正（${UPRIGHT_LABELS[mode]}）: ${parts.join("・")}（垂直の線 ${lines.vertical} 本・水平の線 ${lines.horizontal} 本）`;
}

/** 既定のハイライト復元（復元しない。genzo-model の HighlightRecovery::default）。 */
export function defaultHighlightRecovery(): HighlightRecovery {
  return { method: "clip", amount: 100 };
}

/** 古いコアや古い設定で項目が欠けていても、ハイライト復元の項目をそろえる（欠けた項目は既定値）。 */
export function fullHighlightRecovery(h: Partial<HighlightRecovery> | null | undefined): HighlightRecovery {
  return { ...defaultHighlightRecovery(), ...(h ?? {}) };
}

/** 既定のノイズ軽減（量 0・ディテール 50。genzo-model の NoiseReduction::default）。 */
export function defaultNoiseReduction(): NoiseReduction {
  return { luminance: 0, luminance_detail: 50, luminance_contrast: 0, color: 0, color_detail: 50 };
}

/** 古いコアや古い設定で項目が欠けていても、ノイズ軽減の項目をそろえる（欠けた項目は既定値）。 */
export function fullNoiseReduction(n: Partial<NoiseReduction> | null | undefined): NoiseReduction {
  return { ...defaultNoiseReduction(), ...(n ?? {}) };
}

/** ハイライト復元・ノイズ軽減を使える最初の処理バージョン（genzo-pipeline の ProcessVersion::V2）。 */
export const SENSOR_PROCESS_VERSION = 2;

/** ハイライト復元・ノイズ軽減（DEV-06・DEV-13）の各項目を操作できるか。 */
export interface SensorControls {
  /** 処理バージョンが古く、使えない（更新が必要）。 */
  needsUpgrade: boolean;
  /** RAW 以外の写真で、使えない（センサーの段階の処理のため）。 */
  notRaw: boolean;
  /** ハイライト復元の方式。 */
  highlight: boolean;
  /** ハイライト復元の量（復元する方式のとき）。 */
  highlightAmount: boolean;
  /** 輝度・カラーノイズの軽減の量。 */
  noise: boolean;
  /** 輝度ノイズの軽減のディテール・コントラスト（輝度の量が 0 でないとき）。 */
  luminanceOptions: boolean;
  /** カラーノイズの軽減のディテール（カラーの量が 0 でないとき）。 */
  colorOptions: boolean;
}

/**
 * ハイライト復元・ノイズ軽減の各項目を操作できるか。どちらもセンサーの段階（ステージ 4・6）の処理なので RAW だけで、
 * 処理バージョン 2 から使える。ディテール・コントラストは、それぞれの量が 0 なら効かないので動かせない（Lightroom
 * Classic と同じ）。`disabled` は写真を開いていないなど、パネル全体を使えないとき。
 */
export function sensorControls(
  settings: DevelopSettings,
  source: SourceInfo | null,
  disabled: boolean,
): SensorControls {
  const needsUpgrade = settings.process_version < SENSOR_PROCESS_VERSION;
  const notRaw = source !== null && !source.is_raw;
  const on = !disabled && !needsUpgrade && source !== null && source.is_raw;
  const hr = fullHighlightRecovery(settings.highlight_recovery);
  const nr = fullNoiseReduction(settings.detail?.noise_reduction);
  return {
    needsUpgrade,
    notRaw,
    highlight: on,
    highlightAmount: on && hr.method !== "clip",
    noise: on,
    luminanceOptions: on && nr.luminance > 0,
    colorOptions: on && nr.color > 0,
  };
}

/**
 * すべてリセット（genzo-model の DevelopSettings::default の値にする）。
 *
 * - schema_version・process_version・render_deps は今の値を保つ（コアが管理する値）。
 * - レンズ補正・変形（遠近補正）・ハイライト復元・ノイズ軽減・シャープ・テクスチャ・明瞭度・かすみの除去・効果・色
 *   （カラーミキサー・カラーグレーディング・白黒）・カメラプロファイルとルック（標準の色・ルックなし）は既定に戻す
 *   （画面のパネルで見える項目）。
 * - マスク（ローカル補正。DEV-18・DEV-19）も除く（現像のパネルの「マスク」で見える項目。履歴から取り消せる）。
 */
export function resetAll(s: DevelopSettings): DevelopSettings {
  return {
    ...s,
    lens: defaultLens(),
    transform: defaultTransform(),
    highlight_recovery: defaultHighlightRecovery(),
    detail: {
      sharpening: defaultSharpening(),
      noise_reduction: defaultNoiseReduction(),
    },
    presence: defaultPresence(),
    effects: defaultEffects(),
    profile: defaultProfile(),
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
    color: defaultColor(),
    geometry: defaultGeometry(),
    masks: [],
  };
}
