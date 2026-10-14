// 取り込みのダイアログ（IMP-01〜IMP-06・VID-01）: フォルダを選び、その場で登録するか、コピーしてから登録する。
//
// - 方法: 「その場で追加」（元のファイルはコピー・移動しない。IMP-01）か「コピーして追加」（SD カードなどから、
//   コピー先のフォルダへ規則に従ってコピーし、内容を照合してから登録する。元のファイルは変更・削除しない。IMP-02）。
// - コピー: コピー先のフォルダ、フォルダの規則（撮影日別・1 つのフォルダ・元の構成のまま）、ファイル名の規則
//   （テンプレート・連番・任意の文字。IMP-03）。名前の例はコアの import_name_preview で作る（見本の写真）。
// - 取り込み時に付けるもの（IMP-05）: キーワード（「場所 > 京都, 家族」。LIB-09）と説明のメタデータ（作成者・
//   著作権など。LIB-15）を、新しく登録した写真すべてに付ける。
// - 重複と思われるもの（IMP-04）・RAW と同時記録の JPEG（IMP-06）: 初めの値はカタログの設定で、変えたら
//   カタログの設定に保存して次の既定にする（importLogic.ts の importChoiceRequest）。
// - それ以外の設定（方法・コピー先・規則・付けるもの・サブフォルダ）は、取り込みを始めたときにアプリの設定に
//   保存し、次に開いたときの初めの値にする（genzo-app の ImportPreferences）。
//
// ドロップで渡されたもの（Dialogs.tsx）は、アプリの path_kinds で種類を確かめ、フォルダではなかったもの・
// 見つからなかったものに注意を出す（名前からは推定しない。"2024.Kyoto" のようなフォルダもあるため）。
// 「参照…」で OS のフォルダの選択から選んだものはフォルダなので、確かめない。

import { useEffect, useMemo, useRef, useState } from "react";

import * as appApi from "../api/app";
import * as core from "../api/core";
import type { MetadataField, NamePreview } from "../api/types";
import { Dialog } from "../components/Dialog";
import { COPYRIGHT_STATUS_OPTIONS, MAX_METADATA_CHARS, METADATA_GROUPS } from "../library/metadataEdit";
import { actions, appStore, useApp } from "../state/app";
import { baseName } from "./dialogFormat";
import { onImportFolders, takeImportFolders } from "./importDrop";
import {
  FOLDER_RULE_OPTIONS,
  type ImportChoice,
  type ImportForm,
  type PathKind,
  TEMPLATE_PRESETS,
  TEMPLATE_TOKENS,
  copySettingsOf,
  importChoiceFrom,
  importChoiceRequest,
  importFormFrom,
  importOptionsOf,
  importPreferencesOf,
  insertToken,
  metadataCount,
  nonFolderPaths,
  validateImportForm,
  videoToolsWarning,
} from "./importLogic";
import { chooseFolder } from "./native";
import { PathBox } from "./parts";
import { loadStored, saveStored } from "./storage";

const LAST_FOLDER_KEY = "import.lastFolder";

/** 名前の例を作り直すまでの待ち（入力の途中で毎回コアに問い合わせない）。 */
const PREVIEW_DELAY_MS = 150;

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

