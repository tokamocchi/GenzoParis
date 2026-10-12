// 取り込みのダイアログ（IMP-01・VID-01）: フォルダを選び、その場で登録する（元のファイルはコピー・移動しない）。
//
// ドロップで渡されたもの（Dialogs.tsx）は、アプリの path_kinds で種類を確かめ、フォルダではなかったもの・
// 見つからなかったものに注意を出す（名前からは推定しない。"2024.Kyoto" のようなフォルダもあるため）。
// 「参照…」で OS のフォルダの選択から選んだものはフォルダなので、確かめない。

import { useEffect, useState } from "react";

import * as appApi from "../api/app";
import { Dialog } from "../components/Dialog";
import { actions, useApp } from "../state/app";
import { baseName } from "./dialogFormat";
import { onImportFolders, takeImportFolders } from "./importDrop";
import { type PathKind, nonFolderPaths, videoToolsWarning } from "./importLogic";
import { chooseFolder } from "./native";
import { PathBox } from "./parts";
import { loadStored, saveStored } from "./storage";

const LAST_FOLDER_KEY = "import.lastFolder";

/**
 * 一覧のパスの種類を、アプリの path_kinds で確かめる（`chosen` は「参照…」で選んだフォルダなので確かめない）。
 * 確かめられなかったものは種類が分からないままにする（注意は出さない。ファイルなら取り込みが「フォルダでは
 * ありません」で失敗して知らせる）。
 */
function usePathKinds(folders: string[], chosen: string | null): ReadonlyMap<string, PathKind> {
  const [kinds, setKinds] = useState<ReadonlyMap<string, PathKind>>(() => new Map());
  useEffect(() => {
    const unchecked = folders.filter((f) => f !== chosen && !kinds.has(f));
    if (unchecked.length === 0) return;
    let alive = true;
    appApi
      .pathKinds(unchecked)
      .then((list) => {
        if (!alive) return;
        setKinds((prev) => {
          const next = new Map(prev);
          unchecked.forEach((p, i) => {
            const k = list[i];
            if (k) next.set(p, k);
          });
          return next;
        });
      })
      .catch((e) => console.warn("パスの種類を確かめられません", e));
    return () => {
      alive = false;
    };
  }, [folders, chosen, kinds]);
  return kinds;
}

export function ImportDialog() {
  const tools = useApp((s) => s.catalog?.tools ?? null);
  const [folders, setFolders] = useState<string[]>(() => takeImportFolders() ?? []);
  // 「参照…」で OS のフォルダの選択から選んだもの（フォルダと分かっている）。
  const [chosen, setChosen] = useState<string | null>(null);
  const [recursive, setRecursive] = useState(true);
  const [choosing, setChoosing] = useState(false);
  const kinds = usePathKinds(folders, chosen);

  useEffect(() => onImportFolders(setFolders), []);

  const choose = async () => {
    setChoosing(true);
    try {
      const last = loadStored(LAST_FOLDER_KEY);
      const start = folders[0] ?? (typeof last === "string" ? last : null);
      const p = await chooseFolder("取り込むフォルダを選ぶ", start);
      if (p) {
        setChosen(p);
        setFolders([p]);
      }
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    } finally {
      setChoosing(false);
    }
  };

  const start = () => {
    if (folders.length === 0) return;
    saveStored(LAST_FOLDER_KEY, folders[0]);
    actions.closeDialog();
    // 1 つずつ順に取り込む（途中でカタログを閉じた・取り込みが取り消されたら、残りはやめる）。
    void actions.importFolders(folders, recursive);
  };

  const toolsWarning = videoToolsWarning(tools);
  const notFolders = nonFolderPaths(folders, kinds);

  return (
    <Dialog
      title="フォルダを取り込む"
      width={560}
      onClose={actions.closeDialog}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog}>
            キャンセル
          </button>
          <button className="btn primary" disabled={folders.length === 0} onClick={start}>
            取り込む
          </button>
        </>
      }
    >
      <div className="dlg-section">
        <h3>{folders.length > 1 ? `フォルダ（${folders.length} 個）` : "フォルダ"}</h3>
        <div className="dlg-row">
          <div className="dlg-grow">
            {folders.length <= 1 ? (
              <PathBox path={folders[0]} empty="フォルダを選んでください（ウィンドウへドロップしても選べます）" box />
            ) : (
              <div className="dlg-list">
                {folders.map((f) => (
                  <div key={f} className="dlg-list-item">
                    <div className="dlg-grow">
                      <PathBox path={f} />
                    </div>
                    <button
                      className="btn small ghost"
                      onClick={() => setFolders((list) => list.filter((x) => x !== f))}
                      title="このフォルダを取り込みから除く"
                    >
                      除く
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
          <button className="btn" onClick={() => void choose()} disabled={choosing}>
            参照…
          </button>
        </div>
      </div>
      {(notFolders.files.length > 0 || notFolders.missing.length > 0) && (
        <div className="dlg-warn">
          {notFolders.files.length > 0 && (
            <div>次のものはフォルダではありません（ファイルです）: {notFolders.files.map(baseName).join("、")}</div>
          )}
          {notFolders.missing.length > 0 && (
            <div>次のものが見つかりません（または調べられません）: {notFolders.missing.map(baseName).join("、")}</div>
          )}
          <div className="hint">
            取り込みはフォルダ単位です（このままでは取り込めません）。一覧から除くか、「参照…」でフォルダを選び直してください。
          </div>
        </div>
      )}
      <label className="dlg-check">
        <input type="checkbox" checked={recursive} onChange={(e) => setRecursive(e.target.checked)} />
        <span>
          サブフォルダも含める
          <span className="desc">フォルダの中のフォルダもすべて取り込みます</span>
        </span>
      </label>
      <div className="dlg-note">
        元のファイルはコピー・移動せず、今の場所のままカタログに登録します（元のファイルは変更しません）。対象は
        ARW・DNG・JPEG・TIFF・PNG の写真と、MP4・MOV の動画です。同じフォルダをもう一度取り込んでも二重には登録せず、新しいファイルと変わったファイルだけを処理します。
      </div>
      {toolsWarning && (
        <div className="dlg-warn">
          {toolsWarning}
          <div className="hint">
            FFmpeg はアプリに同梱していません。FFmpeg を入れて（見つからなければ設定の「一般」でフォルダを指定して）、アプリを再起動すると使います。それまでに取り込んだ動画は、再起動の後に動画のフォルダを取り込み直すと、サムネイルと情報を作ります。
          </div>
          <div style={{ marginTop: 6 }}>
            <button className="btn small" onClick={() => actions.openDialog({ kind: "settings", tab: "general" })}>
              設定を開く
            </button>
          </div>
        </div>
      )}
    </Dialog>
  );
}
