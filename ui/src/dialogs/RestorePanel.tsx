// バックアップの一覧と、選んだバックアップからの復元（SYS-04・DATA-04）。設定の「バックアップ」と、カタログを
// 開けないときの画面が使う。
//
// 復元の前に確認を出す: 今のカタログは名前を変えて同じフォルダに残す（削除しない）こと、バックアップの後の
// 変更は含まれないこと。

import { useEffect, useRef, useState } from "react";

import { toApiError } from "../api/errors";
import type { BackupEntry, ErrorInfo } from "../api/types";
import { actions } from "../state/app";
import { restoreFromBackup } from "./catalogActions";
import { restoreDisplacedName } from "./catalogLogic";
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
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const mounted = useRef(true);
  const loadRef = useRef(load);
  loadRef.current = load;

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

  const chosen = backups?.find((b) => b.path === selected) ?? null;

  const restore = async () => {
    if (!chosen) return;
    setRestoring(true);
    setError(null);
    const when = formatLocalDateTime(chosen.created_at);
    const r = await restoreFromBackup(catalogPath, chosen.path);
    if (r.ok) {
      const detail =
        r.displaced.length > 0 ? `以前のカタログは次の名前で残しています:\n${r.displaced.join("\n")}` : undefined;
      if (r.opened) actions.notify("success", `バックアップ（${when}）から復元しました`, detail);
      else actions.notify("warning", `バックアップ（${when}）から復元しましたが、開けませんでした`, detail);
      // 開けなかったときは、この画面（カタログを開けないときの画面）が新しい理由で残るので、操作できるように戻す。
      if (mounted.current) {
        setRestoring(false);
        setConfirming(false);
      }
      return;
    }
    if (mounted.current) {
      setRestoring(false);
      setConfirming(false);
      setError(r.error);
    } else {
      // 画面が閉じた（カタログを開き直した）ので、通知で知らせる。
      actions.notify("error", `復元できません: ${r.error.message}`, r.error.hint ?? undefined);
    }
  };

  return (
    <div>
      {props.intro && <p className="hint">{props.intro}</p>}
      {loadError && <ErrorBox error={loadError} title="バックアップの一覧を読めません" />}
      {!backups && !loadError && <Busy>バックアップを探しています…</Busy>}
      {backups && backups.length === 0 && <div className="dlg-note">バックアップが見つかりません。</div>}
      {backups && backups.length > 0 && (
        <>
          <div className="hint">{formatCount(backups.length)} 個（新しい順。日時は現地時刻）</div>
          <div className="dlg-list" role="listbox" aria-label="バックアップ">
            {backups.map((b) => (
              <div
                key={b.path}
                role="option"
                aria-selected={b.path === selected}
                className={`dlg-list-item selectable${b.path === selected ? " selected" : ""}`}
                onClick={() => {
                  if (restoring) return;
                  setSelected(b.path);
                  setConfirming(false);
                }}
                title={b.path}
              >
                <span style={{ minWidth: 150 }}>{formatLocalDateTime(b.created_at)}</span>
                <span className="dlg-path muted">{baseName(b.path)}</span>
              </div>
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
      {confirming && chosen && (
        <div className="dlg-confirm">
          <p>
            <strong>{formatLocalDateTime(chosen.created_at)}</strong> のバックアップから復元します。
          </p>
          <ul className="hint" style={{ paddingLeft: 18, margin: "4px 0 8px" }}>
            <li>開いていれば、今のカタログを閉じます（保存待ちの現像設定は保存します）。</li>
            <li>
              今のカタログは「{restoreDisplacedName(catalogPath)}」の名前で同じフォルダ（{parentDir(catalogPath) ?? "—"}）に残します（削除しません）。
            </li>
            <li>バックアップの後に行った変更（評価・現像・取り込みなど）は、復元したカタログには含まれません。</li>
            <li>写真・動画の元のファイルは変更しません。</li>
          </ul>
          <div className="dlg-row end">
            <button className="btn" onClick={() => setConfirming(false)} disabled={restoring}>
              やめる
            </button>
            <button className="btn primary" onClick={() => void restore()} disabled={restoring}>
              {restoring ? "復元しています…" : "復元する"}
            </button>
          </div>
        </div>
      )}
      <ErrorBox error={error} title="復元できません" />
    </div>
  );
}
