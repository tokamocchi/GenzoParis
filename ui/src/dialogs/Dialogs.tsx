// ダイアログの切り替え（appStore.dialog の kind ごとに出す。同時に 1 つ）と、ウィンドウへのフォルダの
// ドラッグ＆ドロップでの取り込み（カタログを開いている間だけ。取り込みのダイアログで確かめてから取り込む）。

import { useEffect, useState } from "react";

import * as appApi from "../api/app";
import { actions, appStore, useApp } from "../state/app";
import { CaptureTimeDialog } from "./CaptureTimeDialog";
import { CloseFailedDialog } from "./CloseFailedDialog";
import { CompositeDialog } from "./CompositeDialog";
import { CrashReportsDialog } from "./CrashReportsDialog";
import { DeleteDialog } from "./DeleteDialog";
import "./dialogs.css";
import { ExportDialog } from "./ExportDialog";
import {
  FolderNameDialog,
  MoveFilesDialog,
  MoveFolderDialog,
  RelinkDialog,
  RelocateFolderDialog,
  RemoveFolderDialog,
  RenameFilesDialog,
} from "./FileDialogs";
import { ImportDialog } from "./ImportDialog";
import { offerImportFolders } from "./importDrop";
import { splitDroppedPaths } from "./importLogic";
import { listenDragDrop } from "./native";
import {
  AddToCollectionDialog,
  CollectionDeleteDialog,
  CollectionDialog,
  KeywordDeleteDialog,
  KeywordDialog,
} from "./OrganizeDialogs";
import { PasteSettingsDialog, UpgradeProcessDialog } from "./PasteSettingsDialog";
import {
  ApplyPresetDialog,
  DevelopPresetDialog,
  PresetDeleteDialog,
  PresetNameDialog,
  SnapshotNameDialog,
} from "./PresetDialogs";
import { ProfileBrowserDialog } from "./ProfileBrowserDialog";
import { SettingsDialog } from "./SettingsDialog";
import { ShortcutsDialog } from "./ShortcutsDialog";
import { SmartCollectionDialog } from "./SmartCollectionDialog";
import { AutoStackDialog, VirtualCopyNameDialog } from "./StackDialogs";

/** ドロップを受け付けるか（カタログを開いていて、ほかのダイアログを開いていない）。 */
function canAcceptDrop(): boolean {
  const s = appStore.get();
  return s.catalog !== null && (s.dialog === null || s.dialog.kind === "import");
}

/** ドロップしたパスのうちフォルダを取り込みのダイアログに渡す（種類はアプリに問い合わせる。失敗したら名前で推定）。 */
async function acceptDrop(paths: string[]): Promise<void> {
  let folders: string[];
  let files: string[];
  try {
    const kinds = await appApi.pathKinds(paths);
    folders = paths.filter((_, i) => kinds[i] === "dir");
    files = paths.filter((_, i) => kinds[i] !== "dir");
  } catch {
    ({ folders, files } = splitDroppedPaths(paths));
  }
  if (folders.length === 0) {
    if (files.length > 0) actions.notify("info", "フォルダをドロップしてください（ファイルは 1 つずつ取り込めません）");
    return;
  }
  offerImportFolders(folders);
  if (appStore.get().dialog === null) actions.openDialog({ kind: "import" });
  if (files.length > 0) actions.notify("info", `ファイル ${files.length} 個は除きました（取り込みはフォルダ単位です）`);
}

/** フォルダのドラッグ＆ドロップを受け取る（ドラッグ中は案内を出す）。 */
function useFolderDrop(): boolean {
  const [over, setOver] = useState(false);
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    listenDragDrop((e) => {
      if (e.type === "leave") {
        setOver(false);
        return;
      }
      if (e.type === "enter") {
        setOver(canAcceptDrop());
        return;
      }
      setOver(false);
      if (!canAcceptDrop()) return;
      void acceptDrop(e.paths);
    })
      .then((un) => {
        if (disposed) un();
        else unlisten = un;
      })
      .catch((err) => console.warn("ドラッグ＆ドロップを受け取れません", err));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  return over;
}

