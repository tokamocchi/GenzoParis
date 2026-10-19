// 現像のパネル（右。DEV-03〜10・13〜15・25・26）。
//
// - ヒストグラム・基本補正（カラー / 白黒の切り替え（処理バージョン 2 から。V キー）・WB・露光量・コントラスト・
//   ハイライト / シャドウ・白 / 黒・ハイライト復元（処理バージョン 2 から。RAW だけ）・外観（テクスチャ・明瞭度・
//   かすみの除去。処理バージョン 2 から）・自然な彩度・彩度）・トーンカーブ（パラメトリックとポイント）・カラーミキサー
//   （色相・彩度・輝度。白黒のときは白黒ミキサー。処理バージョン 2 から）・カラーグレーディング（シャドウ・中間調・
//   ハイライト・全体の色相環、ブレンド・バランス。処理バージョン 2 から）・ディテール（シャープ（処理バージョン 2 から）・
//   ノイズ軽減の輝度・カラー（処理バージョン 2 から。RAW だけ））・切り抜きと角度・レンズ補正（プロファイル・色収差の
//   除去・手動の歪曲 / 周辺減光 / 色収差。処理バージョン 2 から）・効果（切り抜き後の周辺光量補正・粒子。処理
//   バージョン 2 から）・プロファイル（カメラプロファイル（DCP。RAW だけ）・ルック（3D LUT）と適用量。取り込み・管理の
//   ダイアログ。処理バージョン 2 から。DEV-17）・マスク（ローカル補正。ブラシ・線形 / 円形グラデーション・色域・輝度範囲
//   の部品と調整。処理バージョン 2 から。DEV-18・DEV-19。MaskPanel.tsx）・すべてリセット・警告と元の画像の情報
//   （処理バージョンの更新）。
// - 値は developStore.settings（送信前を含む最新の値）から読み、変更は applySettings で送る（ドラッグ中は
//   dragging: true、離したら endDrag。state/develop.ts の送信の列）。
// - 写真を開いている間・開けなかったときは、操作を無効にする。

import { useEffect, useState } from "react";

import type {
  CurvePoint,
  DevelopSettings,
  Grain,
  HighlightMethod,
  LensCorrection,
  NoiseReduction,
  PostCropVignette,
  PresenceParams,
  Sharpening,
  WbPreset,
} from "../api/types";
import { HUE_BANDS, RANGES, WB_PRESETS } from "../api/types";
import { Section } from "../components/Section";
import { Slider } from "../components/Slider";
import { comboLabel } from "../keyboard";
import { actions, useApp } from "../state/app";
import { applySettings, developStore, endDrag, upgradeProcessVersion, useDevelop } from "../state/develop";
import { importProfileFiles, refreshProfiles, useProfiles } from "../state/profiles";
import {
  GRADING_RANGES,
  type GradingRange,
  HUE_BAND_INFO,
  MIXER_ASPECTS,
  type MixerAspect,
  bwMixIsNeutral,
  colorControls,
  defaultBlackWhite,
  defaultGrading,
  defaultMixer,
  fullColor,
  gradingIsDefault,
  hsvCss,
  mixerIsNeutral,
  wheelIsNeutral,
  withBlackWhite,
  withBwMix,
  withGrading,
  withMixer,
  withWheel,
} from "./colorLogic";
import { ColorWheel } from "./ColorWheel";
import { ASPECT_CHOICES, aspectKey } from "./cropMath";
import { CURVE_CHANNELS, type CurveChannel, withChannel } from "./curveMath";
import {
  type DetailControls,
  defaultEffects,
  defaultSharpening,
  detailControls,
  fullEffects,
  fullPresence,
  fullSharpening,
  withGrain,
  withPresence,
  withSharpening,
  vignetteCenterOf,
  withVignette,
} from "./detailLogic";
import { Histogram } from "./Histogram";
import { MaskSection } from "./MaskPanel";
import {
  cameraChoices,
  defaultProfile,
  fullProfile,
  lookChoices,
  profileControls,
  profileIsDefault,
  selectsOtherCamera,
  withProfile,
} from "./profileLogic";
import { useGeometryInfo, useLoadedSummary } from "./hooks";
import {
  formatAngle,
  formatEv,
  formatKelvin,
  formatPercent,
  formatSigned,
  parseNumber,
  positionToTemperature,
  sameShown,
  temperatureToPosition,
} from "./scales";
import {
  type WbMode,
  canUpgradeProcess,
  customWb,
  defaultGeometry,
  defaultLens,
  defaultNoiseReduction,
  effectiveWb,
  fullHighlightRecovery,
  fullLens,
  fullNoiseReduction,
  lensControls,
  lensInfoNote,
  resetAll,
  sensorControls,
  wbFromMode,
  wbMode,
} from "./settingsLogic";
import { ToneCurveEditor } from "./ToneCurveEditor";
import {
  cancelCrop,
  changeAngle,
  chooseAspect,
  commitAngle,
  commitCrop,
  selectMask,
  startCrop,
  toggleWbTool,
  useDevelopView,
  viewStore,
} from "./viewState";
import "./develop.css";

/** 写真を開くまでの間に出す値（表示だけで、コアには送らない。操作は無効）。 */
const PLACEHOLDER: DevelopSettings = resetAll({
  schema_version: 0,
  process_version: 0,
  lens: defaultLens(),
  detail: {
    sharpening: defaultSharpening(),
    noise_reduction: defaultNoiseReduction(),
  },
  masks: [],
  render_deps: { camera_profile: { id: "", sha256: "" }, lens_profile: null, raw_decoder: "" },
} as unknown as DevelopSettings);

const NO_WARNINGS: string[] = [];

/**
 * 設定を変えて送る（最新の値から作る。写真を開いていなければ何もしない）。`fn` が null を返したら
 * 変えない（値を変えずに入力欄を離れたときなど）。
 */
function update(fn: (s: DevelopSettings) => DevelopSettings | null, dragging: boolean): void {
  const d = developStore.get();
  if (!d.state || !d.settings) return;
  const next = fn(d.settings);
  if (next === null || next === d.settings) return;
  applySettings(next, dragging);
}

/** −100〜100 のスライダーの書式。 */
const signed0 = (v: number) => formatSigned(v, 0);

interface SliderDef {
  key: string;
  label: string;
  get: (s: DevelopSettings) => number;
  set: (s: DevelopSettings, v: number) => DevelopSettings;
}

const TONE_SLIDERS: SliderDef[] = [
  { key: "contrast", label: "コントラスト", get: (s) => s.contrast, set: (s, v) => ({ ...s, contrast: v }) },
  {
    key: "highlights",
    label: "ハイライト",
    get: (s) => s.tone.highlights,
    set: (s, v) => ({ ...s, tone: { ...s.tone, highlights: v } }),
  },
  {
    key: "shadows",
    label: "シャドウ",
    get: (s) => s.tone.shadows,
    set: (s, v) => ({ ...s, tone: { ...s.tone, shadows: v } }),
  },
  {
    key: "whites",
    label: "白レベル",
    get: (s) => s.tone.whites,
    set: (s, v) => ({ ...s, tone: { ...s.tone, whites: v } }),
  },
  {
    key: "blacks",
    label: "黒レベル",
    get: (s) => s.tone.blacks,
    set: (s, v) => ({ ...s, tone: { ...s.tone, blacks: v } }),
  },
];

const COLOR_SLIDERS: SliderDef[] = [
  {
    key: "vibrance",
    label: "自然な彩度",
    get: (s) => s.color.vibrance,
    set: (s, v) => ({ ...s, color: { ...s.color, vibrance: v } }),
  },
  {
    key: "saturation",
    label: "彩度",
    get: (s) => s.color.saturation,
    set: (s, v) => ({ ...s, color: { ...s.color, saturation: v } }),
  },
];

const PARAMETRIC_SLIDERS: SliderDef[] = (
  [
    ["highlights", "ハイライト"],
    ["lights", "ライト"],
    ["darks", "ダーク"],
    ["shadows", "シャドウ"],
  ] as const
).map(([key, label]) => ({
  key,
  label,
  get: (s: DevelopSettings) => s.tone_curve.parametric[key],
  set: (s: DevelopSettings, v: number) => ({
    ...s,
    tone_curve: { ...s.tone_curve, parametric: { ...s.tone_curve.parametric, [key]: v } },
  }),
}));

