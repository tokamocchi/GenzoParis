// 現像のスライダー（ドラッグの開始・途中・終了を分けて知らせる。ダブルクリックで既定値に戻す）。
//
// - ドラッグ中は onChange(value, true)、離したら onCommit()（コアの end_drag。履歴に 1 件）。
// - バーを離してから onCommit() まで DOUBLE_CLICK_MS だけ待つ。その間にダブルクリック（既定値に戻す）が来たら、
//   戻す変更も同じドラッグの中で送ってから onCommit() する（1 回目のクリックで動いた途中の値を履歴に残さない。
//   取り消すと、ダブルクリックの前の値に戻る）。
// - キーボード（矢印）・数値の入力は onChange(value, false)（コアが 1 秒後に保存して履歴に記録）。
// - スライダーにフォーカスがあるときの矢印・Home・End・PageUp・PageDown は、スライダーの値を変えるだけに
//   する（window の全体のショートカット（←→ の写真の移動など）まで届かないようにする。数字キーの評価などは
//   全体に通す）。
// - 数値の入力欄は IME の全角の数字・記号（"１２"・"－０．５"）も読む。IME の変換中の Enter・Esc では何もしない。

import { useEffect, useRef, useState } from "react";

import { isRangeKey } from "../keyboard";

/**
 * 入力欄の文字を読む前に整える（IME の全角の数字・記号（"１２"・"－０．５"・"＋"）を NFKC で半角にし、"+" を
 * 除き、負の記号 "−" を "-" に読み替える）。
 */
export function normalizeNumberText(s: string): string {
  return s.normalize("NFKC").replace(/\+/g, "").replace(/−/g, "-");
}

/** 入力欄の既定の読み方（空・読めなければ NaN。NaN のときは元の値に戻す）。 */
export function parseSliderText(s: string): number {
  const t = normalizeNumberText(s).trim();
  return t === "" ? Number.NaN : Number(t);
}

/**
 * バーを離してから、ドラッグの終了（onCommit）を送るまでの時間（ms。仮置き: 500。Windows の既定の
 * ダブルクリックの間隔と同じ。これより遅いダブルクリックでは、1 回目のクリックの値が履歴に 1 件残る）。
 */
export const DOUBLE_CLICK_MS = 500;

export interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  /** 既定値（ダブルクリックで戻す）。 */
  defaultValue?: number;
  /**
   * ダブルクリックで既定に戻す処理（既定が 1 つの値ではないもの。ホワイトバランスの「撮影時の設定」など）。
   * 渡すと defaultValue より優先する。
   */
  onReset?: (dragging: boolean) => void;
  /** 既定から変えているか（onReset を渡すときの印。渡さなければ defaultValue と比べる）。 */
  modified?: boolean;
  /** 表示の書式。 */
  format?: (v: number) => string;
  /** 入力欄の値を数値にする（既定は parseSliderText。どちらも normalizeNumberText で整えた文字を受ける）。 */
  parse?: (s: string) => number;
  onChange: (value: number, dragging: boolean) => void;
  /** ドラッグの終了。 */
  onCommit?: () => void;
  disabled?: boolean;
  /** スライダーの目盛りを対数などにするときの変換（値 → 0〜1000 の位置）。 */
  toPosition?: (v: number) => number;
  fromPosition?: (p: number) => number;
  title?: string;
  /** 名前の前に出す色の印（CSS の色。カラーミキサーの帯の色など）。 */
  swatch?: string;
}

