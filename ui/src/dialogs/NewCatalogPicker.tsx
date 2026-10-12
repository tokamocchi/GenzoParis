// 新しいカタログの場所を選ぶ（最初の画面・開けないときの画面・設定の「カタログ」が使う）。
//
// ふだんは保存のダイアログで選ぶ。保存のダイアログを使えない（アプリの権限に dialog:allow-save がない）
// ときは、画面の中に「フォルダ（開くのダイアログで選ぶ）と名前（入力）」の欄を出して選ばせる
// （catalogPick.ts の no_save_dialog）。

import { type ReactNode, useState } from "react";

import { actions } from "../state/app";
import { type CatalogPick, checkNewCatalogInFolder, pickNewCatalog } from "./catalogPick";
import { chooseFolder } from "./native";
import { PathBox } from "./parts";

/** 名前の欄の最初の値（既定のカタログと同じ名前。catalog_file.rs の DEFAULT_CATALOG_FILE は catalog.db）。 */
const DEFAULT_NAME = "catalog";

/**
 * 新しいカタログの場所を選ぶ操作。`start` をボタンから呼び、`form`（保存のダイアログを使えないときの欄。
 * ふだんは null）をボタンの下に置く。選べたら（取り消し以外）`onPick` を呼ぶ。
 */
export function useNewCatalogPicker(
  startDir: string | null,
  onPick: (pick: CatalogPick) => void | Promise<void>,
  disabled = false,
): { start: () => void; form: ReactNode } {
  const [fallback, setFallback] = useState<{ folder: string; name: string } | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const start = () => {
    void pickNewCatalog(startDir).then((pick) => {
      if (pick.kind === "no_save_dialog") {
        setError(null);
        setFallback((f) => f ?? { folder: startDir ?? "", name: DEFAULT_NAME });
        return;
      }
      void onPick(pick);
    });
  };

  const chooseDir = async () => {
    if (!fallback) return;
    try {
      const p = await chooseFolder("新しいカタログを置くフォルダを選ぶ", fallback.folder || startDir);
      if (p) setFallback((f) => (f ? { ...f, folder: p } : f));
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    }
  };

  const create = async () => {
    if (!fallback) return;
    setChecking(true);
    setError(null);
    const pick = await checkNewCatalogInFolder(fallback.folder, fallback.name);
    setChecking(false);
    if (pick.kind === "error") {
      setError(pick.message);
      return;
    }
    setFallback(null);
    await onPick(pick);
  };

  const form = fallback ? (
    <div className="dlg-confirm">
      <p>
        保存のダイアログを開けないため、新しいカタログを置くフォルダを選び、名前を入力してください。
      </p>
      <div className="field">
        <label>フォルダ</label>
        <div className="dlg-row">
          <div className="dlg-grow">
            <PathBox path={fallback.folder} empty="フォルダを選んでください" box />
          </div>
          <button className="btn" onClick={() => void chooseDir()} disabled={disabled || checking}>
            参照…
          </button>
        </div>
      </div>
      <div className="field">
        <label htmlFor="new-catalog-name">名前</label>
        <div className="dlg-row">
          <input
            id="new-catalog-name"
            type="text"
            value={fallback.name}
            onChange={(e) => setFallback((f) => (f ? { ...f, name: e.target.value } : f))}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) void create();
            }}
          />
          <span className="muted">.db</span>
        </div>
      </div>
      {error && <div className="dlg-field-error">{error}</div>}
      <div className="dlg-row end">
        <button className="btn" onClick={() => setFallback(null)} disabled={checking}>
          やめる
        </button>
        <button className="btn primary" onClick={() => void create()} disabled={disabled || checking}>
          このフォルダに作って開く
        </button>
      </div>
    </div>
  ) : null;

  return { start, form };
}