/** コピーの設定の名前の例（コアに問い合わせる。コピーしないときは null）。 */
function useNamePreview(form: ImportForm): NamePreview | null {
  const copy = copySettingsOf(form);
  const key = copy ? JSON.stringify({ ...copy, destination: "" }) : null;
  const [preview, setPreview] = useState<NamePreview | null>(null);
  useEffect(() => {
    if (key === null) {
      setPreview(null);
      return;
    }
    let alive = true;
    const timer = setTimeout(() => {
      core
        .importNamePreview(JSON.parse(key))
        .then((p) => alive && setPreview(p))
        .catch((e) => alive && setPreview({ examples: [], error: String(e?.message ?? e) }));
    }, PREVIEW_DELAY_MS);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [key]);
  return preview;
}

export function ImportDialog() {
  const tools = useApp((s) => s.catalog?.tools ?? null);
  const coreSettings = useApp((s) => s.catalog?.core_settings ?? null);
  const savedPrefs = useApp((s) => s.status?.settings.import ?? null);
  const [choice, setChoice] = useState<ImportChoice>(() => importChoiceFrom(coreSettings));
  const [form, setForm] = useState<ImportForm>(() => importFormFrom(savedPrefs));
  const [folders, setFolders] = useState<string[]>(() => takeImportFolders() ?? []);
  // 「参照…」で OS のフォルダの選択から選んだもの（フォルダと分かっている）。
  const [chosen, setChosen] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const kinds = usePathKinds(folders, chosen);
  const preview = useNamePreview(form);
  const templateRef = useRef<HTMLInputElement>(null);
  const errors = useMemo(() => validateImportForm(form), [form]);
  const update = (patch: Partial<ImportForm>) => setForm((f) => ({ ...f, ...patch }));
  const setField = (field: MetadataField, value: string) =>
    setForm((f) => ({ ...f, fields: { ...f.fields, [field]: value } }));

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

  const chooseDestination = async () => {
    setChoosing(true);
    try {
      const p = await chooseFolder("コピー先のフォルダを選ぶ", form.destination || null);
      if (p) update({ destination: p });
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    } finally {
      setChoosing(false);
    }
  };

  const addToken = (token: string) => {
    const el = templateRef.current;
    const { text, cursor } = insertToken(form.template, token, el?.selectionStart, el?.selectionEnd);
    update({ template: text });
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(cursor, cursor);
    });
  };

  const previewError = form.copy ? (preview?.error ?? null) : null;
  const canStart = folders.length > 0 && errors.length === 0 && previewError === null;

  const start = () => {
    if (!canStart) return;
    saveStored(LAST_FOLDER_KEY, folders[0]);
    actions.closeDialog();
    const { options, update: settingsUpdate } = importChoiceRequest(coreSettings, choice);
    if (settingsUpdate) {
      // 選んだ設定を次の既定にする（保存できなくても、今回の取り込みは選んだ設定で行う）。
      core
        .updateSettings(settingsUpdate)
        .then((saved) => actions.setCoreSettings(saved))
        .catch((e) => actions.reportError(e, "取り込みの設定を既定として保存できません"));
    }
    // 方法・コピー先・規則・付けるものは、アプリの設定に保存して次の初めの値にする。
    appApi
      .saveImportPreferences(importPreferencesOf(form))
      .then((settings) => {
        const status = appStore.get().status;
        if (status) actions.setStatus({ ...status, settings });
      })
      .catch((e) => actions.reportError(e, "取り込みの設定を保存できません"));
    // 1 つずつ順に取り込む（途中でカタログを閉じた・取り込みが取り消されたら、残りはやめる）。
    void actions.importFolders(folders, form.recursive, importOptionsOf(form, options));
  };

  const toolsWarning = videoToolsWarning(tools);
  const notFolders = nonFolderPaths(folders, kinds);
  const tagCount = metadataCount(form);
  const usesText = form.template.includes("{text}");
  const usesSeq = form.template.includes("{seq}");

  return (
    <Dialog
      title="取り込み"
      width={640}
      onClose={actions.closeDialog}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog}>
            キャンセル
          </button>
          <button className="btn primary" disabled={!canStart} onClick={start}>
            {form.copy ? "コピーして取り込む" : "取り込む"}
          </button>
        </>
      }
    >
      <div className="dlg-section">
        <h3>{folders.length > 1 ? `取り込むフォルダ（${folders.length} 個）` : "取り込むフォルダ"}</h3>
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
      <div className="dlg-section">
        <h3>方法</h3>
        <div className="dlg-options" role="radiogroup" aria-label="取り込みの方法">
          <label className={`dlg-option${form.copy ? "" : " on"}`}>
            <input type="radio" name="import-mode" checked={!form.copy} onChange={() => update({ copy: false })} />
            <span className="title">その場で追加</span>
            <span className="desc">元のファイルをコピー・移動せず、今の場所のままカタログに登録します</span>
          </label>
          <label className={`dlg-option${form.copy ? " on" : ""}`}>
            <input type="radio" name="import-mode" checked={form.copy} onChange={() => update({ copy: true })} />
            <span className="title">コピーして追加</span>
            <span className="desc">
              SD カードなどから、コピー先のフォルダへ規則に従ってコピーし、内容を元と照合してから登録します（元のファイルは変更・削除しません）
            </span>
          </label>
        </div>
      </div>
      {form.copy && (
        <div className="dlg-section">
          <h3>コピー先</h3>
          <div className="field">
            <label>フォルダ</label>
            <div className="dlg-row">
              <div className="dlg-grow">
                <PathBox path={form.destination} empty="コピー先のフォルダを選んでください" box />
              </div>
              <button className="btn" onClick={() => void chooseDestination()} disabled={choosing}>
                参照…
              </button>
            </div>
          </div>
          <div className="field">
            <label htmlFor="import-folder-rule">中のフォルダ</label>
            <select
              id="import-folder-rule"
              value={form.folderRule}
              onChange={(e) => update({ folderRule: e.target.value as ImportForm["folderRule"] })}
            >
              {FOLDER_RULE_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>
          <label className="dlg-check">
            <input type="checkbox" checked={form.rename} onChange={(e) => update({ rename: e.target.checked })} />
            <span>
              ファイル名を変える
              <span className="desc">外すと元の名前のままコピーします（同じ名前があれば「-1」などを付けます。上書きしません）</span>
            </span>
          </label>
          {form.rename && (
            <>
              <div className="field">
                <label htmlFor="import-template">ファイル名</label>
                <div className="dlg-row">
                  <input
                    id="import-template"
                    ref={templateRef}
                    className="dlg-grow"
                    type="text"
                    value={form.template}
                    spellCheck={false}
                    onChange={(e) => update({ template: e.target.value })}
                  />
                  <select
                    aria-label="ひな形"
                    value=""
                    onChange={(e) => e.target.value && update({ template: e.target.value })}
                  >
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
                  <label htmlFor="import-custom-text">任意の文字</label>
                  <input
                    id="import-custom-text"
                    type="text"
                    value={form.customText}
                    placeholder="例: 京都旅行"
                    onChange={(e) => update({ customText: e.target.value })}
                  />
                </div>
              )}
              {usesSeq && (
                <div className="field">
                  <label htmlFor="import-seq-start">連番</label>
                  <div className="dlg-row">
                    <span className="muted">開始</span>
                    <input
                      id="import-seq-start"
                      className="dlg-inline-num"
                      type="number"
                      min={0}
                      step={1}
                      value={form.sequenceStart}
                      onChange={(e) => update({ sequenceStart: e.target.value })}
                    />
                    <label htmlFor="import-seq-digits" className="muted">
                      桁数
                    </label>
                    <input
                      id="import-seq-digits"
                      className="dlg-inline-num"
                      type="number"
                      min={1}
                      max={9}
                      step={1}
                      value={form.sequenceDigits}
                      onChange={(e) => update({ sequenceDigits: Number(e.target.value) })}
                    />
                    <span className="hint">RAW と JPEG の組は 1 つに数えます</span>
                  </div>
                </div>
              )}
            </>
          )}
          <div className="import-preview">
            <div className="hint">名前の例（見本の写真。撮影日時の順に連番を付けます）</div>
            {previewError ? (
              <div className="dlg-field-error">{previewError}</div>
            ) : (
              preview?.examples.map((e) => (
                <div key={e.source} className="import-preview-row" title={`撮影日時 ${e.captured}`}>
                  <span className="muted">{e.source}</span>
                  <span className="muted"> → </span>
                  <span>{e.destination}</span>
                </div>
              ))
            )}
          </div>
        </div>
      )}
      <details className="import-tags">
        <summary>
          取り込み時に付けるキーワード・メタデータ{tagCount > 0 ? `（${tagCount} 件）` : ""}
        </summary>
        <p className="hint">
          新しく登録した写真すべてに付けます（登録済みの写真は変えません）。空の項目は付けません。元のファイルには書き込みません。
        </p>
        <div className="field">
          <label htmlFor="import-keywords">キーワード</label>
          <input
            id="import-keywords"
            type="text"
            value={form.keywords}
            placeholder="場所 > 京都, 家族"
            onChange={(e) => update({ keywords: e.target.value })}
          />
        </div>
        {METADATA_GROUPS.map((g) => (
          <details key={g.id} className="import-tag-group" open={g.id === "creator" || g.id === "copyright"}>
            <summary>{g.title}</summary>
            {g.fields.map((d) => (
              <div key={d.field} className="field">
                <label htmlFor={`import-meta-${d.field}`}>{d.label}</label>
                {d.select ? (
                  <select
                    id={`import-meta-${d.field}`}
                    value={form.fields[d.field] ?? ""}
                    onChange={(e) => setField(d.field, e.target.value)}
                  >
                    {COPYRIGHT_STATUS_OPTIONS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.value === "" ? "付けない" : o.label}
                      </option>
                    ))}
                  </select>
                ) : d.multiline ? (
                  <textarea
                    id={`import-meta-${d.field}`}
                    rows={2}
                    maxLength={MAX_METADATA_CHARS}
                    value={form.fields[d.field] ?? ""}
                    onChange={(e) => setField(d.field, e.target.value)}
                  />
                ) : (
                  <input
                    id={`import-meta-${d.field}`}
                    type={d.inputType ?? "text"}
                    maxLength={MAX_METADATA_CHARS}
                    value={form.fields[d.field] ?? ""}
                    onChange={(e) => setField(d.field, e.target.value)}
                  />
                )}
              </div>
            ))}
          </details>
        ))}
      </details>
      <label className="dlg-check">
        <input type="checkbox" checked={form.recursive} onChange={(e) => update({ recursive: e.target.checked })} />
        <span>
          サブフォルダも含める
          <span className="desc">フォルダの中のフォルダもすべて取り込みます</span>
        </span>
      </label>
      <label className="dlg-check">
        <input
          type="checkbox"
          checked={choice.skipDuplicates}
          onChange={(e) => setChoice((c) => ({ ...c, skipDuplicates: e.target.checked }))}
        />
        <span>
          重複と思われるものを取り込まない
          <span className="desc">
            内容が同じファイルが別の場所で登録済みなら登録しません（大きさと先頭・末尾で候補を探し、ファイル全体の内容を比べて確かめます）。取り込まなかったファイルは結果に出します
          </span>
        </span>
      </label>
      <label className="dlg-check">
        <input
          type="checkbox"
          checked={choice.separateRawJpeg}
          onChange={(e) => setChoice((c) => ({ ...c, separateRawJpeg: e.target.checked }))}
        />
        <span>
          RAW と同時記録の JPEG を別々の写真として取り込む
          <span className="desc">
            外すと、同じフォルダで拡張子だけが違う RAW と JPEG を 1 枚の写真（RAW が主、JPEG は付属）にまとめます
          </span>
        </span>
      </label>
      {errors.length > 0 && (
        <div className="dlg-warn">
          {errors.map((e) => (
            <div key={e}>{e}</div>
          ))}
        </div>
      )}
      <div className="dlg-note">
        {form.copy
          ? "コピーは一時ファイルに書いてディスクに確定し、元のファイルと内容（ハッシュ）が同じことを確かめてから名前を付けます。コピー先に同じ名前のファイルがあっても上書きしません。失敗・取り消しのときは、この取り込みで作ったファイルだけを片付けます（元のファイルには触れません。元を消す機能はありません）。RAW と同時記録の JPEG は同じ名前で一緒にコピーします。"
          : "元のファイルはコピー・移動せず、今の場所のままカタログに登録します（元のファイルは変更しません）。同じフォルダをもう一度取り込んでも二重には登録せず、新しいファイルと変わったファイルだけを処理します。"}{" "}
        対象は ARW・DNG・JPEG・TIFF・PNG の写真と、MP4・MOV の動画です。設定は次に取り込むときの既定として覚えます。
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
