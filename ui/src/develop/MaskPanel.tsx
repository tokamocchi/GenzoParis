// 現像のパネルの「マスク」（ローカル補正。DEV-18・DEV-19）。
//
// - マスクの一覧（有効・名前・部品の要約）。追加: ブラシ・線形グラデーション・円形グラデーション・色域（スポイトで
//   色を選んでから作る）・輝度範囲。選んだマスクの複製・削除・名前・重ねて表示（O）。
// - 選んだマスクの部品（合成のしかた・反転・削除・部品の追加）と、選んだ部品の設定（円形のぼかし・角度、ブラシの
//   大きさ・ぼかし・流量・密度・消しゴム・描く、色域の見本の色・許容量、輝度範囲の下端・上端・滑らかさ）。線形・
//   円形の位置は画面の上のつまみで動かす（MaskLayer.tsx）。
// - 調整（差分）: 色温度・色かぶり補正・露光量・コントラスト・ハイライト・シャドウ・白レベル・黒レベル・テクスチャ・
//   明瞭度・かすみの除去・彩度・シャープ。
// - 処理バージョン 1 の写真では使われないので、更新を勧めて編集はできなくする（削除はできる）。

import { type ReactNode, useEffect, useState } from "react";

import type { DevelopSettings, LocalDelta, MaskComponent, MaskMode } from "../api/types";
import { MASK_LIMITS, RANGES } from "../api/types";
import { Section } from "../components/Section";
import { Slider } from "../components/Slider";
import { comboLabel } from "../keyboard";
import { applySettings, developStore, endDrag, useDevelop } from "../state/develop";
import { shallowEqual } from "../state/store";
import {
  DEFAULT_BRUSH,
  MODE_CHOICES,
  SHAPE_LABELS,
  type ShapeKind,
  addComponent,
  boundsOrDefault,
  clearStrokes,
  deltaIsZero,
  deltaRange,
  defaultDelta,
  duplicateMask,
  fullDelta,
  labToCss,
  maskControls,
  maskDisplayName,
  maskSummary,
  newComponent,
  newLinearShape,
  newLuminanceShape,
  newMask,
  newRadialShape,
  removeColorSample,
  removeComponent,
  removeMask,
  sanitizeMaskName,
  shapeKind,
  undoStroke,
  withComponent,
  withDelta,
  withLuminance,
  withMask,
  withShape,
} from "./maskLogic";
import { formatEv, formatSigned, parseNumber, sameShown } from "./scales";
import {
  addMaskWith,
  applyAddedComponent,
  currentMask,
  overlayShown,
  selectComponent,
  selectMask,
  setBrushOptions,
  startMaskColorPick,
  toggleBrushTool,
  toggleMaskOverlay,
  useDevelopView,
} from "./viewState";

/** 設定を変えて送る（写真を開いていなければ・変わらなければ何もしない）。 */
function update(fn: (s: DevelopSettings) => DevelopSettings | null, dragging: boolean): void {
  const d = developStore.get();
  if (!d.state || !d.settings) return;
  const next = fn(d.settings);
  if (next === null || next === d.settings) return;
  applySettings(next, dragging);
}

const signed0 = (v: number) => formatSigned(v, 0);
const int0 = (v: number) => String(Math.round(v));
const percent1 = (v: number) => `${v < 1 ? v.toFixed(2) : v.toFixed(1)}%`;

