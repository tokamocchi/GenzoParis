// 設定の「カタログ」（SYS-05）: カタログに保存する設定（キャッシュの場所と上限・既定の書き出しの色空間・
// 既定のタイムゾーン・取り込みの既定（重複と思われるもの・RAW と JPEG。IMP-04・IMP-06）・内容のハッシュの
// バックグラウンドの計算（ORG-03））、内容による同一性の状況（SettingsIdentity）、カタログの情報、カタログの
// 構造（ORG-07。SettingsSchema）、カタログの切り替え、キャッシュの整理。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { CatalogStats, CoreSettings, ErrorInfo, OutputColorSpace } from "../api/types";
import { actions, useApp } from "../state/app";
import { setCoreSettings, switchCatalog, waitJobUntilClosed } from "./catalogActions";
import { type CatalogPick, pickExistingCatalog } from "./catalogPick";
import { describeGarbage, formatCount, parentDir } from "./dialogFormat";
import { COLOR_SPACE_OPTIONS } from "./exportLogic";
import { chooseFolder } from "./native";
import { useNewCatalogPicker } from "./NewCatalogPicker";
import { Busy, ErrorBox, PathBox } from "./parts";
import { SettingsIdentity } from "./SettingsIdentity";
import { SettingsSchema } from "./SettingsSchema";
import {
  type CatalogSettingsForm,
  catalogSettingsForm,
  catalogSettingsUpdate,
  formatUtcOffset,
  utcOffsetOptions,
} from "./settingsLogic";

/** キャッシュの整理（ジョブ）を実行して結果を知らせる。 */
async function collectGarbage(): Promise<void> {
  try {
    const jobId = await core.collectCacheGarbage();
    actions.trackJob(jobId, "collect_garbage");
    const info = await waitJobUntilClosed(jobId);
    // 終わる前にカタログを閉じた（取り消したことは閉じる処理が知らせる）。
    if (info === "closed") return;
    const r = info.result;
    if (r && r.type === "collect_garbage") actions.notify("success", describeGarbage(r));
    else if (info.state === "cancelled") actions.notify("info", "キャッシュの整理を取り消しました");
  } catch (e) {
    actions.reportError(e, "キャッシュを整理できません");
  }
}

