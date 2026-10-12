// カタログを開けないときの画面（appStore.catalogError）。理由（error.kind）ごとに案内する。
//
// - 壊れている（catalog_corrupt）: バックアップの一覧から選んで復元する（今のカタログは名前を変えて残す）。
// - 他のアプリが使っている（conflict）: そのアプリを終了してから再試行。
// - 版が合わない（incompatible_version）: アプリの更新を案内する（復元は案内しない）。
// - 見つからない（not_found）: ドライブの接続を確かめて再試行するか、別のカタログを選ぶ。バックアップからの
//   復元もできる。
// どの場合も「再試行」「別のカタログを開く」「新しいカタログを作る」「最初の画面に戻る」を出す。
// 「再試行」は、作るつもりで失敗した（フォルダを作れないなど）ときは、作る（catalogActions.ts の
// retryOpenCatalog）。

import { useCallback, useState } from "react";

import * as app from "../api/app";
import { useApp } from "../state/app";
import { clearCatalogError, openCatalogAt, retryOpenCatalog } from "./catalogActions";
import { catalogErrorGuide } from "./catalogLogic";
import { type CatalogPick, pickExistingCatalog } from "./catalogPick";
import { parentDir } from "./dialogFormat";
import "./dialogs.css";
import { useNewCatalogPicker } from "./NewCatalogPicker";
import { Busy, ErrorBox, PathBox } from "./parts";
import { RestorePanel } from "./RestorePanel";

export function CatalogErrorScreen() {
  const catalogError = useApp((s) => s.catalogError);
  const version = useApp((s) => s.status?.version ?? null);
  const [busy, setBusy] = useState(false);
  const [pickError, setPickError] = useState<string | null>(null);
  const [showRestore, setShowRestore] = useState(false);
  const path = catalogError?.path ?? "";
  const load = useCallback(() => app.listCatalogBackups(path), [path]);
  const dir = parentDir(path);

  /** 開く（`create` が "retry" なら、前回と同じ開き方で再試行する）。 */
  const open = async (p: string, create: boolean | "retry") => {
    setBusy(true);
    setPickError(null);
    // 失敗したら catalogError が新しい理由に変わる。
    if (create === "retry") await retryOpenCatalog(p);
    else await openCatalogAt(p, create);
    setBusy(false);
  };

  const handlePick = async (pick: CatalogPick) => {
    if (pick.kind === "cancel" || pick.kind === "no_save_dialog") return;
    if (pick.kind === "error") {
      setPickError(pick.message);
      return;
    }
    await open(pick.path, pick.create);
  };
  const newPicker = useNewCatalogPicker(dir, handlePick, busy);

  if (!catalogError) return null;
  const { error } = catalogError;
  const guide = catalogErrorGuide(error);

  return (
    <div className="startup-screen">
      <div className="startup-card">
        <h1>{guide.title}</h1>
        <PathBox path={path} />
        <ErrorBox error={error} />
        {guide.guidance.map((g) => (
          <p key={g}>{g}</p>
        ))}
        {error.kind === "incompatible_version" && version && <p className="hint">このアプリの版: {version}</p>}

        <div className="dlg-row" style={{ marginTop: 12 }}>
          <button className="btn primary" disabled={busy} onClick={() => void open(path, "retry")}>
            再試行
          </button>
          <button className="btn" disabled={busy} onClick={() => void pickExistingCatalog(dir).then(handlePick)}>
            別のカタログを開く…
          </button>
          <button className="btn" disabled={busy} onClick={newPicker.start}>
            新しいカタログを作る…
          </button>
          <button className="btn ghost" disabled={busy} onClick={clearCatalogError}>
            最初の画面に戻る
          </button>
        </div>
        {newPicker.form}
        {busy && (
          <p>
            <Busy>開いています…</Busy>
          </p>
        )}
        {pickError && <div className="dlg-error">{pickError}</div>}

        {guide.restore !== "none" && (
          <>
            <h2>バックアップから復元</h2>
            {guide.restore === "open" || showRestore ? (
              <RestorePanel
                key={path}
                catalogPath={path}
                load={load}
                intro="このカタログのバックアップ（データのフォルダの backups にあるもの）です。"
              />
            ) : (
              <button className="btn small" onClick={() => setShowRestore(true)}>
                バックアップの一覧を見る
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