function SliderRows(props: { defs: SliderDef[]; settings: DevelopSettings; disabled: boolean }) {
  return (
    <>
      {props.defs.map((d) => (
        <Slider
          key={d.key}
          label={d.label}
          value={d.get(props.settings)}
          min={RANGES.slider.min}
          max={RANGES.slider.max}
          step={1}
          defaultValue={0}
          format={signed0}
          parse={parseNumber}
          disabled={props.disabled}
          onChange={(v, dragging) => update((s) => (sameShown(signed0, v, d.get(s)) ? null : d.set(s, v)), dragging)}
          onCommit={endDrag}
        />
      ))}
    </>
  );
}

// ---------------------------------------------------------------------------
// 基本補正
// ---------------------------------------------------------------------------

function WhiteBalanceRows(props: { settings: DevelopSettings; disabled: boolean }) {
  const source = useDevelop((s) => s.state?.source ?? null);
  const tool = useDevelopView((s) => s.tool);
  const isRaw = !!source?.is_raw;
  const asShot = source?.as_shot_white_balance ?? null;
  const wb = props.settings.white_balance;
  const eff = effectiveWb(wb, asShot);
  const off = props.disabled || !isRaw;
  const mode = wbMode(wb);
  // 色温度・色かぶり補正のダブルクリック: 既定（撮影時の設定）に戻す。
  const resetWb = (dragging: boolean) =>
    update((s) => (s.white_balance === "as_shot" ? null : { ...s, white_balance: "as_shot" }), dragging);
  return (
    <>
      <div className="dp-sub">ホワイトバランス</div>
      <div className="dp-row">
        <select
          value={mode}
          disabled={off}
          aria-label="ホワイトバランス"
          onChange={(e) =>
            update(
              (s) => ({ ...s, white_balance: wbFromMode(e.target.value as WbMode, s.white_balance, asShot) }),
              false,
            )
          }
        >
          <option value="as_shot">撮影時の設定</option>
          {(Object.keys(WB_PRESETS) as WbPreset[]).map((k) => (
            <option key={k} value={k}>
              {WB_PRESETS[k].label}
            </option>
          ))}
          <option value="custom">カスタム</option>
        </select>
        <button
          className={`btn small${tool === "wb" ? " active" : ""}`}
          disabled={off}
          onClick={toggleWbTool}
          title={`スポイト: 中立（灰色）にしたい点をプレビューでクリック（${comboLabel("w")}）`}
          aria-pressed={tool === "wb"}
        >
          スポイト
        </button>
      </div>
      {source && !isRaw && (
        <div className="dp-note">
          RAW 以外の写真では、ホワイトバランスの設定は使われません（元の画像の色から現像します）。
        </div>
      )}
      {isRaw && mode === "as_shot" && !eff.known && (
        <div className="dp-note">
          撮影時の色温度が分からないため、仮の値（{formatKelvin(eff.temperature_k)}）を出しています。
        </div>
      )}
      <Slider
        label="色温度"
        value={eff.temperature_k}
        min={RANGES.temperatureK.min}
        max={RANGES.temperatureK.max}
        step={1}
        format={formatKelvin}
        parse={parseNumber}
        toPosition={temperatureToPosition}
        fromPosition={positionToTemperature}
        disabled={off}
        onChange={(v, dragging) =>
          update((s) => {
            const cur = effectiveWb(s.white_balance, asShot);
            // 表示が変わらない値（入力欄を編集せずに離れたときなど）では、撮影時の設定・プリセットのままにする。
            if (sameShown(formatKelvin, v, cur.temperature_k)) return null;
            return { ...s, white_balance: customWb(Math.round(v), cur.tint) };
          }, dragging)
        }
        onCommit={endDrag}
        onReset={resetWb}
        modified={mode !== "as_shot"}
        title="色温度（K）。動かすとカスタムになります。ダブルクリックで撮影時の設定に戻します"
      />
      <Slider
        label="色かぶり補正"
        value={eff.tint}
        min={RANGES.tint.min}
        max={RANGES.tint.max}
        step={1}
        format={signed0}
        parse={parseNumber}
        disabled={off}
        onChange={(v, dragging) =>
          update((s) => {
            const cur = effectiveWb(s.white_balance, asShot);
            if (sameShown(signed0, v, cur.tint)) return null;
            return { ...s, white_balance: customWb(cur.temperature_k, v) };
          }, dragging)
        }
        onCommit={endDrag}
        onReset={resetWb}
        modified={mode !== "as_shot"}
        title="正で緑を弱め（マゼンタ寄り）、負で緑を強めます。動かすとカスタムになります。ダブルクリックで撮影時の設定に戻します"
      />
    </>
  );
}