export function SettingsCatalog() {
  const catalog = useApp((s) => s.catalog);
  const [current, setCurrent] = useState<CoreSettings | null>(catalog?.core_settings ?? null);
  const [form, setForm] = useState<CatalogSettingsForm | null>(() =>
    catalog ? catalogSettingsForm(catalog.core_settings) : null,
  );
  const [stats, setStats] = useState<CatalogStats | null>(catalog?.stats ?? null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState<string | null>(null);
  /** カタログの切り替え中（今のカタログを閉じている。保存待ちの保存・終了時のバックアップを含む）。 */
  const [switching, setSwitching] = useState(false);
  const [gcRunning, setGcRunning] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    // StrictMode の開発版では、付け直し（cleanup → もう一度）があるので、ここで立て直す。
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const handlePick = async (pick: CatalogPick) => {
    if (pick.kind === "cancel" || pick.kind === "no_save_dialog") return;
    if (pick.kind === "error") {
      setSwitchError(pick.message);
      return;
    }
    setSwitchError(null);
    setSwitching(true);
    try {
      await switchCatalog(pick.path, pick.create);
    } finally {
      // 閉じられたら、このダイアログは閉じている。
      if (mounted.current) setSwitching(false);
    }
  };
  const newPicker = useNewCatalogPicker(catalog ? parentDir(catalog.catalog_path) : null, handlePick, switching);

  // 開いた時点の値は古いことがあるので、コアから読み直す。
  useEffect(() => {
    let alive = true;
    core
      .getSettings()
      .then((s) => {
        if (!alive) return;
        setCurrent(s);
        setForm(catalogSettingsForm(s));
      })
      .catch(() => undefined);
    core
      .catalogStats()
      .then((s) => {
        if (alive) setStats(s);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  if (!catalog || !current || !form) return <p className="muted">カタログが開いていません。</p>;

  const update = (patch: Partial<CatalogSettingsForm>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
    setFormError(null);
  };

  const chooseCacheDir = async () => {
    try {
      const p = await chooseFolder("プレビューのキャッシュの場所を選ぶ", form.cacheDir);
      if (p) update({ cacheDir: p });
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    }
  };

  const save = async () => {
    const r = catalogSettingsUpdate(current, form);
    if ("error" in r) {
      setFormError(r.error);
      return;
    }
    if (!r.changed) {
      actions.notify("info", "変更はありません");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const saved = await core.updateSettings(r.update);
      setCurrent(saved);
      setForm(catalogSettingsForm(saved));
      setCoreSettings(saved);
      const notes: string[] = [];
      if (r.update.preview_cache_dir !== undefined) notes.push("キャッシュの場所は、アプリを再起動して、このカタログを開いたときから使います（今のキャッシュは移しません）。");
      if (r.update.default_utc_offset_minutes !== undefined) {
        notes.push("既定のタイムゾーンで推定していた撮影日時を、バックグラウンドで推定し直します。");
      }
      if (r.update.hash_in_background === false) notes.push("実行中の内容のハッシュの計算を取り消しました。");
      if (r.update.hash_in_background === true || r.update.hash_network_volumes === true) {
        notes.push("内容のハッシュのないファイルを、バックグラウンドで計算します。");
      }
      actions.notify("success", "カタログの設定を保存しました", notes.length > 0 ? notes.join("\n") : undefined);
    } catch (e) {
      setError(toApiError(e).info);
      // キャッシュの上限を超えた分の削除の失敗では、設定は保存済み。今の値を読み直す。
      core
        .getSettings()
        .then((s) => {
          setCurrent(s);
          setCoreSettings(s);
        })
        .catch(() => undefined);
    } finally {
      setBusy(false);
    }
  };

  const runGc = async () => {
    setGcRunning(true);
    await collectGarbage();
    setGcRunning(false);
  };

  const dir = parentDir(catalog.catalog_path);
  const caps = catalog.capabilities;

  return (
    <div>
      <div className="dlg-section">
        <h3>プレビューのキャッシュ</h3>
        <div className="field">
          <label>場所</label>
          <div className="dlg-row">
            <div className="dlg-grow">
              <PathBox path={form.cacheDir} box />
            </div>
            <button className="btn" onClick={() => void chooseCacheDir()} disabled={busy}>
              参照…
            </button>
          </div>
        </div>
        <p className="hint" style={{ marginLeft: 148 }}>
          アプリを再起動して、このカタログを開いたときから使います（今のキャッシュは移しません）。
        </p>
        <div className="field">
          <label htmlFor="cache-gb">上限</label>
          <div className="dlg-row">
            <input
              id="cache-gb"
              className="dlg-inline-num"
              type="text"
              inputMode="decimal"
              value={form.cacheGb}
              onChange={(e) => update({ cacheGb: e.target.value })}
            />
            <span className="muted">GB（1 GB = 10 億バイト。0.1 GB 以上）</span>
          </div>
        </div>
        <div className="dlg-row">
          <button className="btn" onClick={() => void runGc()} disabled={gcRunning}>
            キャッシュを整理
          </button>
          <span className="hint">上限を超えた分と、使われなくなったサムネイル・プレビューを削除します（作り直せます）。</span>
        </div>
      </div>

      <div className="dlg-section">
        <h3>既定の値</h3>
        <div className="field">
          <label htmlFor="default-cs">書き出しの色空間</label>
          <select
            id="default-cs"
            value={form.exportColorSpace}
            onChange={(e) => update({ exportColorSpace: e.target.value as OutputColorSpace })}
          >
            {COLOR_SPACE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </div>
        <div className="field">
          <label htmlFor="default-tz">タイムゾーン</label>
          <select
            id="default-tz"
            value={form.utcOffsetMinutes}
            onChange={(e) => update({ utcOffsetMinutes: Number(e.target.value) })}
          >
            {utcOffsetOptions(form.utcOffsetMinutes).map((m) => (
              <option key={m} value={m}>
                {formatUtcOffset(m)}
              </option>
            ))}
          </select>
        </div>
        <p className="hint" style={{ marginLeft: 148 }}>
          撮影日時にタイムゾーンの記録がない写真・動画の時刻を、このタイムゾーンとみなします。変えると、このタイムゾーンで推定していた撮影日時を推定し直します。
        </p>
        <label className="dlg-check">
          <input
            type="checkbox"
            checked={form.importSkipDuplicates}
            onChange={(e) => update({ importSkipDuplicates: e.target.checked })}
          />
          <span>
            取り込みで、重複と思われるものを取り込まない
            <span className="desc">内容が同じファイルが別の場所で登録済みなら登録しません（取り込みのダイアログの既定）</span>
          </span>
        </label>
        <label className="dlg-check">
          <input
            type="checkbox"
            checked={form.importSeparateRawJpeg}
            onChange={(e) => update({ importSeparateRawJpeg: e.target.checked })}
          />
          <span>
            RAW と同時記録の JPEG を別々の写真として取り込む
            <span className="desc">
              外すと 1 枚の写真（RAW が主、JPEG は付属）にまとめます。取り込み済みの写真は変わりません（取り込みのダイアログの既定）
            </span>
          </span>
        </label>
        <label className="dlg-check">
          <input
            type="checkbox"
            checked={form.hashInBackground}
            onChange={(e) => update({ hashInBackground: e.target.checked })}
          />
          <span>
            内容のハッシュをバックグラウンドで計算する
            <span className="desc">
              カタログを開いている間、ファイルの全体を少しずつ読みます（現像などの操作の間は待ちます）。外すと、実行中の計算を取り消します
            </span>
          </span>
        </label>
        <label className="dlg-check">
          <input
            type="checkbox"
            checked={form.hashNetworkVolumes}
            disabled={!form.hashInBackground}
            onChange={(e) => update({ hashNetworkVolumes: e.target.checked })}
          />
          <span>
            ネットワークのボリューム（NAS など）のファイルも計算する
            <span className="desc">ファイルの全体をネットワーク越しに読むため、時間と通信量がかかります</span>
          </span>
        </label>
        {formError && <div className="dlg-field-error">{formError}</div>}
        <ErrorBox error={error} title="保存できません" />
        <div className="dlg-row end">
          {busy && <Busy>保存しています…</Busy>}
          <button className="btn primary" onClick={() => void save()} disabled={busy}>
            保存
          </button>
        </div>
      </div>

      <SettingsIdentity includeNetwork={form.hashNetworkVolumes} />

      <div className="dlg-section">
        <h3>このカタログ</h3>
        <dl className="dlg-kv">
          <dt>カタログ</dt>
          <dd>
            <PathBox path={catalog.catalog_path} />
          </dd>
          <dt>データのフォルダ</dt>
          <dd>
            <PathBox path={catalog.data_dir} />
          </dd>
          {stats && (
            <>
              <dt>写真・動画</dt>
              <dd>
                {formatCount(stats.assets)} 枚（仮想コピーを含めて {formatCount(stats.variants)} 件、ファイル{" "}
                {formatCount(stats.files)} 個）
              </dd>
              <dt>スタック</dt>
              <dd>{formatCount(stats.stacks ?? 0)} 個</dd>
              <dt>履歴</dt>
              <dd>{formatCount(stats.history_entries)} 件</dd>
              <dt>スキーマの版</dt>
              <dd>{stats.schema_version}</dd>
            </>
          )}
          <dt>RAW のデコーダ</dt>
          <dd>{caps ? (caps.raw_decoder ?? "なし（RAW を開けません）") : "—"}</dd>
          <dt>GPU</dt>
          <dd>{caps ? (caps.gpu ?? "使っていません（CPU で処理）") : "—"}</dd>
          {caps && (
            <>
              <dt>ワーカーの版</dt>
              <dd>{caps.worker_version}</dd>
            </>
          )}
          {catalog.worker_error && (
            <>
              <dt>ワーカーのエラー</dt>
              <dd>
                <ErrorBox error={catalog.worker_error} />
              </dd>
            </>
          )}
          <dt>画面の色空間</dt>
          <dd>
            {catalog.display.description ?? (catalog.display.assumed_srgb ? "sRGB" : "—")}
            {catalog.display.assumed_srgb ? "（プロファイルが使えないため sRGB とみなしています）" : ""}
            {catalog.display.assumed_srgb && catalog.display.reason ? (
              <div className="hint">{catalog.display.reason}</div>
            ) : null}
          </dd>
        </dl>
      </div>

      <SettingsSchema catalogPath={catalog.catalog_path} schemaVersion={stats?.schema_version ?? null} />

      <div className="dlg-section">
        <h3>カタログの切り替え</h3>
        <p className="hint" style={{ marginTop: 0 }}>
          今のカタログを閉じてから開きます（保存待ちの現像設定は保存します）。
        </p>
        <div className="dlg-row">
          <button className="btn" onClick={() => void pickExistingCatalog(dir).then(handlePick)} disabled={switching}>
            別のカタログを開く…
          </button>
          <button className="btn" onClick={newPicker.start} disabled={switching}>
            新しいカタログを作る…
          </button>
          {switching && (
            <Busy>今のカタログを閉じています…（保存待ちの現像設定の保存・終了時のバックアップを含みます）</Busy>
          )}
        </div>
        {newPicker.form}
        {switchError && <div className="dlg-error">{switchError}</div>}
      </div>
    </div>
  );
}
