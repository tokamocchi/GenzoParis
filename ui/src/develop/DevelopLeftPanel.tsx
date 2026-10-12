// 現像の左のパネル（DEV-27・DEV-30・LIB-13）: 履歴、元に戻す・やり直す、設定のコピー・貼り付け・同期、
// 仮想コピー。
//
// - 履歴はコアの history（古い順）を新しい順に出し、今の位置（is_current）を強調する。developStore の
//   historyVersion（ドラッグの終了・Undo / Redo・保存のイベントで増える）で読み直す。
// - 項目をクリックすると、その位置まで Undo / Redo を繰り返して戻る。保存待ちの変更があると Undo の前に
//   保存されて履歴が 1 件増えるので、先に保存してから履歴を読み直して回数を決める（送信の列の中で行う）。

import { useEffect, useState } from "react";

import * as core from "../api/core";
import type { DevelopSettings, HistoryEntryId, HistoryItem, VariantId } from "../api/types";
import { comboLabel } from "../keyboard";
import { actions, appStore, useApp } from "../state/app";
import { developStore, enqueue, redo, undo, useDevelop } from "../state/develop";
import { shallowEqual } from "../state/store";
import { Section } from "../components/Section";
import { historyAvailability, historyRows, historySteps } from "./historyMath";
import "./develop.css";

/** 履歴の項目の位置まで戻る（進む）。 */
function goToHistory(variantId: VariantId, target: HistoryEntryId): Promise<void> {
  return enqueue(async () => {
    if (developStore.get().variantId !== variantId) return;
    let settings: DevelopSettings | null = null;
    try {
      // 保存待ちの変更を先に保存する（Undo の前の保存で履歴が増え、回数がずれないように）。
      await core.flushDevelop();
      const entries = await core.history(variantId);
      const steps = historySteps(entries, target);
      if (steps === null || steps === 0) return;
      for (let i = 0; i < Math.abs(steps); i++) {
        const s = steps < 0 ? await core.undo(variantId) : await core.redo(variantId);
        if (!s) break;
        settings = s;
      }
    } catch (e) {
      actions.reportError(e, "履歴の位置に戻せません");
    } finally {
      // 途中で失敗しても、動いたところまでの状態を画面に反映する。
      try {
        const state = await core.developState();
        developStore.set((cur) => {
          if (cur.variantId !== variantId) return {};
          const same = state && state.variant_id === variantId;
          return {
            settings: same ? state.settings : (settings ?? cur.settings),
            state: same ? state : cur.state,
            historyVersion: cur.historyVersion + 1,
          };
        });
      } catch (e) {
        actions.reportError(e, "現像の状態を読み直せません");
      }
    }
  });
}

