// 現像の左のパネルの「プリセット」（DEV-29）と「スナップショット」（DEV-28）。
//
// - プリセット: フォルダごとに並べる（フォルダは開閉できる。開閉はブラウザに保存）。クリックで今の写真に適用する
//   （設定の貼り付けと同じく、含める項目だけを写し、写真の処理バージョンは変えない。履歴に 1 件残る）。右クリック・
//   「…」で、選んだ写真への適用・今の設定で更新・名前の変更・フォルダへ移す・ファイルに書き出す・削除。見出しの「＋」で
//   今の写真の設定から作り（Ctrl / ⌘ + Shift + N）、「…」でファイルから読み込む。使えないプリセット（新しい版のアプリで
//   作ったものなど）は薄く出し、理由を見せる（削除・書き出しだけできる）。マウスを乗せたときのプレビューはしない
//   （任意の機能。人の判断の表）。
// - スナップショット: 作った順に並べる。クリックで適用する（履歴に 1 件残る）。右クリック・「…」で、ビフォーに設定
//   （ビフォー / アフターの表示。DEV-31）・名前の変更・削除。見出しの「＋」で今の状態から作る（Ctrl / ⌘ + N）。

import { useEffect, useState } from "react";

import * as core from "../api/core";
import type { PresetInfo, SnapshotInfo, VariantId } from "../api/types";
import { MENU_SEPARATOR, type MenuAt, MenuButton, type MenuEntry, PopupMenu } from "../components/Menu";
import { Section } from "../components/Section";
import { loadStored, saveStored } from "../dialogs/storage";
import { comboLabel } from "../keyboard";
import { actions, useApp } from "../state/app";
import { useDevelop } from "../state/develop";
import {
  applyDevelopPreset,
  applySnapshot,
  deleteSnapshot,
  exportPresetFile,
  importPresetFiles,
  reloadPresets,
  usePresets,
} from "../state/presets";
import { shallowEqual } from "../state/store";
import { groupPresets, presetTooltip } from "./presetLogic";
import { showAsBefore } from "./viewState";

/** 閉じたフォルダを保存するキー。 */
const CLOSED_FOLDERS_KEY = "develop.presets.closed";

