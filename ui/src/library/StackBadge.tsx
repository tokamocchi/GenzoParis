// スタックの印（グリッド・フィルムストリップのセル。LIB-12）。折りたたんだスタックは写真の数、展開した
// スタックは「何枚目 / 写真の数」を出し、クリックで折りたたみを切り替える（S と同じ。state/stacks.ts）。
// セルのクリック（選択）・ドラッグ・ダブルクリック（ルーペ）には渡さない。

import type { StackBadge, VariantId } from "../api/types";
import { toggleStack } from "../state/stacks";
import "./library.css";
import { stackBadgeLabel, stackBadgeTitle } from "./stackLogic";

export function StackBadgeButton(props: { id: VariantId; badge: StackBadge; wide: boolean; className?: string }) {
  const { badge } = props;
  const title = stackBadgeTitle(badge);
  return (
    <button
      type="button"
      className={`stack-badge${badge.collapsed ? " collapsed" : ""}${props.className ? ` ${props.className}` : ""}`}
      title={title}
      aria-label={title}
      aria-expanded={!badge.collapsed}
      onClick={(e) => {
        e.stopPropagation();
        void toggleStack(props.id);
      }}
      onPointerDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      {badge.collapsed && (
        <span className="stack-icon" aria-hidden="true">
          ❐
        </span>
      )}
      {stackBadgeLabel(badge, props.wide)}
    </button>
  );
}
