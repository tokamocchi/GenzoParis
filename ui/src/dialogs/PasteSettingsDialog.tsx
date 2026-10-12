// 現像設定の貼り付け（一括適用。DEV-30）のダイアログ: コピーした設定のうち、選んだ項目を選んだ写真へ写す。
//
// 選んだ項目はブラウザに保存して次に使う。現像中の写真が貼り付け先に入っていれば、現像の送信の列に入れて
// 順序を保ち（送信待ちの値で上書きしない）、貼り付けた後にコアの状態を読み直す。

import { useMemo, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions, useApp } from "../state/app";
import { developStore, enqueue, reloadState } from "../state/develop";
import { formatCount } from "./dialogFormat";
import { loadedSummaries } from "./loadedItems";
import {
  type LoadedItem,
  PASTE_GROUP_ITEMS,
  type PasteSelection,
  anySelected,
  preparePasteTargets,
  restorePasteSelection,
  toSettingGroups,
} from "./pasteLogic";
import { ErrorBox } from "./parts";
import { loadStored, saveStored } from "./storage";

const STORE_KEY = "paste.groups";

/** 種類・名前の分かる写真（選んだ写真の概要、読み込み済みの検索結果の順）。 */
function* knownSummaries(): Generator<LoadedItem> {
  yield* actions.selectedSummaries();
  yield* loadedSummaries();
}

export function PasteSettingsDialog(props: { targets: VariantId[] }) {
  const clipboard = useApp((s) => s.clipboard);
  const [sel, setSel] = useState<PasteSelection>(() => restorePasteSelection(loadStored(STORE_KEY)));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  // 選んだ写真の概要（選んだときの項目）と読み込み済みの検索結果を 1 回だけ見て、動画とコピー元を調べる（開いた
  // ときの内容で決める）。検索結果のページは上限で捨てるので、選んだ写真の種類は選んだときの項目で確かめる。
  const prepared = useMemo(
    () => preparePasteTargets(props.targets, clipboard?.from ?? null, knownSummaries()),
    [props.targets, clipboard],
  );
  const source = prepared.source;
  const sourceName = clipboard ? (source?.file_name ?? `ID ${clipboard.from}`) : null;
  // 動画には現像設定がない（コピーしても既定の設定になる）ので、貼り付けない。
  const sourceIsVideo = source?.kind === "video";

  const toggle = (key: keyof PasteSelection, v: boolean) => {
    const next = { ...sel, [key]: v };
    setSel(next);
    saveStored(STORE_KEY, next);
  };

  const paste = async () => {
    if (!clipboard || sourceIsVideo || prepared.ids.length === 0 || !anySelected(sel)) return;
    setBusy(true);
    setError(null);
    const groups = toSettingGroups(sel);
    const ids = prepared.ids;
    const developing = developStore.get().variantId;
    const includesDeveloping = developing !== null && ids.includes(developing);
    try {
      let count = 0;
      let failure: unknown = null;
      // 失敗は列の外で扱う（列の中で投げると、現像の画面のエラーとして出てしまう）。
      const run = async () => {
        try {
          count = await core.pasteSettings(clipboard.settings, ids, groups);
        } catch (e) {
          failure = e;
        }
      };
      // 現像中の写真が含まれていれば、送信の列の順序を保つ（送信待ちの値が後から上書きしないように）。
      if (includesDeveloping) await enqueue(run);
      else await run();
      if (failure !== null) throw failure;
      if (includesDeveloping) await reloadState();
      actions.closeDialog();
      actions.notify("success", `${formatCount(count)} 枚に現像設定を貼り付けました`);
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  const notes: string[] = [];
  if (prepared.skippedSource) notes.push("コピー元の写真は除きます。");
  if (prepared.skippedVideos > 0) {
    notes.push(`動画 ${formatCount(prepared.skippedVideos)} 本は除きます（動画には現像設定を保存しません）。`);
  }

  return (
    <Dialog
      title={`現像設定を貼り付け（${formatCount(prepared.ids.length)} 枚）`}
      width={520}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button
            className="btn primary"
            onClick={() => void paste()}
            disabled={busy || !clipboard || sourceIsVideo || prepared.ids.length === 0 || !anySelected(sel)}
          >
            {busy ? "貼り付け中…" : "貼り付ける"}
          </button>
        </>
      }
    >
      {!clipboard ? (
        <div className="dlg-warn">先に現像設定をコピーしてください（写真を選んで Ctrl / ⌘ + Shift + C）。</div>
      ) : (
        <>
          <p className="muted" style={{ marginTop: 0 }}>
            コピー元: {sourceName}
          </p>
          {sourceIsVideo && (
            <div className="dlg-warn">
              コピー元は動画です。動画には現像設定がないため貼り付けられません。写真を選んで、もう一度コピーしてください。
            </div>
          )}
          <div className="dlg-section">
            <h3>写す項目</h3>
            {PASTE_GROUP_ITEMS.map((item) => (
              <label key={item.key} className="dlg-check">
                <input
                  type="checkbox"
                  checked={sel[item.key]}
                  disabled={busy}
                  onChange={(e) => toggle(item.key, e.target.checked)}
                />
                <span>
                  {item.label}
                  <span className="desc">{item.detail}</span>
                </span>
              </label>
            ))}
            {!anySelected(sel) && <div className="dlg-field-error">項目を 1 つ以上選んでください</div>}
          </div>
          {notes.length > 0 && <div className="dlg-note">{notes.join("\n")}</div>}
          {prepared.ids.length === 0 && <div className="dlg-warn">貼り付け先の写真がありません。</div>}
          <div className="dlg-note">
            それぞれの写真の履歴に 1 件記録します（現像の画面の履歴から元に戻せます）。元のファイルは変更しません。
          </div>
        </>
      )}
      <ErrorBox error={error} title="貼り付けられません" />
    </Dialog>
  );
}
