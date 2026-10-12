// 画面の全体（起動・カタログの選択・レイアウト・全体のショートカット）。
//
// レイアウト:
//
// | 上 | TopBar（カタログ・表示モード・取り込み・書き出し・ジョブ・設定） |
// | 左 | グリッド・ルーペ: LibraryLeftPanel（フォルダ・絞り込み） / 現像: DevelopLeftPanel（履歴） |
// | 中 | グリッド: FilterBar ＋ Grid / ルーペ: Loupe / 現像: DevelopView |
// | 右 | グリッド・ルーペ: MetadataPanel / 現像: DevelopPanel |
// | 下 | ルーペ・現像: Filmstrip、StatusBar |

import { useEffect, useState } from "react";

import * as appApi from "./api/app";
import { onCloseFailed, startCoreEvents } from "./api/events";
import { displayTarget } from "./api/frames";
import { StatusBar } from "./components/StatusBar";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { DevelopLeftPanel } from "./develop/DevelopLeftPanel";
import { DevelopPanel } from "./develop/DevelopPanel";
import { DevelopView } from "./develop/DevelopView";
import { CatalogErrorScreen } from "./dialogs/CatalogErrorScreen";
import { Dialogs } from "./dialogs/Dialogs";
import { WelcomeScreen } from "./dialogs/WelcomeScreen";
import { useHotkeys } from "./keyboard";
import { FilterBar } from "./library/FilterBar";
import { Grid } from "./library/Grid";
import { LibraryLeftPanel } from "./library/LibraryLeftPanel";
import { MetadataPanel } from "./library/MetadataPanel";
import { actions, appStore, useApp } from "./state/app";
import { developStore, installDevelopEvents, redo, undo } from "./state/develop";
import { Filmstrip } from "./viewer/Filmstrip";
import { Loupe } from "./viewer/Loupe";

/** 起動: イベントの受け取り、画面の色空間、前回のカタログを開く。 */
async function boot(setFatal: (m: string) => void): Promise<void> {
  actions.installEventHandlers();
  installDevelopEvents();
  try {
    await startCoreEvents();
    await onCloseFailed((failure) => actions.openDialog({ kind: "close_failed", failure, quitting: true }));
    await appApi.setDisplayTarget(displayTarget());
    const status = await appApi.appStatus();
    actions.setStatus(status);
    if (status.notice) actions.notify("warning", status.notice);
    if (status.catalog) {
      await actions.catalogOpened(status.catalog);
    } else if (status.settings.last_catalog) {
      await actions.openCatalog(status.settings.last_catalog, false);
    }
  } catch (e) {
    setFatal(actions.describeError(e));
  }
}

/** 全体のショートカット（グリッドの矢印キーは Grid が受け持つ）。 */
function useGlobalHotkeys(enabled: boolean) {
  const mode = useApp((s) => s.mode);
  useHotkeys(
    {
      g: () => actions.setMode("grid"),
      e: () => actions.setMode("loupe"),
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
      ...(mode !== "grid"
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
      Delete: () => {
        const ids = actions.targets();
        if (ids.length > 0) actions.openDialog({ kind: "delete", deleteKind: "remove_from_catalog", variantIds: ids });
      },
      Backspace: () => {
        const ids = actions.targets();
        if (ids.length > 0) actions.openDialog({ kind: "delete", deleteKind: "remove_from_catalog", variantIds: ids });
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
          {mode === "develop" && <DevelopView />}
        </section>
        <aside className="right">{mode === "develop" ? <DevelopPanel /> : <MetadataPanel />}</aside>
      </div>
      {mode !== "grid" ? <Filmstrip /> : <div />}
      <StatusBar />
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