/** 差分のスライダー（見出しごと）。 */
const DELTA_GROUPS: { title: string; items: { key: keyof LocalDelta; label: string; title: string }[] }[] = [
  {
    title: "ホワイトバランス",
    items: [
      { key: "temperature", label: "色温度", title: "正で暖かく（黄）、負で冷たく（青）します" },
      { key: "tint", label: "色かぶり補正", title: "正でマゼンタ寄り、負で緑寄りにします" },
    ],
  },
  {
    title: "トーン",
    items: [
      { key: "exposure_ev", label: "露光量", title: "明るさ（EV）" },
      { key: "contrast", label: "コントラスト", title: "明暗の差" },
      { key: "highlights", label: "ハイライト", title: "明るい部分" },
      { key: "shadows", label: "シャドウ", title: "暗い部分" },
      { key: "whites", label: "白レベル", title: "最も明るい部分" },
      { key: "blacks", label: "黒レベル", title: "最も暗い部分" },
    ],
  },
  {
    title: "外観・色・ディテール",
    items: [
      { key: "texture", label: "テクスチャ", title: "中くらいの細部の質感" },
      { key: "clarity", label: "明瞭度", title: "中間調の局所のコントラスト" },
      { key: "dehaze", label: "かすみの除去", title: "かすみを取り除き（正）・足します（負）" },
      { key: "saturation", label: "彩度", title: "色の鮮やかさ" },
      { key: "sharpness", label: "シャープ", title: "全体のシャープに足します（負の値はぼかします）" },
    ],
  },
];

function DeltaSliders(props: { index: number; delta: LocalDelta; disabled: boolean }) {
  const { index, delta, disabled } = props;
  return (
    <>
      {DELTA_GROUPS.map((g) => (
        <div key={g.title}>
          <div className="dp-sub">{g.title}</div>
          {g.items.map((it) => {
            const r = deltaRange(it.key);
            const ev = it.key === "exposure_ev";
            const fmt = ev ? formatEv : signed0;
            return (
              <Slider
                key={it.key}
                label={it.label}
                value={delta[it.key]}
                min={r.min}
                max={r.max}
                step={ev ? 0.01 : 1}
                defaultValue={0}
                format={fmt}
                parse={parseNumber}
                disabled={disabled}
                title={it.title}
                onChange={(v, dragging) =>
                  update((s) => {
                    const cur = fullDelta(s.masks?.[index]?.delta)[it.key];
                    if (sameShown(fmt, v, cur)) return null;
                    const rounded = ev ? Math.round(v * 100) / 100 : Math.round(v);
                    return withDelta(s, index, { [it.key]: rounded });
                  }, dragging)
                }
                onCommit={endDrag}
              />
            );
          })}
        </div>
      ))}
    </>
  );
}

/** 部品の追加のボタン（`onAdd` に種類を渡す）。 */
function AddButtons(props: { disabled: boolean; onAdd: (kind: ShapeKind) => void; pickActive?: boolean }) {
  const kinds: ShapeKind[] = ["brush", "linear", "radial", "color_range", "luminance_range"];
  return (
    <div className="dp-row dp-wrap">
      {kinds.map((k) => (
        <button
          key={k}
          type="button"
          className={`btn small${k === "color_range" && props.pickActive ? " active" : ""}`}
          disabled={props.disabled}
          title={
            k === "color_range"
              ? "スポイトでプレビューの色をクリックして作ります（もう一度押すとやめる）"
              : `${SHAPE_LABELS[k]}を足します`
          }
          onClick={() => props.onAdd(k)}
        >
          ＋ {SHAPE_LABELS[k]}
        </button>
      ))}
    </div>
  );
}

