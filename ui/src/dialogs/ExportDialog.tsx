// 書き出しのダイアログ（EXP-01〜05）: 書き出しプリセット・形式・色空間・大きさ・解像度・出力シャープ・透かし・GPS の
// 削除・動画・同じ名前の扱い・ファイル名・書き出し先。
//
// 開始したら閉じて、進捗は上の帯のジョブの表示に出す。終わったら結果（書いた・飛ばした・失敗の数と理由）を
// 通知する（詳細の全体は、上の帯の「結果」で後から見直せる）。前回の設定・書き出し先・選んだプリセットは、ブラウザに
// 保存して次に使う。
//
// 書き出しプリセット（EXP-02）: 上の「プリセット」で選ぶと入力をその値にする（書き出し先のないプリセットでは今の
// 書き出し先のまま）。「新しく保存…」で今の入力をプリセットにし（書き出し先を含めるかを選ぶ）、「更新」で選んだ
// プリセットを今の入力で置き換える。名前の変更・削除・ファイルへの書き出し・読み込みは「…」から（このダイアログの
// 中で行う。ダイアログは同時に 1 つなので、ほかのダイアログは開かない）。プリセットはアプリに保存し、ほかの
// カタログ・CLI（genzo export --preset）でも使える。
//
// ファイル名（EXP-02）: 「元のファイル名」か、取り込みのリネームと同じテンプレート（撮影日・連番・元の名前など）。
// 名前の例はコアの export_name_preview で作る（見本の写真）。
//
// 出力シャープ・透かし（EXP-03）: 透かしの文字のフォントは、利用者が OS のフォントのファイルを選ぶ（アプリは同梱しない。
// 最初の場所はコアの system_font_dirs）。透かしの見本はコアの watermark_preview（ワーカーで作り、灰色の濃淡の見本に
// 重ねた PNG）を、入力を変えて少し待ってから作り直して表示する。動画（EXP-05）は元のファイルのままコピーする（選んだ
// 写真のうち動画と分かっているものの数で注意書きを出す）。

import { useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type {
  ErrorInfo,
  ExportFormat,
  NamePreview,
  WatermarkPreview,
  OutputColorSpace,
  PresetInfo,
  RenameRule,
  VariantId,
} from "../api/types";
import { Dialog } from "../components/Dialog";
import { MenuButton } from "../components/Menu";
import { groupPresets, presetFolders, validateFolderName, validatePresetName } from "../develop/presetLogic";
import { actions, useApp } from "../state/app";
import { exportPresetFile, importPresetFiles, reloadPresets, usePresets } from "../state/presets";
import { waitJobUntilClosed } from "./catalogActions";
import { formatCount } from "./dialogFormat";
import {
  COLOR_SPACE_OPTIONS,
  CONFLICT_OPTIONS,
  type ExportForm,
  FORMAT_OPTIONS,
  JPEG_QUALITY_MAX,
  JPEG_QUALITY_MIN,
  MAX_LONG_EDGE,
  DEFAULT_RESOLUTION,
  MAX_FONT_INDEX,
  RESOLUTION_MAX,
  RESOLUTION_MIN,
  SHARPEN_AMOUNT_OPTIONS,
  SHARPEN_MEDIA_OPTIONS,
  WATERMARK_ANCHOR_OPTIONS,
  WATERMARK_MARGIN_MAX,
  WATERMARK_OPACITY_MAX,
  WATERMARK_OPACITY_MIN,
  WATERMARK_SIZE_MAX,
  WATERMARK_SIZE_MIN,
  colorSpaceLabel,
  describeExportJob,
  describeExportPreset,
  formFromPreset,
  formMatchesPreset,
  hasErrors,
  presetFromForm,
  restoreExportForm,
  restoreExportPresetId,
  savedExport,
  switchWatermarkKind,
  toExportSettings,
  toFileNaming,
  toWatermark,
  validateExportForm,
  videoNotice,
} from "./exportLogic";
import { TEMPLATE_PRESETS, TEMPLATE_TOKENS, insertToken } from "./importLogic";
import { chooseFolder, chooseFontFile, chooseWatermarkImage } from "./native";
import { ErrorBox, PathBox } from "./parts";
import { notifyResult } from "./resultLog";
import { loadStored, saveStored } from "./storage";

const STORE_KEY = "export.last";

/** 名前の例を問い合わせるまでの時間（ミリ秒。入力の途中で問い合わせすぎない。仮置き）。 */
const PREVIEW_DELAY_MS = 250;

/** 書き出しの終わりを待って、結果を知らせる。 */
async function followExport(jobId: number): Promise<void> {
  try {
    const info = await waitJobUntilClosed(jobId);
    // 終わる前にカタログを閉じた（取り消したことは閉じる処理が知らせる）。
    if (info === "closed") return;
    const notice = describeExportJob(info);
    if (notice) notifyResult(notice);
  } catch (e) {
    actions.reportError(e, "書き出しの結果を受け取れません");
  }
}

/** ファイル名の例（コアに問い合わせる。入力が正しくなければ null）。 */
function useExportNamePreview(form: ExportForm): NamePreview | null {
  const naming = toFileNaming(form);
  const settings = toExportSettings(form);
  const key = settings ? JSON.stringify({ naming, format: settings.format }) : null;
  const [preview, setPreview] = useState<NamePreview | null>(null);
  useEffect(() => {
    if (key === null) {
      setPreview(null);
      return;
    }
    let alive = true;
    const timer = setTimeout(() => {
      const { naming, format } = JSON.parse(key) as { naming: RenameRule | null; format: ExportFormat };
      core
        .exportNamePreview(naming, format)
        .then((p) => alive && setPreview(p))
        .catch((e) => alive && setPreview({ examples: [], error: toApiError(e).info.message }));
    }, PREVIEW_DELAY_MS);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [key]);
  return preview;
}

/** 透かしの見本を作り直すまでの時間（ミリ秒。入力の途中で作りすぎない。仮置き）。 */
const WATERMARK_PREVIEW_DELAY_MS = 400;

/** 透かしの見本（コアに問い合わせる。透かしを付けない・入力が正しくなければ null）。 */
function useWatermarkPreview(form: ExportForm): { preview: WatermarkPreview | null; error: string | null; busy: boolean } {
  const watermark = toWatermark(form);
  const key = watermark ? JSON.stringify(watermark) : null;
  const [state, setState] = useState<{ preview: WatermarkPreview | null; error: string | null; busy: boolean }>({
    preview: null,
    error: null,
    busy: false,
  });
  useEffect(() => {
    if (key === null) {
      setState({ preview: null, error: null, busy: false });
      return;
    }
    let alive = true;
    setState((s) => ({ ...s, busy: true }));
    const timer = setTimeout(() => {
      core
        .watermarkPreview(JSON.parse(key))
        .then((p) => alive && setState({ preview: p, error: null, busy: false }))
        .catch((e) => alive && setState({ preview: null, error: toApiError(e).info.message, busy: false }));
    }, WATERMARK_PREVIEW_DELAY_MS);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [key]);
  return state;
}

/** 解像度と出力シャープの欄（EXP-03）。 */
function SharpeningFields(props: { form: ExportForm; update: (patch: Partial<ExportForm>) => void; error?: string }) {
  const { form, update } = props;
  return (
    <>
      <div className="field">
        <label htmlFor="export-ppi">解像度</label>
        <div className="dlg-row">
          <input
            id="export-ppi"
            className="dlg-inline-num"
            type="number"
            min={RESOLUTION_MIN}
            max={RESOLUTION_MAX}
            step={1}
            value={form.resolution}
            onChange={(e) => update({ resolution: e.target.value })}
          />
          <span className="muted">ppi</span>
          <span className="hint">
            画素数は変えません（印刷の大きさの目安。既定 {DEFAULT_RESOLUTION}。紙の出力シャープの半径にも使います）
          </span>
        </div>
      </div>
      {props.error && <div className="dlg-field-error">{props.error}</div>}
      <div className="field">
        <label>出力シャープ</label>
        <div className="dlg-row">
          <label className="dlg-check" style={{ margin: 0 }}>
            <input type="checkbox" checked={form.sharpen} onChange={(e) => update({ sharpen: e.target.checked })} />
            かける
          </label>
          <select
            aria-label="出力シャープの対象"
            value={form.sharpenMedia}
            disabled={!form.sharpen}
            onChange={(e) => update({ sharpenMedia: e.target.value as ExportForm["sharpenMedia"] })}
          >
            {SHARPEN_MEDIA_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <select
            aria-label="出力シャープの適用量"
            value={form.sharpenAmount}
            disabled={!form.sharpen}
            onChange={(e) => update({ sharpenAmount: e.target.value as ExportForm["sharpenAmount"] })}
          >
            {SHARPEN_AMOUNT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </div>
      </div>
      {form.sharpen && (
        <div className="field">
          <span />
          <span className="hint">
            縮小の後に、書き出す画像の画素の大きさに合わせてかけます（紙は解像度から半径を決めます。現像のシャープとは別です）
          </span>
        </div>
      )}
    </>
  );
}

/** 透かしの欄（EXP-03）。 */
function WatermarkFields(props: { form: ExportForm; update: (patch: Partial<ExportForm>) => void; error?: string }) {
  const { form, update } = props;
  const { preview, error, busy } = useWatermarkPreview(form);
  const [fontDir, setFontDir] = useState<string | null>(null);
  const pickFont = async () => {
    try {
      let dir = fontDir;
      if (!form.watermarkFont && dir === null) {
        dir = (await core.systemFontDirs())[0] ?? "";
        setFontDir(dir);
      }
      const p = await chooseFontFile("透かしのフォントのファイルを選ぶ", form.watermarkFont || dir || null);
      if (p) update({ watermarkFont: p });
    } catch (e) {
      actions.reportError(e, "フォントのファイルを選べません");
    }
  };
  const pickImage = async () => {
    try {
      const p = await chooseWatermarkImage("透かしの画像を選ぶ", form.watermarkImage || null);
      if (p) update({ watermarkImage: p });
    } catch (e) {
      actions.reportError(e, "画像のファイルを選べません");
    }
  };
  return (
    <>
      <div className="field">
        <label>透かし</label>
        <div className="dlg-row">
          <label className="dlg-check" style={{ margin: 0 }}>
            <input type="checkbox" checked={form.watermark} onChange={(e) => update({ watermark: e.target.checked })} />
            付ける
          </label>
          {form.watermark && (
            <>
              <label className="dlg-check" style={{ margin: 0 }}>
                <input
                  type="radio"
                  name="export-watermark-kind"
                  checked={form.watermarkKind === "text"}
                  onChange={() => update(switchWatermarkKind(form, "text"))}
                />
                文字
              </label>
              <label className="dlg-check" style={{ margin: 0 }}>
                <input
                  type="radio"
                  name="export-watermark-kind"
                  checked={form.watermarkKind === "image"}
                  onChange={() => update(switchWatermarkKind(form, "image"))}
                />
                画像
              </label>
            </>
          )}
        </div>
      </div>
      {form.watermark && form.watermarkKind === "text" && (
        <>
          <div className="field">
            <label htmlFor="export-watermark-text">文字</label>
            <textarea
              id="export-watermark-text"
              className="dlg-grow"
              rows={2}
              value={form.watermarkText}
              placeholder="例: © 2026 山田太郎"
              onChange={(e) => update({ watermarkText: e.target.value })}
            />
          </div>
          <div className="field">
            <label>フォント</label>
            <div className="dlg-row">
              <div className="dlg-grow">
                <PathBox path={form.watermarkFont} empty="OS のフォントのファイル（.ttf・.otf・.ttc）を選んでください" box />
              </div>
              <button className="btn" onClick={() => void pickFont()}>
                参照…
              </button>
            </div>
          </div>
          <div className="field">
            <label htmlFor="export-watermark-index">番号・色</label>
            <div className="dlg-row">
              <input
                id="export-watermark-index"
                className="dlg-inline-num"
                type="number"
                min={0}
                max={MAX_FONT_INDEX}
                step={1}
                title="コレクション（.ttc）の中の番号（0 から）"
                value={form.watermarkFontIndex}
                onChange={(e) => update({ watermarkFontIndex: e.target.value })}
              />
              <span className="hint">.ttc の中の番号</span>
              <input
                type="color"
                aria-label="文字の色"
                value={/^#[0-9a-fA-F]{6}$/.test(form.watermarkColor) ? form.watermarkColor.toLowerCase() : "#ffffff"}
                onChange={(e) => update({ watermarkColor: e.target.value.toUpperCase() })}
              />
              <span className="muted">{form.watermarkColor}</span>
            </div>
          </div>
        </>
      )}
      {form.watermark && form.watermarkKind === "image" && (
        <div className="field">
          <label>画像</label>
          <div className="dlg-row">
            <div className="dlg-grow">
              <PathBox path={form.watermarkImage} empty="PNG（透明度を使います）・JPEG・TIFF を選んでください" box />
            </div>
            <button className="btn" onClick={() => void pickImage()}>
              参照…
            </button>
          </div>
        </div>
      )}
      {form.watermark && (
        <>
          <div className="field">
            <label>位置</label>
            <div className="export-anchors" role="radiogroup" aria-label="透かしの位置">
              {WATERMARK_ANCHOR_OPTIONS.map((o) => (
                <button
                  key={o.value}
                  type="button"
                  role="radio"
                  aria-checked={form.watermarkAnchor === o.value}
                  className={`btn small${form.watermarkAnchor === o.value ? " primary" : ""}`}
                  title={o.label}
                  onClick={() => update({ watermarkAnchor: o.value })}
                >
                  {o.label}
                </button>
              ))}
            </div>
          </div>
          <div className="field">
            <label htmlFor="export-watermark-size">大きさ</label>
            <div className="dlg-row">
              <input
                id="export-watermark-size"
                className="dlg-inline-num"
                type="number"
                min={WATERMARK_SIZE_MIN}
                max={WATERMARK_SIZE_MAX}
                step={1}
                value={form.watermarkSize}
                onChange={(e) => update({ watermarkSize: e.target.value })}
              />
              <span className="muted">%</span>
              <label htmlFor="export-watermark-opacity" className="muted">
                不透明度
              </label>
              <input
                id="export-watermark-opacity"
                className="dlg-inline-num"
                type="number"
                min={WATERMARK_OPACITY_MIN}
                max={WATERMARK_OPACITY_MAX}
                step={1}
                value={form.watermarkOpacity}
                onChange={(e) => update({ watermarkOpacity: e.target.value })}
              />
              <span className="muted">%</span>
              <label htmlFor="export-watermark-margin" className="muted">
                余白
              </label>
              <input
                id="export-watermark-margin"
                className="dlg-inline-num"
                type="number"
                min={0}
                max={WATERMARK_MARGIN_MAX}
                step={1}
                value={form.watermarkMargin}
                onChange={(e) => update({ watermarkMargin: e.target.value })}
              />
              <span className="muted">%</span>
            </div>
          </div>
          <div className="field">
            <span />
            <span className="hint">
              大きさ・余白は写真の短辺に対する割合です（文字は文字の大きさ、画像は幅）。写真に収まらなければ縮めます
            </span>
          </div>
          {props.error && <div className="dlg-field-error">{props.error}</div>}
          <div className="export-watermark-preview">
            <div className="hint">見本（灰色の濃淡の上。{busy ? "作っています…" : "書き出しと同じ計算で重ねます"}）</div>
            {error && <div className="dlg-field-error">{error}</div>}
            {preview && (
              <>
                <img
                  src={`data:image/png;base64,${preview.png_base64}`}
                  width={preview.width / 2}
                  height={preview.height / 2}
                  alt="透かしの見本"
                />
                {preview.warnings.map((w) => (
                  <div key={w} className="dlg-warn">
                    {w}
                  </div>
                ))}
              </>
            )}
          </div>
        </>
      )}
    </>
  );
}

/** プリセットの欄の、このダイアログの中の編集（新しく保存・名前の変更・削除の確認）。 */
type PresetEdit =
  | { mode: "save"; name: string; folder: string; withDest: boolean }
  | { mode: "rename"; name: string; folder: string }
  | { mode: "delete" };

/**
 * 書き出しプリセットの欄（EXP-02）。使えないプリセット（新しい版のアプリで作った・壊れたファイル）も一覧の最後に
 * 理由を付けて出し、選ぶと理由を見せて削除だけできる（入力は変えない。決定事項ログ No.162）。別の OS で作った
 * プリセットの使わない値（書き出し先・透かしのファイル）は、選んだときに知らせる（PresetInfo の warnings）。
 */
function PresetBar(props: {
  presets: PresetInfo[] | null;
  /** 選んだ使えるプリセット（入力に使う）。 */
  selected: PresetInfo | null;
  /** 一覧で選んでいるもの（使えないものを含む）。 */
  chosen: PresetInfo | null;
  changed: boolean;
  form: ExportForm;
  busy: boolean;
  onSelect: (p: PresetInfo | null) => void;
}) {
  const { presets, selected, chosen, changed, form } = props;
  const [edit, setEdit] = useState<PresetEdit | null>(null);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const folders = useMemo(() => presetFolders(presets ?? []), [presets]);
  const disabled = props.busy || working;
  const run = async (f: () => Promise<void>) => {
    setWorking(true);
    setError(null);
    try {
      await f();
      setEdit(null);
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setWorking(false);
    }
  };
  const save = (e: Extract<PresetEdit, { mode: "save" }>) =>
    run(async () => {
      const settings = presetFromForm(form, e.withDest);
      if (!settings) throw new Error("入力に誤りがあるため保存できません（赤い説明の項目を直してください）");
      const p = await core.createExportPreset(e.name, e.folder, settings);
      const list = await reloadPresets("export");
      props.onSelect(list?.find((x) => x.id === p.id) ?? p);
      actions.notify("success", `書き出しプリセット「${p.name}」を保存しました`);
    });
  const update = () =>
    run(async () => {
      if (!selected?.export) return;
      const settings = presetFromForm(form, selected.export.destination !== null, selected.export);
      if (!settings) throw new Error("入力に誤りがあるため更新できません（赤い説明の項目を直してください）");
      const p = await core.updateExportPreset(selected.id, settings);
      const list = await reloadPresets("export");
      props.onSelect(list?.find((x) => x.id === p.id) ?? p);
      actions.notify("success", `書き出しプリセット「${p.name}」を今の設定で更新しました`);
    });
  const rename = (e: Extract<PresetEdit, { mode: "rename" }>) =>
    run(async () => {
      if (!selected) return;
      let p = selected;
      if (e.name.trim() !== selected.name) p = await core.renamePreset("export", selected.id, e.name);
      if (e.folder.trim() !== selected.folder) p = await core.movePreset("export", selected.id, e.folder);
      const list = await reloadPresets("export");
      props.onSelect(list?.find((x) => x.id === p.id) ?? p);
    });
  const remove = () =>
    run(async () => {
      if (!chosen) return;
      await core.deletePreset("export", chosen.id);
      await reloadPresets("export");
      props.onSelect(null);
      actions.notify("info", `書き出しプリセット「${chosen.name}」を削除しました`);
    });

  const usable = (presets ?? []).filter((p) => p.problem === null);
  const unusable = (presets ?? []).filter((p) => p.problem !== null);
  const nameProblem = edit && edit.mode !== "delete" ? validatePresetName(edit.name) : null;
  const folderProblem = edit && edit.mode !== "delete" ? validateFolderName(edit.folder) : null;
  return (
    <div className="dlg-section export-presets">
      <div className="field">
        <label htmlFor="export-preset">プリセット</label>
        <div className="dlg-row">
          <select
            id="export-preset"
            className="dlg-grow"
            value={chosen?.id ?? ""}
            disabled={disabled}
            onChange={(e) => props.onSelect((presets ?? []).find((p) => p.id === e.target.value) ?? null)}
          >
            <option value="">（なし）前回の設定</option>
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
            {unusable.length > 0 && (
              <optgroup label="使えないプリセット（削除だけできます）">
                {unusable.map((p) => (
                  <option key={p.id} value={p.id} title={p.problem ?? undefined}>
                    {p.folder ? `${p.folder} / ` : ""}
                    {p.name}（使えません）
                  </option>
                ))}
              </optgroup>
            )}
          </select>
          <button
            type="button"
            className="btn"
            disabled={disabled}
            title="今の入力を、名前を付けて書き出しプリセットにする"
            onClick={() => setEdit({ mode: "save", name: "", folder: selected?.folder ?? "", withDest: false })}
          >
            新しく保存…
          </button>
          <button
            type="button"
            className="btn"
            disabled={disabled || !selected || !changed}
            title={selected ? `「${selected.name}」を今の入力で置き換える` : "プリセットを選ぶと更新できます"}
            onClick={() => void update()}
          >
            更新
          </button>
          <MenuButton
            label="…"
            title="書き出しプリセットの操作"
            items={() => [
              {
                label: "名前・フォルダを変える…",
                disabled: !selected,
                onSelect: () => selected && setEdit({ mode: "rename", name: selected.name, folder: selected.folder }),
              },
              {
                label: "ファイルに書き出す…",
                disabled: !selected,
                onSelect: () => selected && void exportPresetFile("export", selected),
              },
              { label: "ファイルから読み込む…", onSelect: () => void importPresetFiles("export") },
              { label: "削除…", danger: true, disabled: !chosen, onSelect: () => setEdit({ mode: "delete" }) },
            ]}
          />
        </div>
      </div>
      {selected?.export && (
        <div className="hint">
          {describeExportPreset(selected.export)}
          {changed ? "（入力を変えています。「更新」でプリセットに保存できます）" : ""}
        </div>
      )}
      {selected && (selected.warnings ?? []).length > 0 && (
        <div className="dlg-note">
          {(selected.warnings ?? []).map((w) => (
            <div key={w}>{w}</div>
          ))}
        </div>
      )}
      {chosen && chosen.problem !== null && (
        <div className="dlg-warn">
          書き出しプリセット「{chosen.name}」は使えません: {chosen.problem}
          （入力は変えていません。「…」の「削除…」で削除できます）
        </div>
      )}
      {edit && edit.mode !== "delete" && (
        <form
          className="export-preset-edit"
          onSubmit={(e) => {
            e.preventDefault();
            if (nameProblem || folderProblem || disabled) return;
            void (edit.mode === "save" ? save(edit) : rename(edit));
          }}
        >
          <div className="field">
            <label htmlFor="export-preset-name">名前</label>
            <input
              id="export-preset-name"
              autoFocus
              value={edit.name}
              onChange={(e) => setEdit({ ...edit, name: e.target.value })}
            />
          </div>
          <div className="field">
            <label htmlFor="export-preset-folder">フォルダ</label>
            <input
              id="export-preset-folder"
              list="export-preset-folders"
              placeholder="フォルダなし"
              value={edit.folder}
              onChange={(e) => setEdit({ ...edit, folder: e.target.value })}
            />
            <datalist id="export-preset-folders">
              {folders.map((f) => (
                <option key={f} value={f} />
              ))}
            </datalist>
          </div>
          {edit.mode === "save" && (
            <label className="dlg-check">
              <input
                type="checkbox"
                checked={edit.withDest}
                disabled={form.destDir.trim() === ""}
                onChange={(e) => setEdit({ ...edit, withDest: e.target.checked })}
              />
              <span>
                書き出し先のフォルダも保存する
                <span className="desc">外すと、プリセットを選んでも書き出し先は変わりません（書き出すたびに選びます）</span>
              </span>
            </label>
          )}
          {edit.name.trim() !== "" && nameProblem && <div className="dlg-field-error">{nameProblem}</div>}
          {folderProblem && <div className="dlg-field-error">{folderProblem}</div>}
          <div className="dlg-row end">
            <button type="button" className="btn" onClick={() => setEdit(null)} disabled={working}>
              やめる
            </button>
            <button type="submit" className="btn primary" disabled={disabled || nameProblem !== null || folderProblem !== null}>
              {edit.mode === "save" ? "保存する" : "変える"}
            </button>
          </div>
        </form>
      )}
      {edit?.mode === "delete" && chosen && (
        <div className="dlg-warn">
          書き出しプリセット「{chosen.name}」を削除しますか？（書き出したファイルは変わりません）
          <div className="dlg-row end">
            <button type="button" className="btn" onClick={() => setEdit(null)} disabled={working}>
              やめる
            </button>
            <button type="button" className="btn danger" onClick={() => void remove()} disabled={working}>
              削除する
            </button>
          </div>
        </div>
      )}
      <ErrorBox error={error} title="プリセットを保存・変更できません" />
    </div>
  );
}

/** ファイル名の欄（EXP-02）。 */
function NamingFields(props: { form: ExportForm; update: (patch: Partial<ExportForm>) => void; error?: string }) {
  const { form, update } = props;
  const templateRef = useRef<HTMLInputElement>(null);
  const preview = useExportNamePreview(form);
  const addToken = (token: string) => {
    const el = templateRef.current;
    const { text, cursor } = insertToken(form.template, token, el?.selectionStart, el?.selectionEnd);
    update({ template: text });
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(cursor, cursor);
    });
  };
  const usesText = form.template.includes("{text}");
  const usesSeq = form.template.includes("{seq}");
  return (
    <>
      <div className="field">
        <label>ファイル名</label>
        <div className="dlg-row">
          <label className="dlg-check" style={{ margin: 0 }}>
            <input type="radio" name="export-naming" checked={!form.rename} onChange={() => update({ rename: false })} />
            元のファイル名
          </label>
          <label className="dlg-check" style={{ margin: 0 }}>
            <input type="radio" name="export-naming" checked={form.rename} onChange={() => update({ rename: true })} />
            テンプレート
          </label>
        </div>
      </div>
      {form.rename && (
        <>
          <div className="field">
            <label htmlFor="export-template">テンプレート</label>
            <div className="dlg-row">
              <input
                id="export-template"
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
              <label htmlFor="export-custom-text">任意の文字</label>
              <input
                id="export-custom-text"
                type="text"
                value={form.customText}
                placeholder="例: 京都旅行"
                onChange={(e) => update({ customText: e.target.value })}
              />
            </div>
          )}
          {usesSeq && (
            <div className="field">
              <label htmlFor="export-seq-start">連番</label>
              <div className="dlg-row">
                <span className="muted">開始</span>
                <input
                  id="export-seq-start"
                  className="dlg-inline-num"
                  type="number"
                  min={0}
                  step={1}
                  value={form.sequenceStart}
                  onChange={(e) => update({ sequenceStart: e.target.value })}
                />
                <label htmlFor="export-seq-digits" className="muted">
                  桁数
                </label>
                <input
                  id="export-seq-digits"
                  className="dlg-inline-num"
                  type="number"
                  min={1}
                  max={9}
                  step={1}
                  value={form.sequenceDigits}
                  onChange={(e) => update({ sequenceDigits: Number(e.target.value) })}
                />
                <span className="hint">書き出す写真・動画の順（書き出さない動画は数えません）</span>
              </div>
            </div>
          )}
        </>
      )}
      {props.error && <div className="dlg-field-error">{props.error}</div>}
      <div className="import-preview">
        <div className="hint">名前の例（見本の写真。仮想コピーは「-名前」を足します）</div>
        {preview?.error ? (
          <div className="dlg-field-error">{preview.error}</div>
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
    </>
  );
}

export function ExportDialog(props: { variantIds: VariantId[] }) {
  const coreDefault: OutputColorSpace = useApp((s) => s.catalog?.core_settings.default_export_color_space ?? "srgb");
  const presets = usePresets((s) => s.export);
  const [form, setForm] = useState<ExportForm>(() => restoreExportForm(loadStored(STORE_KEY), coreDefault));
  const [presetId, setPresetId] = useState<string | null>(() => restoreExportPresetId(loadStored(STORE_KEY)));
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const errors = useMemo(() => validateExportForm(form), [form]);
  const count = props.variantIds.length;
  // 選んだうち、動画と分かっているもの（EXP-05。動画は元のファイルのままコピーする）。
  const videoCount = useMemo(() => actions.knownVideoCount(props.variantIds), [props.variantIds]);
  const notice = videoNotice(form, videoCount);
  useEffect(() => {
    void reloadPresets("export");
  }, []);
  // 一覧で選んでいるもの（使えないものを含む。理由を見せて削除だけできる）と、入力に使う使えるもの。前回の
  // プリセットが消えた・使えなくなったら、使えるものは選んでいないことにする（入力は前回の設定のまま）。
  const chosen = presets?.find((p) => p.id === presetId) ?? null;
  const selected = chosen !== null && chosen.problem === null ? chosen : null;
  const changed = selected?.export ? !formMatchesPreset(form, selected.export) : false;

  const update = (patch: Partial<ExportForm>) => setForm((f) => ({ ...f, ...patch }));
  const choosePreset = (p: PresetInfo | null) => {
    setPresetId(p?.id ?? null);
    if (p?.export && p.problem === null) {
      const preset = p.export;
      setForm((f) => formFromPreset(preset, f));
    }
  };

  const chooseDest = async () => {
    try {
      const p = await chooseFolder("書き出し先のフォルダを選ぶ", form.destDir || null);
      if (p) update({ destDir: p });
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    }
  };

  const start = async () => {
    setTouched(true);
    const settings = toExportSettings(form);
    if (!settings || hasErrors(errors) || count === 0) return;
    setBusy(true);
    setError(null);
    let jobId: number;
    try {
      jobId = await core.exportVariants(props.variantIds, settings, form.destDir.trim(), toFileNaming(form));
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
      return;
    }
    saveStored(STORE_KEY, savedExport(form, coreDefault, selected?.id ?? null));
    actions.trackJob(jobId, "export", `書き出し: ${formatCount(count)} 件`);
    actions.closeDialog();
    void followExport(jobId);
  };

  const show = (msg: string | undefined) => (touched && msg ? <div className="dlg-field-error">{msg}</div> : null);

  return (
    <Dialog
      title={`書き出し（${formatCount(count)} 枚）`}
      width={620}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void start()} disabled={busy || count === 0}>
            書き出す
          </button>
        </>
      }
    >
      <PresetBar
        presets={presets}
        selected={selected}
        chosen={chosen}
        changed={changed}
        form={form}
        busy={busy}
        onSelect={choosePreset}
      />

      <div className="field">
        <label htmlFor="export-format">形式</label>
        <div className="dlg-row">
          <select
            id="export-format"
            value={form.format}
            onChange={(e) => update({ format: e.target.value as ExportForm["format"] })}
          >
            {FORMAT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          {form.format === "jpeg" && (
            <>
              <label htmlFor="export-quality" className="muted">
                品質
              </label>
              <input
                id="export-quality"
                className="dlg-inline-num"
                type="number"
                min={JPEG_QUALITY_MIN}
                max={JPEG_QUALITY_MAX}
                step={1}
                value={form.quality}
                onChange={(e) => update({ quality: e.target.value })}
              />
              <span className="muted">
                （{JPEG_QUALITY_MIN}〜{JPEG_QUALITY_MAX}）
              </span>
            </>
          )}
        </div>
      </div>
      {show(errors.quality)}

      <div className="field">
        <label htmlFor="export-cs">色空間</label>
        <div className="dlg-row">
          <select
            id="export-cs"
            value={form.colorSpace}
            onChange={(e) => update({ colorSpace: e.target.value as OutputColorSpace })}
          >
            {COLOR_SPACE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <span className="hint">既定: {colorSpaceLabel(coreDefault)}（設定の「カタログ」で変えられます）</span>
        </div>
      </div>

      <div className="field">
        <label>大きさ</label>
        <div className="dlg-row">
          <label className="dlg-check" style={{ margin: 0 }}>
            <input
              type="radio"
              name="export-size"
              checked={form.sizeMode === "original"}
              onChange={() => update({ sizeMode: "original" })}
            />
            元の大きさ（切り抜いた後）
          </label>
          <label className="dlg-check" style={{ margin: 0 }}>
            <input
              type="radio"
              name="export-size"
              checked={form.sizeMode === "long_edge"}
              onChange={() => update({ sizeMode: "long_edge" })}
            />
            長辺
          </label>
          <input
            className="dlg-inline-num"
            type="number"
            min={1}
            max={MAX_LONG_EDGE}
            step={1}
            aria-label="長辺の画素数"
            value={form.longEdge}
            disabled={form.sizeMode !== "long_edge"}
            onChange={(e) => update({ longEdge: e.target.value })}
          />
          <span className="muted">px（拡大はしません）</span>
        </div>
      </div>
      {show(errors.longEdge)}

      <SharpeningFields form={form} update={update} error={touched ? errors.resolution : undefined} />

      <WatermarkFields form={form} update={update} error={touched ? errors.watermark : undefined} />

      <div className="field">
        <label>メタデータ</label>
        <label className="dlg-check" style={{ margin: 0 }}>
          <input type="checkbox" checked={form.removeGps} onChange={(e) => update({ removeGps: e.target.checked })} />
          <span>
            位置情報（GPS）を削除する
            <span className="desc">撮影場所を知られたくない写真を共有するときはオンのままにしてください</span>
          </span>
        </label>
      </div>
      <div className="field">
        <label />
        <label className="dlg-check" style={{ margin: 0 }}>
          <input
            type="checkbox"
            checked={form.removeKeywords}
            onChange={(e) => update({ removeKeywords: e.target.checked })}
          />
          <span>
            キーワードを書き出さない
            <span className="desc">
              オフなら、付けたキーワードと上の階層を XMP（dc:subject）に書きます。キーワードごとに「書き出すファイルに含める」を外したものは書きません
            </span>
          </span>
        </label>
      </div>

      <div className="field">
        <label>動画</label>
        <label className="dlg-check" style={{ margin: 0 }}>
          <input type="checkbox" checked={form.videos} onChange={(e) => update({ videos: e.target.checked })} />
          <span>
            動画を書き出す
            <span className="desc">再エンコードせずに元のファイルのままコピーします（内容を照合してから確定します。元のファイルは変えません）</span>
          </span>
        </label>
      </div>
      {notice && <div className={notice.level === "warn" ? "dlg-warn" : "dlg-note"}>{notice.text}</div>}

      <NamingFields form={form} update={update} error={touched ? errors.naming : undefined} />

      <div className="field">
        <label htmlFor="export-conflict">同じ名前があるとき</label>
        <select
          id="export-conflict"
          value={form.onConflict}
          onChange={(e) => update({ onConflict: e.target.value as ExportForm["onConflict"] })}
        >
          {CONFLICT_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </div>
      {form.onConflict === "overwrite" && (
        <div className="dlg-warn">
          書き出し先の同じ名前のファイルを置き換えます。カタログに登録した元のファイル（原本）は上書きしません（その写真は書き出さずに失敗にします）。
        </div>
      )}

      <div className="field">
        <label>書き出し先</label>
        <div className="dlg-row">
          <div className="dlg-grow">
            <PathBox path={form.destDir} empty="フォルダを選んでください" box />
          </div>
          <button className="btn" onClick={() => void chooseDest()} disabled={busy}>
            参照…
          </button>
        </div>
      </div>
      {show(errors.destDir)}

      <div className="dlg-note">
        カタログに保存した現像設定で書き出します（元のファイルは変更しません）。動画は現像せずに元のファイルのままコピーします。
      </div>
      <ErrorBox error={error} title="書き出しを始められません" />
    </Dialog>
  );
}