/** 日時の表示（"10/12 14:05"。この PC の時刻）。 */
function shortTime(iso: string | null): string {
  if (!iso) return "";
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(t.getMonth() + 1)}/${p(t.getDate())} ${p(t.getHours())}:${p(t.getMinutes())}`;
}

/** 行の中の「…」のボタン・メニューのクリックか（行のクリックの操作をしない）。 */
function fromMenu(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(".menu, .menu-button") !== null;
}

/** 今の写真に当てる（失敗は知らせる）。 */
function applyToCurrent(p: PresetInfo, variantId: VariantId): void {
  void applyDevelopPreset(p, [variantId]).catch((e) => actions.reportError(e, "プリセットを適用できません"));
}

/** プリセットの操作のメニュー。 */
function presetMenu(p: PresetInfo, variantId: VariantId | null, others: VariantId[]): MenuEntry[] {
  const usable = p.problem === null;
  const items: MenuEntry[] = [
    {
      label: "今の写真に適用",
      disabled: !usable || variantId === null,
      onSelect: () => variantId !== null && applyToCurrent(p, variantId),
    },
    {
      label: others.length > 0 ? `選んだ ${others.length + 1} 枚に適用…` : "選んだ写真に適用…",
      disabled: !usable || variantId === null,
      onSelect: () =>
        variantId !== null &&
        actions.openDialog({ kind: "apply_preset", targets: [variantId, ...others], presetId: p.id }),
    },
    MENU_SEPARATOR,
    {
      label: "今の設定で更新…",
      disabled: !usable || variantId === null,
      onSelect: () =>
        variantId !== null && actions.openDialog({ kind: "develop_preset", mode: "update", variantId, presetId: p.id }),
    },
    {
      label: "名前を変える…",
      disabled: !usable,
      onSelect: () =>
        actions.openDialog({ kind: "preset_name", mode: "rename", presetKind: "develop", presetId: p.id, currentName: p.name }),
    },
    {
      label: "フォルダへ移す…",
      disabled: !usable,
      onSelect: () =>
        actions.openDialog({ kind: "preset_name", mode: "move", presetKind: "develop", presetId: p.id, currentName: p.folder }),
    },
    { label: "ファイルに書き出す…", onSelect: () => void exportPresetFile("develop", p) },
    MENU_SEPARATOR,
    {
      label: "削除…",
      danger: true,
      onSelect: () => actions.openDialog({ kind: "preset_delete", presetKind: "develop", presetId: p.id, name: p.name }),
    },
  ];
  return items;
}

/** 閉じたフォルダ（ブラウザに保存）。 */
function useClosedFolders(): [Set<string>, (folder: string) => void] {
  const [closed, setClosed] = useState<Set<string>>(() => {
    const v = loadStored(CLOSED_FOLDERS_KEY);
    return new Set(Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
  });
  const toggle = (folder: string) =>
    setClosed((cur) => {
      const next = new Set(cur);
      if (next.has(folder)) next.delete(folder);
      else next.add(folder);
      saveStored(CLOSED_FOLDERS_KEY, [...next]);
      return next;
    });
  return [closed, toggle];
}

/** 現像プリセットの一覧（DEV-29）。`variantId` は現像中の写真（開いていなければ null）。 */
export function DevelopPresetsSection(props: { variantId: VariantId | null }) {
  const { variantId } = props;
  const list = usePresets((s) => s.develop);
  const error = usePresets((s) => s.errors.develop);
  const current = useApp((s) => s.selection.current);
  const selected = useApp((s) => s.selection.ids, shallowEqual);
  const others = selected.filter((id) => id !== current && id !== variantId);
  const [menu, setMenu] = useState<{ at: MenuAt; items: MenuEntry[] } | null>(null);
  const [closed, toggleFolder] = useClosedFolders();
  useEffect(() => {
    void reloadPresets("develop");
  }, []);

  const head = (
    <span className="dl-section-buttons">
      <button
        type="button"
        className="btn ghost small"
        disabled={variantId === null}
        title={`今の写真の設定からプリセットを作る（${comboLabel("mod+shift+n")}）`}
        aria-label="プリセットを作る"
        onClick={() => variantId !== null && actions.openDialog({ kind: "develop_preset", mode: "create", variantId })}
      >
        ＋
      </button>
      <MenuButton
        label="…"
        title="プリセットの操作"
        items={() => [
          { label: "ファイルから読み込む…", onSelect: () => void importPresetFiles("develop") },
          { label: "一覧を読み直す", onSelect: () => void reloadPresets("develop") },
        ]}
      />
    </span>
  );
  return (
    <Section id="develop.presets" title="プリセット" right={head}>
      {error && (
        <div>
          <p className="error-text">プリセットの一覧を読めません: {error}</p>
          <button type="button" className="btn small" onClick={() => void reloadPresets("develop")}>
            読み直す
          </button>
        </div>
      )}
      {!error && list === null && <div className="muted">読み込んでいます…</div>}
      {list && list.length === 0 && (
        <p className="hint">プリセットはまだありません。「＋」で今の写真の設定から作れます（ほかのカタログでも使えます）。</p>
      )}
      {list && list.length > 0 && (
        <ul className="dl-history dl-presets" role="tree" aria-label="現像プリセット">
          {groupPresets(list).map((g) => {
            const open = g.folder === "" || !closed.has(g.folder);
            return (
              <li key={`folder:${g.folder}`} className="dl-preset-group" role="none">
                {g.folder !== "" && (
                  <div
                    className="dl-preset-folder"
                    role="treeitem"
                    aria-expanded={open}
                    onClick={() => toggleFolder(g.folder)}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      setMenu({
                        at: { x: e.clientX, y: e.clientY },
                        items: [
                          {
                            label: "フォルダの名前を変える…",
                            onSelect: () =>
                              actions.openDialog({
                                kind: "preset_name",
                                mode: "rename_folder",
                                presetKind: "develop",
                                folder: g.folder,
                                currentName: g.folder,
                              }),
                          },
                        ],
                      });
                    }}
                  >
                    <span className="chev">{open ? "▾" : "▸"}</span> {g.folder}
                    <span className="time">{g.presets.length}</span>
                  </div>
                )}
                {open && (
                  <ul role="group">
                    {g.presets.map((p) => (
                      <li
                        key={p.id}
                        role="treeitem"
                        className={`${p.problem ? "unusable" : ""}${g.folder !== "" ? " nested" : ""}`}
                        title={presetTooltip(p)}
                        aria-disabled={p.problem !== null || variantId === null}
                        onClick={(e) => {
                          if (fromMenu(e.target)) return;
                          if (p.problem === null && variantId !== null) applyToCurrent(p, variantId);
                        }}
                        onContextMenu={(e) => {
                          e.preventDefault();
                          setMenu({ at: { x: e.clientX, y: e.clientY }, items: presetMenu(p, variantId, others) });
                        }}
                      >
                        <span className="name">{p.name}</span>
                        <MenuButton label="…" title={`${p.name} の操作`} items={() => presetMenu(p, variantId, others)} />
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {menu && <PopupMenu at={menu.at} items={menu.items} onClose={() => setMenu(null)} label="プリセットの操作" />}
    </Section>
  );
}

/** スナップショットの操作のメニュー。 */
function snapshotMenu(s: SnapshotInfo): MenuEntry[] {
  return [
    { label: "適用（元に戻せます）", onSelect: () => void applySnapshot(s) },
    {
      label: "ビフォーに設定",
      onSelect: () => showAsBefore({ kind: "snapshot", snapshot_id: s.id }),
    },
    MENU_SEPARATOR,
    {
      label: "名前を変える…",
      onSelect: () =>
        actions.openDialog({
          kind: "snapshot_name",
          mode: "rename",
          variantId: s.variant_id,
          snapshotId: s.id,
          currentName: s.name,
        }),
    },
    { label: "削除", danger: true, onSelect: () => void deleteSnapshot(s.id) },
  ];
}

/** 写真のスナップショット（作る・名前の変更・削除・適用の後と、履歴が変わったときに読み直す）。 */
function useSnapshots(variantId: VariantId | null): { list: SnapshotInfo[] | null; error: string | null } {
  const version = usePresets((s) => s.snapshotsVersion);
  const historyVersion = useDevelop((s) => s.historyVersion);
  const [result, setResult] = useState<{ id: VariantId | null; list: SnapshotInfo[] | null; error: string | null }>({
    id: null,
    list: null,
    error: null,
  });
  useEffect(() => {
    if (variantId === null) return;
    let alive = true;
    core
      .snapshots(variantId)
      .then((list) => alive && setResult({ id: variantId, list, error: null }))
      .catch((e) => alive && setResult({ id: variantId, list: null, error: actions.describeError(e) }));
    return () => {
      alive = false;
    };
  }, [variantId, version, historyVersion]);
  return result.id === variantId ? { list: result.list, error: result.error } : { list: null, error: null };
}

/** スナップショットの一覧（DEV-28）。`variantId` は現像中の写真（開いていなければ null）。 */
export function SnapshotsSection(props: { variantId: VariantId | null; latestProcessVersion: number | null }) {
  const { variantId, latestProcessVersion } = props;
  const { list, error } = useSnapshots(variantId);
  const [menu, setMenu] = useState<{ at: MenuAt; snapshot: SnapshotInfo } | null>(null);
  const head = (
    <button
      type="button"
      className="btn ghost small"
      disabled={variantId === null}
      title={`今の状態でスナップショットを作る（${comboLabel("mod+n")}）`}
      aria-label="スナップショットを作る"
      onClick={() => variantId !== null && actions.openDialog({ kind: "snapshot_name", mode: "create", variantId })}
    >
      ＋
    </button>
  );
  return (
    <Section id="develop.snapshots" title="スナップショット" right={head}>
      {variantId === null && <div className="muted">—</div>}
      {error && <div className="error-text">{error}</div>}
      {variantId !== null && !error && list === null && <div className="muted">読み込んでいます…</div>}
      {list && list.length === 0 && <div className="muted">まだスナップショットはありません</div>}
      {list && list.length > 0 && (
        <ul className="dl-history" aria-label="スナップショット">
          {list.map((s) => {
            const old = latestProcessVersion !== null && s.process_version < latestProcessVersion;
            return (
              <li
                key={s.id}
                title={`クリックでこの状態に戻す（履歴に残ります）${old ? `\n処理バージョン ${s.process_version} の状態です` : ""}`}
                onClick={(e) => {
                  if (!fromMenu(e.target)) void applySnapshot(s);
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ at: { x: e.clientX, y: e.clientY }, snapshot: s });
                }}
              >
                <span className="name">
                  {s.name}
                  {old && <span className="muted">（版 {s.process_version}）</span>}
                </span>
                <span className="time">{shortTime(s.created_at)}</span>
                <MenuButton label="…" title={`${s.name} の操作`} items={() => snapshotMenu(s)} />
              </li>
            );
          })}
        </ul>
      )}
      {menu && (
        <PopupMenu at={menu.at} items={snapshotMenu(menu.snapshot)} onClose={() => setMenu(null)} label="スナップショットの操作" />
      )}
    </Section>
  );
}