/** 選んだ部品の設定。 */
function ComponentEditor(props: { index: number; j: number; component: MaskComponent; editable: boolean }) {
  const { index, j, component, editable } = props;
  const tool = useDevelopView((s) => s.tool);
  const brush = useDevelopView((s) => s.brush);
  const pickTarget = useDevelopView((s) => s.pickTarget);
  const shape = component.shape;
  if ("linear" in shape) {
    return (
      <div className="dp-note">
        画面の上で、実線の丸（100%）と点線の丸（0%）をドラッグして位置と幅を決めます。真ん中の点線をドラッグすると全体が
        動きます。
      </div>
    );
  }
  if ("radial" in shape) {
    const r = shape.radial;
    return (
      <>
        <div className="dp-note">画面の上で、中心・縁の丸（半径）・外の丸（回転）をドラッグします。</div>
        <Slider
          label="ぼかし"
          value={r.feather}
          min={RANGES.amount.min}
          max={RANGES.amount.max}
          step={1}
          defaultValue={50}
          format={int0}
          parse={parseNumber}
          disabled={!editable}
          title="縁から内側のこの割合の範囲で、なめらかに 0 にします"
          onChange={(v, dragging) =>
            update(
              (s) => (sameShown(int0, v, r.feather) ? null : withShape(s, index, j, "radial", (x) => ({ ...x, feather: Math.round(v) }))),
              dragging,
            )
          }
          onCommit={endDrag}
        />
        <Slider
          label="角度"
          value={r.angle_deg}
          min={RANGES.maskAngleDeg.min}
          max={RANGES.maskAngleDeg.max}
          step={1}
          defaultValue={0}
          format={(v) => `${Math.round(v)}°`}
          parse={parseNumber}
          disabled={!editable}
          title="時計回りの回転"
          onChange={(v, dragging) =>
            update(
              (s) => (Math.round(v) === Math.round(r.angle_deg) ? null : withShape(s, index, j, "radial", (x) => ({ ...x, angle_deg: Math.round(v) }))),
              dragging,
            )
          }
          onCommit={endDrag}
        />
      </>
    );
  }
  if ("brush" in shape) {
    const strokes = shape.brush.strokes.length;
    const painting = tool === "brush";
    return (
      <>
        <div className="dp-row">
          <button
            type="button"
            className={`btn small${painting ? " primary" : ""}`}
            disabled={!editable}
            aria-pressed={painting}
            onClick={() => toggleBrushTool()}
            title="プレビューの上をドラッグして描きます（Esc で終わる）"
          >
            {painting ? "描くのをやめる" : "ブラシで描く"}
          </button>
          <label className={`dp-check${!editable ? " disabled" : ""}`} title="Alt を押しながら描いても消せます">
            <input
              type="checkbox"
              checked={brush.erase}
              disabled={!editable}
              onChange={(e) => setBrushOptions({ erase: e.target.checked })}
            />
            消しゴム
          </label>
        </div>
        <Slider
          label="大きさ"
          value={brush.size * 100}
          min={RANGES.brushSize.min * 100}
          max={RANGES.brushSize.max * 100}
          step={0.1}
          defaultValue={DEFAULT_BRUSH.size * 100}
          format={percent1}
          parse={parseNumber}
          disabled={!editable}
          title={`半径（写真の長辺に対する %。${comboLabel("[")} / ${comboLabel("]")} でも変えられます）。次に描くストロークに使います`}
          onChange={(v) => setBrushOptions({ size: Math.round(v * 100) / 10000 })}
        />
        <Slider
          label="ぼかし"
          value={brush.feather}
          min={0}
          max={100}
          step={1}
          defaultValue={DEFAULT_BRUSH.feather}
          format={int0}
          parse={parseNumber}
          disabled={!editable}
          title="縁をなめらかにする幅（100 で中心から外へなめらかに 0）"
          onChange={(v) => setBrushOptions({ feather: Math.round(v) })}
        />
        <Slider
          label="流量"
          value={brush.flow}
          min={0}
          max={100}
          step={1}
          defaultValue={DEFAULT_BRUSH.flow}
          format={int0}
          parse={parseNumber}
          disabled={!editable}
          title="1 本のストロークで、密度までの差のどれだけを塗るか"
          onChange={(v) => setBrushOptions({ flow: Math.round(v) })}
        />
        <Slider
          label="密度"
          value={brush.density}
          min={0}
          max={100}
          step={1}
          defaultValue={DEFAULT_BRUSH.density}
          format={int0}
          parse={parseNumber}
          disabled={!editable}
          title="描くときの重みの上限（消しゴムでは消す強さ）"
          onChange={(v) => setBrushOptions({ density: Math.round(v) })}
        />
        <div className="dp-row">
          <span className="dp-label">{strokes.toLocaleString("ja-JP")} 本</span>
          <span className="grow" />
          <button
            type="button"
            className="btn small"
            disabled={!editable || strokes === 0}
            onClick={() => update((s) => undoStroke(s, index, j), false)}
          >
            最後の 1 本を消す
          </button>
          <button
            type="button"
            className="btn small"
            disabled={!editable || strokes === 0}
            onClick={() => update((s) => clearStrokes(s, index, j), false)}
          >
            すべて消す
          </button>
        </div>
      </>
    );
  }
  if ("color_range" in shape) {
    const r = shape.color_range;
    const picking = tool === "mask_color" && pickTarget?.kind === "add_sample";
    return (
      <>
        <div className="dp-row dp-wrap" aria-label="見本の色">
          {r.samples.map((sm, k) => (
            <span key={k} className="dp-swatch-item">
              <span className="dp-swatch" style={{ background: labToCss(sm.lab) }} />
              <button
                type="button"
                className="btn ghost small"
                disabled={!editable || r.samples.length <= 1}
                aria-label={`見本 ${k + 1} を除く`}
                title={r.samples.length <= 1 ? "見本は 1 つ以上要ります" : "この見本を除く"}
                onClick={() => update((s) => removeColorSample(s, index, j, k), false)}
              >
                ✕
              </button>
            </span>
          ))}
          <button
            type="button"
            className={`btn small${picking ? " active" : ""}`}
            disabled={!editable || r.samples.length >= MASK_LIMITS.colorSamples}
            title={`スポイトでプレビューの色をクリックして見本を足します（${MASK_LIMITS.colorSamples} つまで）`}
            onClick={() => startMaskColorPick({ kind: "add_sample", mask: index, component: j })}
          >
            見本を足す
          </button>
        </div>
        <Slider
          label="許容量"
          value={r.amount}
          min={0}
          max={100}
          step={1}
          defaultValue={50}
          format={int0}
          parse={parseNumber}
          disabled={!editable}
          title="大きいほど、見本から離れた色まで選びます"
          onChange={(v, dragging) =>
            update(
              (s) => (sameShown(int0, v, r.amount) ? null : withShape(s, index, j, "color_range", (x) => ({ ...x, amount: Math.round(v) }))),
              dragging,
            )
          }
          onCommit={endDrag}
        />
      </>
    );
  }
  const l = shape.luminance_range;
  const lum = (key: "min" | "max" | "smoothness", label: string, title: string) => (
    <Slider
      label={label}
      value={l[key]}
      min={0}
      max={100}
      step={1}
      defaultValue={key === "max" ? 100 : key === "min" ? 0 : 50}
      format={int0}
      parse={parseNumber}
      disabled={!editable}
      title={title}
      onChange={(v, dragging) =>
        update((s) => (sameShown(int0, v, l[key]) ? null : withLuminance(s, index, j, { [key]: Math.round(v) })), dragging)
      }
      onCommit={endDrag}
    />
  );
  return (
    <>
      <div
        className="dp-lum-bar"
        aria-hidden
        style={{
          background: `linear-gradient(to right, transparent ${l.min}%, rgba(255,255,255,0.35) ${l.min}%, rgba(255,255,255,0.35) ${l.max}%, transparent ${l.max}%), linear-gradient(to right, #000, #fff)`,
        }}
      />
      {lum("min", "下端", "選ぶ明るさの下端（表示の明るさの %）")}
      {lum("max", "上端", "選ぶ明るさの上端")}
      {lum("smoothness", "滑らかさ", "範囲の外へなめらかに 0 にする幅")}
    </>
  );
}

