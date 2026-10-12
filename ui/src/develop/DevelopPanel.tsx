// 現像のパネル（右。DEV-03〜05・07・08・15・26）。
//
// - ヒストグラム・基本補正（WB・露光量・コントラスト・ハイライト / シャドウ・白 / 黒・自然な彩度・彩度）・
//   トーンカーブ（パラメトリックとポイント）・切り抜きと角度・すべてリセット・警告と元の画像の情報。
// - 値は developStore.settings（送信前を含む最新の値）から読み、変更は applySettings で送る（ドラッグ中は
//   dragging: true、離したら endDrag。state/develop.ts の送信の列）。
// - 写真を開いている間・開けなかったときは、操作を無効にする。

import { useState } from "react";

import type { CurvePoint, DevelopSettings, WbPreset } from "../api/types";
import { RANGES, WB_PRESETS } from "../api/types";
import { Section } from "../components/Section";
import { Slider } from "../components/Slider";
import { comboLabel } from "../keyboard";
import { useApp } from "../state/app";
import { applySettings, developStore, endDrag, useDevelop } from "../state/develop";
import { ASPECT_CHOICES, type GeometryInfo, aspectKey, gBoxSize } from "./cropMath";
import { CURVE_CHANNELS, type CurveChannel, withChannel } from "./curveMath";
import { Histogram } from "./Histogram";
import { useGeometryInfo, useLoadedSummary } from "./hooks";
import {
  formatAngle,
  formatEv,
  formatKelvin,
  formatSigned,
  parseNumber,
  positionToTemperature,
  temperatureToPosition,
} from "./scales";
import { type WbMode, customWb, defaultGeometry, effectiveWb, resetAll, wbFromMode, wbMode } from "./settingsLogic";
import { ToneCurveEditor } from "./ToneCurveEditor";
import {
  cancelCrop,
  chooseAspect,
  commitCrop,
  constrainDraft,
  refitDraftForAngle,
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
  lens: { profile_enabled: false, distortion_scale: 100, vignetting_scale: 100, remove_chromatic_aberration: false },
  detail: {
    sharpening: { amount: 0, radius: 1, detail: 0, masking: 0 },
    noise_reduction: { luminance: 0, color: 0 },
  },
  masks: [],
  render_deps: { camera_profile: { id: "", sha256: "" }, lens_profile: null, raw_decoder: "" },
} as unknown as DevelopSettings);

const NO_WARNINGS: string[] = [];

/** 設定を変えて送る（最新の値から作る。写真を開いていなければ何もしない）。 */
function update(fn: (s: DevelopSettings) => DevelopSettings, dragging: boolean): void {
  const d = developStore.get();
  if (!d.state || !d.settings) return;
  applySettings(fn(d.settings), dragging);
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
          onChange={(v, dragging) => update((s) => d.set(s, v), dragging)}
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
        onChange={(v, dragging) => update((s) => ({ ...s, exposure_ev: Math.round(v * 100) / 100 }), dragging)}
        onCommit={endDrag}
      />
      <SliderRows defs={TONE_SLIDERS} settings={settings} disabled={disabled} />
      <div className="dp-sub">色</div>
      <SliderRows defs={COLOR_SLIDERS} settings={settings} disabled={disabled} />
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

/** 角度を変えたときの、縦横比の計算に使う大きさ（O は同じで、外接矩形だけ変わる）。 */
function infoAtAngle(info: GeometryInfo | null, angle: number): GeometryInfo | null {
  if (!info) return null;
  const g = gBoxSize(info.ow, info.oh, angle);
  return { ow: info.ow, oh: info.oh, gw: g.width, gh: g.height };
}

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
        onChange={(v, dragging) => {
          const angle = Math.round(v * 100) / 100;
          update((s) => ({ ...s, geometry: { ...s.geometry, angle_deg: angle } }), dragging);
          if (viewStore.get().crop) {
            refitDraftForAngle(infoAtAngle(info, angle));
            if (!dragging) void constrainDraft();
          }
        }}
        onCommit={() => {
          endDrag();
          if (viewStore.get().crop) void constrainDraft();
        }}
        title="正の値で反時計回りに回転します"
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
      <CropSection settings={shown} disabled={disabled} />
      <div className="dp-actions">
        <button
          className="btn"
          disabled={disabled}
          onClick={() => update((s) => resetAll(s), false)}
          title="切り抜きを含むすべての調整を元に戻す（レンズ補正・ディテール・部分補正は保つ）"
        >
          すべてリセット
        </button>
      </div>
      <InfoSection />
    </div>
  );
}
