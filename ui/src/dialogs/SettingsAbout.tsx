// 設定の「このアプリについて」（DIST-05）: アプリの版とライセンス、同梱したライセンスの文書（アプリ自体・
// Rust の crate・画面の npm のパッケージ・同梱した C / C++ のライブラリ・LibRaw）を選んで読む。フォルダを OS の
// ファイル管理で開ける。LibRaw を同梱した配布物では、LibRaw の著作権表示とライセンスの案内を出す。
//
// 文書の一覧は、このタブを初めて開いたときに読む（設定を開くたびに読まない）。本文は選んだものだけを読む
// （Rust の crate の文書は約 0.5 MB）。

import { useEffect, useState } from "react";

import * as app from "../api/app";
import { toApiError } from "../api/errors";
import type { ErrorInfo, LicenseDocuments } from "../api/types";
import { useApp } from "../state/app";
import {
  APP_LICENSE_NOTE,
  LIBRAW_CREDIT,
  SOURCE_URL,
  hasLibrawDocuments,
  licenseOptionLabel,
  noLicenseDocumentsMessage,
  orderLicenseDocuments,
} from "./licensesLogic";
import { Busy, ErrorBox } from "./parts";

export function SettingsAbout(props: { active: boolean }) {
  const status = useApp((s) => s.status);
  const [docs, setDocs] = useState<LicenseDocuments | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const ordered = docs ? orderLicenseDocuments(docs.documents) : [];

  useEffect(() => {
    if (!props.active || docs !== null) return;
    let alive = true;
    app
      .licenseDocuments()
      .then((d) => {
        if (!alive) return;
        setDocs(d);
        setSelected(orderLicenseDocuments(d.documents)[0]?.path ?? null);
      })
      .catch((e) => {
        if (alive) setError(toApiError(e).info);
      });
    return () => {
      alive = false;
    };
  }, [props.active, docs]);

  useEffect(() => {
    if (selected === null) return;
    let alive = true;
    setLoading(true);
    setText(null);
    setError(null);
    app
      .readLicenseDocument(selected)
      .then((t) => {
        if (alive) setText(t);
      })
      .catch((e) => {
        if (alive) setError(toApiError(e).info);
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [selected]);

  const openFolder = async () => {
    setError(null);
    try {
      await app.openLicenseFolder();
    } catch (e) {
      setError(toApiError(e).info);
    }
  };

  return (
    <div>
      <div className="dlg-section">
        <h3>GenzoParis</h3>
        {status && (
          <p style={{ marginTop: 0 }}>
            版 {status.version}（{status.platform}）
          </p>
        )}
        <p className="hint">
          {APP_LICENSE_NOTE}ソースコード: <span className="dlg-path">{SOURCE_URL}</span>
        </p>
        {docs && hasLibrawDocuments(docs.documents) && <p className="hint">{LIBRAW_CREDIT}</p>}
      </div>
      <div className="dlg-section">
        <h3>ライセンス</h3>
        <p className="hint" style={{ marginTop: 0 }}>
          GenzoParis と、GenzoParis が使っている第三者のソフトウェア（ライブラリ）のライセンスの文書です。
        </p>
        {docs && ordered.length === 0 && <div className="dlg-note">{noLicenseDocumentsMessage(docs.dir)}</div>}
        <div className="dlg-row">
          <select
            className="dlg-grow"
            aria-label="ライセンスの文書"
            value={selected ?? ""}
            onChange={(e) => setSelected(e.target.value)}
            disabled={ordered.length === 0}
          >
            {ordered.map((d) => (
              <option key={d.path} value={d.path}>
                {licenseOptionLabel(d)}
              </option>
            ))}
          </select>
          <button className="btn" onClick={() => void openFolder()} disabled={!docs?.dir}>
            フォルダを開く
          </button>
        </div>
        {loading && <Busy>読み込んでいます…</Busy>}
        {text !== null && <pre className="dlg-pre license-text">{text}</pre>}
        <ErrorBox error={error} title="ライセンスの文書を読めません" />
      </div>
    </div>
  );
}
