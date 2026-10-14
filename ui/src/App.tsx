// 画面の全体（起動・カタログの選択・レイアウト・全体のショートカット）。
//
// レイアウト:
//
// | 上 | TopBar（カタログ・表示モード・取り込み・書き出し・ジョブ・設定） |
// | 左 | グリッド・ルーペ・比較・選別: LibraryLeftPanel（フォルダ・絞り込み） / 現像: DevelopLeftPanel（履歴） |
// | 中 | グリッド: FilterBar ＋ Grid / ルーペ: Loupe / 比較: CompareView / 選別: SurveyView / 現像: DevelopView |
// | 右 | グリッド・ルーペ・比較・選別: MetadataPanel / 現像: DevelopPanel |
// | 下 | ルーペ・比較・選別・現像: Filmstrip、StatusBar |

import { useEffect, useState } from "react";

import * as appApi from "./api/app";
import { inTauri } from "./api/bridge";
import { onCloseFailed, onQuitRequested, startCoreEvents } from "./api/events";
import { displayTarget } from "./api/frames";
import { StatusBar } from "./components/StatusBar";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { DevelopLeftPanel } from "./develop/DevelopLeftPanel";
import { DevelopPanel } from "./develop/DevelopPanel";
import { DevelopView } from "./develop/DevelopView";
import { CatalogErrorScreen } from "./dialogs/CatalogErrorScreen";
import { quitOnRequest } from "./dialogs/catalogActions";
import { Dialogs } from "./dialogs/Dialogs";
import { uiErrorText } from "./dialogs/logsLogic";
import { WelcomeScreen } from "./dialogs/WelcomeScreen";
import { useHotkeys } from "./keyboard";
import { FilterBar } from "./library/FilterBar";
import { Grid } from "./library/Grid";
import { LibraryLeftPanel } from "./library/LibraryLeftPanel";
import { MetadataPanel } from "./library/MetadataPanel";
import { PhotoDragGhost } from "./library/PhotoDragGhost";
import { actions, appStore, useApp } from "./state/app";
import { openCompare, openSurvey } from "./state/compare";
import { developStore, installDevelopEvents, redo, undo } from "./state/develop";
import { openRenameFiles } from "./state/files";
import { deleteKeyAction, focusKeywordInput, toggleTargetCollection } from "./state/organize";
import { createVirtualCopyOf, groupIntoStack, setStackTop, toggleStack, unstackTargets } from "./state/stacks";
import { CompareView } from "./viewer/CompareView";
import { Filmstrip } from "./viewer/Filmstrip";
import { Loupe } from "./viewer/Loupe";
import { SurveyView } from "./viewer/SurveyView";

/** 画面のエラー（例外・処理されなかった Promise の失敗）をアプリのログに残す（SYS-07。Tauri の中だけ）。 */
function installUiErrorReporting(): void {
  if (!inTauri()) return;
  window.addEventListener("error", (e) => {
    const t = uiErrorText(e.error ?? e.message);
    const where = e.filename ? `${e.filename}:${e.lineno}:${e.colno}` : null;
    appApi.reportUiError(t.message, [where, t.detail].filter(Boolean).join("\n") || null);
  });
  window.addEventListener("unhandledrejection", (e) => {
    const t = uiErrorText(e.reason);
    appApi.reportUiError(`処理されなかった失敗: ${t.message}`, t.detail);
  });
}

/** 起動: イベントの受け取り、画面の色空間、前回のカタログを開く。 */
async function boot(setFatal: (m: string) => void): Promise<void> {
  installUiErrorReporting();
  actions.installEventHandlers();
  installDevelopEvents();
  try {
    await startCoreEvents();
    await onCloseFailed((failure) => actions.openDialog({ kind: "close_failed", failure, quitting: true }));
    await onQuitRequested((id) => void quitOnRequest(id));
    await appApi.setDisplayTarget(displayTarget());
    const status = await appApi.appStatus();
    actions.setStatus(status);
    if (status.notice) actions.notify("warning", status.notice);
    try {
      if (status.catalog) {
        await actions.catalogOpened(status.catalog);
      } else if (status.settings.last_catalog) {
        await actions.openCatalog(status.settings.last_catalog, false);
      }
    } finally {
      // 前回までの異常終了の知らせ（SYS-07）。カタログを開くとダイアログを閉じるので、開いた後に出す。
      if (status.crash_reports.length > 0) {
        actions.openDialog({ kind: "crash_reports", reports: status.crash_reports });
      }
    }
  } catch (e) {
    setFatal(actions.describeError(e));
  }
}

