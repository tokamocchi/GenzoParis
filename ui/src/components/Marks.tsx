// 評価（★）・フラグ・カラーラベルの表示。

import type { ColorLabel, Flag, Rating } from "../api/types";
import { COLOR_LABEL_NAMES } from "../format";

export function Stars(props: { rating: Rating; onSet?: (r: Rating) => void; size?: number }) {
  return (
    <span className="stars" style={props.size ? { fontSize: props.size } : undefined} aria-label={`評価 ${props.rating}`}>
      {[1, 2, 3, 4, 5].map((i) => (
        <span
          key={i}
          className={i <= props.rating ? "on" : undefined}
          style={props.onSet ? { cursor: "pointer" } : undefined}
          onClick={
            props.onSet
              ? (e) => {
                  e.stopPropagation();
                  props.onSet!((props.rating === i ? 0 : i) as Rating);
                }
              : undefined
          }
        >
          ★
        </span>
      ))}
    </span>
  );
}

export function FlagMark(props: { flag: Flag }) {
  if (props.flag === 1) return <span className="flag-picked" title="採用">⚑</span>;
  if (props.flag === -1) return <span className="flag-rejected" title="不採用">✕</span>;
  return null;
}

export function LabelDot(props: { label: ColorLabel | null }) {
  if (!props.label) return null;
  return <span className={`label-dot ${props.label}`} title={`ラベル: ${COLOR_LABEL_NAMES[props.label]}`} />;
}