/** 処理（カラー / 白黒。DEV-25。V キー）の切り替え。 */
function TreatmentRow(props: { settings: DevelopSettings; disabled: boolean }) {
  const c = colorControls(props.settings, props.disabled);
  const choices = [
    { bw: false, label: "カラー" },
    { bw: true, label: "白黒" },
  ];
  return (
    <div className="dp-row">
      <span className="dp-label">処理</span>
      <div className="dp-tabs grow" role="radiogroup" aria-label="処理（カラー / 白黒）" style={{ margin: 0 }}>
        {choices.map((ch) => (
          <button
            key={ch.label}
            role="radio"
            aria-checked={c.blackWhite === ch.bw}
            className={c.blackWhite === ch.bw ? "on" : ""}
            disabled={!c.treatment}
            onClick={() => update((s) => (colorControls(s, false).blackWhite === ch.bw ? null : withBlackWhite(s, ch.bw)), false)}
            title={
              c.needsUpgrade
                ? "処理バージョンを更新すると白黒にできます"
                : `${ch.bw ? "白黒にする" : "カラーに戻す"}（${comboLabel("v")} で切り替え）`
            }
          >
            {ch.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function BasicSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const c = colorControls(settings, disabled);
  return (
    <Section id="develop.basic" title="基本補正">
      <TreatmentRow settings={settings} disabled={disabled} />
      <WhiteBalanceRows settings={settings} disabled={disabled} />
      <div className="dp-sub">トーン</div>
      <Slider
        label="露光量"
        value={settings.exposure_ev}
        min={RANGES.exposureEv.min}
        max={RANGES.exposureEv.max}
        step={0.01}
        defaultValue={0}
        format={formatEv}
        parse={parseNumber}
        disabled={disabled}
        onChange={(v, dragging) =>
          update(
            (s) => (sameShown(formatEv, v, s.exposure_ev) ? null : { ...s, exposure_ev: Math.round(v * 100) / 100 }),
            dragging,
          )
        }
        onCommit={endDrag}
      />
      <SliderRows defs={TONE_SLIDERS} settings={settings} disabled={disabled} />
      <HighlightRecoveryRows settings={settings} disabled={disabled} />
      <PresenceRows settings={settings} disabled={disabled} />
      <div className="dp-sub">色</div>
      <SliderRows defs={COLOR_SLIDERS} settings={settings} disabled={!c.saturation} />
      {c.blackWhite && !c.needsUpgrade && !disabled && (
        <div className="dp-note">白黒のときは、彩度・自然な彩度・カラーミキサーは使われません（値は保ちます）。</div>
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// カラーミキサー・白黒ミキサー（DEV-09・DEV-25）・カラーグレーディング（DEV-10）。処理バージョン 2 から。
// ---------------------------------------------------------------------------

const signedBand = (v: number) => formatSigned(v, 0);

function MixerSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const [aspect, setAspect] = useState<MixerAspect>("hue");
  const c = colorControls(settings, disabled);
  const color = fullColor(settings.color);
  const bw = c.blackWhite && !c.needsUpgrade;
  const changed = bw ? !bwMixIsNeutral(color.black_white.mix) : !mixerIsNeutral(color.mixer);
  const aspectChanged = (a: MixerAspect) => HUE_BANDS.some((b) => color.mixer[b][a] !== 0);
  return (
    <Section id="develop.mixer" title={bw ? "白黒ミキサー" : "カラーミキサー"} defaultOpen={false}>
      {c.needsUpgrade && !disabled && (
        <UpgradeNote what="カラーミキサー・白黒" processVersion={settings.process_version} />
      )}
      {bw ? (
        <>
          <div className="dp-note">色ごとの明るさ（白黒にしたときの灰色の明るさ）を調整します。</div>
          {HUE_BANDS.map((band) => (
            <Slider
              key={band}
              label={HUE_BAND_INFO[band].label}
              swatch={hsvCss(HUE_BAND_INFO[band].hue)}
              value={color.black_white.mix[band]}
              min={RANGES.slider.min}
              max={RANGES.slider.max}
              step={1}
              defaultValue={0}
              format={signedBand}
              parse={parseNumber}
              disabled={!c.bwMix}
              onChange={(v, dragging) =>
                update(
                  (s) =>
                    sameShown(signedBand, v, fullColor(s.color).black_white.mix[band])
                      ? null
                      : withBwMix(s, band, Math.round(v)),
                  dragging,
                )
              }
              onCommit={endDrag}
              title={`${HUE_BAND_INFO[band].label}の部分を明るく（正）・暗く（負）します`}
            />
          ))}
        </>
      ) : (
        <>
          <div className="dp-tabs" role="tablist" aria-label="カラーミキサーの調整">
            {MIXER_ASPECTS.map((a) => (
              <button
                key={a.key}
                role="tab"
                aria-selected={aspect === a.key}
                className={aspect === a.key ? "on" : ""}
                onClick={() => setAspect(a.key)}
                title={a.title}
              >
                {a.label}
                {aspectChanged(a.key) ? " •" : ""}
              </button>
            ))}
          </div>
          {HUE_BANDS.map((band) => (
            <Slider
              key={`${aspect}.${band}`}
              label={HUE_BAND_INFO[band].label}
              swatch={hsvCss(HUE_BAND_INFO[band].hue)}
              value={color.mixer[band][aspect]}
              min={RANGES.slider.min}
              max={RANGES.slider.max}
              step={1}
              defaultValue={0}
              format={signedBand}
              parse={parseNumber}
              disabled={!c.mixer}
              onChange={(v, dragging) =>
                update(
                  (s) =>
                    sameShown(signedBand, v, fullColor(s.color).mixer[band][aspect])
                      ? null
                      : withMixer(s, band, aspect, Math.round(v)),
                  dragging,
                )
              }
              onCommit={endDrag}
              title={`${HUE_BAND_INFO[band].label}の${MIXER_ASPECTS.find((a) => a.key === aspect)?.title ?? ""}`}
            />
          ))}
        </>
      )}
      <div className="dp-row">
        <span className="grow" />
        <button
          className="btn small"
          disabled={disabled || c.needsUpgrade || !changed}
          onClick={() =>
            update((s) => {
              const cur = fullColor(s.color);
              return bw
                ? { ...s, color: { ...cur, black_white: { ...cur.black_white, mix: defaultBlackWhite().mix } } }
                : { ...s, color: { ...cur, mixer: defaultMixer() } };
            }, false)
          }
          title={bw ? "白黒ミキサーを元に戻す" : "カラーミキサーを元に戻す（色相・彩度・輝度のすべて）"}
        >
          リセット
        </button>
      </div>
    </Section>
  );
}

function GradingSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const [range, setRange] = useState<GradingRange>("shadows");
  const c = colorControls(settings, disabled);
  const g = fullColor(settings.color).grading;
  const w = g[range];
  const off = !c.grading;
  const pct = (v: number) => String(Math.round(v));
  const deg = (v: number) => `${Math.round(v)}°`;
  const rangeLabel = GRADING_RANGES.find((r) => r.key === range)?.label ?? "";
  return (
    <Section id="develop.grading" title="カラーグレーディング" defaultOpen={false}>
      {c.needsUpgrade && !disabled && (
        <UpgradeNote what="カラーグレーディング" processVersion={settings.process_version} />
      )}
      <div className="dp-tabs" role="tablist" aria-label="カラーグレーディングの範囲">
        {GRADING_RANGES.map((r) => (
          <button
            key={r.key}
            role="tab"
            aria-selected={range === r.key}
            className={range === r.key ? "on" : ""}
            onClick={() => setRange(r.key)}
            title={r.title}
          >
            {r.label}
            {!wheelIsNeutral(g[r.key]) && (
              <span
                className="dp-tab-swatch"
                style={{ background: hsvCss(g[r.key].hue, Math.max(0.25, g[r.key].saturation / 100)) }}
                aria-hidden="true"
              />
            )}
          </button>
        ))}
      </div>
      <div className="dp-wheel-row">
        <ColorWheel
          hue={w.hue}
          saturation={w.saturation}
          size={150}
          disabled={off}
          label={rangeLabel}
          onChange={(v, dragging) =>
            update((s) => {
              const cur = fullColor(s.color).grading[range];
              if (cur.hue === v.hue && cur.saturation === v.saturation) return null;
              return withWheel(s, range, v);
            }, dragging)
          }
          onCommit={endDrag}
        />
      </div>
      <Slider
        label="色相"
        value={w.hue}
        min={RANGES.hueDeg.min}
        max={RANGES.hueDeg.max}
        step={1}
        defaultValue={0}
        format={deg}
        parse={parseNumber}
        disabled={off}
        onChange={(v, dragging) =>
          update(
            (s) =>
              sameShown(deg, v, fullColor(s.color).grading[range].hue)
                ? null
                : withWheel(s, range, { hue: Math.round(v) }),
            dragging,
          )
        }
        onCommit={endDrag}
        title="色付けの色相（0 がレッド・120 がグリーン・240 がブルー）"
      />
      <Slider
        label="彩度"
        value={w.saturation}
        min={RANGES.amount.min}
        max={RANGES.amount.max}
        step={1}
        defaultValue={0}
        format={pct}
        parse={parseNumber}
        disabled={off}
        onChange={(v, dragging) =>
          update(
            (s) =>
              sameShown(pct, v, fullColor(s.color).grading[range].saturation)
                ? null
                : withWheel(s, range, { saturation: Math.round(v) }),
            dragging,
          )
        }
        onCommit={endDrag}
        title="色付けの強さ（0 なら色相は効きません）"
      />
      <Slider
        label="輝度"
        value={w.luminance}
        min={RANGES.slider.min}
        max={RANGES.slider.max}
        step={1}
        defaultValue={0}
        format={signed0}
        parse={parseNumber}
        disabled={off}
        onChange={(v, dragging) =>
          update(
            (s) =>
              sameShown(signed0, v, fullColor(s.color).grading[range].luminance)
                ? null
                : withWheel(s, range, { luminance: Math.round(v) }),
            dragging,
          )
        }
        onCommit={endDrag}
        title={`${rangeLabel}の明るさ（正で明るく）`}
      />
      <div className="dp-sub">範囲</div>
      <Slider
        label="ブレンド"
        value={g.blending}
        min={RANGES.amount.min}
        max={RANGES.amount.max}
        step={1}
        defaultValue={50}
        format={pct}
        parse={parseNumber}
        disabled={off}
        onChange={(v, dragging) =>
          update(
            (s) =>
              sameShown(pct, v, fullColor(s.color).grading.blending)
                ? null
                : withGrading(s, { blending: Math.round(v) }),
            dragging,
          )
        }
        onCommit={endDrag}
        title="シャドウ・中間調・ハイライトの範囲の重なり（大きいほど広く重なり、なめらかに移ります）"
      />
      <Slider
        label="バランス"
        value={g.balance}
        min={RANGES.slider.min}
        max={RANGES.slider.max}
        step={1}
        defaultValue={0}
        format={signed0}
        parse={parseNumber}
        disabled={off}
        onChange={(v, dragging) =>
          update(
            (s) =>
              sameShown(signed0, v, fullColor(s.color).grading.balance)
                ? null
                : withGrading(s, { balance: Math.round(v) }),
            dragging,
          )
        }
        onCommit={endDrag}
        title="正でハイライトの範囲を広げ（境目を暗い側へ）、負でシャドウの範囲を広げます"
      />
      <div className="dp-row">
        <span className="grow" />
        <button
          className="btn small"
          disabled={disabled || c.needsUpgrade || gradingIsDefault(g)}
          onClick={() => update((s) => ({ ...s, color: { ...fullColor(s.color), grading: defaultGrading() } }), false)}
          title="カラーグレーディングを元に戻す（すべての範囲・ブレンド・バランス）"
        >
          リセット
        </button>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// ハイライト復元（DEV-06）・ノイズ軽減（DEV-13）。処理バージョン 2 から。RAW だけ。
// ---------------------------------------------------------------------------

const HIGHLIGHT_METHODS: { value: HighlightMethod; label: string; title: string }[] = [
  { value: "clip", label: "白で切りそろえる", title: "白飛びしたところを白にそろえます（復元しない。これまでと同じ）" },
  {
    value: "neutral",
    label: "明るさだけ復元",
    title: "白飛びしたチャンネルの明るさを、ほかのチャンネルから復元します（色は付けない）",
  },
  {
    value: "reconstruct",
    label: "色を保って復元",
    title: "白飛びしたチャンネルを、近くの白飛びしていない部分の色の比から復元します",
  },
];

/** 処理バージョンが古いときの知らせと、更新のボタン（ハイライト復元・ディテールのパネル）。 */
function UpgradeNote(props: { what: string; processVersion: number }) {
  const latest = useDevelop((s) => s.state?.latest_process_version);
  return (
    <div className="dp-note">
      {`この写真の処理バージョン（${props.processVersion}）では、${props.what}は使われません。`}
      {"処理バージョンを更新すると使えます（使わなければ見え方は変わりません。履歴から取り消せます）。"}
      {canUpgradeProcess(props.processVersion, latest) && (
        <div className="dp-row">
          <button className="btn small" onClick={() => void upgradeProcessVersion()}>
            処理バージョンを {latest} に更新
          </button>
        </div>
      )}
    </div>
  );
}

function HighlightRecoveryRows(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const source = useDevelop((s) => s.state?.source ?? null);
  const c = sensorControls(settings, source, disabled);
  const hr = fullHighlightRecovery(settings.highlight_recovery);
  const pct = (v: number) => `${Math.round(v)}`;
  return (
    <>
      <div className="dp-sub">ハイライト復元</div>
      {c.needsUpgrade && !disabled && (
        <UpgradeNote what="ハイライト復元" processVersion={settings.process_version} />
      )}
      <div className="dp-row">
        <select
          value={hr.method}
          disabled={!c.highlight}
          aria-label="ハイライト復元の方式"
          title={HIGHLIGHT_METHODS.find((m) => m.value === hr.method)?.title}
          onChange={(e) =>
            update(
              (s) => ({
                ...s,
                highlight_recovery: {
                  ...fullHighlightRecovery(s.highlight_recovery),
                  method: e.target.value as HighlightMethod,
                },
              }),
              false,
            )
          }
        >
          {HIGHLIGHT_METHODS.map((m) => (
            <option key={m.value} value={m.value} title={m.title}>
              {m.label}
            </option>
          ))}
        </select>
      </div>
      <Slider
        label="量"
        value={hr.amount}
        min={RANGES.amount.min}
        max={RANGES.amount.max}
        step={1}
        defaultValue={100}
        format={pct}
        parse={parseNumber}
        disabled={!c.highlightAmount}
        onChange={(v, dragging) =>
          update((s) => {
            const cur = fullHighlightRecovery(s.highlight_recovery);
            if (sameShown(pct, v, cur.amount)) return null;
            return { ...s, highlight_recovery: { ...cur, amount: Math.round(v) } };
          }, dragging)
        }
        onCommit={endDrag}
        title="復元の強さ（0 で白で切りそろえたのと同じ、100 で方式どおりに復元）"
      />
      {c.notRaw && !c.needsUpgrade && (
        <div className="dp-note">RAW 以外の写真では、ハイライト復元は使われません。</div>
      )}
    </>
  );
}

interface NoiseSliderDef {
  key: keyof NoiseReduction;
  label: string;
  defaultValue: number;
  title: string;
  /** 動かせるか（量が 0 でないときだけ動かせる項目がある）。 */
  enabled: (c: ReturnType<typeof sensorControls>) => boolean;
}

const LUMINANCE_SLIDERS: NoiseSliderDef[] = [
  {
    key: "luminance",
    label: "輝度",
    defaultValue: 0,
    title: "明るさのざらつき（輝度ノイズ）を減らします",
    enabled: (c) => c.noise,
  },
  {
    key: "luminance_detail",
    label: "ディテール",
    defaultValue: 50,
    title: "大きいほど細かい模様を残します（細かい段の軽減を弱めます）",
    enabled: (c) => c.luminanceOptions,
  },
  {
    key: "luminance_contrast",
    label: "コントラスト",
    defaultValue: 0,
    title: "大きいほど大きな模様の明暗を残します（粗い段の軽減を弱めます）",
    enabled: (c) => c.luminanceOptions,
  },
];

const COLOR_NOISE_SLIDERS: NoiseSliderDef[] = [
  {
    key: "color",
    label: "カラー",
    defaultValue: 0,
    title: "色のまだら（カラーノイズ）を減らします",
    enabled: (c) => c.noise,
  },
  {
    key: "color_detail",
    label: "ディテール",
    defaultValue: 50,
    title: "大きいほど細かい色の境界を残します（細かい段の軽減を弱めます）",
    enabled: (c) => c.colorOptions,
  },
];

function NoiseSliders(props: {
  defs: NoiseSliderDef[];
  nr: NoiseReduction;
  controls: ReturnType<typeof sensorControls>;
}) {
  const fmt = (v: number) => String(Math.round(v));
  return (
    <>
      {props.defs.map((d) => (
        <Slider
          key={d.key}
          label={d.label}
          value={props.nr[d.key]}
          min={RANGES.amount.min}
          max={RANGES.amount.max}
          step={1}
          defaultValue={d.defaultValue}
          format={fmt}
          parse={parseNumber}
          disabled={!d.enabled(props.controls)}
          onChange={(v, dragging) =>
            update((s) => {
              const nr = fullNoiseReduction(s.detail?.noise_reduction);
              if (sameShown(fmt, v, nr[d.key])) return null;
              return { ...s, detail: { ...s.detail, noise_reduction: { ...nr, [d.key]: Math.round(v) } } };
            }, dragging)
          }
          onCommit={endDrag}
          title={d.title}
        />
      ))}
    </>
  );
}

function DetailSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const source = useDevelop((s) => s.state?.source ?? null);
  const c = sensorControls(settings, source, disabled);
  const nr = fullNoiseReduction(settings.detail?.noise_reduction);
  const changed = JSON.stringify(nr) !== JSON.stringify(defaultNoiseReduction());
  return (
    <Section id="develop.detail" title="ディテール" defaultOpen={false}>
      {c.needsUpgrade && !disabled && (
        <UpgradeNote what="シャープ・ノイズ軽減" processVersion={settings.process_version} />
      )}
      <SharpenRows settings={settings} disabled={disabled} />
      <div className="dp-sub">ノイズ軽減</div>
      <NoiseSliders defs={LUMINANCE_SLIDERS} nr={nr} controls={c} />
      <NoiseSliders defs={COLOR_NOISE_SLIDERS} nr={nr} controls={c} />
      {c.notRaw && !c.needsUpgrade && <div className="dp-note">RAW 以外の写真では、ノイズ軽減は使われません。</div>}
      <div className="dp-row">
        <span className="grow" />
        <button
          className="btn small"
          disabled={disabled || c.needsUpgrade || !changed}
          onClick={() =>
            update((s) => ({ ...s, detail: { ...s.detail, noise_reduction: defaultNoiseReduction() } }), false)
          }
          title="ノイズ軽減を元に戻す（軽減しない）"
        >
          リセット
        </button>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 外観（テクスチャ・明瞭度・かすみの除去。DEV-11）・シャープ（DEV-12）・効果（DEV-24）。処理バージョン 2 から。
// ---------------------------------------------------------------------------

/** 0〜100 などの整数の書式。 */
const int0 = (v: number) => String(Math.round(v));

/** 1 つのスライダーの定義（`T` は設定の一部。値を読んで、変えた設定を作る）。 */
interface PartSliderDef<T> {
  key: keyof T & string;
  label: string;
  min: number;
  max: number;
  step: number;
  defaultValue: number;
  format: (v: number) => string;
  title: string;
  /** 動かせるか。 */
  enabled: (c: DetailControls) => boolean;
}

/** 設定の一部（`read` で読む）のスライダーの列。変えたら `write` で設定を作って送る。 */
function PartSliders<T>(props: {
  defs: PartSliderDef<T>[];
  value: T;
  controls: DetailControls;
  read: (s: DevelopSettings) => T;
  write: (s: DevelopSettings, patch: Partial<T>) => DevelopSettings;
}) {
  return (
    <>
      {props.defs.map((d) => (
        <Slider
          key={d.key}
          label={d.label}
          value={props.value[d.key] as unknown as number}
          min={d.min}
          max={d.max}
          step={d.step}
          defaultValue={d.defaultValue}
          format={d.format}
          parse={parseNumber}
          disabled={!d.enabled(props.controls)}
          onChange={(v, dragging) =>
            update((s) => {
              const cur = props.read(s)[d.key] as unknown as number;
              if (sameShown(d.format, v, cur)) return null;
              const rounded = Math.round(v / d.step) * d.step;
              return props.write(s, { [d.key]: Number(rounded.toFixed(2)) } as Partial<T>);
            }, dragging)
          }
          onCommit={endDrag}
          title={d.title}
        />
      ))}
    </>
  );
}

const PRESENCE_SLIDERS: PartSliderDef<PresenceParams>[] = [
  {
    key: "texture",
    label: "テクスチャ",
    min: RANGES.slider.min,
    max: RANGES.slider.max,
    step: 1,
    defaultValue: 0,
    format: signed0,
    title: "中くらいの細部（肌・葉・布の質感）を強め（正）・なめらかにします（負）",
    enabled: (c) => c.presence,
  },
  {
    key: "clarity",
    label: "明瞭度",
    min: RANGES.slider.min,
    max: RANGES.slider.max,
    step: 1,
    defaultValue: 0,
    format: signed0,
    title: "大きめの範囲の明暗の差（中間調の局所のコントラスト）を強め（正）・弱めます（負）",
    enabled: (c) => c.presence,
  },
  {
    key: "dehaze",
    label: "かすみの除去",
    min: RANGES.slider.min,
    max: RANGES.slider.max,
    step: 1,
    defaultValue: 0,
    format: signed0,
    title: "かすみ（白い膜）を取り除き（正）・足します（負）",
    enabled: (c) => c.presence,
  },
];

function PresenceRows(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const c = detailControls(settings, disabled);
  return (
    <>
      <div className="dp-sub">外観</div>
      {c.needsUpgrade && !disabled && (
        <UpgradeNote what="テクスチャ・明瞭度・かすみの除去" processVersion={settings.process_version} />
      )}
      <PartSliders
        defs={PRESENCE_SLIDERS}
        value={fullPresence(settings.presence)}
        controls={c}
        read={(s) => fullPresence(s.presence)}
        write={withPresence}
      />
    </>
  );
}

const SHARPEN_SLIDERS: PartSliderDef<Sharpening>[] = [
  {
    key: "amount",
    label: "適用量",
    min: RANGES.sharpenAmount.min,
    max: RANGES.sharpenAmount.max,
    step: 1,
    defaultValue: 0,
    format: int0,
    title: "輪郭をくっきりさせる強さ（0 でかけない）",
    enabled: (c) => c.sharpen,
  },
  {
    key: "radius",
    label: "半径",
    min: RANGES.sharpenRadius.min,
    max: RANGES.sharpenRadius.max,
    step: 0.1,
    defaultValue: 1,
    format: (v) => v.toFixed(1),
    title: "強める細部の大きさ（写真の画素の単位）",
    enabled: (c) => c.sharpenOptions,
  },
  {
    key: "detail",
    label: "ディテール",
    min: RANGES.amount.min,
    max: RANGES.amount.max,
    step: 1,
    defaultValue: 25,
    format: int0,
    title: "大きいほど細かい模様も強め、小さいほど輪郭だけを強めて、にじみ（ハロー）を抑えます",
    enabled: (c) => c.sharpenOptions,
  },
  {
    key: "masking",
    label: "マスク",
    min: RANGES.amount.min,
    max: RANGES.amount.max,
    step: 1,
    defaultValue: 0,
    format: int0,
    title: "大きいほど、輪郭のない平らな部分（空・肌）にはかけません",
    enabled: (c) => c.sharpenOptions,
  },
];

function SharpenRows(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const c = detailControls(settings, disabled);
  return (
    <>
      <div className="dp-sub">シャープ</div>
      <PartSliders
        defs={SHARPEN_SLIDERS}
        value={fullSharpening(settings.detail?.sharpening)}
        controls={c}
        read={(s) => fullSharpening(s.detail?.sharpening)}
        write={withSharpening}
      />
      {!c.needsUpgrade && !disabled && (
        <div className="dp-note">シャープの効き方は等倍（100%）の表示で確かめてください（縮小した表示では小さく見えます）。</div>
      )}
      <div className="dp-row">
        <span className="grow" />
        <button
          className="btn small"
          disabled={disabled || c.needsUpgrade || !c.sharpenChanged}
          onClick={() => update((s) => withSharpening(s, defaultSharpening()), false)}
          title="シャープを元に戻す（かけない）"
        >
          リセット
        </button>
      </div>
    </>
  );
}

const VIGNETTE_SLIDERS: PartSliderDef<PostCropVignette>[] = [
  {
    key: "amount",
    label: "量",
    min: RANGES.slider.min,
    max: RANGES.slider.max,
    step: 1,
    defaultValue: 0,
    format: signed0,
    title: "切り抜いた後の四隅を暗く（負）・明るく（正）します",
    enabled: (c) => c.vignette,
  },
  {
    key: "midpoint",
    label: "中点",
    min: RANGES.amount.min,
    max: RANGES.amount.max,
    step: 1,
    defaultValue: 50,
    format: int0,
    title: "小さいほど中心の近くから効きます",
    enabled: (c) => c.vignetteOptions,
  },
  {
    key: "roundness",
    label: "丸み",
    min: RANGES.slider.min,
    max: RANGES.slider.max,
    step: 1,
    defaultValue: 0,
    format: signed0,
    title: "0 で画像の縦横比に合わせた楕円、正で円、負で角の丸い四角に近づきます",
    enabled: (c) => c.vignetteOptions,
  },
  {
    key: "feather",
    label: "ぼかし",
    min: RANGES.amount.min,
    max: RANGES.amount.max,
    step: 1,
    defaultValue: 50,
    format: int0,
    title: "境目の幅（0 でくっきり）",
    enabled: (c) => c.vignetteOptions,
  },
  {
    key: "highlights",
    label: "ハイライト",
    min: RANGES.amount.min,
    max: RANGES.amount.max,
    step: 1,
    defaultValue: 0,
    format: int0,
    title: "暗くするとき、明るい部分（光源など）を暗くしにくくします",
    enabled: (c) => c.vignetteHighlights,
  },
];

const GRAIN_SLIDERS: PartSliderDef<Grain>[] = [
  {
    key: "amount",
    label: "量",
    min: RANGES.amount.min,
    max: RANGES.amount.max,
    step: 1,
    defaultValue: 0,
    format: int0,
    title: "フィルムのような粒子を足します（0 で足さない）",
    enabled: (c) => c.grain,
  },
  {
    key: "size",
    label: "大きさ",
    min: RANGES.amount.min,
    max: RANGES.amount.max,
    step: 1,
    defaultValue: 25,
    format: int0,
    title: "粒の大きさ",
    enabled: (c) => c.grainOptions,
  },
  {
    key: "roughness",
    label: "粗さ",
    min: RANGES.amount.min,
    max: RANGES.amount.max,
    step: 1,
    defaultValue: 50,
    format: int0,
    title: "大きいほど、粒の大きさが不ぞろいになります",
    enabled: (c) => c.grainOptions,
  },
];

/** 周辺光量補正の中心の軸（手動の中心の横・縦。表示は切り抜く前の画像の幅・高さに対する %）。 */
const VIGNETTE_CENTER_AXES = [
  { key: "x", label: "中心（横）", title: "中心の横の位置（切り抜く前の画像の左端 0%〜右端 100%）" },
  { key: "y", label: "中心（縦）", title: "中心の縦の位置（切り抜く前の画像の上端 0%〜下端 100%）" },
] as const;

/**
 * 周辺光量補正の中心。既定は切り抜きの中心（切り抜きと一緒に動く）。「中心を手動で設定」で、切り抜きによらない
 * 位置にする（切り替えたときは今の切り抜きの中心から始めるので、見え方は変わらない）。
 */
function VignetteCenterControls(props: { settings: DevelopSettings; controls: DetailControls }) {
  const { settings, controls: c } = props;
  // 効果の区切りを開いている（この部品を描いている）間は、プレビューに中心の印を出す（DevelopView。ドラッグで動かす）。
  useEffect(() => {
    viewStore.set({ vignetteHandle: true });
    return () => viewStore.set({ vignetteHandle: false });
  }, []);
  const info = useGeometryInfo();
  const manual = fullEffects(settings.effects).vignette.center != null;
  const center = vignetteCenterOf(settings, info);
  // 切り抜きの中心（描くときの切り抜きの中心。手動の値を消した位置）。
  const cropCenter = vignetteCenterOf(withVignette(settings, { center: null }), info);
  return (
    <>
      <label
        className={`dp-check${!c.vignetteOptions ? " disabled" : ""}`}
        title="切り抜きの中心の代わりに、決めた位置を中心にします（切り抜きを変えても動きません）"
      >
        <input
          type="checkbox"
          checked={manual}
          disabled={!c.vignetteOptions}
          onChange={(e) => {
            const on = e.target.checked;
            update((s) => withVignette(s, { center: on ? vignetteCenterOf(s, info) : null }), false);
          }}
        />
        中心を手動で設定
      </label>
      {c.vignetteOptions && <div className="dp-note">プレビューの中心の印をドラッグしても動かせます（手動になります）。</div>}
      {VIGNETTE_CENTER_AXES.map((a) => (
        <Slider
          key={a.key}
          label={a.label}
          value={center[a.key] * 100}
          min={0}
          max={100}
          step={0.5}
          defaultValue={cropCenter[a.key] * 100}
          format={(v) => `${v.toFixed(1)}%`}
          parse={parseNumber}
          disabled={!c.vignetteCenter}
          onChange={(v, dragging) =>
            update((s) => {
              const cur = vignetteCenterOf(s, info);
              const next = Number((Math.min(100, Math.max(0, Math.round(v * 2) / 2)) / 100).toFixed(4));
              if (cur[a.key] === next) return null;
              return withVignette(s, { center: { ...cur, [a.key]: next } });
            }, dragging)
          }
          onCommit={endDrag}
          title={a.title}
        />
      ))}
    </>
  );
}

function EffectsSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const c = detailControls(settings, disabled);
  const fx = fullEffects(settings.effects);
  return (
    <Section id="develop.effects" title="効果" defaultOpen={false}>
      {c.needsUpgrade && !disabled && <UpgradeNote what="効果" processVersion={settings.process_version} />}
      <div className="dp-sub">切り抜き後の周辺光量補正</div>
      <PartSliders
        defs={VIGNETTE_SLIDERS}
        value={fx.vignette}
        controls={c}
        read={(s) => fullEffects(s.effects).vignette}
        write={withVignette}
      />
      <VignetteCenterControls settings={settings} controls={c} />
      <div className="dp-sub">粒子</div>
      <PartSliders
        defs={GRAIN_SLIDERS}
        value={fx.grain}
        controls={c}
        read={(s) => fullEffects(s.effects).grain}
        write={withGrain}
      />
      <div className="dp-row">
        <span className="grow" />
        <button
          className="btn small"
          disabled={disabled || c.needsUpgrade || !c.effectsChanged}
          onClick={() => update((s) => ({ ...s, effects: defaultEffects() }), false)}
          title="効果を元に戻す（周辺光量補正・粒子なし）"
        >
          リセット
        </button>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// カメラプロファイル・ルック（DEV-17）
// ---------------------------------------------------------------------------

/** 選択肢の値（null は空文字列）。 */
const choiceValue = (id: string | null) => id ?? "";

function ProfileSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const source = useDevelop((s) => s.state?.source ?? null);
  const variantId = useDevelop((s) => s.variantId);
  const entries = useProfiles((s) => s.entries);
  const busy = useProfiles((s) => s.busy);
  const camera = source?.camera ?? null;
  const p = fullProfile(settings.profile);
  const c = profileControls(settings, source, disabled);
  const list = entries ?? [];
  const [showAll, setShowAll] = useState(false);
  // 写真（機種）が変わったら一覧を読み直す（その機種用かが変わる）。
  useEffect(() => {
    void refreshProfiles(camera);
  }, [variantId, camera]);
  const otherSelected = selectsOtherCamera(list, p.camera);
  const cams = cameraChoices(list, p.camera, showAll || otherSelected);
  const looks = lookChoices(list, p.look);
  const pct = (v: number) => `${Math.round(v)}`;
  const hasOthers = list.some((e) => e.kind === "camera" && e.matches_camera !== true);
  return (
    <Section id="develop.profile" title="プロファイル" defaultOpen={false}>
      {c.needsUpgrade && !disabled && (
        <UpgradeNote what="カメラプロファイル・ルック" processVersion={settings.process_version} />
      )}
      <div className="dp-sub">カメラプロファイル</div>
      <div className="dp-row">
        <select
          value={choiceValue(p.camera)}
          disabled={!c.camera}
          aria-label="カメラプロファイル"
          title="RAW の色の解釈（カメラの色 → 作業色空間）。取り込んだ DCP から選びます"
          onChange={(e) => {
            const id = e.target.value === "" ? null : e.target.value;
            update((s) => (fullProfile(s.profile).camera === id ? null : withProfile(s, { camera: id })), false);
          }}
        >
          {cams.map((o) => (
            <option key={choiceValue(o.id)} value={choiceValue(o.id)} disabled={o.disabled}>
              {o.label}
            </option>
          ))}
        </select>
      </div>
      {hasOthers && (
        <label className={`dp-check${!c.camera ? " disabled" : ""}`} title="色が合わないことがあります">
          <input
            type="checkbox"
            checked={showAll || otherSelected}
            disabled={!c.camera || otherSelected}
            onChange={(e) => setShowAll(e.target.checked)}
          />
          ほかの機種用も表示
        </label>
      )}
      {c.notRaw && !c.needsUpgrade && (
        <div className="dp-note">RAW 以外の写真では、カメラプロファイルは使われません（ルックは使えます）。</div>
      )}
      {!c.notRaw && source && camera === null && (
        <div className="dp-note">この写真の機種が分からないため、「ほかの機種用も表示」で選んでください。</div>
      )}
      <div className="dp-sub">ルック</div>
      <div className="dp-row">
        <select
          value={choiceValue(p.look)}
          disabled={!c.look}
          aria-label="ルック"
          title="仕上げの色（3D LUT）。トーン・色の調整の後に当てます"
          onChange={(e) => {
            const id = e.target.value === "" ? null : e.target.value;
            update((s) => (fullProfile(s.profile).look === id ? null : withProfile(s, { look: id })), false);
          }}
        >
          {looks.map((o) => (
            <option key={choiceValue(o.id)} value={choiceValue(o.id)} disabled={o.disabled}>
              {o.label}
            </option>
          ))}
        </select>
      </div>
      <Slider
        label="適用量"
        value={p.look_amount}
        min={RANGES.amount.min}
        max={RANGES.amount.max}
        step={1}
        defaultValue={100}
        format={pct}
        parse={parseNumber}
        disabled={!c.lookAmount}
        onChange={(v, dragging) =>
          update((s) => {
            const cur = fullProfile(s.profile);
            if (sameShown(pct, v, cur.look_amount)) return null;
            return withProfile(s, { look_amount: Math.round(v) });
          }, dragging)
        }
        onCommit={endDrag}
        title="ルックの強さ（0 でルックなしと同じ、100 で LUT どおり）"
      />
      <div className="dp-row">
        <button
          className="btn small"
          disabled={busy}
          onClick={() => void importProfileFiles(false)}
          title="カメラプロファイル（.dcp）・ルック（.cube）のファイルを取り込む（元のファイルは変えません）"
        >
          取り込む…
        </button>
        <button
          className="btn small"
          onClick={() => actions.openDialog({ kind: "profiles" })}
          title="取り込んだものの一覧・詳細・除く"
        >
          管理…
        </button>
        <span className="grow" />
        <button
          className="btn small"
          disabled={disabled || c.needsUpgrade || profileIsDefault(settings.profile)}
          onClick={() => update((s) => ({ ...s, profile: defaultProfile() }), false)}
          title="標準の色・ルックなしに戻す"
        >
          リセット
        </button>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// トーンカーブ
// ---------------------------------------------------------------------------

function CurveSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const [channel, setChannel] = useState<CurveChannel>("luma");
  const histogram = useDevelop((s) => s.frame?.header.histogram ?? null);
  const bins = histogram ? histogram[channel] : null;
  const points = settings.tone_curve[channel];
  const setPoints = (pts: CurvePoint[], dragging: boolean) =>
    update((s) => ({ ...s, tone_curve: withChannel(s.tone_curve, channel, pts) }), dragging);
  return (
    <Section id="develop.curve" title="トーンカーブ">
      <SliderRows defs={PARAMETRIC_SLIDERS} settings={settings} disabled={disabled} />
      <div className="dp-tabs" role="tablist" aria-label="ポイントカーブのチャンネル">
        {CURVE_CHANNELS.map((c) => (
          <button
            key={c.key}
            role="tab"
            aria-selected={channel === c.key}
            className={channel === c.key ? "on" : ""}
            onClick={() => setChannel(c.key)}
          >
            {c.label}
            {settings.tone_curve[c.key].length > 0 ? " •" : ""}
          </button>
        ))}
      </div>
      <ToneCurveEditor
        channel={channel}
        points={points}
        parametric={settings.tone_curve.parametric}
        histogram={bins}
        disabled={disabled}
        onChange={setPoints}
        onCommit={endDrag}
      />
      <div className="dp-row">
        <span className="dp-note grow">クリックで点を追加、ダブルクリック・外へのドラッグ・Delete で削除</span>
        <button className="btn small" disabled={disabled || points.length === 0} onClick={() => setPoints([], false)}>
          リセット
        </button>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 切り抜きと角度
// ---------------------------------------------------------------------------

function CropSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const draft = useDevelopView((s) => s.crop);
  const info = useGeometryInfo();
  const aspect = draft ? draft.aspect : (settings.geometry.aspect ?? null);
  const key = aspectKey(aspect);
  const known = ASPECT_CHOICES.some((c) => c.key === key);
  const g = settings.geometry;
  const cropped = g.crop.left !== 0 || g.crop.top !== 0 || g.crop.right !== 1 || g.crop.bottom !== 1;
  return (
    <Section id="develop.crop" title="切り抜きと角度">
      <div className="dp-row">
        <span className="dp-label">縦横比</span>
        <select
          value={key}
          disabled={disabled}
          aria-label="縦横比"
          onChange={(e) => {
            const choice = ASPECT_CHOICES.find((c) => c.key === e.target.value);
            if (choice) void chooseAspect(choice.aspect, info);
          }}
        >
          {ASPECT_CHOICES.map((c) => (
            <option key={c.key} value={c.key}>
              {c.label}
            </option>
          ))}
          {!known && <option value={key}>{key.replace(":", " : ")}</option>}
        </select>
      </div>
      <Slider
        label="角度"
        value={g.angle_deg}
        min={RANGES.angleDeg.min}
        max={RANGES.angleDeg.max}
        step={0.01}
        defaultValue={0}
        format={formatAngle}
        parse={parseNumber}
        disabled={disabled}
        onChange={(v, dragging) => changeAngle(Math.round(v * 100) / 100, dragging, info)}
        onCommit={commitAngle}
        title="正の値で反時計回りに回転します。縦横比を固定していれば、切り抜きの枠もその比に合わせ直します"
      />
      <div className="dp-row">
        {draft ? (
          <>
            <button className="btn small primary" onClick={() => void commitCrop()} title="切り抜きを保存する（Enter）">
              完了
            </button>
            <button className="btn small" onClick={() => void cancelCrop()} title="枠を保存せずに戻る（Esc）">
              取り消し
            </button>
          </>
        ) : (
          <button
            className="btn small"
            disabled={disabled}
            onClick={() => void startCrop()}
            title={`切り抜く前の全体の上で枠を動かす（${comboLabel("r")}）`}
          >
            切り抜きを編集
          </button>
        )}
        <span className="grow" />
        <button
          className="btn small"
          disabled={disabled || (!cropped && g.angle_deg === 0 && !g.aspect)}
          onClick={() => {
            if (viewStore.get().crop) void cancelCrop();
            update((s) => ({ ...s, geometry: defaultGeometry() }), false);
          }}
          title="切り抜き・角度・縦横比を元に戻す"
        >
          リセット
        </button>
      </div>
      {cropped && !draft && <div className="dp-note">切り抜きあり（プレビューは切り抜いた後の画像です）</div>}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// レンズ補正（DEV-14）
// ---------------------------------------------------------------------------

interface LensSliderDef {
  key: keyof LensCorrection;
  label: string;
  min: number;
  max: number;
  defaultValue: number;
  format: (v: number) => string;
  title: string;
}

const PROFILE_SLIDERS: Record<"distortion_scale" | "vignetting_scale", LensSliderDef> = {
  distortion_scale: {
    key: "distortion_scale",
    label: "歪曲（適用量）",
    min: RANGES.lensScale.min,
    max: RANGES.lensScale.max,
    defaultValue: 100,
    format: formatPercent,
    title: "埋め込みの情報による歪曲補正の強さ（100% で情報どおり）",
  },
  vignetting_scale: {
    key: "vignetting_scale",
    label: "周辺減光（適用量）",
    min: RANGES.lensScale.min,
    max: RANGES.lensScale.max,
    defaultValue: 100,
    format: formatPercent,
    title: "埋め込みの情報による周辺減光補正の強さ（100% で情報どおり）",
  },
};

const MANUAL_SLIDERS: Record<
  "distortion" | "vignetting_amount" | "vignetting_midpoint" | "ca_red_cyan" | "ca_blue_yellow",
  LensSliderDef
> = {
  distortion: {
    key: "distortion",
    label: "歪曲",
    min: RANGES.lensManual.min,
    max: RANGES.lensManual.max,
    defaultValue: 0,
    format: signed0,
    title: "正で樽型の歪曲（線が外へふくらむ）を、負で糸巻き型の歪曲を補正します。画像の欠けないように拡大します",
  },
  vignetting_amount: {
    key: "vignetting_amount",
    label: "周辺減光",
    min: RANGES.lensManual.min,
    max: RANGES.lensManual.max,
    defaultValue: 0,
    format: signed0,
    title: "正で四隅を明るくし、負で暗くします",
  },
  vignetting_midpoint: {
    key: "vignetting_midpoint",
    label: "中点",
    min: RANGES.lensMidpoint.min,
    max: RANGES.lensMidpoint.max,
    defaultValue: 50,
    format: (v) => String(Math.round(v)),
    title: "周辺減光の補正を始める位置（小さいほど中心に近い位置から補正します）",
  },
  ca_red_cyan: {
    key: "ca_red_cyan",
    label: "赤 / シアン",
    min: RANGES.lensManual.min,
    max: RANGES.lensManual.max,
    defaultValue: 0,
    format: signed0,
    title: "倍率色収差の補正: 正で赤の像を大きくします（縁の赤・シアンの色ずれを打ち消す向きに動かします）",
  },
  ca_blue_yellow: {
    key: "ca_blue_yellow",
    label: "青 / 黄",
    min: RANGES.lensManual.min,
    max: RANGES.lensManual.max,
    defaultValue: 0,
    format: signed0,
    title: "倍率色収差の補正: 正で青の像を大きくします（縁の青・黄の色ずれを打ち消す向きに動かします）",
  },
};

function LensSlider(props: { def: LensSliderDef; lens: LensCorrection; disabled: boolean }) {
  const { def } = props;
  return (
    <Slider
      label={def.label}
      value={props.lens[def.key] as number}
      min={def.min}
      max={def.max}
      step={1}
      defaultValue={def.defaultValue}
      format={def.format}
      parse={parseNumber}
      disabled={props.disabled}
      onChange={(v, dragging) =>
        update((s) => {
          const lens = fullLens(s.lens);
          if (sameShown(def.format, v, lens[def.key] as number)) return null;
          return { ...s, lens: { ...lens, [def.key]: Math.round(v) } };
        }, dragging)
      }
      onCommit={endDrag}
      title={def.title}
    />
  );
}

function LensCheck(props: {
  label: string;
  checked: boolean;
  disabled: boolean;
  title: string;
  onChange: (on: boolean) => void;
}) {
  return (
    <label className={`dp-check${props.disabled ? " disabled" : ""}`} title={props.title}>
      <input
        type="checkbox"
        checked={props.checked}
        disabled={props.disabled}
        onChange={(e) => props.onChange(e.target.checked)}
      />
      {props.label}
    </label>
  );
}

function setLens(patch: Partial<LensCorrection>): void {
  update((s) => ({ ...s, lens: { ...fullLens(s.lens), ...patch } }), false);
}

function LensSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  const source = useDevelop((s) => s.state?.source ?? null);
  const latest = useDevelop((s) => s.state?.latest_process_version);
  const lens = fullLens(settings.lens);
  const c = lensControls(settings, source, disabled);
  const isRaw = !!source?.is_raw;
  const infoNote = lensInfoNote(source);
  const changed = JSON.stringify(lens) !== JSON.stringify(defaultLens());
  return (
    <Section id="develop.lens" title="レンズ補正" defaultOpen={false}>
      {c.needsUpgrade && !disabled && (
        <div className="dp-note">
          {`この写真の処理バージョン（${settings.process_version}）では、レンズ補正は使われません。`}
          {"処理バージョンを更新すると使えます（レンズ補正を使わなければ見え方は変わりません。履歴から取り消せます）。"}
          {canUpgradeProcess(settings.process_version, latest) && (
            <div className="dp-row">
              <button className="btn small" onClick={() => void upgradeProcessVersion()}>
                処理バージョンを {latest} に更新
              </button>
            </div>
          )}
        </div>
      )}
      <div className="dp-sub">プロファイル</div>
      <LensCheck
        label="プロファイルによる補正を有効"
        checked={lens.profile_enabled}
        disabled={!c.profile}
        title="RAW に埋め込まれた補正の情報で、歪曲と周辺減光を補正します"
        onChange={(on) => setLens({ profile_enabled: on })}
      />
      <LensCheck
        label="色収差を除去"
        checked={lens.remove_chromatic_aberration}
        disabled={!c.removeCa}
        title="RAW に埋め込まれた補正の情報で、倍率色収差（縁の色ずれ）を補正します"
        onChange={(on) => setLens({ remove_chromatic_aberration: on })}
      />
      {infoNote !== null && <div className="dp-note">{infoNote}</div>}
      <LensSlider def={PROFILE_SLIDERS.distortion_scale} lens={lens} disabled={!c.distortionScale} />
      <LensSlider def={PROFILE_SLIDERS.vignetting_scale} lens={lens} disabled={!c.vignettingScale} />
      <div className="dp-sub">手動</div>
      <LensSlider def={MANUAL_SLIDERS.distortion} lens={lens} disabled={!c.manualDistortion} />
      <LensSlider def={MANUAL_SLIDERS.vignetting_amount} lens={lens} disabled={!c.manualSensor} />
      <LensSlider def={MANUAL_SLIDERS.vignetting_midpoint} lens={lens} disabled={!c.manualSensor} />
      <div className="dp-sub">色収差</div>
      <LensSlider def={MANUAL_SLIDERS.ca_red_cyan} lens={lens} disabled={!c.manualSensor} />
      <LensSlider def={MANUAL_SLIDERS.ca_blue_yellow} lens={lens} disabled={!c.manualSensor} />
      {source && !isRaw && !c.needsUpgrade && (
        <div className="dp-note">
          RAW 以外の写真では、プロファイルによる補正・周辺減光・色収差の補正は使われません（歪曲の手動の補正は使えます）。
        </div>
      )}
      <div className="dp-row">
        <span className="grow" />
        <button
          className="btn small"
          disabled={disabled || c.needsUpgrade || !changed}
          onClick={() => update((s) => ({ ...s, lens: defaultLens() }), false)}
          title="レンズ補正を元に戻す（補正なし）"
        >
          リセット
        </button>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// 情報
// ---------------------------------------------------------------------------

/** 現像の警告（プレビューの描画の警告。隠さずに出す）。 */
function Warnings() {
  const warnings = useDevelop((s) => s.frame?.header.warnings ?? NO_WARNINGS);
  if (warnings.length === 0) return null;
  return (
    <div className="section">
      <div className="section-body">
        <ul className="dp-warn" aria-label="現像の警告">
          {warnings.map((w, i) => (
            <li key={i}>{w}</li>
          ))}
        </ul>
      </div>
    </div>
  );
}

function InfoSection() {
  const source = useDevelop((s) => s.state?.source ?? null);
  const frameSize = useDevelop((s) => (s.frame ? `${s.frame.width} × ${s.frame.height}` : null));
  const outSize = useDevelop((s) =>
    s.frame?.header.output_width && s.frame.header.output_height && !s.frame.header.uncropped
      ? `${s.frame.header.output_width} × ${s.frame.header.output_height}`
      : null,
  );
  const assumedSrgb = useDevelop((s) => !!s.frame?.header.display_assumed_srgb);
  const processVersion = useDevelop((s) => (s.state && s.settings ? s.settings.process_version : null));
  const latest = useDevelop((s) => s.state?.latest_process_version);
  return (
    <Section id="develop.info" title="情報" defaultOpen={false}>
      {source ? (
        <dl className="dp-info">
          <dt>種類</dt>
          <dd>{source.is_raw ? "RAW" : "RAW 以外"}</dd>
          <dt>デコーダ</dt>
          <dd>{source.decoder ?? "—"}</dd>
          <dt>機種</dt>
          <dd>{source.camera ?? "—"}</dd>
          <dt>元の大きさ</dt>
          <dd>
            {source.width} × {source.height}（向きを反映する前）
          </dd>
          {outSize && (
            <>
              <dt>出力の大きさ</dt>
              <dd>{outSize}</dd>
            </>
          )}
          {frameSize && (
            <>
              <dt>プレビュー</dt>
              <dd>{frameSize}</dd>
            </>
          )}
          {assumedSrgb && (
            <>
              <dt>画面の色</dt>
              <dd>sRGB とみなして表示しています</dd>
            </>
          )}
          {processVersion !== null && (
            <>
              <dt>処理バージョン</dt>
              <dd>
                {processVersion}
                {canUpgradeProcess(processVersion, latest) ? `（最新は ${latest}）` : ""}
              </dd>
            </>
          )}
        </dl>
      ) : (
        <div className="dp-note">—</div>
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// パネル
// ---------------------------------------------------------------------------

export function DevelopPanel() {
  const current = useApp((s) => s.selection.current);
  const isVideo = useLoadedSummary(current, (s) => s.kind) === "video";
  const settings = useDevelop((s) => s.settings);
  const hasState = useDevelop((s) => s.state !== null);
  const opening = useDevelop((s) => s.opening);
  const variantId = useDevelop((s) => s.variantId);
  const openError = useDevelop((s) => (s.state ? null : s.error));

  if (current === null) return <div className="dp-empty">写真を選んでください</div>;
  if (isVideo) return <div className="dp-empty">動画は現像できません</div>;

  const ready = hasState && !!settings && !opening && variantId === current;
  const shown = ready && settings ? settings : PLACEHOLDER;
  const disabled = !ready;
  return (
    <div className="dp">
      <Section id="develop.histogram" title="ヒストグラム">
        <Histogram />
      </Section>
      <Warnings />
      {!ready && (
        <div className="dp-note" style={{ padding: "6px 10px" }}>
          {openError && !opening ? `開けません: ${openError.message}` : "写真を開いています…"}
        </div>
      )}
      <ProfileSection settings={shown} disabled={disabled} />
      <BasicSection settings={shown} disabled={disabled} />
      <CurveSection settings={shown} disabled={disabled} />
      <MixerSection settings={shown} disabled={disabled} />
      <GradingSection settings={shown} disabled={disabled} />
      <DetailSection settings={shown} disabled={disabled} />
      <CropSection settings={shown} disabled={disabled} />
      <MaskSection
        settings={shown}
        disabled={disabled}
        upgradeNote={<UpgradeNote what="ローカル補正（マスク）" processVersion={shown.process_version} />}
      />
      <LensSection settings={shown} disabled={disabled} />
      <EffectsSection settings={shown} disabled={disabled} />
      <div className="dp-actions">
        <button
          className="btn"
          disabled={disabled}
          onClick={() => {
            // 編集中の枠を後で保存して、リセットした切り抜きを上書きしないように、先に編集を取り消す。
            if (viewStore.get().crop) void cancelCrop();
            update((s) => resetAll(s), false);
            selectMask(null);
          }}
          title="切り抜き・レンズ補正・ハイライト復元・ノイズ軽減・シャープ・テクスチャ・明瞭度・かすみの除去・効果・カラーミキサー・カラーグレーディング・白黒・カメラプロファイル・ルック・マスクを含むすべての調整を元に戻す（履歴から取り消せます）"
        >
          すべてリセット
        </button>
      </div>
      <InfoSection />
    </div>
  );
}
