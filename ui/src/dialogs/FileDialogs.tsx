// アプリの中でのファイル操作（FILE-02）と、見つからない写真の再リンク（FILE-04）のダイアログ。
//
// - 写真の移動: 移動先のフォルダ（落とした先、または選ぶ）への予定（移せない写真・別のドライブへのコピーの
//   量）を出してから移す（ジョブ。state/files.ts）。移動先に同じ名前のファイルがあれば、その写真は移さない
//   （上書きしない）。RAW と同時記録の JPEG は一緒に動く。
// - 写真の名前の変更: テンプレート（取り込みのリネームと同じ項目）か、1 枚なら名前を直接入れる。予定（新しい
//   名前と、変えられない理由）を入力のたびに出す。
// - フォルダ: 作る・名前を変える・別のフォルダの中へ移す（同じドライブの中だけ）。ディスクのフォルダも変わる。
// - 再リンク: 探すフォルダを選び、候補（内容が同じ・大きさと先頭と末尾が同じ・名前と撮影日時が同じ）から
//   選んで結び付け直す（確からしい候補が 1 つだけなら既定で選ぶ。library/fileOpsLogic.ts）。
// - フォルダの場所の指定し直し: 新しい場所を選び、登録したファイルがいくつ見つかるかを確かめてから、カタログの
//   中のフォルダの場所を変える（ファイルは動かさない）。
// - フォルダをカタログから除く: 写真が登録されていないフォルダ（見つからないまま残ったフォルダ・再リンクで空に
//   なったフォルダ）を配下ごと除く（カタログの中だけ。写真があればコアが断る）。

import { useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type {
  ErrorInfo,
  FileMovePlan,
  FileRenamePlan,
  FolderId,
  FolderInfo,
  RelinkSearch,
  RelocatePlan,
  RenameSpec,
  VariantId,
} from "../api/types";
import { Dialog } from "../components/Dialog";
import {
  type FolderChoice,
  RELINK_MATCH_LABEL,
  defaultRelinkChoice,
  describeMovePlan,
  describeMoveReport,
  describeFolderRemoval,
  describeRelinkReport,
  describeRelocatePlan,
  describeRenamePlan,
  folderChoices,
  folderSubtreeIds,
  isSafeMatch,
  relinkRequests,
  splitExtension,
  validateName,
} from "../library/fileOpsLogic";
import { folderName } from "../library/folderTree";
import { actions, useApp } from "../state/app";
import { startMoveFiles } from "../state/files";
import { searchCache } from "../state/search";
import { waitJobUntilClosed } from "./catalogActions";
import { baseName, formatCount } from "./dialogFormat";
import { MAX_CUSTOM_TEXT_CHARS, MAX_TEMPLATE_CHARS, TEMPLATE_PRESETS, TEMPLATE_TOKENS, insertToken } from "./importLogic";
import { chooseFolder } from "./native";
import { Busy, ErrorBox, PathBox } from "./parts";
import { notifyResult } from "./resultLog";
import { loadStored, saveStored } from "./storage";

/** 予定を作り直すまでの時間（ms。入力の途中で問い合わせすぎない。仮置き）。 */
const PLAN_DELAY_MS = 300;
/** 予定の一覧に出す写真の数の上限（残りは「ほか N 枚」）。 */
const PLAN_LIST_LIMIT = 50;
/** 名前の変更の前回のテンプレートを保存する鍵（dialogs/storage.ts）。 */
const RENAME_STORE_KEY = "rename.last";

/** フォルダの一覧（開いたときに 1 回読む）。 */
function useFolders(): { folders: FolderInfo[] | null; error: string | null } {
  const [folders, setFolders] = useState<FolderInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    core
      .folders()
      .then((f) => alive && setFolders(f))
      .catch((e: unknown) => alive && setError(actions.describeError(e)));
    return () => {
      alive = false;
    };
  }, []);
  return { folders, error };
}

