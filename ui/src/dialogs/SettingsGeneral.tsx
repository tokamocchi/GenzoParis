// 設定の「一般」（アプリの設定。カタログの外の settings.json に保存し、次にカタログを開いたときから使う）:
// FFmpeg のフォルダ（決定事項ログ No.62）・GPU・終了時に毎回バックアップ。

import { useEffect, useState } from "react";

import * as app from "../api/app";
import { toApiError } from "../api/errors";
import type { ErrorInfo, ExternalTools, GpuMode } from "../api/types";
import { actions, appStore, useApp } from "../state/app";
import { chooseFolder } from "./native";
import { Busy, ErrorBox, PathBox } from "./parts";

function ToolLine(props: { name: string; path: string | null | undefined; checking: boolean }) {
  return (
    <div className="dlg-row">
      <span style={{ width: 64 }}>{props.name}</span>
      {props.checking ? (
        <span className="muted">確かめています…</span>
      ) : props.path ? (
        <>
          <span>見つかりました</span>
          <span className="dlg-path muted dlg-grow">{props.path}</span>
        </>
      ) : (
        <span className="warn-text">見つかりません</span>
      )}
    </div>
  );
}

export function SettingsGeneral() {
  const status = useApp((s) => s.status);
  const openTools = useApp((s) => s.catalog?.tools ?? null);
  const saved = status?.settings ?? null;
  const [ffmpegDir, setFfmpegDir] = useState<string | null>(saved?.ffmpeg_dir ?? null);
  const [gpu, setGpu] = useState<GpuMode>(saved?.gpu ?? "auto");
  const [backupOnClose, setBackupOnClose] = useState(saved?.backup_on_close ?? false);
  const [tools, setTools] = useState<ExternalTools | null>(status?.tools ?? null);
  const [checking, setChecking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  // フォルダを変えたら、そこで見つかるかを確かめる。
  useEffect(() => {
    let alive = true;
    setChecking(true);
    app
      .locateTools(ffmpegDir)
      .then((t) => {
        if (alive) setTools(t);
      })
      .catch((e) => {
        if (alive) setError(toApiError(e).info);
      })
      .finally(() => {
        if (alive) setChecking(false);
      });
    return () => {
      alive = false;
    };
  }, [ffmpegDir]);

  const dirty =
    !!saved &&
    ((saved.ffmpeg_dir ?? null) !== ffmpegDir || saved.gpu !== gpu || saved.backup_on_close !== backupOnClose);

  const choose = async () => {
    try {
      const p = await chooseFolder("ffmpeg・ffprobe のあるフォルダを選ぶ", ffmpegDir);
      if (p) setFfmpegDir(p);
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    }
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const settings = await app.updateAppSettings({ ffmpeg_dir: ffmpegDir, gpu, backup_on_close: backupOnClose });
      const cur = appStore.get().status;
      if (cur) actions.setStatus({ ...cur, settings, tools: tools ?? cur.tools });
      actions.notify("success", "設定を保存しました", "次にカタログを開いたときから有効です");
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  const openDiffers =
    openTools !== null &&
    tools !== null &&
    (openTools.ffmpeg !== tools.ffmpeg || openTools.ffprobe !== tools.ffprobe);

  return (
    <div>
      <div className="dlg-section">
        <h3>FFmpeg（動画のサムネイルと情報）</h3>
        <p className="hint" style={{ marginTop: 0 }}>
          FFmpeg はアプリに同梱していません。ご自分で入れた ffmpeg・ffprobe を使います（Windows は winget、macOS は
          Homebrew などで入れられます）。フォルダを指定しなければ、PATH と OS ごとの既定の場所を探します。
        </p>
        <div className="dlg-row">
          <div className="dlg-grow">
            <PathBox path={ffmpegDir} empty="自動（PATH と既定の場所を探す）" box />
          </div>
          <button className="btn" onClick={() => void choose()} disabled={busy}>
            参照…
          </button>
          <button className="btn" onClick={() => setFfmpegDir(null)} disabled={busy || ffmpegDir === null}>
            自動
          </button>
        </div>
        <div style={{ marginTop: 6 }}>
          <ToolLine name="ffmpeg" path={tools?.ffmpeg} checking={checking} />
          <ToolLine name="ffprobe" path={tools?.ffprobe} checking={checking} />
        </div>
        {openDiffers && (
          <div className="dlg-note">今開いているカタログは、開いたときに見つけたもの（または見つからなかった状態）を使っています。</div>
        )}
      </div>

      <div className="dlg-section">
        <h3>GPU</h3>
        <div className="dlg-row">
          <label className="dlg-check" style={{ margin: 0 }}>
            <input type="radio" name="gpu" checked={gpu === "auto"} onChange={() => setGpu("auto")} />
            自動（使えれば使う）
          </label>
          <label className="dlg-check" style={{ margin: 0 }}>
            <input type="radio" name="gpu" checked={gpu === "off"} onChange={() => setGpu("off")} />
            使わない（CPU で処理する）
          </label>
        </div>
        <p className="hint">表示が乱れる・落ちるときは「使わない」を試してください。</p>
      </div>

      <div className="dlg-section">
        <h3>バックアップ</h3>
        <label className="dlg-check">
          <input type="checkbox" checked={backupOnClose} onChange={(e) => setBackupOnClose(e.target.checked)} />
          <span>
            アプリの終了時に毎回カタログをバックアップする
            <span className="desc">
              オフのときも、前回のバックアップから 1 日以上たっていれば、カタログを開いたときと終了時に自動でバックアップします（新しいものから 5 世代を残します）
            </span>
          </span>
        </label>
      </div>

      <ErrorBox error={error} title="保存できません" />
      <div className="dlg-row end">
        {busy && <Busy>保存しています…</Busy>}
        <span className="hint">次にカタログを開いたときから有効です</span>
        <button className="btn primary" onClick={() => void save()} disabled={busy || !dirty}>
          保存
        </button>
      </div>
      {status && (
        <p className="hint">
          GenzoParis {status.version}（{status.platform}）
        </p>
      )}
    </div>
  );
}
