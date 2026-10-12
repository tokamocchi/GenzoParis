// カタログの選択の画面（カタログを開いていないとき。初回の起動など）。決定事項ログ No.63。
//
// - 既定の場所（ピクチャの GenzoParis/catalog.db）に作って開く（あれば開く）。Windows で既定の場所が
//   OneDrive で同期されるフォルダの中なら、注意を出す（oneDriveNotice）。
// - 別の場所に作る（保存のダイアログ。使えなければフォルダと名前で選ぶ。カタログでないファイルは上書き
//   しない）。
// - 既存のカタログを開く。最近開いたカタログを開く。
//
// 開けなければ、カタログを開けないときの画面（CatalogErrorScreen）になる。

import { useEffect, useState } from "react";

import * as app from "../api/app";
import type { CatalogFileState } from "../api/types";
import { actions, appStore, useApp } from "../state/app";
import { openCatalogAt } from "./catalogActions";
import { type CatalogPick, pickExistingCatalog } from "./catalogPick";
import { baseName, parentDir } from "./dialogFormat";
import "./dialogs.css";
import { useNewCatalogPicker } from "./NewCatalogPicker";
import { Busy } from "./parts";

/**
 * 既定の場所が OneDrive で同期されるフォルダの中のときの注意（Windows の「PC フォルダーのバックアップ」で
 * ピクチャが OneDrive の下に移った場合。アプリの app_status の default_catalog_in_onedrive）。既定の場所
 * （決定事項ログ No.63）を変えるかは人が決めるので、ここでは注意を出すだけにする。20 GB は標準プレビューの
 * キャッシュの上限の既定値（genzo-catalog の DEFAULT_PREVIEW_CAPACITY_BYTES）。`exists` なら、既にカタログが
 * ある（作る場所の案内はしない）。
 */
export function oneDriveNotice(exists: boolean): string {
  const base =
    "この場所は OneDrive で同期されるフォルダの中です。カタログとプレビューのキャッシュ（既定で最大 20 GB）が同期の対象になります。";
  return exists ? base : `${base}同期しない場所に作ることをおすすめします（「別の場所に新しいカタログを作る」）。`;
}

export function WelcomeScreen() {
  const status = useApp((s) => s.status);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [states, setStates] = useState<Record<string, CatalogFileState | "unknown">>({});

  const defaultPath = status?.default_catalog ?? null;
  const recent = status?.settings.recent_catalogs ?? [];

  // 最近開いたカタログの一覧を新しくする（別のカタログを開いた後に戻ってきたとき）。
  useEffect(() => {
    app
      .appStatus()
      .then((s) => {
        if (!appStore.get().catalog && !s.catalog) actions.setStatus(s);
      })
      .catch(() => undefined);
  }, []);

  // 既定の場所と最近のカタログが、あるか・カタログかを確かめる。
  const pathsKey = [defaultPath ?? "", ...recent].join("\n");
  useEffect(() => {
    let alive = true;
    const paths = pathsKey.split("\n").filter(Boolean);
    void Promise.all(
      paths.map(async (p) => [p, await app.inspectCatalogPath(p).catch(() => "unknown" as const)] as const),
    ).then((entries) => {
      if (alive) setStates(Object.fromEntries(entries));
    });
    return () => {
      alive = false;
    };
  }, [pathsKey]);

  const open = async (path: string, create: boolean) => {
    setBusy(path);
    setError(null);
    // 失敗したら appStore.catalogError が入り、開けないときの画面に変わる。
    await openCatalogAt(path, create);
    setBusy(null);
  };

  const handlePick = async (pick: CatalogPick) => {
    if (pick.kind === "cancel" || pick.kind === "no_save_dialog") return;
    if (pick.kind === "error") {
      setError(pick.message);
      return;
    }
    await open(pick.path, pick.create);
  };

  const defaultState = defaultPath ? states[defaultPath] : undefined;
  const startDir = defaultPath ? parentDir(defaultPath) : null;
  const newPicker = useNewCatalogPicker(startDir, handlePick, busy !== null);

  return (
    <div className="startup-screen">
      <div className="startup-card">
        <h1>GenzoParis</h1>
        <p className="muted">写真の現像とカタログの管理</p>

        <h2>カタログを選んでください</h2>
        <p className="hint">
          カタログは、写真・動画の登録・評価・現像設定をまとめて保存する 1 つのファイル（.db）です。写真のファイルはコピー・移動しません。サムネイルとバックアップは、カタログの隣の「（カタログの名前）-data」フォルダに置きます。
        </p>

        <div className="startup-actions">
          {defaultPath && (
            <div className="startup-action">
              <strong>{defaultState === "catalog" ? "既定の場所のカタログを開く" : "既定の場所に作って開く"}</strong>
              <span className="desc">
                <span className="dlg-path">{defaultPath}</span>
                {defaultState === "not_catalog" && (
                  <span className="warn-text">
                    {"\n"}この場所にはカタログでないファイル（またはフォルダ）があるため使えません。別の場所に作ってください。
                  </span>
                )}
                {status?.default_catalog_in_onedrive && defaultState !== "not_catalog" && (
                  <span className="warn-text">{"\n" + oneDriveNotice(defaultState === "catalog")}</span>
                )}
              </span>
              <button
                className="btn primary"
                disabled={busy !== null || defaultState === "not_catalog" || defaultState === undefined}
                onClick={() => void open(defaultPath, defaultState !== "catalog")}
              >
                {defaultState === "catalog" ? "開く" : "作って開く"}
              </button>
            </div>
          )}
          <div className="startup-action">
            <strong>別の場所に新しいカタログを作る</strong>
            <span className="desc">外付けのドライブなど、好きな場所に作れます</span>
            <button className="btn" disabled={busy !== null} onClick={newPicker.start}>
              場所を選ぶ…
            </button>
          </div>
          {newPicker.form}
          <div className="startup-action">
            <strong>既存のカタログを開く</strong>
            <span className="desc">前に作ったカタログ（.db）を選びます（コマンドラインの genzo で作ったものも開けます）</span>
            <button className="btn" disabled={busy !== null} onClick={() => void pickExistingCatalog(startDir).then(handlePick)}>
              選ぶ…
            </button>
          </div>
        </div>

        {busy && (
          <p>
            <Busy>{baseName(busy)} を開いています…（初めて開くときや大きなカタログでは時間がかかります）</Busy>
          </p>
        )}
        {error && <div className="dlg-error">{error}</div>}

        {recent.length > 0 && (
          <>
            <h2>最近開いたカタログ</h2>
            <div className="recent-list">
              {recent.map((p) => {
                const st = states[p];
                return (
                  <button key={p} className="recent-item" disabled={busy !== null} onClick={() => void open(p, false)} title={p}>
                    <span className="dlg-grow">
                      <span className="name">{baseName(p)}</span>
                      {st === "missing" && <span className="warn-text">（見つかりません）</span>}
                      {st === "not_catalog" && <span className="warn-text">（カタログではありません）</span>}
                      <br />
                      <span className="path">{p}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          </>
        )}

        <div className="dlg-row" style={{ marginTop: 18 }}>
          <button className="btn ghost small" onClick={() => actions.openDialog({ kind: "settings", tab: "general" })}>
            設定（FFmpeg・GPU）…
          </button>
          {status && <span className="hint">版 {status.version}</span>}
        </div>
      </div>
    </div>
  );
}
