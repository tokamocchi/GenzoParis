// 線のアイコン（16 × 16 の枠・線の太さ 1.5）。文字の記号（▾ ⚑ × … ⚙ など）の代わりに使う。
//
// - 色は文字の色（currentColor）。大きさは `size`（px。既定 14）か、CSS の width / height。
// - 飾りなので読み上げない（aria-hidden）。意味は、置く側の aria-label・title で伝える。
// - 外部のアイコンのパッケージは使わない（形はここに書いた線だけ）。

import type { ReactNode } from "react";

const PATHS = {
  chevronDown: <path d="M4 6l4 4 4-4" />,
  chevronRight: <path d="M6 4l4 4-4 4" />,
  close: <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />,
  plus: <path d="M8 3v10M3 8h10" />,
  minus: <path d="M3.5 8h9" />,
  check: <path d="M3.2 8.6l3 3 6.4-7.2" />,
  more: (
    <>
      <circle cx="3.5" cy="8" r=".6" />
      <circle cx="8" cy="8" r=".6" />
      <circle cx="12.5" cy="8" r=".6" />
    </>
  ),
  settings: (
    <>
      <path d="M2 5h6.5M12.5 5H14M2 11h1.5M7.5 11H14" />
      <circle cx="10.5" cy="5" r="1.8" />
      <circle cx="5.5" cy="11" r="1.8" />
    </>
  ),
  search: (
    <>
      <circle cx="7" cy="7" r="4.2" />
      <path d="M10.2 10.2l3.3 3.3" />
    </>
  ),
  filter: <path d="M2.5 4h11M4.5 8h7M6.5 12h3" />,
  flag: <path d="M4 14V2.8h8l-2.2 2.9L12 8.6H4" />,
  arrowUp: <path d="M8 13V3.5M4 7.2l4-4 4 4" />,
  arrowDown: <path d="M8 3v9.5M4 8.8l4 4 4-4" />,
  folder: <path d="M2 12.5v-9h4.2l1.4 1.9H14v7.1z" />,
  folderPlus: <path d="M2 12.5v-9h4.2l1.4 1.9H14v7.1zM8 7.6v3.4M6.3 9.3h3.4" />,
  layers: <path d="M8 2.2l6 3-6 3-6-3zM2 8.2l6 3 6-3M2 11l6 3 6-3" />,
  album: (
    <>
      <rect x="2.5" y="3" width="11" height="10" rx="1.5" />
      <path d="M2.8 11l3-3 2.6 2.6 1.8-1.8 3 3" />
    </>
  ),
  sparkle: <path d="M8 2l1.5 4.5L14 8l-4.5 1.5L8 14l-1.5-4.5L2 8l4.5-1.5z" />,
  undo: <path d="M3 6h6.5a3.5 3.5 0 010 7H6M3 6l3-3M3 6l3 3" />,
  redo: <path d="M13 6H6.5a3.5 3.5 0 000 7H10M13 6l-3-3M13 6l-3 3" />,
  copy: (
    <>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
      <path d="M10.5 5.5v-2a1 1 0 00-1-1h-6a1 1 0 00-1 1v6a1 1 0 001 1h2" />
    </>
  ),
  pin: (
    <>
      <path d="M8 14s4.5-4 4.5-7.5a4.5 4.5 0 00-9 0C3.5 10 8 14 8 14z" />
      <circle cx="8" cy="6.5" r="1.5" />
    </>
  ),
  alert: <path d="M8 2.5l6 10.5H2zM8 6.5v3M8 11.3v.1" />,
  // 塗りの形（星・再生など）。fill は CSS（.icon.solid）で付ける。
  star: <path d="M8 1.6l1.95 4 4.4.6-3.2 3.1.78 4.4L8 11.6l-3.93 2.1.78-4.4-3.2-3.1 4.4-.6z" />,
  play: <path d="M5 3l8 5-8 5z" />,
  pause: <path d="M4.5 3h2.4v10H4.5zM9.1 3h2.4v10H9.1z" />,
  stepBack: <path d="M11.5 3.5L5.5 8l6 4.5zM4 3.5v9" />,
  stepForward: <path d="M4.5 3.5l6 4.5-6 4.5zM12 3.5v9" />,
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof PATHS;

/** 塗りで描く形。 */
const SOLID: ReadonlySet<IconName> = new Set<IconName>(["star", "play", "pause"]);

export function Icon(props: { name: IconName; size?: number; className?: string }) {
  const size = props.size;
  return (
    <svg
      className={`icon${SOLID.has(props.name) ? " solid" : ""}${props.className ? ` ${props.className}` : ""}`}
      viewBox="0 0 16 16"
      // CSS（.icon の width / height）より優先させるため、属性ではなく style で渡す。
      style={size ? { width: size, height: size } : undefined}
      aria-hidden="true"
      focusable="false"
    >
      {PATHS[props.name]}
    </svg>
  );
}