export function Dialogs() {
  const dialog = useApp((s) => s.dialog);
  const over = useFolderDrop();

  let body = null;
  switch (dialog?.kind) {
    case "import":
      body = <ImportDialog />;
      break;
    case "export":
      body = <ExportDialog variantIds={dialog.variantIds} />;
      break;
    case "composite":
      body = <CompositeDialog variantIds={dialog.variantIds} mode={dialog.mode} />;
      break;
    case "delete":
      body = <DeleteDialog deleteKind={dialog.deleteKind} variantIds={dialog.variantIds} />;
      break;
    case "paste_settings":
      body = <PasteSettingsDialog targets={dialog.targets} />;
      break;
    case "upgrade_process":
      body = <UpgradeProcessDialog targets={dialog.targets} />;
      break;
    case "settings":
      body = <SettingsDialog tab={dialog.tab} />;
      break;
    case "shortcuts":
      body = <ShortcutsDialog />;
      break;
    case "close_failed":
      // 理由が変わったら出し直す（再試行の結果など）。
      body = <CloseFailedDialog key={JSON.stringify(dialog.failure)} failure={dialog.failure} quitting={dialog.quitting} />;
      break;
    case "crash_reports":
      body = <CrashReportsDialog reports={dialog.reports} />;
      break;
    case "keyword":
      body = <KeywordDialog mode={dialog.mode} keywordId={dialog.keywordId} parentId={dialog.parentId} />;
      break;
    case "keyword_delete":
      body = <KeywordDeleteDialog keywordId={dialog.keywordId} />;
      break;
    case "collection":
      body = (
        <CollectionDialog
          mode={dialog.mode}
          collectionKind={dialog.collectionKind}
          collectionId={dialog.collectionId}
          parentId={dialog.parentId}
          variantIds={dialog.variantIds}
        />
      );
      break;
    case "collection_delete":
      body = <CollectionDeleteDialog collectionId={dialog.collectionId} />;
      break;
    case "smart_collection":
      body = <SmartCollectionDialog collectionId={dialog.collectionId} parentId={dialog.parentId} />;
      break;
    case "add_to_collection":
      body = <AddToCollectionDialog variantIds={dialog.variantIds} />;
      break;
    case "virtual_copy_name":
      body = <VirtualCopyNameDialog variantId={dialog.variantId} />;
      break;
    case "auto_stack":
      body = <AutoStackDialog />;
      break;
    case "capture_time":
      body = <CaptureTimeDialog variantIds={dialog.variantIds} current={dialog.current} />;
      break;
    case "move_files":
      body = <MoveFilesDialog variantIds={dialog.variantIds} destFolder={dialog.destFolder} />;
      break;
    case "rename_files":
      body = <RenameFilesDialog variantIds={dialog.variantIds} />;
      break;
    case "folder_name":
      body = <FolderNameDialog mode={dialog.mode} folderId={dialog.folderId} currentName={dialog.currentName} />;
      break;
    case "move_folder":
      body = <MoveFolderDialog folderId={dialog.folderId} />;
      break;
    case "relink":
      body = <RelinkDialog variantIds={dialog.variantIds} />;
      break;
    case "relocate_folder":
      body = <RelocateFolderDialog folderId={dialog.folderId} />;
      break;
    case "remove_folder":
      body = <RemoveFolderDialog folderId={dialog.folderId} />;
      break;
    case "profiles":
      body = <ProfileBrowserDialog />;
      break;
    case "snapshot_name":
      body = (
        <SnapshotNameDialog
          mode={dialog.mode}
          variantId={dialog.variantId}
          snapshotId={dialog.snapshotId}
          entryId={dialog.entryId}
          currentName={dialog.currentName}
        />
      );
      break;
    case "develop_preset":
      body = <DevelopPresetDialog mode={dialog.mode} variantId={dialog.variantId} presetId={dialog.presetId} />;
      break;
    case "apply_preset":
      body = <ApplyPresetDialog targets={dialog.targets} presetId={dialog.presetId} />;
      break;
    case "preset_name":
      body = (
        <PresetNameDialog
          mode={dialog.mode}
          presetKind={dialog.presetKind}
          presetId={dialog.presetId}
          folder={dialog.folder}
          currentName={dialog.currentName}
        />
      );
      break;
    case "preset_delete":
      body = <PresetDeleteDialog presetKind={dialog.presetKind} presetId={dialog.presetId} name={dialog.name} />;
      break;
    case undefined:
      break;
  }
  return (
    <>
      {body}
      {over && <div className="drop-overlay">フォルダをドロップすると取り込みます</div>}
    </>
  );
}
