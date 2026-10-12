// 現像のスライダー（ドラッグの開始・途中・終了を分けて知らせる。ダブルクリックで既定値に戻す）。
//
// - ドラッグ中は onChange(value, true)、離したら onCommit()（コアの end_drag。履歴に 1 件）。
// - キーボード（矢印）・数値の入力は onChange(value, false)（コアが 1 秒後に保存して履歴に記録）。

import { useEffect, useRef, useState } from "react";

export interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  /** 既定値（ダブルクリックで戻す）。 */
  defaultValue?: number;
  /** 表示の書式。 */
  format?: (v: number) => string;
  /** 入力欄の値を数値にする（既定は Number）。 */
  parse?: (s: string) => number;
  onChange: (value: number, dragging: boolean) => void;
  /** ドラッグの終了。 */
  onCommit?: () => void;
  disabled?: boolean;
  /** スライダーの目盛りを対数などにするときの変換（値 → 0〜1000 の位置）。 */
  toPosition?: (v: number) => number;
  fromPosition?: (p: number) => number;
  title?: string;
}

export function Slider(p: SliderProps) {
  const step = p.step ?? 1;
  const fmt = p.format ?? ((v: number) => (Number.isInteger(step) ? String(Math.round(v)) : v.toFixed(2)));
  const dragging = useRef(false);
  const [text, setText] = useState(fmt(p.value));
  const [editing, setEditing] = useState(false);
  // 入力欄を利用者が書き換えたか（書き換えずにフォーカスを外したときは値を送らない。Esc で取り消す）。
  const edited = useRef(false);
  useEffect(() => {
    if (!editing) setText(fmt(p.value));
    // fmt は描画ごとに作られる。値が変わったときだけ表示を更新する。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.value, editing]);

  const usePos = !!(p.toPosition && p.fromPosition);
  const posValue = usePos ? p.toPosition!(p.value) : p.value;
  const clamp = (v: number) => Math.min(p.max, Math.max(p.min, v));

  const endDrag = () => {
    if (dragging.current) {
      dragging.current = false;
      p.onCommit?.();
    }
  };

  const changed = p.defaultValue !== undefined && Math.abs(p.value - p.defaultValue) > 1e-9;

  return (
    <div className={`slider${changed ? " changed" : ""}`} title={p.title}>
      <span className="slider-label" onDoubleClick={() => p.defaultValue !== undefined && p.onChange(p.defaultValue, false)}>
        {p.label}
      </span>
      <input
        className="slider-value"
        type="text"
        inputMode="decimal"
        value={text}
        disabled={p.disabled}
        onFocus={() => {
          edited.current = false;
          setEditing(true);
        }}
        onChange={(e) => {
          edited.current = true;
          setText(e.target.value);
        }}
        onBlur={() => {
          setEditing(false);
          if (!edited.current) return;
          edited.current = false;
          const v = (p.parse ?? Number)(text.replace(/[+＋]/g, "").replace("−", "-"));
          if (Number.isFinite(v)) p.onChange(clamp(v), false);
          else setText(fmt(p.value));
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          if (e.key === "Escape") {
            // 書き換えを取り消す（フォーカスを外しても値を送らない）。
            edited.current = false;
            setText(fmt(p.value));
            setEditing(false);
            (e.target as HTMLInputElement).blur();
          }
        }}
      />
      <input
        type="range"
        min={usePos ? 0 : p.min}
        max={usePos ? 1000 : p.max}
        step={usePos ? 1 : step}
        value={posValue}
        disabled={p.disabled}
        aria-label={p.label}
        onPointerDown={() => {
          dragging.current = true;
        }}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onLostPointerCapture={endDrag}
        onDoubleClick={() => p.defaultValue !== undefined && p.onChange(p.defaultValue, false)}
        onChange={(e) => {
          const raw = Number(e.target.value);
          const v = usePos ? p.fromPosition!(raw) : raw;
          p.onChange(clamp(v), dragging.current);
        }}
        onKeyUp={() => {
          if (!dragging.current) return;
          endDrag();
        }}
      />
    </div>
  );
}