/** 部品の一覧（選んだマスク）。 */
function ComponentRows(props: { index: number; components: MaskComponent[]; selected: number | null; editable: boolean }) {
  const { index, components, selected, editable } = props;
  return (
    <ul className="dp-mask-list" aria-label="部品">
      {components.map((c, j) => {
        const kind = shapeKind(c.shape);
        return (
          <li key={j} className={j === selected ? "selected" : undefined}>
            <button type="button" className="dp-mask-name" onClick={() => selectComponent(j)} aria-pressed={j === selected}>
              {j + 1}. {SHAPE_LABELS[kind]}
            </button>
            <select
              value={c.mode}
              disabled={!editable}
              aria-label={`部品 ${j + 1} の合成`}
              title={MODE_CHOICES.find((m) => m.mode === c.mode)?.title}
              onChange={(e) => update((s) => withComponent(s, index, j, (x) => ({ ...x, mode: e.target.value as MaskMode })), false)}
            >
              {MODE_CHOICES.map((m) => (
                <option key={m.mode} value={m.mode} title={m.title}>
                  {m.label}
                </option>
              ))}
            </select>
            <label className={`dp-check${!editable ? " disabled" : ""}`} title="部品の範囲を反転します">
              <input
                type="checkbox"
                checked={c.invert}
                disabled={!editable}
                onChange={(e) => update((s) => withComponent(s, index, j, (x) => ({ ...x, invert: e.target.checked })), false)}
              />
              反転
            </label>
            <button
              type="button"
              className="btn ghost small"
              disabled={!editable}
              aria-label={`部品 ${j + 1} を除く`}
              title="この部品を除く"
              onClick={() => {
                update((s) => removeComponent(s, index, j), false);
                selectComponent(components.length > 1 ? Math.max(0, j - 1) : null);
              }}
            >
              ✕
            </button>
          </li>
        );
      })}
    </ul>
  );
}

