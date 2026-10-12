// 書き出しのダイアログ（EXP-01・EXP-04）: 形式・色空間・大きさ・GPS の削除・同じ名前の扱い・書き出し先。
//
// 開始したら閉じて、進捗は上の帯のジョブの表示に出す。終わったら結果（書いた・飛ばした・失敗の数と理由）を
// 通知する。前回の設定と書き出し先は、ブラウザに保存して次に使う。

import { useMemo, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo, OutputColorSpace, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions, useApp } from "../state/app";
import { formatCount } from "./dialogFormat";
import {
  COLOR_SPACE_OPTIONS,
  CONFLICT_OPTIONS,
  type ExportForm,
  FORMAT_OPTIONS,
  JPEG_QUALITY_MAX,
  JPEG_QUALITY_MIN,
  MAX_LONG_EDGE,
  colorSpaceLabel,
  describeExportJob,
  hasErrors,
  restoreExportForm,
  savedExport,
  toExportSettings,
  validateExportForm,
} from "./exportLogic";
import { chooseFolder } from "./native";
import { ErrorBox, PathBox } from "./parts";
import { loadStored, saveStored } from "./storage";

const STORE_KEY = "export.last";

/** 書き出しの終わりを待って、結果を知らせる。 */
async function followExport(jobId: number): Promise<void> {
  try {
    const info = await actions.waitJob(jobId);
    const notice = describeExportJob(info);
    if (notice) actions.notify(notice.level, notice.message, notice.detail);
  } catch (e) {
    actions.reportError(e, "書き出しの結果を受け取れません");
  }
}

export function ExportDialog(props: { variantIds: VariantId[] }) {
  const coreDefault: OutputColorSpace = useApp((s) => s.catalog?.core_settings.default_export_color_space ?? "srgb");
  const [form, setForm] = useState<ExportForm>(() => restoreExportForm(loadStored(STORE_KEY), coreDefault));
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const errors = useMemo(() => validateExportForm(form), [form]);
  const count = props.variantIds.length;

  const update = (patch: Partial<ExportForm>) => setForm((f) => ({ ...f, ...patch }));

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
      jobId = await core.exportVariants(props.variantIds, settings, form.destDir.trim());
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
      return;
    }
    saveStored(STORE_KEY, savedExport(form, coreDefault));
    actions.trackJob(jobId, "export", `書き出し: ${formatCount(count)} 枚`);
    actions.closeDialog();
    void followExport(jobId);
  };

  const show = (msg: string | undefined) => (touched && msg ? <div className="dlg-field-error">{msg}</div> : null);

  return (
    <Dialog
      title={`書き出し（${formatCount(count)} 枚）`}
      width={580}
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
        カタログに保存した現像設定で書き出します（元のファイルは変更しません）。動画は書き出しません（今の版は写真だけ）。
      </div>
      <ErrorBox error={error} title="書き出しを始められません" />
    </Dialog>
  );
}
