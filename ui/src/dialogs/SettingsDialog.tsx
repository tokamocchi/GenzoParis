// 設定のダイアログ（SYS-05・SYS-04）: 「一般」「表示」（ルーペ上の撮影情報。LIB-17）「カタログ」「バックアップ」のタブ。
//
// タブを切り替えても入力が消えないよう、すべてのタブを描いておき、選んでいないものは隠す。

import { useState } from "react";

import { Dialog } from "../components/Dialog";
import { actions, useApp } from "../state/app";
import { SettingsBackup } from "./SettingsBackup";
import { SettingsCatalog } from "./SettingsCatalog";
import { SettingsGeneral } from "./SettingsGeneral";
import { SettingsView } from "./SettingsView";

type Tab = "general" | "view" | "catalog" | "backup";

/** カタログを開いていなくても使えるタブ。 */
const WITHOUT_CATALOG: Tab[] = ["general", "view"];

const TABS: { id: Tab; label: string }[] = [
  { id: "general", label: "一般" },
  { id: "view", label: "表示" },
  { id: "catalog", label: "カタログ" },
  { id: "backup", label: "バックアップ" },
];

export function SettingsDialog(props: { tab?: Tab }) {
  const [tab, setTab] = useState<Tab>(props.tab ?? "general");
  const hasCatalog = useApp((s) => s.catalog !== null);
  return (
    <Dialog
      title="設定"
      width={720}
      onClose={actions.closeDialog}
      actions={
        <button className="btn" onClick={actions.closeDialog}>
          閉じる
        </button>
      }
    >
      <div className="dlg-tabs" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            className={tab === t.id ? "on" : ""}
            disabled={!WITHOUT_CATALOG.includes(t.id) && !hasCatalog}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div role="tabpanel" hidden={tab !== "general"}>
        <SettingsGeneral />
      </div>
      <div role="tabpanel" hidden={tab !== "view"}>
        <SettingsView />
      </div>
      {hasCatalog && (
        <>
          <div role="tabpanel" hidden={tab !== "catalog"}>
            <SettingsCatalog />
          </div>
          <div role="tabpanel" hidden={tab !== "backup"}>
            <SettingsBackup />
          </div>
        </>
      )}
    </Dialog>
  );
}
