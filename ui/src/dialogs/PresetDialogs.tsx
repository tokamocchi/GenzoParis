// スナップショット（DEV-28）・現像プリセット（DEV-29）・プリセットの名前（DEV-29・EXP-02）のダイアログ。
//
// - スナップショットの作成・名前の変更（SnapshotNameDialog）: 作るときの既定の名前は今の日時（履歴の段階から作る
//   ときは、その項目の名前）。
// - 現像プリセットの作成・今の設定で更新（DevelopPresetDialog）: 名前・フォルダ（既存のフォルダを候補に出す）・含める
//   項目（設定の貼り付けと同じ 11 項目。作るときの既定は、切り抜きと角度・マスクを除くすべて。前回の選択を保存）。
//   元にするのは現像中の写真の今の設定（保存待ちの変更を含む。コアが読む）。
// - 選んだ写真への適用（ApplyPresetDialog）: プリセットを選び、「処理バージョンが古い写真は更新する」を選べる（既定は
//   オフ。設定の貼り付けと同じ）。写真の右クリックのメニュー・現像のパネルのプリセットのメニューから開く。動画は除いて
//   写真にだけ適用する（コアが除き、読み込み済みで動画と分かっているものは先に知らせる）。
// - 名前の変更・フォルダへ移す・フォルダの名前の変更（PresetNameDialog）、削除の確認（PresetDeleteDialog）。
//   書き出しプリセット（EXP-02）にも使う。
// - 名前の規則は画面でも確かめ（presetLogic.ts）、コアのエラー（同じ名前がある など）はダイアログの中に出す。