export function Slider(p: SliderProps) {
  const step = p.step ?? 1;
  const fmt = p.format ?? ((v: number) => (Number.isInteger(step) ? String(Math.round(v)) : v.toFixed(2)));
  const dragging = useRef(false);
  // 離した後、ドラッグの終了を送るのを待っている間のタイマー。
  const commitTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onCommitRef = useRef(p.onCommit);
  onCommitRef.current = p.onCommit;
  const rangeRef = useRef<HTMLInputElement>(null);
  // 待っている間に、このバー以外を押した・キーを押したときに終了をすぐに送る（取り消し・ほかのスライダー・
  // 写真の切り替えなどが、終了より先に動かないように）。
  const outsideRef = useRef<((e: Event) => void) | null>(null);
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

  const cancelCommitTimer = () => {
    if (commitTimer.current !== null) {
      clearTimeout(commitTimer.current);
      commitTimer.current = null;
    }
    if (outsideRef.current) {
      window.removeEventListener("pointerdown", outsideRef.current, true);
      window.removeEventListener("keydown", outsideRef.current, true);
      outsideRef.current = null;
    }
  };
  /** ドラッグの終了をすぐに送る（待っている終了も含む）。 */
  const commitNow = () => {
    const pending = dragging.current || commitTimer.current !== null;
    cancelCommitTimer();
    dragging.current = false;
    if (pending) onCommitRef.current?.();
  };
  /** バーを離した: ダブルクリックが来るかもしれないので、終了を少し待ってから送る。 */
  const endDrag = () => {
    if (!dragging.current) return;
    dragging.current = false;
    cancelCommitTimer();
    commitTimer.current = setTimeout(() => {
      cancelCommitTimer();
      onCommitRef.current?.();
    }, DOUBLE_CLICK_MS);
    const outside = (e: Event) => {
      if (e.type === "pointerdown" && e.target === rangeRef.current) return;
      cancelCommitTimer();
      onCommitRef.current?.();
    };
    outsideRef.current = outside;
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("keydown", outside, true);
  };
  // 外されるとき（写真・パネルが変わったなど）は、待っている終了をすぐに送る。
  useEffect(
    () => () => {
      if (commitTimer.current !== null) {
        cancelCommitTimer();
        onCommitRef.current?.();
      }
    },
    // cancelCommitTimer は ref だけを使う。
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  const changed = p.modified ?? (p.defaultValue !== undefined && Math.abs(p.value - p.defaultValue) > 1e-9);
  /**
   * 既定に戻す（ダブルクリック）。バーのダブルクリックでは 1 回目のクリックの変更がドラッグとして送られて
   * いるので、戻す変更も同じドラッグの中で送ってから終了を送る（履歴は「前の値 → 既定」の 1 件）。
   */
  const reset = () => {
    if (p.disabled) return;
    const inDrag = dragging.current || commitTimer.current !== null;
    if (p.onReset) p.onReset(inDrag);
    else if (p.defaultValue !== undefined) p.onChange(p.defaultValue, inDrag);
    if (inDrag) commitNow();
  };

  return (
    <div className={`slider${changed ? " changed" : ""}`} title={p.title}>
      <span className="slider-label" onDoubleClick={reset}>
        {p.swatch && <span className="slider-swatch" style={{ background: p.swatch }} aria-hidden="true" />}
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
          const v = (p.parse ?? parseSliderText)(normalizeNumberText(text));
          if (Number.isFinite(v)) p.onChange(clamp(v), false);
          else setText(fmt(p.value));
        }}
        onKeyDown={(e) => {
          // IME の変換中（変換を確定する Enter・取り消す Esc を含む）は何もしない。
          if (e.nativeEvent.isComposing || e.keyCode === 229) return;
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
        ref={rangeRef}
        type="range"
        min={usePos ? 0 : p.min}
        max={usePos ? 1000 : p.max}
        step={usePos ? 1 : step}
        value={posValue}
        disabled={p.disabled}
        aria-label={p.label}
        onPointerDown={() => {
          // 待っている終了は送らずに、同じドラッグの続きにする（ダブルクリックの 2 回目のクリック）。
          cancelCommitTimer();
          dragging.current = true;
        }}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onLostPointerCapture={endDrag}
        onDoubleClick={reset}
        // 矢印・Home・End などはスライダーの値を変える（window の全体のショートカット（←→ の写真の移動）まで
        // 届かないようにする）。値の変化は下の onChange で知らせる（ドラッグ中でなければ onChange(value, false)）。
        onKeyDown={(e) => {
          if (isRangeKey(e.key)) e.stopPropagation();
        }}
        onChange={(e) => {
          const raw = Number(e.target.value);
          const v = usePos ? p.fromPosition!(raw) : raw;
          p.onChange(clamp(v), dragging.current);
        }}
        onKeyUp={() => {
          if (!dragging.current) return;
          commitNow();
        }}
      />
    </div>
  );
}