/**
 * マスクの名前の入力欄（Enter・フォーカスを外したときに送る。Esc で元に戻す。IME の変換中の Enter では送らない）。
 */
function NameInput(props: { index: number; name: string; disabled: boolean }) {
  const [text, setText] = useState(props.name);
  const [editing, setEditing] = useState(false);
  useEffect(() => {
    if (!editing) setText(props.name);
  }, [props.name, editing]);
  const commit = () => {
    setEditing(false);
    const name = sanitizeMaskName(text.trim());
    update((s) => withMask(s, props.index, (x) => (x.name === name ? x : { ...x, name })), false);
  };
  return (
    <div className="dp-row">
      <span className="dp-label">名前</span>
      <input
        type="text"
        className="dp-mask-rename"
        value={text}
        placeholder={maskDisplayName({ name: "" }, props.index)}
        disabled={props.disabled}
        aria-label="マスクの名前"
        maxLength={MASK_LIMITS.nameChars}
        onFocus={() => setEditing(true)}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing || e.keyCode === 229) return;
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          if (e.key === "Escape") {
            e.stopPropagation();
            setText(props.name);
            setEditing(false);
            (e.target as HTMLInputElement).blur();
          }
        }}
      />
    </div>
  );
}

export function MaskSection(props: { settings: DevelopSettings; disabled: boolean; upgradeNote: ReactNode }) {
  const { settings, disabled } = props;
  const c = maskControls(settings, disabled);
  const view = useDevelopView(
    (s) => ({ maskIndex: s.maskIndex, componentIndex: s.componentIndex, overlay: overlayShown(s), tool: s.tool, pickTarget: s.pickTarget }),
    shallowEqual,
  );
  const gb = useDevelop((s) => s.frame?.header.g_bounds ?? null);
  const masks = settings.masks ?? [];
  const sel = currentMask(settings, view);
  const editable = c.editable;
  const pickingNewMask = view.tool === "mask_color" && view.pickTarget?.kind === "new_mask";
  const pickingNewComponent = view.tool === "mask_color" && view.pickTarget?.kind === "new_component";

  const addNewMask = (kind: ShapeKind) => {
    const s = developStore.get().settings;
    if (!s) return;
    const b = boundsOrDefault(gb);
    switch (kind) {
      case "color_range":
        startMaskColorPick({ kind: "new_mask" });
        return;
      case "brush":
        addMaskWith(s, newMask({ brush: { strokes: [] } }), true);
        return;
      case "linear":
        addMaskWith(s, newMask(newLinearShape(b)), false);
        return;
      case "radial":
        addMaskWith(s, newMask(newRadialShape(b)), false);
        return;
      case "luminance_range":
        addMaskWith(s, newMask(newLuminanceShape()), false);
        return;
    }
  };
  const addNewComponent = (kind: ShapeKind) => {
    const s = developStore.get().settings;
    if (!s || !sel) return;
    const b = boundsOrDefault(gb);
    if (kind === "color_range") {
      startMaskColorPick({ kind: "new_component", mask: sel.index });
      return;
    }
    const shape =
      kind === "brush"
        ? { brush: { strokes: [] } }
        : kind === "linear"
          ? newLinearShape(b)
          : kind === "radial"
            ? newRadialShape(b)
            : newLuminanceShape();
    const next = addComponent(s, sel.index, newComponent(shape));
    if (next !== s) applyAddedComponent(next, sel.index, kind === "brush");
  };

  return (
    <Section id="develop.masks" title={masks.length > 0 ? `マスク（${masks.length}）` : "マスク"} defaultOpen={false}>
      {c.needsUpgrade && !disabled && props.upgradeNote}
      <div className="dp-sub">マスクを足す</div>
      <AddButtons disabled={!c.canAdd} onAdd={addNewMask} pickActive={pickingNewMask} />
      {masks.length === 0 ? (
        <div className="dp-note">
          マスクを足すと、写真の一部だけを調整できます（ブラシで塗る・グラデーション・色や明るさの範囲）。
        </div>
      ) : (
        <ul className="dp-mask-list" aria-label="マスク">
          {masks.map((m, i) => (
            <li key={i} className={sel?.index === i ? "selected" : undefined}>
              <input
                type="checkbox"
                checked={m.enabled}
                disabled={!editable}
                aria-label={`${maskDisplayName(m, i)}を使う`}
                title="オフにすると、このマスクの調整を使いません"
                onChange={(e) => update((s) => withMask(s, i, (x) => ({ ...x, enabled: e.target.checked })), false)}
              />
              <button
                type="button"
                className="dp-mask-name"
                aria-pressed={sel?.index === i}
                onClick={() => selectMask(sel?.index === i ? null : i)}
                title={maskSummary(m)}
              >
                {maskDisplayName(m, i)}
                {deltaIsZero(m.delta) && <span className="dp-mask-hint">（調整なし）</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
      {sel && (
        <>
          <NameInput key={sel.index} index={sel.index} name={sel.mask.name} disabled={!editable} />
          <div className="dp-row">
            <label
              className="dp-check"
              title={`選んだマスクの範囲を赤で重ねて表示します（${comboLabel("o")}。ブラシで描いている間と、それ以外とで別に切り替えます。描いている間は、既定で表示します）`}
            >
              <input type="checkbox" checked={view.overlay} onChange={() => toggleMaskOverlay()} />
              重ねて表示
            </label>
            <span className="grow" />
            <button
              type="button"
              className="btn small"
              disabled={!c.canAdd}
              onClick={() => {
                const s = developStore.get().settings;
                if (!s) return;
                const next = duplicateMask(s, sel.index);
                if (next === s) return;
                applySettings(next, false);
                selectMask(next.masks.length - 1);
              }}
              title="このマスクの写しを足します"
            >
              複製
            </button>
            <button
              type="button"
              className="btn small"
              disabled={disabled}
              onClick={() => {
                update((s) => removeMask(s, sel.index), false);
                selectMask(null);
              }}
              title="このマスクを除きます（履歴から元に戻せます）"
            >
              削除
            </button>
          </div>
          <div className="dp-sub">部品（上から順に合成）</div>
          <ComponentRows index={sel.index} components={sel.mask.components} selected={sel.component} editable={editable} />
          <AddButtons
            disabled={!editable || sel.mask.components.length >= MASK_LIMITS.components}
            onAdd={addNewComponent}
            pickActive={pickingNewComponent}
          />
          {sel.component !== null && (
            <>
              <div className="dp-sub">{SHAPE_LABELS[shapeKind(sel.mask.components[sel.component].shape)]}</div>
              <ComponentEditor
                index={sel.index}
                j={sel.component}
                component={sel.mask.components[sel.component]}
                editable={editable}
              />
            </>
          )}
          <DeltaSliders index={sel.index} delta={fullDelta(sel.mask.delta)} disabled={!editable} />
          <div className="dp-row">
            <span className="grow" />
            <button
              type="button"
              className="btn small"
              disabled={!editable || deltaIsZero(sel.mask.delta)}
              onClick={() => update((s) => withMask(s, sel.index, (x) => ({ ...x, delta: defaultDelta() })), false)}
              title="このマスクの調整をすべて 0 に戻します（範囲は保つ）"
            >
              調整をリセット
            </button>
          </div>
        </>
      )}
    </Section>
  );
}
