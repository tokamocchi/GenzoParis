// 現像のパネルの「スポット修正」（DEV-21。修復・コピースタンプ）。
//
// - 道具（Q）: 使っている間、プレビューの空いているところをクリックすると円、ドラッグすると線（電線など）のスポットを
//   足す（RetouchLayer.tsx。コピー元はコアが選ぶ）。次に足すスポットの直し方・大きさ・ぼかし・不透明度を選べる。
// - スポットの一覧（有効・番号・形）。選んだスポットの直し方・大きさ・ぼかし・不透明度・コピー元の選び直し・削除。
//   位置とコピー元は画面の上でドラッグして動かす。すべて削除。
// - 処理バージョン 1 の写真では使われないので、更新を勧めて編集はできなくする（削除はできる）。

import type { ReactNode } from "react";

import type { DevelopSettings, RetouchMode, RetouchSpot } from "../api/types";
import { RANGES, RETOUCH_LIMITS } from "../api/types";
import { Section } from "../components/Section";
import { Slider } from "../components/Slider";
import { comboLabel } from "../keyboard";
import { applySettings, developStore, endDrag } from "../state/develop";
import { shallowEqual } from "../state/store";
import {
  DEFAULT_RETOUCH,
  MODE_LABELS,
  MODE_TITLES,
  clearSpots,
  removeSpot,
  retouchControls,
  retouchSize,
  spotLabel,
  spotSummary,
  spotsOf,
  withSpot,
} from "./retouchLogic";
import { parseNumber, sameShown } from "./scales";
import {
  currentSpot,
  reselectSource,
  selectSpot,
  setRetouchOptions,
  toggleRetouchTool,
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

const int0 = (v: number) => String(Math.round(v));
const percent2 = (v: number) => `${v.toFixed(2)}%`;
const MODES: RetouchMode[] = ["heal", "clone"];

/** 直し方の選択。 */
function ModeSelect(props: { value: RetouchMode; disabled: boolean; label: string; onChange: (m: RetouchMode) => void }) {
  return (
    <div className="dp-row">
      <span className="dp-label">直し方</span>
      <select
        value={props.value}
        disabled={props.disabled}
        aria-label={props.label}
        title={MODE_TITLES[props.value]}
        onChange={(e) => props.onChange(e.target.value as RetouchMode)}
      >
        {MODES.map((m) => (
          <option key={m} value={m} title={MODE_TITLES[m]}>
            {MODE_LABELS[m]}
          </option>
        ))}
      </select>
    </div>
  );
}

/** 大きさ・ぼかし・不透明度のスライダー（`onChange` に項目と値を渡す）。 */
function ValueSliders(props: {
  values: Pick<RetouchSpot, "size" | "feather" | "opacity">;
  disabled: boolean;
  sizeTitle: string;
  onChange: (key: "size" | "feather" | "opacity", value: number, dragging: boolean) => void;
  onCommit?: () => void;
}) {
  const { values, disabled } = props;
  return (
    <>
      <Slider
        label="大きさ"
        value={values.size * 100}
        min={RANGES.retouchSize.min * 100}
        max={RANGES.retouchSize.max * 100}
        step={0.01}
        defaultValue={DEFAULT_RETOUCH.size * 100}
        format={percent2}
        parse={parseNumber}
        disabled={disabled}
        title={props.sizeTitle}
        onChange={(v, dragging) => props.onChange("size", retouchSize(v / 100), dragging)}
        onCommit={props.onCommit}
      />
      <Slider
        label="ぼかし"
        value={values.feather}
        min={RANGES.amount.min}
        max={RANGES.amount.max}
        step={1}
        defaultValue={DEFAULT_RETOUCH.feather}
        format={int0}
        parse={parseNumber}
        disabled={disabled}
        title="縁から内側のこの割合の幅で、なめらかに元の画像に戻します"
        onChange={(v, dragging) => props.onChange("feather", Math.round(v), dragging)}
        onCommit={props.onCommit}
      />
      <Slider
        label="不透明度"
        value={values.opacity}
        min={RANGES.amount.min}
        max={RANGES.amount.max}
        step={1}
        defaultValue={DEFAULT_RETOUCH.opacity}
        format={int0}
        parse={parseNumber}
        disabled={disabled}
        title="直した結果をどれだけ重ねるか"
        onChange={(v, dragging) => props.onChange("opacity", Math.round(v), dragging)}
        onCommit={props.onCommit}
      />
    </>
  );
}

export function RetouchSection(props: { settings: DevelopSettings; disabled: boolean; upgradeNote: ReactNode }) {
  const { settings, disabled } = props;
  const c = retouchControls(settings, disabled);
  const view = useDevelopView((s) => ({ tool: s.tool, spotIndex: s.spotIndex, retouch: s.retouch }), shallowEqual);
  const spots = spotsOf(settings);
  const sel = currentSpot(settings, view);
  const active = view.tool === "retouch";
  const editable = c.editable;
  const key = comboLabel("q");

  return (
    <Section id="develop.retouch" title={spots.length > 0 ? `スポット修正（${spots.length}）` : "スポット修正"} defaultOpen={false}>
      {c.needsUpgrade && !disabled && props.upgradeNote}
      <div className="dp-row">
        <button
          type="button"
          className={`btn small${active ? " primary" : ""}`}
          disabled={!editable}
          aria-pressed={active}
          onClick={() => toggleRetouchTool()}
          title={`プレビューの上をクリックすると円、ドラッグすると線（電線など）を消します（${key}。Esc で終わる）`}
        >
          {active ? "スポットを足すのをやめる" : `スポットを足す（${key}）`}
        </button>
      </div>
      {active && c.canAdd && (
        <div className="dp-note">
          消したいところをクリック（円）かドラッグ（線）します。コピー元は自動で選びます（点線の形をドラッグすると動かせます）。
        </div>
      )}
      {editable && !c.canAdd && (
        <div className="dp-note">
          スポットは {RETOUCH_LIMITS.spots.toLocaleString("ja-JP")} 個までです。これ以上は足せません（いまのスポットは動かす・変える・除くことができます）。
        </div>
      )}
      <div className="dp-sub">次に足すスポット</div>
      <ModeSelect
        value={view.retouch.mode}
        disabled={!editable}
        label="次に足すスポットの直し方"
        onChange={(mode) => setRetouchOptions({ mode })}
      />
      <ValueSliders
        values={view.retouch}
        disabled={!editable}
        sizeTitle={`半径（写真の長辺に対する %。道具を使っている間は ${comboLabel("[")} / ${comboLabel("]")} でも変えられます）`}
        onChange={(k, v) => setRetouchOptions({ [k]: v })}
      />
      {spots.length === 0 ? (
        <div className="dp-note">センサーのゴミ・電線・小さな写り込みを、まわりの模様で消せます。</div>
      ) : (
        <>
          <ul className="dp-mask-list" aria-label="スポット">
            {spots.map((r, i) => (
              <li key={i} className={sel?.index === i ? "selected" : undefined}>
                <input
                  type="checkbox"
                  checked={r.enabled}
                  disabled={!editable}
                  aria-label={`${spotLabel(i)}を使う`}
                  title="オフにすると、このスポットを使いません"
                  onChange={(e) => update((s) => withSpot(s, i, (x) => ({ ...x, enabled: e.target.checked })), false)}
                />
                <button
                  type="button"
                  className="dp-mask-name"
                  aria-pressed={sel?.index === i}
                  onClick={() => selectSpot(sel?.index === i ? null : i)}
                >
                  {spotLabel(i)}
                  <span className="dp-mask-hint">
                    （{MODE_LABELS[r.mode]}・{spotSummary(r)}）
                  </span>
                </button>
              </li>
            ))}
          </ul>
          <div className="dp-row">
            <span className="grow" />
            <button
              type="button"
              className="btn small"
              disabled={disabled}
              onClick={() => {
                update((s) => clearSpots(s), false);
                selectSpot(null);
              }}
              title="スポットをすべて除きます（履歴から元に戻せます）"
            >
              すべて削除
            </button>
          </div>
        </>
      )}
      {sel && (
        <>
          <div className="dp-sub">{spotLabel(sel.index)}</div>
          <ModeSelect
            value={sel.spot.mode}
            disabled={!editable}
            label={`${spotLabel(sel.index)}の直し方`}
            onChange={(mode) => update((s) => withSpot(s, sel.index, (x) => (x.mode === mode ? x : { ...x, mode })), false)}
          />
          <ValueSliders
            values={sel.spot}
            disabled={!editable}
            sizeTitle="半径（写真の長辺に対する %）"
            onChange={(k, v, dragging) =>
              update(
                (s) =>
                  withSpot(s, sel.index, (x) =>
                    sameShown(k === "size" ? percent2 : int0, k === "size" ? v * 100 : v, k === "size" ? x[k] * 100 : x[k])
                      ? x
                      : { ...x, [k]: v },
                  ),
                dragging,
              )
            }
            onCommit={endDrag}
          />
          <div className="dp-row">
            <button
              type="button"
              className="btn small"
              disabled={!editable}
              onClick={() => void reselectSource(sel.index)}
              title="まわりとよく似た場所を、コピー元として選び直します"
            >
              コピー元を選び直す
            </button>
            <span className="grow" />
            <button
              type="button"
              className="btn small"
              disabled={disabled}
              onClick={() => {
                update((s) => removeSpot(s, sel.index), false);
                selectSpot(null);
              }}
              title="このスポットを除きます（Delete。履歴から元に戻せます）"
            >
              削除
            </button>
          </div>
        </>
      )}
    </Section>
  );
}