import { useEffect, useMemo, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo, HistoryEntryId, PresetKind, SnapshotId, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import {
  defaultSnapshotName,
  describeGroups,
  groupPresets,
  presetApplyTargets,
  presetFolders,
  selectionOf,
  validateFolderName,
  validatePresetName,
  validateSnapshotName,
} from "../develop/presetLogic";
import { actions } from "../state/app";
import { applyDevelopPreset, createSnapshot, reloadPresets, renameSnapshot, usePresets } from "../state/presets";
import { formatCount } from "./dialogFormat";
import { ErrorBox } from "./parts";
import {
  DEFAULT_PASTE_SELECTION,
  PASTE_GROUP_ITEMS,
  type PasteSelection,
  anySelected,
  canOfferProcessUpgrade,
  processUpgradeOption,
  restorePasteSelection,
  toSettingGroups,
} from "./pasteLogic";
import { loadStored, saveStored } from "./storage";

/** 現像プリセットの含める項目の前回の選択を保存するキー。 */
const PRESET_GROUPS_KEY = "preset.groups";

/** 失敗を ErrorInfo にする。 */
const info = (e: unknown): ErrorInfo => toApiError(e).info;

// ---------------------------------------------------------------------------
// スナップショット（DEV-28）
// ---------------------------------------------------------------------------

export function SnapshotNameDialog(props: {
  mode: "create" | "rename";
  variantId: VariantId;
  snapshotId?: SnapshotId;
  entryId?: HistoryEntryId;
  currentName?: string;
}) {
  const [name, setName] = useState(() => props.currentName ?? defaultSnapshotName(new Date()));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const problem = validateSnapshotName(name);
  const creating = props.mode === "create";
  const ok = async () => {
    if (problem || busy) return;
    setBusy(true);
    setError(null);
    try {
      if (creating) {
        const s = await createSnapshot(props.variantId, name, props.entryId ?? null);
        actions.notify("success", `スナップショット「${s.name}」を作りました`);
      } else if (props.snapshotId !== undefined) {
        await renameSnapshot(props.snapshotId, name);
      }
      actions.closeDialog();
    } catch (e) {
      setError(info(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={creating ? "スナップショットを作る" : "スナップショットの名前を変える"}
      width={440}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || problem !== null}>
            {creating ? "作る" : "変える"}
          </button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void ok();
        }}
      >
        <div className="field">
          <label htmlFor="snapshot-name">名前</label>
          <input id="snapshot-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        {name.trim() !== "" && problem && <p className="hint">{problem}</p>}
        {creating && (
          <p className="hint">
            {props.entryId !== undefined
              ? "履歴のこの段階の設定を、名前を付けて保存します（今の設定は変わりません）。"
              : "今の設定を、名前を付けて保存します。後でクリックすると、この状態に戻せます（履歴に残ります）。"}
          </p>
        )}
      </form>
      <ErrorBox error={error} />
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// 現像プリセット（DEV-29）
// ---------------------------------------------------------------------------

/** フォルダの入力欄（既存のフォルダを候補に出す）。 */
function FolderInput(props: { id: string; value: string; folders: string[]; onChange: (v: string) => void }) {
  const listId = `${props.id}-list`;
  return (
    <>
      <input
        id={props.id}
        value={props.value}
        list={listId}
        placeholder="フォルダなし"
        onChange={(e) => props.onChange(e.target.value)}
      />
      <datalist id={listId}>
        {props.folders.map((f) => (
          <option key={f} value={f} />
        ))}
      </datalist>
    </>
  );
}

export function DevelopPresetDialog(props: { mode: "create" | "update"; variantId: VariantId; presetId?: string }) {
  const presets = usePresets((s) => s.develop);
  const preset = presets?.find((p) => p.id === props.presetId) ?? null;
  const creating = props.mode === "create";
  const [name, setName] = useState("");
  const [folder, setFolder] = useState("");
  const [sel, setSel] = useState<PasteSelection>(() =>
    creating ? restorePasteSelection(loadStored(PRESET_GROUPS_KEY) ?? DEFAULT_PASTE_SELECTION) : { ...DEFAULT_PASTE_SELECTION },
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  // 更新のときは、今のプリセットの含める項目から始める（一覧を読んでから）。
  const [loaded, setLoaded] = useState(creating);
  useEffect(() => {
    if (presets === null) void reloadPresets("develop");
  }, [presets]);
  useEffect(() => {
    if (!creating && !loaded && preset) {
      setSel(selectionOf(preset.groups, DEFAULT_PASTE_SELECTION));
      setLoaded(true);
    }
  }, [creating, loaded, preset]);
  const folders = useMemo(() => presetFolders(presets ?? []), [presets]);
  const nameProblem = creating ? validatePresetName(name) : null;
  const folderProblem = creating ? validateFolderName(folder) : null;
  const disabled = busy || nameProblem !== null || folderProblem !== null || !anySelected(sel) || (!creating && !preset);

  const toggle = (key: keyof PasteSelection, v: boolean) => setSel((cur) => ({ ...cur, [key]: v }));
  const ok = async () => {
    if (disabled) return;
    setBusy(true);
    setError(null);
    try {
      const groups = toSettingGroups(sel);
      if (creating) {
        const p = await core.createDevelopPreset(name, folder, groups, props.variantId);
        saveStored(PRESET_GROUPS_KEY, sel);
        actions.notify("success", `プリセット「${p.name}」を作りました`);
      } else if (preset) {
        await core.updateDevelopPreset(preset.id, groups, props.variantId);
        actions.notify("success", `プリセット「${preset.name}」を今の設定で更新しました`);
      }
      await reloadPresets("develop");
      actions.closeDialog();
    } catch (e) {
      setError(info(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={creating ? "現像プリセットを作る" : `プリセット「${preset?.name ?? "…"}」を今の設定で更新`}
      width={520}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={disabled}>
            {creating ? "作る" : "更新する"}
          </button>
        </>
      }
    >
      {creating && (
        <>
          <div className="field">
            <label htmlFor="preset-name">名前</label>
            <input id="preset-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          {name.trim() !== "" && nameProblem && <div className="dlg-field-error">{nameProblem}</div>}
          <div className="field">
            <label htmlFor="preset-folder">フォルダ</label>
            <FolderInput id="preset-folder" value={folder} folders={folders} onChange={setFolder} />
          </div>
          {folderProblem && <div className="dlg-field-error">{folderProblem}</div>}
        </>
      )}
      {!creating && !preset && <div className="dlg-warn">プリセットが見つかりません（一覧を読み直してください）。</div>}
      <div className="dlg-section">
        <h3>含める項目</h3>
        {PASTE_GROUP_ITEMS.map((item) => (
          <label key={item.key} className="dlg-check">
            <input
              type="checkbox"
              checked={sel[item.key]}
              disabled={busy}
              onChange={(e) => toggle(item.key, e.target.checked)}
            />
            <span>
              {item.label}
              <span className="desc">{item.detail}</span>
            </span>
          </label>
        ))}
        {!anySelected(sel) && <div className="dlg-field-error">項目を 1 つ以上選んでください</div>}
      </div>
      <div className="dlg-note">
        今の写真の設定のうち、選んだ項目だけを保存します（ほかの項目は当てたときに変えません）。プリセットはアプリに保存し、
        ほかのカタログでも使えます。カメラプロファイル・ルックは、当てるカタログに同じ名前で取り込んであるときに使われます。
      </div>
      <ErrorBox error={error} title={creating ? "プリセットを作れません" : "プリセットを更新できません"} />
    </Dialog>
  );
}

export function ApplyPresetDialog(props: { targets: VariantId[]; presetId?: string }) {
  const presets = usePresets((s) => s.develop);
  const [chosen, setChosen] = useState<string>(props.presetId ?? "");
  const [upgrade, setUpgrade] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  useEffect(() => {
    void reloadPresets("develop");
  }, []);
  const usable = (presets ?? []).filter((p) => p.problem === null);
  const preset = usable.find((p) => p.id === chosen) ?? null;
  const version = preset?.process_version ?? 1;
  const offerUpgrade = canOfferProcessUpgrade(version);
  // 動画には適用しない（コアが除く。読み込み済みで動画と分かっているものは先に知らせる）。
  const targets = useMemo(
    () => presetApplyTargets(props.targets, actions.knownVideoCount(props.targets)),
    [props.targets],
  );
  const count = targets.photos;
  const ok = async () => {
    if (!preset || busy || count === 0) return;
    setBusy(true);
    setError(null);
    try {
      await applyDevelopPreset(preset, props.targets, offerUpgrade && upgrade ? "upgrade" : "keep");
      actions.closeDialog();
    } catch (e) {
      setError(info(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={`現像プリセットを適用（${formatCount(count)} 枚）`}
      width={500}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || !preset || count === 0}>
            {busy ? "適用中…" : "適用する"}
          </button>
        </>
      }
    >
      {presets !== null && usable.length === 0 ? (
        <div className="dlg-warn">使えるプリセットがありません。現像の画面の左のパネルの「プリセット」で作れます。</div>
      ) : (
        <div className="field">
          <label htmlFor="apply-preset">プリセット</label>
          <select id="apply-preset" value={chosen} onChange={(e) => setChosen(e.target.value)} disabled={busy}>
            <option value="">選んでください</option>
            {groupPresets(usable).map((g) =>
              g.folder === "" ? (
                g.presets.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))
              ) : (
                <optgroup key={g.folder} label={g.folder}>
                  {g.presets.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </optgroup>
              ),
            )}
          </select>
        </div>
      )}
      {preset && <p className="hint">含める項目: {describeGroups(preset.groups)}</p>}
      {targets.note && <div className={targets.photos === 0 ? "dlg-warn" : "dlg-note"}>{targets.note}</div>}
      {preset && offerUpgrade && (
        <div className="dlg-section">
          <h3>処理バージョン</h3>
          <label className="dlg-check">
            <input type="checkbox" checked={upgrade} disabled={busy} onChange={(e) => setUpgrade(e.target.checked)} />
            <span>
              {processUpgradeOption(version).label}
              <span className="desc">{processUpgradeOption(version).detail}</span>
            </span>
          </label>
        </div>
      )}
      <div className="dlg-note">
        それぞれの写真の履歴に 1 件記録します（現像の画面の履歴から元に戻せます）。動画には適用しません。元のファイルは変更しません。
      </div>
      <ErrorBox error={error} title="適用できません" />
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// プリセットの名前・フォルダ・削除（DEV-29・EXP-02）
// ---------------------------------------------------------------------------

const KIND_LABEL: Record<PresetKind, string> = { develop: "現像プリセット", export: "書き出しプリセット" };

export function PresetNameDialog(props: {
  mode: "rename" | "move" | "rename_folder";
  presetKind: PresetKind;
  presetId?: string;
  folder?: string;
  currentName: string;
}) {
  const list = usePresets((s) => (props.presetKind === "develop" ? s.develop : s.export));
  const [value, setValue] = useState(props.currentName);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const folders = useMemo(() => presetFolders(list ?? []), [list]);
  const isName = props.mode === "rename";
  const problem = isName ? validatePresetName(value) : validateFolderName(value);
  const unchanged = value.trim() === props.currentName.trim();
  const title =
    props.mode === "rename" ? "名前を変える" : props.mode === "move" ? "フォルダへ移す" : "フォルダの名前を変える";
  const ok = async () => {
    if (problem || busy || unchanged) return;
    setBusy(true);
    setError(null);
    try {
      if (props.mode === "rename" && props.presetId) {
        await core.renamePreset(props.presetKind, props.presetId, value);
      } else if (props.mode === "move" && props.presetId) {
        await core.movePreset(props.presetKind, props.presetId, value);
      } else if (props.mode === "rename_folder" && props.folder !== undefined) {
        const n = await core.renamePresetFolder(props.presetKind, props.folder, value);
        actions.notify("success", `${formatCount(n)} 件のプリセットを移しました`);
      }
      await reloadPresets(props.presetKind);
      actions.closeDialog();
    } catch (e) {
      setError(info(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={`${KIND_LABEL[props.presetKind]}: ${title}`}
      width={440}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || problem !== null || unchanged}>
            変える
          </button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void ok();
        }}
      >
        <div className="field">
          <label htmlFor="preset-rename">{isName ? "名前" : "フォルダ"}</label>
          {isName ? (
            <input id="preset-rename" autoFocus value={value} onChange={(e) => setValue(e.target.value)} />
          ) : (
            <FolderInput id="preset-rename" value={value} folders={folders} onChange={setValue} />
          )}
        </div>
        {(isName ? value.trim() !== "" : true) && problem && <p className="hint">{problem}</p>}
        {!isName && <p className="hint">空にすると、フォルダなしに移します。</p>}
      </form>
      <ErrorBox error={error} />
    </Dialog>
  );
}

export function PresetDeleteDialog(props: { presetKind: PresetKind; presetId: string; name: string }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const ok = async () => {
    setBusy(true);
    setError(null);
    try {
      await core.deletePreset(props.presetKind, props.presetId);
      await reloadPresets(props.presetKind);
      actions.notify("info", `プリセット「${props.name}」を削除しました`);
      actions.closeDialog();
    } catch (e) {
      setError(info(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={`${KIND_LABEL[props.presetKind]}を削除`}
      width={440}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn danger" onClick={() => void ok()} disabled={busy}>
            削除する
          </button>
        </>
      }
    >
      <p>
        プリセット「{props.name}」を削除します。写真の現像設定は変わりません（当てた写真の設定はそのまま残ります）。
      </p>
      <p className="hint">ほかの PC に渡すなどで残したいときは、先に「ファイルに書き出す」で保存してください。</p>
      <ErrorBox error={error} title="削除できません" />
    </Dialog>
  );
}