/**
 * 全体のショートカット（グリッドの矢印キーは Grid、比較表示・選別表示の矢印キーは CompareView・SurveyView が
 * 受け持つ）。
 */
function useGlobalHotkeys(enabled: boolean) {
  const mode = useApp((s) => s.mode);
  useHotkeys(
    {
      g: () => actions.setMode("grid"),
      e: () => actions.setMode("loupe"),
      // 比較表示・選別表示（LIB-03。state/compare.ts。← → などは CompareView・SurveyView が受け持つ）。
      c: () => void openCompare(),
      n: () => openSurvey(),
      d: () => actions.setMode("develop"),
      Escape: () => {
        if (appStore.get().mode !== "grid") actions.setMode("grid");
      },
      // 選別（LIB-04・05）
      "0": () => actions.setRating(0),
      "1": () => actions.setRating(1),
      "2": () => actions.setRating(2),
      "3": () => actions.setRating(3),
      "4": () => actions.setRating(4),
      "5": () => actions.setRating(5),
      p: () => actions.setFlag(1),
      x: () => actions.setFlag(-1),
      u: () => actions.setFlag(0),
      // カラーラベル（すべて同じラベルなら外す）。
      "6": () => actions.toggleColorLabel("red"),
      "7": () => actions.toggleColorLabel("yellow"),
      "8": () => actions.toggleColorLabel("green"),
      "9": () => actions.toggleColorLabel("blue"),
      ...(mode === "loupe" || mode === "develop"
        ? {
            ArrowLeft: () => actions.moveSelection(-1),
            ArrowRight: () => actions.moveSelection(1),
          }
        : {}),
      "mod+a": () => actions.selectAll(),
      "mod+d": () => actions.selectNone(),
      "mod+shift+i": () => actions.openDialog({ kind: "import" }),
      "mod+shift+e": () => {
        const ids = actions.targets();
        if (ids.length > 0) actions.openDialog({ kind: "export", variantIds: ids });
      },
      "mod+,": () => actions.openDialog({ kind: "settings" }),
      "shift+?": () => actions.openDialog({ kind: "shortcuts" }),
      // 手動のコレクションを表示している間は、そのコレクションから外す（写真は消えない。state/organize.ts）。
      Delete: () => void deleteKeyAction(),
      Backspace: () => void deleteKeyAction(),
      // スタック（LIB-12）・仮想コピー（LIB-13）。state/stacks.ts
      "mod+g": () => void groupIntoStack(),
      "mod+shift+g": () => void unstackTargets(),
      s: () => void toggleStack(),
      "shift+s": () => void setStackTop(),
      "mod+'": () => void createVirtualCopyOf(),
      // 日本語配列では「'」が Shift + 7 にある（keyboard.ts は Shift の付いた数字のキーを数字で表す）。
      "mod+shift+7": () => void createVirtualCopyOf(),
      // キーワード・コレクション（LIB-09・LIB-10）
      b: () => void toggleTargetCollection(),
      "mod+k": () => {
        // 右のパネルのキーワードの欄（グリッド・ルーペ）。現像の画面にはないので、グリッドに戻らない。
        if (appStore.get().mode !== "develop") focusKeywordInput();
      },
      "mod+n": () => {
        // 現像の画面では、今の状態でスナップショットを作る（DEV-28。Lightroom と同じ）。
        if (appStore.get().mode === "develop") {
          const v = developStore.get();
          if (v.state && v.variantId !== null) {
            actions.openDialog({ kind: "snapshot_name", mode: "create", variantId: v.variantId });
          }
          return;
        }
        actions.openDialog({
          kind: "collection",
          mode: "create",
          collectionKind: "manual",
          parentId: null,
          variantIds: actions.targets(),
        });
      },
      // 現像の画面: 今の写真の設定から現像プリセットを作る（DEV-29。Lightroom と同じ）。
      "mod+shift+n": () => {
        if (appStore.get().mode !== "develop") return;
        const v = developStore.get();
        if (v.state && v.variantId !== null) {
          actions.openDialog({ kind: "develop_preset", mode: "create", variantId: v.variantId });
        }
      },
      // ファイルの名前の変更（FILE-02）。現像の画面では使わない（入力欄の F2 とも重ならないように）。
      F2: () => {
        if (appStore.get().mode !== "develop") openRenameFiles();
      },
      // 現像設定のコピー・貼り付け（DEV-30）と Undo / Redo（DEV-27）
      "mod+shift+c": async () => {
        const id = appStore.get().selection.current;
        if (id === null) return;
        // 現像中の写真なら、画面の最新の値（送信前を含む）をコピーする。動画は写さない（種類は、検索結果の
        // ページが捨てられていても確かめる。state/app.ts の copySettingsOf）。
        const d = developStore.get();
        await actions.copySettingsOf(id, d.variantId === id ? d.settings : null);
      },
      // 貼り付けは、ルーペ・現像でもフィルムストリップで選んだ写真を含める（state/app.ts の pasteTargets）。
      "mod+shift+v": () => {
        const targets = actions.pasteTargets();
        if (!appStore.get().clipboard) {
          actions.notify("info", "先に現像設定をコピーしてください（Ctrl / ⌘ + Shift + C）");
          return;
        }
        if (targets.length > 0) actions.openDialog({ kind: "paste_settings", targets });
      },
      "mod+z": () => {
        if (appStore.get().mode === "develop") void undo();
      },
      "mod+shift+z": () => {
        if (appStore.get().mode === "develop") void redo();
      },
      "mod+y": () => {
        if (appStore.get().mode === "develop") void redo();
      },
    },
    enabled,
  );
}

