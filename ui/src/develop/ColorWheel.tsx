// カラーグレーディングの色相環（DEV-10）。点をドラッグして色相（角度。右がレッド、反時計回り）と強さ（中心からの
// 距離。縁が 100）を選ぶ。ドラッグ中は onChange(…, true)、離したら onCommit()（スライダーと同じ。コアの end_drag）。
// ダブルクリックで強さを 0 に戻す。キーボードでの操作は下のスライダー（色相・強さ）で行う。

import { type PointerEvent as ReactPointerEvent, useRef } from "react";

import { hsvCss, hueSatToWheelPoint, wheelBackground, wheelPointToHueSat } from "./colorLogic";

export function ColorWheel(props: {
  hue: number;
  saturation: number;
  size: number;
  disabled: boolean;
  label: string;
  onChange: (v: { hue: number; saturation: number }, dragging: boolean) => void;
  onCommit: () => void;
}) {
  const { hue, saturation, size, disabled } = props;
  const ref = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const radius = size / 2;
  const point = hueSatToWheelPoint(hue, saturation, radius);

  const pick = (e: ReactPointerEvent<HTMLDivElement>) => {
    const r = ref.current?.getBoundingClientRect();
    if (!r) return null;
    return wheelPointToHueSat(e.clientX - (r.left + r.width / 2), e.clientY - (r.top + r.height / 2), r.width / 2, hue);
  };
  const end = () => {
    if (dragging.current) {
      dragging.current = false;
      props.onCommit();
    }
  };

  return (
    <div
      ref={ref}
      className={`dp-wheel${disabled ? " disabled" : ""}`}
      style={{ width: size, height: size, background: wheelBackground() }}
      role="img"
      aria-label={`${props.label}: 色相 ${Math.round(hue)}°・強さ ${Math.round(saturation)}`}
      title="ドラッグで色相（向き）と強さ（中心からの距離）を選ぶ。ダブルクリックで強さを 0 に戻す"
      onPointerDown={(e) => {
        if (disabled || e.button !== 0) return;
        const v = pick(e);
        if (!v) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        dragging.current = true;
        props.onChange(v, true);
      }}
      onPointerMove={(e) => {
        if (!dragging.current || disabled) return;
        const v = pick(e);
        if (v) props.onChange(v, true);
      }}
      onPointerUp={end}
      onPointerCancel={end}
      onLostPointerCapture={end}
      onDoubleClick={() => {
        if (!disabled) props.onChange({ hue, saturation: 0 }, false);
      }}
    >
      <span
        className="dp-wheel-dot"
        style={{
          left: radius + point.x,
          top: radius + point.y,
          background: hsvCss(hue, saturation / 100),
        }}
      />
    </div>
  );
}