function FolderSelect(props: {
  id: string;
  choices: FolderChoice[];
  value: FolderId | null;
  onChange: (id: FolderId | null) => void;
  disabled?: boolean;
}) {
  return (
    <select
      id={props.id}
      value={props.value === null ? "" : String(props.value)}
      disabled={props.disabled}
      onChange={(e) => props.onChange(e.target.value === "" ? null : Number(e.target.value))}
    >
      <option value="">フォルダを選ぶ…</option>
      {props.choices.map((c) => (
        <option key={c.id} value={c.id}>
          {c.label}
        </option>
      ))}
    </select>
  );
}

/** 予定の写真の一覧（移す・名前を変えるファイルの、元の名前 → 新しい名前。変えられない理由）。 */
function PlanList(props: { plan: FileMovePlan | FileRenamePlan; showDir?: boolean }) {
  const shown = props.plan.photos.slice(0, PLAN_LIST_LIMIT);
  const rest = props.plan.photos.length - shown.length;
  return (
    <div className="dlg-list" aria-label="予定">
      {shown.map((p) => (
        <div key={p.variant_id} className="dlg-list-item column">
          {p.files.map((f) => (
            <div key={f.file_id} className={`dlg-path${p.problem ? " empty" : ""}`} title={f.to ?? undefined}>
              {f.from ? baseName(f.from) : "（場所が分かりません）"}
              {" → "}
              {f.to ? (props.showDir ? f.to : baseName(f.to)) : "—"}
              {f.method === "copy" ? "（コピーして照合）" : ""}
            </div>
          ))}
          {p.problem && <div className="warn-text">{p.problem}</div>}
        </div>
      ))}
      {rest > 0 && <div className="dlg-list-more">ほか {formatCount(rest)} 枚</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// 写真の移動
// ---------------------------------------------------------------------------

export function MoveFilesDialog(props: { variantIds: VariantId[]; destFolder: FolderId | null }) {
  const { folders, error: foldersError } = useFolders();
  const [dest, setDest] = useState<FolderId | null>(props.destFolder);
  const [plan, setPlan] = useState<FileMovePlan | null>(null);
  const [planError, setPlanError] = useState<ErrorInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const choices = useMemo(() => folderChoices(folders ?? []), [folders]);
  const count = props.variantIds.length;

  useEffect(() => {
    setPlan(null);
    setPlanError(null);
    if (dest === null) return;
    let alive = true;
    core
      .planMoveFiles(props.variantIds, dest)
      .then((p) => alive && setPlan(p))
      .catch((e: unknown) => alive && setPlanError(toApiError(e).info));
    return () => {
      alive = false;
    };
  }, [dest, props.variantIds]);

  const destInfo = folders?.find((f) => f.folder_id === dest);
  const destLabel = destInfo ? folderName(destInfo) : dest !== null ? `フォルダ ${dest}` : "";
  const summary = plan ? describeMovePlan(plan) : null;
  const ok = async () => {
    if (dest === null || !plan || plan.movable === 0) return;
    setBusy(true);
    setError(null);
    try {
      await startMoveFiles(props.variantIds, dest, destLabel);
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={`フォルダへ移動（${formatCount(count)} 枚）`}
      width={620}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || !plan || plan.movable === 0}>
            移す
          </button>
        </>
      }
    >
      {foldersError && <p className="error-text">フォルダの一覧を読めません: {foldersError}</p>}
      <div className="field">
        <label htmlFor="move-dest">移動先</label>
        <FolderSelect id="move-dest" choices={choices} value={dest} onChange={setDest} disabled={busy} />
      </div>
      {plan?.dest_path && <PathBox path={plan.dest_path} box />}
      {dest !== null && !plan && !planError && <Busy>予定を確かめています…</Busy>}
      <ErrorBox error={planError} title="予定を作れません" />
      {summary && (
        <>
          {summary.lines.map((l) => (
            <p key={l}>{l}</p>
          ))}
          {summary.problems.length > 0 && (
            <div className="dlg-warn">
              <div>移せない写真（飛ばします）:</div>
              {summary.problems.map((l) => (
                <div key={l}>{l}</div>
              ))}
            </div>
          )}
          {plan && <PlanList plan={plan} />}
        </>
      )}
      <p className="hint">
        移動先に同じ名前のファイルがあれば、その写真は移しません（上書きしません）。現像設定・評価・キーワードはそのまま
        残ります。途中で失敗した写真は元の場所に戻します。
      </p>
      <ErrorBox error={error} />
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// 写真の名前の変更
// ---------------------------------------------------------------------------

interface RenameForm {
  mode: "template" | "name";
  name: string;
  template: string;
  customText: string;
  sequenceStart: string;
  sequenceDigits: number;
}

function isStoredRename(v: unknown): v is Partial<RenameForm> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function initialRenameForm(variantIds: VariantId[]): RenameForm {
  const stored = loadStored(RENAME_STORE_KEY);
  const s = isStoredRename(stored) ? stored : {};
  const single = variantIds.length === 1;
  const current = single ? searchCache.getLoadedById(variantIds[0]) : undefined;
  return {
    mode: single ? "name" : "template",
    name: current ? splitExtension(current.file_name).stem : "",
    template: typeof s.template === "string" && s.template !== "" ? s.template : "{date}-{seq}",
    customText: typeof s.customText === "string" ? s.customText : "",
    sequenceStart: typeof s.sequenceStart === "string" ? s.sequenceStart : "1",
    sequenceDigits: typeof s.sequenceDigits === "number" ? s.sequenceDigits : 4,
  };
}

/** 入力の名前の付け方（入力に誤りがあれば、その理由）。 */
function renameSpecOf(form: RenameForm): { spec: RenameSpec | null; problem: string | null } {
  if (form.mode === "name") {
    const problem = validateName(form.name);
    return problem ? { spec: null, problem } : { spec: { kind: "name", name: form.name.trim() }, problem: null };
  }
  if (form.template.trim() === "") return { spec: null, problem: "テンプレートを入れてください" };
  if ([...form.template].length > MAX_TEMPLATE_CHARS) return { spec: null, problem: `テンプレートは ${MAX_TEMPLATE_CHARS} 文字までです` };
  if ([...form.customText].length > MAX_CUSTOM_TEXT_CHARS)
    return { spec: null, problem: `任意の文字は ${MAX_CUSTOM_TEXT_CHARS} 文字までです` };
  const start = Number(form.sequenceStart.trim());
  if (!/^\d+$/.test(form.sequenceStart.trim())) return { spec: null, problem: "連番の開始番号は 0 以上の整数です" };
  if (!(form.sequenceDigits >= 1 && form.sequenceDigits <= 9)) return { spec: null, problem: "連番の桁数は 1〜9 です" };
  return {
    spec: {
      kind: "template",
      rule: {
        template: form.template,
        custom_text: form.customText,
        sequence_start: start,
        sequence_digits: form.sequenceDigits,
      },
    },
    problem: null,
  };
}

export function RenameFilesDialog(props: { variantIds: VariantId[] }) {
  const [form, setForm] = useState<RenameForm>(() => initialRenameForm(props.variantIds));
  const [plan, setPlan] = useState<FileRenamePlan | null>(null);
  const [planError, setPlanError] = useState<ErrorInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const templateRef = useRef<HTMLInputElement>(null);
  const count = props.variantIds.length;
  const single = count === 1;
  const update = (patch: Partial<RenameForm>) => setForm((f) => ({ ...f, ...patch }));
  const { spec, problem } = renameSpecOf(form);
  const specKey = spec ? JSON.stringify(spec) : null;

  useEffect(() => {
    setPlan(null);
    setPlanError(null);
    if (specKey === null) return;
    let alive = true;
    const t = setTimeout(() => {
      core
        .planRenameFiles(props.variantIds, JSON.parse(specKey) as RenameSpec)
        .then((p) => alive && setPlan(p))
        .catch((e: unknown) => alive && setPlanError(toApiError(e).info));
    }, PLAN_DELAY_MS);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [specKey, props.variantIds]);

  const addToken = (token: string) => {
    const el = templateRef.current;
    const { text, cursor } = insertToken(form.template, token, el?.selectionStart, el?.selectionEnd);
    update({ template: text });
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(cursor, cursor);
    });
  };

  const summary = plan ? describeRenamePlan(plan) : null;
  const canRun = spec !== null && plan !== null && plan.renamable > 0 && !busy;
  const ok = async () => {
    if (!spec || !canRun) return;
    setBusy(true);
    setError(null);
    try {
      const r = await core.renameFiles(props.variantIds, spec);
      if (form.mode === "template") {
        saveStored(RENAME_STORE_KEY, {
          template: form.template,
          customText: form.customText,
          sequenceStart: form.sequenceStart,
          sequenceDigits: form.sequenceDigits,
        });
      }
      notifyResult(describeMoveReport(r, "名前の変更"));
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
    }
  };
  const usesText = form.template.includes("{text}");
  const usesSeq = form.template.includes("{seq}");

  return (
    <Dialog
      title={`ファイルの名前を変える（${formatCount(count)} 枚）`}
      width={620}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={!canRun}>
            変える
          </button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (canRun) void ok();
        }}
      >
        {single && (
          <div className="dlg-row" role="radiogroup" aria-label="名前の付け方">
            <label className="dlg-check">
              <input type="radio" checked={form.mode === "name"} onChange={() => update({ mode: "name" })} />
              名前を入れる
            </label>
            <label className="dlg-check">
              <input type="radio" checked={form.mode === "template"} onChange={() => update({ mode: "template" })} />
              テンプレート
            </label>
          </div>
        )}
        {form.mode === "name" ? (
          <div className="field">
            <label htmlFor="rename-name">新しい名前</label>
            <input
              id="rename-name"
              autoFocus
              type="text"
              value={form.name}
              spellCheck={false}
              onChange={(e) => update({ name: e.target.value })}
            />
          </div>
        ) : (
          <>
            <div className="field">
              <label htmlFor="rename-template">テンプレート</label>
              <div className="dlg-row">
                <input
                  id="rename-template"
                  ref={templateRef}
                  className="dlg-grow"
                  type="text"
                  value={form.template}
                  spellCheck={false}
                  onChange={(e) => update({ template: e.target.value })}
                />
                <select aria-label="ひな形" value="" onChange={(e) => e.target.value && update({ template: e.target.value })}>
                  <option value="">ひな形…</option>
                  {TEMPLATE_PRESETS.map((p) => (
                    <option key={p.template} value={p.template}>
                      {p.label}
                    </option>
                  ))}
                </select>
              </div>
            </div>
            <div className="field">
              <span />
              <div className="import-tokens" aria-label="項目を入れる">
                {TEMPLATE_TOKENS.map((t) => (
                  <button
                    key={t.token}
                    type="button"
                    className="btn small ghost"
                    title={`${t.token} を入れる`}
                    onClick={() => addToken(t.token)}
                  >
                    {t.label}
                  </button>
                ))}
              </div>
            </div>
            {usesText && (
              <div className="field">
                <label htmlFor="rename-text">任意の文字</label>
                <input id="rename-text" type="text" value={form.customText} onChange={(e) => update({ customText: e.target.value })} />
              </div>
            )}
            {usesSeq && (
              <div className="field">
                <label htmlFor="rename-seq-start">連番</label>
                <div className="dlg-row">
                  <span className="muted">開始</span>
                  <input
                    id="rename-seq-start"
                    className="dlg-inline-num"
                    type="number"
                    min={0}
                    step={1}
                    value={form.sequenceStart}
                    onChange={(e) => update({ sequenceStart: e.target.value })}
                  />
                  <label htmlFor="rename-seq-digits" className="muted">
                    桁数
                  </label>
                  <input
                    id="rename-seq-digits"
                    className="dlg-inline-num"
                    type="number"
                    min={1}
                    max={9}
                    step={1}
                    value={form.sequenceDigits}
                    onChange={(e) => update({ sequenceDigits: Number(e.target.value) })}
                  />
                  <span className="hint">選んだ順（表示の順）に付けます</span>
                </div>
              </div>
            )}
          </>
        )}
      </form>
      {problem && <p className="hint">{problem}</p>}
      <ErrorBox error={planError} title="この名前にはできません" />
      {spec && !plan && !planError && <Busy>新しい名前を確かめています…</Busy>}
      {summary && (
        <>
          {summary.lines.map((l) => (
            <p key={l}>{l}</p>
          ))}
          {summary.problems.length > 0 && (
            <div className="dlg-warn">
              {summary.problems.map((l) => (
                <div key={l}>{l}</div>
              ))}
            </div>
          )}
          {plan && <PlanList plan={plan} />}
        </>
      )}
      <p className="hint">
        ディスクのファイルの名前を変えます（拡張子は変えません）。RAW と同時記録の JPEG は同じ名前にそろえます。同じ名前の
        ファイルがあれば、その写真は変えません（上書きしません）。
      </p>
      <ErrorBox error={error} />
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// フォルダの作成・名前の変更・移動
// ---------------------------------------------------------------------------

export function FolderNameDialog(props: { mode: "create" | "rename"; folderId: FolderId; currentName?: string }) {
  const [name, setName] = useState(props.mode === "rename" ? (props.currentName ?? "") : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const { folders } = useFolders();
  const folder = folders?.find((f) => f.folder_id === props.folderId);
  const problem = validateName(name);
  const unchanged = props.mode === "rename" && name.trim() === (props.currentName ?? "");
  const disabled = busy || problem !== null || unchanged;
  const ok = async () => {
    if (disabled) return;
    setBusy(true);
    setError(null);
    try {
      const f =
        props.mode === "create" ? await core.createFolder(props.folderId, name.trim()) : await core.renameFolder(props.folderId, name.trim());
      actions.notify("success", props.mode === "create" ? `フォルダ「${folderName(f)}」を作りました` : `フォルダの名前を「${folderName(f)}」にしました`);
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={props.mode === "create" ? "フォルダを作る" : "フォルダの名前を変える"}
      width={480}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={disabled}>
            {props.mode === "create" ? "作る" : "変える"}
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
          <label>{props.mode === "create" ? "作る場所" : "フォルダ"}</label>
          <PathBox path={folder?.path ?? null} empty="（読み込み中）" />
        </div>
        <div className="field">
          <label htmlFor="folder-name">名前</label>
          <input id="folder-name" autoFocus type="text" value={name} spellCheck={false} onChange={(e) => setName(e.target.value)} />
        </div>
      </form>
      {name.trim() !== "" && problem && <p className="hint">{problem}</p>}
      <p className="hint">
        {props.mode === "create"
          ? "ディスクにフォルダを作り、カタログに登録します。"
          : "ディスクのフォルダの名前も変えます（中の写真の登録はそのまま）。同じ名前のフォルダがあれば変えません。"}
      </p>
      <ErrorBox error={error} />
    </Dialog>
  );
}

export function MoveFolderDialog(props: { folderId: FolderId }) {
  const { folders, error: foldersError } = useFolders();
  const [dest, setDest] = useState<FolderId | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const folder = folders?.find((f) => f.folder_id === props.folderId);
  const choices = useMemo(
    () =>
      folderChoices(folders ?? [], { exclude: props.folderId, volume: folder?.volume_id ?? null }).filter(
        (c) => c.id !== folder?.parent_id,
      ),
    [folders, folder, props.folderId],
  );
  const ok = async () => {
    if (dest === null) return;
    setBusy(true);
    setError(null);
    try {
      const f = await core.moveFolder(props.folderId, dest);
      actions.notify("success", `フォルダを移しました: ${f.path ?? f.rel_path}`);
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
    }
  };
  return (
    <Dialog
      title="フォルダを移す"
      width={560}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || dest === null}>
            移す
          </button>
        </>
      }
    >
      {foldersError && <p className="error-text">フォルダの一覧を読めません: {foldersError}</p>}
      <div className="field">
        <label>移すフォルダ</label>
        <PathBox path={folder?.path ?? null} empty="（読み込み中）" />
      </div>
      <div className="field">
        <label htmlFor="move-folder-dest">移動先（この中へ）</label>
        <FolderSelect id="move-folder-dest" choices={choices} value={dest} onChange={setDest} disabled={busy} />
      </div>
      <p className="hint">
        ディスクの上でフォルダごと移します（同じドライブの中だけ。別のドライブへは写真を選んで「フォルダへ移動」で移します）。
        移動先に同じ名前のフォルダがあれば移しません。
      </p>
      <ErrorBox error={error} />
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// 再リンク
// ---------------------------------------------------------------------------

export function RelinkDialog(props: { variantIds: VariantId[] | null }) {
  const [dir, setDir] = useState<string>("");
  const [recursive, setRecursive] = useState(true);
  const [searching, setSearching] = useState<number | null>(null);
  const [result, setResult] = useState<RelinkSearch | null>(null);
  const [choices, setChoices] = useState<Map<number, string | null>>(new Map());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const alive = useRef(true);
  useEffect(
    () => () => {
      alive.current = false;
    },
    [],
  );

  const choose = async () => {
    try {
      const p = await chooseFolder("見つからない写真を探すフォルダを選ぶ", dir || null);
      if (p) setDir(p);
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    }
  };

  const search = async () => {
    if (dir.trim() === "") return;
    setError(null);
    setResult(null);
    let jobId: number;
    try {
      jobId = await core.findRelinkCandidates(props.variantIds, dir.trim(), recursive);
    } catch (e) {
      setError(toApiError(e).info);
      return;
    }
    actions.trackJob(jobId, "find_relink_candidates");
    setSearching(jobId);
    try {
      const info = await waitJobUntilClosed(jobId);
      if (!alive.current || info === "closed") return;
      if (info.result?.type === "relink_candidates") {
        const r = info.result;
        setResult(r);
        setChoices(new Map(r.targets.map((t) => [t.file_id, defaultRelinkChoice(t)])));
      } else if (info.state === "cancelled") {
        actions.notify("info", "候補の探索を取り消しました");
      } else if (info.error) {
        setError(info.error);
      }
    } catch (e) {
      if (alive.current) setError(toApiError(e).info);
    } finally {
      if (alive.current) setSearching(null);
    }
  };

  const cancel = () => {
    if (searching !== null) void core.cancelJob(searching).catch((e: unknown) => actions.reportError(e, "取り消せません"));
  };

  const requests = result ? relinkRequests(result.targets, choices) : [];
  const ok = async () => {
    if (requests.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      const r = await core.relinkFiles(requests);
      notifyResult(describeRelinkReport(r));
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
    }
  };
  const close = () => {
    cancel();
    actions.closeDialog();
  };
  const targetText = props.variantIds === null ? "すべての見つからない写真" : `選んだ ${formatCount(props.variantIds.length)} 枚の中の見つからない写真`;

  return (
    <Dialog
      title="見つからない写真を探す"
      width={720}
      onClose={close}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={close} disabled={busy}>
            {searching !== null ? "取り消して閉じる" : "キャンセル"}
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || searching !== null || requests.length === 0}>
            {requests.length > 0 ? `${formatCount(requests.length)} 個を結び付ける` : "結び付ける"}
          </button>
        </>
      }
    >
      <p>
        {targetText}の元ファイルを、選んだフォルダから探します。名前・大きさ・撮影日時・内容のハッシュで照らし合わせ、
        結び付ける候補を選んでから結び付け直します（ファイルは動かしません）。
      </p>
      <div className="field">
        <label>探すフォルダ</label>
        <div className="dlg-row">
          <PathBox path={dir || null} empty="（選んでください）" />
          <button type="button" className="btn small" onClick={() => void choose()} disabled={searching !== null || busy}>
            選ぶ…
          </button>
        </div>
      </div>
      <label className="dlg-check">
        <input type="checkbox" checked={recursive} onChange={(e) => setRecursive(e.target.checked)} disabled={searching !== null} />
        サブフォルダも探す
      </label>
      <div className="dlg-row">
        <button
          type="button"
          className="btn"
          onClick={() => void search()}
          disabled={dir.trim() === "" || searching !== null || busy}
        >
          探す
        </button>
        {searching !== null && (
          <>
            <Busy>探しています…（進捗は上の帯に出ます）</Busy>
            <button type="button" className="btn small" onClick={cancel}>
              取り消す
            </button>
          </>
        )}
      </div>
      {result && (
        <>
          <p>
            {formatCount(result.scanned_files)} 個のファイルを調べました。見つからない写真のファイル {formatCount(result.targets.length)}{" "}
            個のうち、候補のあるもの {formatCount(result.targets.filter((t) => t.candidates.length > 0).length)} 個。
            {result.cancelled ? "（途中で取り消しました）" : ""}
          </p>
          {result.unreadable.length > 0 && (
            <p className="hint">読めなかったフォルダ・ファイル: {formatCount(result.unreadable.length)} 個</p>
          )}
          {result.targets.length === 0 ? (
            <p className="hint">見つからない写真はありません。</p>
          ) : (
            <div className="dlg-list" style={{ maxHeight: 320 }} aria-label="候補">
              {result.targets.slice(0, PLAN_LIST_LIMIT * 4).map((t) => (
                <div key={t.file_id} className="dlg-list-item column">
                  <div className="dlg-path" title={t.last_path ?? undefined}>
                    {t.name}
                    <span className="hint">（前の場所: {t.last_path ?? "不明"}）</span>
                  </div>
                  {t.candidates.length === 0 ? (
                    <div className="hint">候補はありません</div>
                  ) : (
                    <select
                      aria-label={`${t.name} の候補`}
                      value={choices.get(t.file_id) ?? ""}
                      onChange={(e) =>
                        setChoices((m) => new Map(m).set(t.file_id, e.target.value === "" ? null : e.target.value))
                      }
                    >
                      <option value="">結び付けない</option>
                      {t.candidates.map((c) => (
                        <option key={c.path} value={c.path}>
                          {isSafeMatch(c.matched) ? "" : "⚠ "}
                          {c.path}（{RELINK_MATCH_LABEL[c.matched]}）
                        </option>
                      ))}
                    </select>
                  )}
                </div>
              ))}
            </div>
          )}
          {requests.some((r) => r.accept_changed) && (
            <div className="dlg-warn">
              内容が登録のものと違う候補（⚠）を選んでいます。結び付けると、そのファイルを読み直します（現像設定などは残ります）。
            </div>
          )}
        </>
      )}
      <ErrorBox error={error} />
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// フォルダの場所の指定し直し
// ---------------------------------------------------------------------------

export function RelocateFolderDialog(props: { folderId: FolderId }) {
  const { folders } = useFolders();
  const folder = folders?.find((f) => f.folder_id === props.folderId);
  const [to, setTo] = useState<string>("");
  const [plan, setPlan] = useState<RelocatePlan | null>(null);
  const [planError, setPlanError] = useState<ErrorInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  useEffect(() => {
    setPlan(null);
    setPlanError(null);
    if (to === "") return;
    let alive = true;
    core
      .planRelocateFolder(props.folderId, to)
      .then((p) => alive && setPlan(p))
      .catch((e: unknown) => alive && setPlanError(toApiError(e).info));
    return () => {
      alive = false;
    };
  }, [to, props.folderId]);

  const choose = async () => {
    try {
      const p = await chooseFolder("フォルダの新しい場所を選ぶ", to || null);
      if (p) setTo(p);
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    }
  };
  const ok = async () => {
    if (!plan || plan.problem) return;
    setBusy(true);
    setError(null);
    try {
      const r = await core.relocateFolder(props.folderId, to);
      actions.notify("success", `フォルダの場所を指定し直しました: ${r.to}`, describeRelocatePlan(r));
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
    }
  };
  return (
    <Dialog
      title="フォルダの場所を指定し直す"
      width={620}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || !plan || plan.problem !== null}>
            指定し直す
          </button>
        </>
      }
    >
      <p>
        フォルダを OS の側で移した・名前を変えたときに、カタログの中のフォルダの場所を新しい場所に結び付け直します（配下の
        フォルダも。ファイルは動かしません）。
      </p>
      <div className="field">
        <label>前の場所</label>
        <PathBox path={plan?.from ?? folder?.path ?? null} empty="（読み込み中）" />
      </div>
      <div className="field">
        <label>新しい場所</label>
        <div className="dlg-row">
          <PathBox path={to || null} empty="（選んでください）" />
          <button type="button" className="btn small" onClick={() => void choose()} disabled={busy}>
            選ぶ…
          </button>
        </div>
      </div>
      {to !== "" && !plan && !planError && <Busy>新しい場所を確かめています…</Busy>}
      <ErrorBox error={planError} title="新しい場所を確かめられません" />
      {plan && (
        <>
          <p>{describeRelocatePlan(plan)}</p>
          {plan.problem && <div className="dlg-warn">{plan.problem}</div>}
          {!plan.problem && plan.files > 0 && plan.found === 0 && (
            <div className="dlg-warn">
              新しい場所に、登録した写真のファイルが 1 つも見つかりません。場所が正しいか確かめてください（指定し直しても、
              見つからない写真のままです。もう一度指定し直せます）。
            </div>
          )}
        </>
      )}
      <ErrorBox error={error} />
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// フォルダをカタログから除く
// ---------------------------------------------------------------------------

export function RemoveFolderDialog(props: { folderId: FolderId }) {
  const { folders, error: foldersError } = useFolders();
  const folder = folders?.find((f) => f.folder_id === props.folderId);
  const current = useApp((s) => s.filter.folder_id ?? null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const ok = async () => {
    if (!folders) return;
    const subtree = folderSubtreeIds(folders, props.folderId);
    setBusy(true);
    setError(null);
    try {
      const n = await core.removeFolder(props.folderId);
      // 表示中のフォルダ（配下を含む）を除いたら、すべての写真の表示に戻す。
      if (current !== null && subtree.includes(current)) actions.showAll();
      actions.notify("success", `フォルダをカタログから除きました（${formatCount(n)} 個）: ${folder?.path ?? folder?.rel_path ?? ""}`);
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
    }
  };
  return (
    <Dialog
      title="フォルダをカタログから除く"
      width={560}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || !folders}>
            除く
          </button>
        </>
      }
    >
      {foldersError && <p className="error-text">フォルダの一覧を読めません: {foldersError}</p>}
      <div className="field">
        <label>フォルダ</label>
        <PathBox path={folder?.path ?? folder?.rel_path ?? null} empty="（読み込み中）" />
      </div>
      {folders && <p>{describeFolderRemoval(folders, props.folderId)}</p>}
      <p className="hint">ディスクにフォルダがあれば、もう一度取り込むとカタログに戻ります。</p>
      <ErrorBox error={error} />
    </Dialog>
  );
}