function Workspace() {
  const mode = useApp((s) => s.mode);
  const dialog = useApp((s) => s.dialog);
  useGlobalHotkeys(dialog === null);
  return (
    <div className="app">
      <TopBar />
      <div className="main">
        <aside className="left">{mode === "develop" ? <DevelopLeftPanel /> : <LibraryLeftPanel />}</aside>
        <section className="center">
          {mode === "grid" && (
            <>
              <FilterBar />
              <Grid />
            </>
          )}
          {mode === "loupe" && <Loupe />}
          {mode === "compare" && <CompareView />}
          {mode === "survey" && <SurveyView />}
          {mode === "develop" && <DevelopView />}
        </section>
        <aside className="right">{mode === "develop" ? <DevelopPanel /> : <MetadataPanel />}</aside>
      </div>
      {mode !== "grid" ? <Filmstrip /> : <div />}
      <StatusBar />
      <PhotoDragGhost />
    </div>
  );
}

export function App() {
  const status = useApp((s) => s.status);
  const catalog = useApp((s) => s.catalog);
  const catalogError = useApp((s) => s.catalogError);
  const catalogOpening = useApp((s) => s.catalogOpening);
  const [fatal, setFatal] = useState<string | null>(null);

  useEffect(() => {
    void boot(setFatal);
  }, []);

  let body;
  if (fatal) {
    body = (
      <div className="empty-state">
        <h2>起動できません</h2>
        <p className="error-text">{fatal}</p>
        <button className="btn" onClick={() => location.reload()}>
          再読み込み
        </button>
      </div>
    );
  } else if (!status) {
    body = (
      <div className="empty-state">
        <div className="spinner" style={{ margin: "0 auto" }} />
      </div>
    );
  } else if (!catalog && catalogOpening) {
    body = (
      <div className="empty-state">
        <div className="spinner" style={{ margin: "0 auto 12px" }} />
        <div>カタログを開いています…</div>
        <div className="hint" title={catalogOpening}>
          {catalogOpening}
        </div>
      </div>
    );
  } else if (!catalog && catalogError) {
    body = <CatalogErrorScreen />;
  } else if (!catalog) {
    body = <WelcomeScreen />;
  } else {
    body = <Workspace />;
  }
  return (
    <>
      {body}
      <Dialogs />
      <Toasts />
    </>
  );
}
