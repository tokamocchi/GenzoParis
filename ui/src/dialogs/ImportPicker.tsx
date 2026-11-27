// 取り込む前の選択（IMP-02 の残り）: 取り込み元のフォルダの写真・動画をサムネイルで並べ、取り込むファイルを選ぶ
// （取り込みのダイアログの中に出す）。
//
// サムネイルは genzo:// の /import-thumb（コアが覚えている走査の結果の番号で指定する）。画面に入ったものから読む
// （loading="lazy"）。クリックで選ぶ・外す、Shift ＋ クリックで範囲をそろえる。

import { useRef } from "react";

import type { ImportSourceScan } from "../api/types";
import { importThumbUrl } from "../api/uri";
import {
  type PickSelection,
  describePick,
  pickNotes,
  selectAll,
  selectKind,
  setPickRange,
  sizeText,
  togglePick,
} from "./importPickLogic";

export function ImportPicker(props: {
  scan: ImportSourceScan;
  selection: PickSelection;
  onChange: (next: Set<string>) => void;
}) {
  const { scan, selection } = props;
  /** 最後にクリックした位置（Shift ＋ クリックの範囲の起点）。 */
  const anchor = useRef<number | null>(null);
  const hasVideos = scan.files.some((f) => f.kind === "video");
  const hasPhotos = scan.files.some((f) => f.kind === "photo");
  const notes = pickNotes(scan);

  const click = (index: number, shift: boolean) => {
    const file = scan.files[index];
    const on = !selection.has(file.rel_path);
    if (shift && anchor.current !== null) props.onChange(setPickRange(scan, selection, anchor.current, index, on));
    else props.onChange(togglePick(selection, file.rel_path));
    anchor.current = index;
  };

  return (
    <div className="dlg-pick">
      <div className="dlg-row">
        <span className="dlg-grow" aria-live="polite">
          {describePick(scan, selection)}
        </span>
        <button type="button" className="btn small" onClick={() => props.onChange(selectAll(scan))} disabled={scan.truncated}>
          すべて選ぶ
        </button>
        <button type="button" className="btn small" onClick={() => props.onChange(new Set())} disabled={scan.truncated}>
          すべて外す
        </button>
        {hasVideos && hasPhotos && (
          <>
            <button type="button" className="btn small" onClick={() => props.onChange(selectKind(scan, "photo"))} disabled={scan.truncated}>
              写真だけ
            </button>
            <button type="button" className="btn small" onClick={() => props.onChange(selectKind(scan, "video"))} disabled={scan.truncated}>
              動画だけ
            </button>
          </>
        )}
      </div>
      {notes.map((t) => (
        <div key={t} className="dlg-warn">
          {t}
        </div>
      ))}
      {scan.files.length === 0 ? (
        <p className="hint">取り込める写真・動画がありません</p>
      ) : (
        <div className="dlg-pick-grid" role="group" aria-label="取り込むファイル">
          {scan.files.map((f, i) => {
            const on = selection.has(f.rel_path);
            return (
              <label key={f.rel_path} className={`dlg-pick-item${on ? " on" : ""}`} title={`${f.rel_path}${f.size !== null ? `（${sizeText(f.size)}）` : ""}`}>
                <input
                  type="checkbox"
                  checked={on}
                  disabled={scan.truncated}
                  aria-label={f.rel_path}
                  onClick={(e) => {
                    // 選ぶ・外すは onClick で決める（Shift の有無を見るため）。
                    e.preventDefault();
                    click(i, e.shiftKey);
                  }}
                  onChange={() => undefined}
                />
                <img src={importThumbUrl(scan.scan_id, f.index)} alt="" loading="lazy" decoding="async" draggable={false} />
                <span className="name">
                  {f.kind === "video" ? "▶ " : ""}
                  {f.name}
                </span>
              </label>
            );
          })}
        </div>
      )}
      <p className="hint">クリックで選ぶ・外す、Shift ＋ クリックで範囲をそろえます。ここで外したファイルは取り込みません（元のファイルは変えません）。</p>
    </div>
  );
}
