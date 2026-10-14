// 仮想コピー（LIB-13）の名前の変更と、撮影時刻の間隔による自動スタック（LIB-12）のダイアログ。
//
// - 名前の規則（空でない・200 文字まで・制御文字なし）は画面でも確かめる（library/stackLogic.ts の
//   validateCopyName。コアも同じ規則で確かめ、エラーはダイアログの中に出す）。
// - 自動スタックは、表示中の条件（appStore.filter。フォルダ・コレクション・絞り込み）に合う写真のうち、
//   スタックに入っていないものが対象。間隔を変えるたびに、作るスタックの数の見込み（dry_run）を出す。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { AutoStackReport, ErrorInfo, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import {
  DEFAULT_AUTO_STACK_GAP_SECONDS,
  MAX_AUTO_STACK_GAP_SECONDS,
  describeAutoStack,
  parseGapSeconds,
  validateCopyName,
} from "../library/stackLogic";
import { actions, appStore } from "../state/app";
import { renameVirtualCopy } from "../state/stacks";
import { Busy, ErrorBox } from "./parts";

/** 見込みを数えるまでの時間（ms。入力の途中で数えすぎない。仮置き）。 */
const PREVIEW_DELAY_MS = 300;

export function VirtualCopyNameDialog(props: { variantId: VariantId }) {
  const [name, setName] = useState<string | null>(null);
  const [current, setCurrent] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  useEffect(() => {
    let alive = true;
    core
      .variantDetails(props.variantId)
      .then((d) => alive && setCurrent(d.summary.variant_name ?? ""))
      .catch((e: unknown) => alive && setLoadError(actions.describeError(e)));
    return () => {
      alive = false;
    };
  }, [props.variantId]);
  const value = name ?? current ?? "";
  const problem = validateCopyName(value);
  const disabled = current === null || problem !== null || value.trim() === current;
  const ok = async () => {
    setBusy(true);
    setError(null);
    try {
      await renameVirtualCopy(props.variantId, value);
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title="仮想コピーの名前を変える"
      width={440}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || disabled}>
            変える
          </button>
        </>
      }
    >
      {loadError && <p className="error-text">仮想コピーを読めません: {loadError}</p>}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled && !busy) void ok();
        }}
      >
        <div className="field">
          <label htmlFor="vc-name">名前</label>
          <input id="vc-name" autoFocus value={value} onChange={(e) => setName(e.target.value)} disabled={current === null} />
        </div>
        {value.trim() !== "" && problem && <p className="hint">{problem}</p>}
        <p className="hint">書き出すファイルの名前は「元のファイル名-仮想コピーの名前」になります。</p>
      </form>
      <ErrorBox error={error} />
    </Dialog>
  );
}

export function AutoStackDialog() {
  const [text, setText] = useState(String(DEFAULT_AUTO_STACK_GAP_SECONDS));
  const [preview, setPreview] = useState<AutoStackReport | null>(null);
  const [counting, setCounting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const seq = useRef(0);
  const gap = parseGapSeconds(text);
  // 開いたときの条件で数える（ダイアログを開いている間は条件を変えられない）。
  const [filter] = useState(() => appStore.get().filter);

  useEffect(() => {
    const my = ++seq.current;
    setPreview(null);
    setError(null);
    if (gap === null) {
      setCounting(false);
      return;
    }
    setCounting(true);
    const t = setTimeout(() => {
      core
        .autoStack(filter, gap, true)
        .then((r) => {
          if (my === seq.current) setPreview(r);
        })
        .catch((e: unknown) => {
          if (my === seq.current) setError(toApiError(e).info);
        })
        .finally(() => {
          if (my === seq.current) setCounting(false);
        });
    }, PREVIEW_DELAY_MS);
    return () => clearTimeout(t);
  }, [gap, filter]);

  const run = async () => {
    if (gap === null) return;
    setBusy(true);
    setError(null);
    try {
      const r = await core.autoStack(filter, gap, false);
      actions.closeDialog();
      actions.notify(r.stacks > 0 ? "success" : "info", describeAutoStack(r)[0]);
      await actions.runSearch();
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };
  const disabled = gap === null || !preview || preview.stacks === 0;
  return (
    <Dialog
      title="撮影時刻でスタック"
      width={480}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void run()} disabled={busy || disabled}>
            {busy ? "作っています…" : "スタックを作る"}
          </button>
        </>
      }
    >
      <p style={{ marginTop: 0 }}>
        表示中の写真（今のフォルダ・コレクション・絞り込みの条件に合うもの）のうち、スタックに入っていないものを撮影日時の順に並べ、前の写真との間隔が指定の時間以下のものを 1 つのスタックにまとめます（2 枚以上のものだけ）。作ったスタックは折りたたみ、一番上は最初に撮った写真です。
      </p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled && !busy) void run();
        }}
      >
        <div className="field">
          <label htmlFor="as-gap">間隔（秒）</label>
          <input
            id="as-gap"
            autoFocus
            inputMode="decimal"
            value={text}
            onChange={(e) => setText(e.target.value)}
            aria-invalid={gap === null}
          />
        </div>
        {gap === null && <p className="hint">0〜{MAX_AUTO_STACK_GAP_SECONDS.toLocaleString("ja-JP")} 秒の数を入れてください（小数も使えます）。</p>}
      </form>
      <div className="dlg-section">
        {counting && <Busy>見込みを数えています…</Busy>}
        {preview && !counting && (
          <div className="dlg-note">
            {describeAutoStack(preview).map((l) => (
              <div key={l}>{l}</div>
            ))}
          </div>
        )}
      </div>
      <ErrorBox error={error} />
    </Dialog>
  );
}