/** 履歴の時刻（"10/12 14:05"。この PC の時刻）。 */
function formatTime(iso: string): string {
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(t.getMonth() + 1)}/${p(t.getDate())} ${p(t.getHours())}:${p(t.getMinutes())}`;
}

/** 現像中の写真の設定（画面の最新の値。送信前を含む）。 */
function currentSettings() {
  const d = developStore.get();
  return d.state && d.settings && d.variantId !== null ? { settings: d.settings, from: d.variantId } : null;
}

function copySettings(): void {
  const c = currentSettings();
  if (!c) return;
  actions.setClipboard(c);
  actions.notify("info", "現像設定をコピーしました");
}

function pasteSettings(): void {
  if (!appStore.get().clipboard) {
    actions.notify("info", `先に現像設定をコピーしてください（${comboLabel("mod+shift+c")}）`);
    return;
  }
  const targets = actions.targets();
  if (targets.length > 0) actions.openDialog({ kind: "paste_settings", targets });
}

/** 選んだ他の写真に、今の写真の設定を写す（コピーして貼り付けのダイアログを開く）。 */
function syncToSelection(ids: VariantId[]): void {
  const c = currentSettings();
  if (!c) return;
  const targets = ids.filter((id) => id !== c.from);
  if (targets.length === 0) return;
  actions.setClipboard(c);
  actions.openDialog({ kind: "paste_settings", targets });
}

async function createVirtualCopy(variantId: VariantId): Promise<void> {
  const result: { id: VariantId | null } = { id: null };
  // 送信待ちの設定を送ってから作る（コアは保存待ちの変更を保存してから写す）。
  await enqueue(async () => {
    try {
      result.id = await core.createVirtualCopy(variantId);
    } catch (e) {
      actions.reportError(e, "仮想コピーを作れません");
    }
  });
  const newId = result.id;
  if (newId === null) return;
  await actions.runSearch();
  try {
    const r = await core.indexOf(newId);
    if (r.index !== null) await actions.selectIndex(r.index);
    actions.notify(
      "success",
      r.index !== null ? "仮想コピーを作りました" : "仮想コピーを作りました（今の絞り込みでは表示されません）",
    );
  } catch (e) {
    actions.reportError(e, "仮想コピーを選べません");
  }
}

/** 履歴を読む（historyVersion が増えたら読み直す）。 */
function useHistory(variantId: VariantId | null): { entries: HistoryItem[] | null; error: string | null } {
  const historyVersion = useDevelop((s) => s.historyVersion);
  const [result, setResult] = useState<{ id: VariantId | null; entries: HistoryItem[] | null; error: string | null }>({
    id: null,
    entries: null,
    error: null,
  });
  useEffect(() => {
    if (variantId === null) return;
    let alive = true;
    core
      .history(variantId)
      .then((entries) => {
        if (alive) setResult({ id: variantId, entries, error: null });
      })
      .catch((e) => {
        if (alive) setResult({ id: variantId, entries: null, error: actions.describeError(e) });
      });
    return () => {
      alive = false;
    };
  }, [variantId, historyVersion]);
  // 別の写真の履歴は出さない。
  return result.id === variantId ? { entries: result.entries, error: result.error } : { entries: null, error: null };
}

function HistoryList(props: { variantId: VariantId; entries: HistoryItem[] | null; error: string | null }) {
  const { variantId, entries, error } = props;
  if (error) return <div className="error-text">{error}</div>;
  if (!entries) return <div className="muted">読み込んでいます…</div>;
  if (entries.length === 0) return <div className="muted">まだ履歴はありません</div>;
  return (
    <ul className="dl-history">
      {historyRows(entries).map(({ item, current, undone }) => (
        <li
          key={item.id}
          className={current ? "current" : undone ? "undone" : undefined}
          aria-current={current ? "step" : undefined}
          title={current ? "今の状態" : "クリックでこの状態に戻す"}
          onClick={() => {
            if (!current) void goToHistory(variantId, item.id);
          }}
        >
          <span>{item.label}</span>
          <span className="time">{formatTime(item.created_at)}</span>
        </li>
      ))}
    </ul>
  );
}

export function DevelopLeftPanel() {
  const current = useApp((s) => s.selection.current);
  const selected = useApp((s) => s.selection.ids, shallowEqual);
  const hasClipboard = useApp((s) => s.clipboard !== null);
  const variantId = useDevelop((s) => s.variantId);
  const state = useDevelop((s) => s.state);
  const opening = useDevelop((s) => s.opening);
  const ready = state !== null && !opening && variantId !== null && variantId === current;
  const others = selected.filter((id) => id !== current).length;
  const history = useHistory(ready ? variantId : null);
  // コアの状態の can_undo / can_redo はドラッグの終了・Undo / Redo のときにしか読み直さないので、
  // 保存のたびに読み直す履歴からも判断する。
  const avail = historyAvailability(history.entries);
  const canUndo = !!state?.can_undo || !!avail?.canUndo;
  const canRedo = avail ? avail.canRedo : !!state?.can_redo;

  return (
    <div>
      <div className="dl-buttons">
        <button
          className="btn small"
          disabled={!ready || !canUndo}
          onClick={() => void undo()}
          title={`元に戻す（${comboLabel("mod+z")}）`}
        >
          元に戻す
        </button>
        <button
          className="btn small"
          disabled={!ready || !canRedo}
          onClick={() => void redo()}
          title={`やり直す（${comboLabel("mod+shift+z")}）`}
        >
          やり直す
        </button>
        <button
          className="btn small"
          disabled={!ready}
          onClick={copySettings}
          title={`現像設定をコピーする（${comboLabel("mod+shift+c")}）`}
        >
          設定をコピー
        </button>
        <button
          className="btn small"
          disabled={!ready || !hasClipboard}
          onClick={pasteSettings}
          title={`コピーした現像設定を貼り付ける（${comboLabel("mod+shift+v")}）`}
        >
          貼り付け
        </button>
        {others > 0 && (
          <button
            className="btn small wide"
            disabled={!ready}
            onClick={() => syncToSelection(selected)}
            title="今の写真の現像設定を、選んだ他の写真に写す（写す項目はダイアログで選ぶ）"
          >
            選んだ {others.toLocaleString()} 枚に同期…
          </button>
        )}
        <button
          className="btn small wide"
          disabled={!ready || variantId === null}
          onClick={() => variantId !== null && void createVirtualCopy(variantId)}
          title="元のファイルを複製せずに、別の現像設定を持てる写しを作る"
        >
          仮想コピーを作る
        </button>
      </div>
      <Section id="develop.history" title="履歴">
        {ready && variantId !== null ? (
          <HistoryList variantId={variantId} entries={history.entries} error={history.error} />
        ) : (
          <div className="muted">—</div>
        )}
      </Section>
    </div>
  );
}
