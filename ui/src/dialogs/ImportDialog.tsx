// 取り込みのダイアログ（IMP-01・VID-01）: フォルダを選び、その場で登録する（元のファイルはコピー・移動しない）。

import { useEffect, useState } from "react";

import { Dialog } from "../components/Dialog";
import { actions, useApp } from "../state/app";
import { onImportFolders, takeImportFolders } from "./importDrop";
import { videoToolsWarning } from "./importLogic";
import { chooseFolder } from "./native";
import { PathBox } from "./parts";
import { loadStored, saveStored } from "./storage";

const LAST_FOLDER_KEY = "import.lastFolder";

/** フォルダを順に取り込む（1 つずつ。結果は actions.importFolder が知らせる）。 */
async function importAll(folders: string[], recursive: boolean): Promise<void> {
  for (const f of folders) {
    await actions.importFolder(f, recursive);
  }
}

export function ImportDialog() {
  const tools = useApp((s) => s.catalog?.tools ?? null);
  const [folders, setFolders] = useState<string[]>(() => takeImportFolders() ?? []);
  const [recursive, setRecursive] = useState(true);
  const [choosing, setChoosing] = useState(false);

  useEffect(() => onImportFolders(setFolders), []);

  const choose = async () => {
    setChoosing(true);
    try {
      const last = loadStored(LAST_FOLDER_KEY);
      const start = folders[0] ?? (typeof last === "string" ? last : null);
      const p = await chooseFolder("取り込むフォルダを選ぶ", start);
      if (p) setFolders([p]);
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
    void importAll(folders, recursive);
  };

  const toolsWarning = videoToolsWarning(tools);

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
                    <PathBox path={f} />
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
            FFmpeg はアプリに同梱していません。FFmpeg を入れて、見つからなければ設定の「一般」でフォルダを指定してください（次にカタログを開いたときから使います）。
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
