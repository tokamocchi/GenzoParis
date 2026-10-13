// 現像のパネル（右。DEV-03〜08・13〜15・26）。
//
// - ヒストグラム・基本補正（WB・露光量・コントラスト・ハイライト / シャドウ・白 / 黒・ハイライト復元（処理バージョン 2
//   から。RAW だけ）・自然な彩度・彩度）・トーンカーブ（パラメトリックとポイント）・ディテール（ノイズ軽減の輝度・
//   カラー。処理バージョン 2 から。RAW だけ）・切り抜きと角度・レンズ補正（プロファイル・色収差の除去・手動の
//   歪曲 / 周辺減光 / 色収差。処理バージョン 2 から）・すべてリセット・警告と元の画像の情報（処理バージョンの
//   更新）。
// - 値は developStore.settings（送信前を含む最新の値）から読み、変更は applySettings で送る（ドラッグ中は
//   dragging: true、離したら endDrag。state/develop.ts の送信の列）。
// - 写真を開いている間・開けなかったときは、操作を無効にする。

import { useState } from "react";

import type {
  CurvePoint,
  DevelopSettings,
  HighlightMethod,
  LensCorrection,
  NoiseReduction,
  WbPreset,
} from "../api/types";
import { RANGES, WB_PRESETS } from "../api/types";
import { Section } from "../components/Section";
import { Slider } from "../components/Slider";
import { comboLabel } from "../keyboard";
import { useApp } from "../state/app";
import { applySettings, developStore, endDrag, upgradeProcessVersion, useDevelop } from "../state/develop";
import { ASPECT_CHOICES, aspectKey } from "./cropMath";
import { CURVE_CHANNELS, type CurveChannel, withChannel } from "./curveMath";
import { Histogram } from "./Histogram";
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
    sharpening: { amount: 0, radius: 1, detail: 0, masking: 0 },
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
        title="色温度（K）。動かすとカスタムになります"
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
        title="正で緑を弱め（マゼンタ寄り）、負で緑を強めます。動かすとカスタムになります"
      />
    </>
  );
}

function BasicSection(props: { settings: DevelopSettings; disabled: boolean }) {
  const { settings, disabled } = props;
  return (
    <Section id="develop.basic" title="基本補正">
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
      <div className="dp-sub">色</div>
      <SliderRows defs={COLOR_SLIDERS} settings={settings} disabled={disabled} />
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
      {c.needsUpgrade && !disabled && <UpgradeNote what="ノイズ軽減" processVersion={settings.process_version} />}
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
      <BasicSection settings={shown} disabled={disabled} />
      <CurveSection settings={shown} disabled={disabled} />
      <DetailSection settings={shown} disabled={disabled} />
      <CropSection settings={shown} disabled={disabled} />
      <LensSection settings={shown} disabled={disabled} />
      <div className="dp-actions">
        <button
          className="btn"
          disabled={disabled}
          onClick={() => {
            // 編集中の枠を後で保存して、リセットした切り抜きを上書きしないように、先に編集を取り消す。
            if (viewStore.get().crop) void cancelCrop();
            update((s) => resetAll(s), false);
          }}
          title="切り抜き・レンズ補正・ハイライト復元・ノイズ軽減を含むすべての調整を元に戻す（シャープ・部分補正は保つ）"
        >
          すべてリセット
        </button>
      </div>
      <InfoSection />
    </div>
  );
}
