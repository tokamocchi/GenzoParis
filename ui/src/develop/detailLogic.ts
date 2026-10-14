// テクスチャ・明瞭度・かすみの除去（DEV-11）・シャープ（DEV-12）・効果（切り抜き後の周辺光量補正・粒子。DEV-24）の
// 設定の組み立てと操作の可否（DOM を使わない純粋な計算）。どれも処理バージョン 2 から使え、RAW 以外の写真にも使える
// （仕上げの段階の処理）。

import type {
  DevelopSettings,
  EffectsParams,
  Grain,
  PostCropVignette,
  PresenceParams,
  Sharpening,
} from "../api/types";

/** テクスチャ・明瞭度・かすみの除去・シャープ・効果を使える最初の処理バージョン（genzo-pipeline の ProcessVersion::V2）。 */
export const DETAIL_PROCESS_VERSION = 2;

/** 既定のテクスチャ・明瞭度・かすみの除去（何もしない。genzo-model の PresenceParams::default）。 */
export function defaultPresence(): PresenceParams {
  return { texture: 0, clarity: 0, dehaze: 0 };
}

/** 古いコアや古い設定で項目が欠けていても、テクスチャ・明瞭度・かすみの除去の項目をそろえる。 */
export function fullPresence(p: Partial<PresenceParams> | null | undefined): PresenceParams {
  return { ...defaultPresence(), ...(p ?? {}) };
}

/** 既定のシャープ（適用量 0・半径 1・ディテール 25・マスク 0。genzo-model の Sharpening::default）。 */
export function defaultSharpening(): Sharpening {
  return { amount: 0, radius: 1, detail: 25, masking: 0 };
}

/** 古いコアや古い設定で項目が欠けていても、シャープの項目をそろえる。 */
export function fullSharpening(s: Partial<Sharpening> | null | undefined): Sharpening {
  return { ...defaultSharpening(), ...(s ?? {}) };
}

/** 既定の切り抜き後の周辺光量補正（量 0。genzo-model の PostCropVignette::default）。 */
export function defaultVignette(): PostCropVignette {
  return { amount: 0, midpoint: 50, roundness: 0, feather: 50, highlights: 0 };
}

/** 既定の粒子（量 0。genzo-model の Grain::default）。 */
export function defaultGrain(): Grain {
  return { amount: 0, size: 25, roughness: 50 };
}

/** 既定の効果。 */
export function defaultEffects(): EffectsParams {
  return { vignette: defaultVignette(), grain: defaultGrain() };
}

/** 古いコアや古い設定で項目が欠けていても、効果の項目をそろえる（欠けた項目は既定値）。 */
export function fullEffects(e: Partial<EffectsParams> | null | undefined): EffectsParams {
  return {
    vignette: { ...defaultVignette(), ...(e?.vignette ?? {}) },
    grain: { ...defaultGrain(), ...(e?.grain ?? {}) },
  };
}

/** テクスチャ・明瞭度・かすみの除去の一部を変えた設定。 */
export function withPresence(s: DevelopSettings, patch: Partial<PresenceParams>): DevelopSettings {
  return { ...s, presence: { ...fullPresence(s.presence), ...patch } };
}

/** シャープの一部を変えた設定。 */
export function withSharpening(s: DevelopSettings, patch: Partial<Sharpening>): DevelopSettings {
  return { ...s, detail: { ...s.detail, sharpening: { ...fullSharpening(s.detail?.sharpening), ...patch } } };
}

/** 切り抜き後の周辺光量補正の一部を変えた設定。 */
export function withVignette(s: DevelopSettings, patch: Partial<PostCropVignette>): DevelopSettings {
  const e = fullEffects(s.effects);
  return { ...s, effects: { ...e, vignette: { ...e.vignette, ...patch } } };
}

/** 粒子の一部を変えた設定。 */
export function withGrain(s: DevelopSettings, patch: Partial<Grain>): DevelopSettings {
  const e = fullEffects(s.effects);
  return { ...s, effects: { ...e, grain: { ...e.grain, ...patch } } };
}

/** 2 つの値の組が同じか（項目ごとに比べる。項目の順によらない）。 */
function sameValues<T extends object>(a: T, b: T): boolean {
  return (Object.keys(a) as (keyof T)[]).every((k) => a[k] === b[k]);
}

/** テクスチャ・明瞭度・かすみの除去・シャープ・効果の各項目を操作できるか。 */
export interface DetailControls {
  /** 処理バージョンが古く、使えない（更新が必要）。 */
  needsUpgrade: boolean;
  /** テクスチャ・明瞭度・かすみの除去。 */
  presence: boolean;
  /** シャープの適用量。 */
  sharpen: boolean;
  /** シャープの半径・ディテール・マスク（適用量が 0 でないとき）。 */
  sharpenOptions: boolean;
  /** 周辺光量補正の量。 */
  vignette: boolean;
  /** 周辺光量補正の中点・丸み・ぼかし（量が 0 でないとき）。 */
  vignetteOptions: boolean;
  /** 周辺光量補正のハイライトの保護（暗くするとき。量が負）。 */
  vignetteHighlights: boolean;
  /** 粒子の量。 */
  grain: boolean;
  /** 粒子の大きさ・粗さ（量が 0 でないとき）。 */
  grainOptions: boolean;
  /** シャープを既定に戻せる（既定と違う）。 */
  sharpenChanged: boolean;
  /** 効果を既定に戻せる（既定と違う）。 */
  effectsChanged: boolean;
}

/**
 * 各項目を操作できるか。どれも処理バージョン 2 から使える。量が 0 なら効かない項目（シャープの半径など）は動かせない
 * （ノイズ軽減のディテールと同じ扱い）。`disabled` は写真を開いていないなど、パネル全体を使えないとき。
 */
export function detailControls(settings: DevelopSettings, disabled: boolean): DetailControls {
  const needsUpgrade = settings.process_version < DETAIL_PROCESS_VERSION;
  const on = !disabled && !needsUpgrade;
  const sh = fullSharpening(settings.detail?.sharpening);
  const fx = fullEffects(settings.effects);
  return {
    needsUpgrade,
    presence: on,
    sharpen: on,
    sharpenOptions: on && sh.amount > 0,
    vignette: on,
    vignetteOptions: on && fx.vignette.amount !== 0,
    vignetteHighlights: on && fx.vignette.amount < 0,
    grain: on,
    grainOptions: on && fx.grain.amount > 0,
    sharpenChanged: !sameValues(sh, defaultSharpening()),
    effectsChanged: !sameValues(fx.vignette, defaultVignette()) || !sameValues(fx.grain, defaultGrain()),
  };
}
