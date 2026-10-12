// バックアップの一覧と、選んだバックアップからの復元（SYS-04・DATA-04）。設定の「バックアップ」と、カタログを
// 開けないときの画面が使う。
//
// 復元の前に確認を出す: 今のカタログは名前を変えて同じフォルダに残す（削除しない。ファイルがなければ退避する
// ものはない）こと、バックアップの後の変更は含まれないこと。開いているカタログの保存待ちの現像設定を保存
// できずに閉じられなかったときは、「再試行」「保存せずに閉じてから復元」「やめる」を選ばせる（決定事項ログ
// No.61）。
//
// 一覧はキーボードでも選べる（↑↓・Home・End で選択を移し、Tab で一覧から出る。UX-02）。

import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import * as app from "../api/app";
import { toApiError } from "../api/errors";
import type { BackupEntry, CatalogFileState, ErrorInfo } from "../api/types";
import { actions } from "../state/app";
import { type RestoreResult, restoreDiscardingUnsaved, restoreFromBackup } from "./catalogActions";
import { restoreTargetNote } from "./catalogLogic";
import { baseName, formatCount, formatLocalDateTime, parentDir } from "./dialogFormat";
import { Busy, ErrorBox } from "./parts";

export function RestorePanel(props: {
  catalogPath: string;
  /** バックアップの一覧を読む（新しい順）。 */
  load: () => Promise<BackupEntry[]>;
  /** 変わると一覧を読み直す。 */
  reloadToken?: number;
  /** 一覧の上の説明。 */
  intro?: string;
}) {
  const { catalogPath, load, reloadToken } = props;
  const [backups, setBackups] = useState<BackupEntry[] | null>(null);
  const [loadError, setLoadError] = useState<ErrorInfo | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  /** 確認を出したときに調べた、今のカタログのファイルの状態（調べている間・分からなければ null）。 */
  const [target, setTarget] = useState<CatalogFileState | null>(null);
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  /** 保存待ちの現像設定を保存できずに閉じられなかった（選ばせる）。 */
  const [closeFailed, setCloseFailed] = useState(false);
  const mounted = useRef(true);
  const loadRef = useRef(load);
  loadRef.current = load;
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    let alive = true;
    setLoadError(null);
    loadRef
      .current()
      .then((list) => {
        if (!alive) return;
        setBackups(list);
        setSelected((cur) => (cur && list.some((b) => b.path === cur) ? cur : null));
      })
      .catch((e) => {
        if (alive) setLoadError(toApiError(e).info);
      });
    return () => {
      alive = false;
    };
  }, [catalogPath, reloadToken]);

  // 確認を出したら、今のカタログのファイルがあるかを調べる（なければ退避するものはない）。
  useEffect(() => {
    if (!confirming) return;
    let alive = true;
    setTarget(null);
    app
      .inspectCatalogPath(catalogPath)
      .then((s) => {
        if (alive) setTarget(s);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [confirming, catalogPath]);

  const chosen = backups?.find((b) => b.path === selected) ?? null;

  const select = (path: string) => {
    if (restoring) return;
    setSelected(path);
    setConfirming(false);
    setCloseFailed(false);
  };

  // ↑↓・Home・End で選択を移す（選んだ項目にフォーカスを移す）。
  const onListKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!backups || backups.length === 0) return;
    const cur = backups.findIndex((b) => b.path === selected);
    let next: number | null = null;
    if (e.key === "ArrowDown") next = cur < 0 ? 0 : Math.min(backups.length - 1, cur + 1);
    else if (e.key === "ArrowUp") next = cur < 0 ? 0 : Math.max(0, cur - 1);
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = backups.length - 1;
    if (next === null) return;
    e.preventDefault();
    select(backups[next].path);
    listRef.current?.querySelectorAll<HTMLButtonElement>("button[role=radio]")[next]?.focus();
  };

  /** 結果を知らせる（`discard` は、保存せずに閉じてから復元したか）。 */
  const finish = (r: RestoreResult, when: string) => {
    if (r.ok) {
      const detail =
        r.displaced.length > 0 ? `以前のカタログは次の名前で残しています:\n${r.displaced.join("\n")}` : undefined;
      if (r.opened) actions.notify("success", `バックアップ（${when}）から復元しました`, detail);
      else actions.notify("warning", `バックアップ（${when}）から復元しましたが、開けませんでした`, detail);
      // 開けなかったときは、この画面（カタログを開けないときの画面）が新しい理由で残るので、操作できるように戻す。
      if (mounted.current) {
        setRestoring(false);
        setConfirming(false);
        setCloseFailed(false);
      }
      return;
    }
    if (mounted.current) {
      setRestoring(false);
      setCloseFailed(r.closeFailed);
      setConfirming(r.closeFailed);
      setError(r.error);
    } else {
      // 画面が閉じた（カタログを閉じた・開き直した）ので、通知で知らせる。
      actions.notify("error", `復元できません: ${r.error.message}`, r.error.hint ?? undefined);
    }
  };

  const restore = async (discard: boolean) => {
    if (!chosen) return;
    setRestoring(true);
    setError(null);
    const when = formatLocalDateTime(chosen.created_at);
    const r = discard
      ? await restoreDiscardingUnsaved(catalogPath, chosen.path)
      : await restoreFromBackup(catalogPath, chosen.path);
    finish(r, when);
  };

  const focusIndex = Math.max(
    0,
    backups ? backups.findIndex((b) => b.path === selected) : 0,
  );

  return (
    <div>
      {props.intro && <p className="hint">{props.intro}</p>}
      {loadError && <ErrorBox error={loadError} title="バックアップの一覧を読めません" />}
      {!backups && !loadError && <Busy>バックアップを探しています…</Busy>}
      {backups && backups.length === 0 && <div className="dlg-note">バックアップが見つかりません。</div>}
      {backups && backups.length > 0 && (
        <>
          <div className="hint">{formatCount(backups.length)} 個（新しい順。日時は現地時刻。↑↓ で選べます）</div>
          <div className="dlg-list" role="radiogroup" aria-label="バックアップ" ref={listRef} onKeyDown={onListKey}>
            {backups.map((b, i) => (
              <button
                key={b.path}
                type="button"
                role="radio"
                aria-checked={b.path === selected}
                tabIndex={i === focusIndex ? 0 : -1}
                className={`dlg-list-item selectable${b.path === selected ? " selected" : ""}`}
                onClick={() => select(b.path)}
                title={b.path}
              >
                <span style={{ minWidth: 150 }}>{formatLocalDateTime(b.created_at)}</span>
                <span className="dlg-path muted">{baseName(b.path)}</span>
              </button>
            ))}
          </div>
          {!confirming && (
            <div className="dlg-row end">
              <button className="btn" disabled={!chosen || restoring} onClick={() => setConfirming(true)}>
                選んだバックアップから復元…
              </button>
            </div>
          )}
        </>
      )}
      {confirming && chosen && !closeFailed && (
        <div className="dlg-confirm">
          <p>
            <strong>{formatLocalDateTime(chosen.created_at)}</strong> のバックアップから復元します。
          </p>
          <ul className="hint" style={{ paddingLeft: 18, margin: "4px 0 8px" }}>
            <li>開いていれば、今のカタログを閉じます（保存待ちの現像設定は保存します）。</li>
            <li>{restoreTargetNote(catalogPath, target, parentDir(catalogPath) ?? "—")}</li>
            <li>バックアップの後に行った変更（評価・現像・取り込みなど）は、復元したカタログには含まれません。</li>
            <li>写真・動画の元のファイルは変更しません。</li>
          </ul>
          <div className="dlg-row end">
            <button className="btn" onClick={() => setConfirming(false)} disabled={restoring}>
              やめる
            </button>
            <button className="btn primary" onClick={() => void restore(false)} disabled={restoring}>
              {restoring ? "復元しています…" : "復元する"}
            </button>
          </div>
        </div>
      )}
      {confirming && chosen && closeFailed && (
        <div className="dlg-confirm">
          <p>
            今のカタログの保存待ちの現像設定を保存できないため、カタログを閉じられませんでした（復元はしていません）。
          </p>
          <p className="hint">
            空き容量・カタログのフォルダの権限を確かめて「再試行」を押すか、保存していない現像設定を捨てて閉じてから復元してください（捨てた設定は戻せません）。
          </p>
          <div className="dlg-row end">
            <button
              className="btn"
              onClick={() => {
                setCloseFailed(false);
                setConfirming(false);
                setError(null);
              }}
              disabled={restoring}
            >
              やめる
            </button>
            <button className="btn danger" onClick={() => void restore(true)} disabled={restoring}>
              保存せずに閉じてから復元
            </button>
            <button className="btn primary" onClick={() => void restore(false)} disabled={restoring}>
              {restoring ? "復元しています…" : "再試行"}
            </button>
          </div>
        </div>
      )}
      <ErrorBox error={error} title="復元できません" />
    </div>
  );
}
