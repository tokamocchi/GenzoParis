// 写真をドラッグしている間の案内（ポインターの横に枚数と落とす先を出す。library/photoDrag.ts）。

import { usePhotoDrag } from "./photoDrag";

export function PhotoDragGhost() {
  const drag = usePhotoDrag();
  if (!drag) return null;
  const n = drag.ids.length.toLocaleString("ja-JP");
  const t = drag.target;
  let text = `${n} 枚`;
  let cls = "photo-drag-ghost";
  if (t && t.ok) {
    if (t.kind === "collection") text = `${n} 枚を「${t.label}」に入れる`;
    else if (t.kind === "keyword") text = `${n} 枚にキーワード「${t.label}」を付ける`;
    else text = `${n} 枚を「${t.label}」へ移す（予定を確かめてから）`;
    cls += " ok";
  } else if (t) {
    text =
      t.kind === "folder"
        ? `「${t.label}」へは移せません（ドライブがつながっていないか、フォルダが見つかりません）`
        : `「${t.label}」には入れられません（写真を入れられるのは手動のコレクションだけです）`;
    cls += " no";
  }
  return (
    <div className={cls} style={{ left: drag.x + 14, top: drag.y + 10 }} aria-live="polite">
      {text}
    </div>
  );
}
