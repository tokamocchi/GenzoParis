// メタデータのパネル（LIB-14・VID-03）と、評価・フラグ・カラーラベル・キャプションの操作（LIB-04）。
//
// - 操作の対象（selection.current）の詳細を core.variantDetails で読む（対象・detailsVersion が変わったら
//   読み直す。古い応答は捨てる）。新しい詳細が届くまでは前の詳細を薄く出し、1 枚用の評価などの操作は
//   止める（前の写真の値で、新しい対象を変えないように）。
// - 評価・フラグ・ラベルは actions.setRating などで付ける（グリッドでは選択中のすべて、ルーペでは表示中の
//   1 枚。actions.targets）。グリッドで複数を選んでいるときは「N 件を選択」と、まとめて付ける操作を出す。
// - キャプションは操作の対象の 1 枚に付ける。フォーカスを外したとき・Ctrl / ⌘ ＋ Enter で保存する
//   （IME の変換を確定する Enter では保存しない）。空なら削除（null）。

import { useEffect, useRef, useState } from "react";

import * as appApi from "../api/app";
import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ColorLabel, Flag, VariantDetails, VariantId, VariantSummary } from "../api/types";
import { FlagMark, LabelDot, Stars } from "../components/Marks";
import { Section } from "../components/Section";
import { COLOR_LABEL_NAMES, formatAperture, formatBytes, formatCaptureTime, formatFocal, formatShutter } from "../format";
import { comboLabel } from "../keyboard";
import { actions, useApp } from "../state/app";
import { registerFlush } from "../state/flush";
import "./library.css";
import {
  captureNote,
  dirName,
  fileRoleLabel,
  fileCheckNotice,
  fileStatusText,
  fileStatusTitle,
  formatDimensions,
  formatGps,
  formatIso,
  hasVideoInfo,
  primaryFile,
  videoRows,
  virtualCopyLabel,
} from "./metadataLogic";

/** 操作の対象が変わってから詳細を読むまでの時間（ms。キーを押し続けて次々に移るときに読みすぎない。仮置き）。 */
const DETAILS_DELAY_MS = 60;

const FLAG_BUTTONS: { flag: Flag; label: string; key: string }[] = [
  { flag: 1, label: "採用", key: "p" },
  { flag: 0, label: "なし", key: "u" },
  { flag: -1, label: "不採用", key: "x" },
];

const LABEL_BUTTONS: { label: ColorLabel | null; key: string | null }[] = [
  { label: "red", key: "6" },
  { label: "yellow", key: "7" },
  { label: "green", key: "8" },
  { label: "blue", key: "9" },
  { label: "purple", key: null },
  { label: null, key: null },
];

export function MetadataPanel() {
  const mode = useApp((s) => s.mode);
  const current = useApp((s) => s.selection.current);
  const selectedCount = useApp((s) => s.selection.ids.length);
  const detailsVersion = useApp((s) => s.detailsVersion);
  const [reload, setReload] = useState(0);
  const { details, error, notFound } = useDetails(current, detailsVersion, reload);
  const multi = mode === "grid" && selectedCount > 1;

  if (current === null && !multi) {
    return (
      <div className="lib-meta">
        <div className="empty-state">写真を選ぶと、ここに情報が表示されます。</div>
      </div>
    );
  }

  const stale = details !== null && details.summary.variant_id !== current;
  return (
    <div className="lib-meta">
      {multi && <BatchMarks count={selectedCount} />}
      {current !== null && error && !details && (
        <div className="lib-meta-head">
          <p className="error-text">情報を読めません: {error}</p>
          <button type="button" className="btn small" onClick={() => setReload((n) => n + 1)}>
            読み直す
          </button>
        </div>
      )}
      {current !== null && !details && notFound && (
        <div className="lib-meta-head">
          <p className="hint">この写真はカタログにありません（削除された可能性があります）。</p>
        </div>
      )}
      {current !== null && !details && !error && !notFound && (
        <div className="lib-meta-head">
          <div className="spinner" />
        </div>
      )}
      {details && (
        <Details details={details} stale={stale} multi={multi} onReload={() => setReload((n) => n + 1)} />
      )}
    </div>
  );
}

interface DetailsState {
  /** 読んだ対象（error・notFound は、この対象のときだけ出す）。 */
  id: VariantId | null;
  details: VariantDetails | null;
  error: string | null;
  /** カタログにない（not_found。削除された写真など）。 */
  notFound: boolean;
}

/** 操作の対象の詳細（対象が変わっても、新しい詳細が届くまでは前のものを薄く出す）。 */
function useDetails(id: VariantId | null, version: number, reload: number) {
  const [state, setState] = useState<DetailsState>({ id: null, details: null, error: null, notFound: false });
  const seq = useRef(0);
  useEffect(() => {
    const my = ++seq.current;
    if (id === null) {
      setState({ id: null, details: null, error: null, notFound: false });
      return;
    }
    const t = setTimeout(() => {
      core
        .variantDetails(id)
        .then((details) => {
          if (seq.current === my) setState({ id, details, error: null, notFound: false });
        })
        .catch((e: unknown) => {
          if (seq.current !== my) return;
          const err = toApiError(e);
          if (err.kind === "cancelled" || err.kind === "stale") return;
          const notFound = err.kind === "not_found";
          setState({ id, details: null, error: notFound ? null : err.info.message, notFound });
        });
    }, DETAILS_DELAY_MS);
    return () => clearTimeout(t);
  }, [id, version, reload]);
  // 別の対象の読み込みの失敗は出さない（新しい対象を読み込み中として扱う）。
  const mine = state.id === id;
  return {
    details: state.details,
    error: mine ? state.error : null,
    notFound: mine && state.notFound,
  };
}

// ---------------------------------------------------------------------------
// 選別の操作
// ---------------------------------------------------------------------------

/**
 * 評価・フラグ・ラベルの操作（`summary` が null なら値を出さない（まとめて付けるとき））。`disabled` の間は
 * 操作できない（新しい対象の詳細が届くまでの間。前の写真の値で、新しい対象を変えないように）。
 */
function MarkControls(props: { summary: VariantSummary | null; disabled?: boolean }) {
  const s = props.summary;
  const disabled = props.disabled ?? false;
  return (
    <>
      <div className="lib-mark-row">
        <span className="label">評価</span>
        <Stars rating={s?.rating ?? 0} onSet={disabled ? undefined : (r) => void actions.setRating(r)} />
        {!s && (
          <button type="button" className="btn small" onClick={() => void actions.setRating(0)} title="評価を外す（0）">
            評価なし
          </button>
        )}
      </div>
      <div className="lib-mark-row">
        <span className="label">フラグ</span>
        {FLAG_BUTTONS.map((f) => (
          <button
            key={f.flag}
            type="button"
            className={`btn small${s?.flag === f.flag ? " active" : ""}`}
            aria-pressed={s ? s.flag === f.flag : undefined}
            disabled={disabled}
            title={`${f.label}（${comboLabel(f.key)}）`}
            onClick={() => void actions.setFlag(f.flag)}
          >
            {f.flag !== 0 && <FlagMark flag={f.flag} />}
            {f.label}
          </button>
        ))}
      </div>
      <div className="lib-mark-row">
        <span className="label">ラベル</span>
        <span className="lib-labels">
          {LABEL_BUTTONS.map((b) => {
            const name = b.label ? COLOR_LABEL_NAMES[b.label] : "ラベルなし";
            const on = s ? s.color_label === b.label : false;
            return (
              <button
                key={b.label ?? "none"}
                type="button"
                className={`lib-label-btn${on ? " on" : ""}`}
                aria-pressed={s ? on : undefined}
                aria-label={name}
                title={b.key ? `${name}（${comboLabel(b.key)}）` : name}
                disabled={disabled}
                onClick={() => void actions.setColorLabel(b.label)}
              >
                {b.label ? <LabelDot label={b.label} /> : "なし"}
              </button>
            );
          })}
        </span>
      </div>
    </>
  );
}

/** グリッドで複数を選んでいるとき（まとめて付ける）。 */
function BatchMarks({ count }: { count: number }) {
  return (
    <div className="lib-meta-head">
      <div className="title">{count.toLocaleString("ja-JP")} 件を選択</div>
      <div className="sub">評価・フラグ・ラベルは、選択中のすべてに付きます。</div>
      <MarkControls summary={null} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// 詳細
// ---------------------------------------------------------------------------

function Details(props: { details: VariantDetails; stale: boolean; multi: boolean; onReload: () => void }) {
  const { details, stale, multi } = props;
  const s = details.summary;
  const primary = primaryFile(details.files);
  const note = captureNote(s.capture);
  const vc = virtualCopyLabel(s);
  const status = fileStatusTitle(s.file_status);
  const ffprobe = useApp((st) => st.catalog?.tools.ffprobe ?? null);
  const isVideo = s.kind === "video";

  return (
    <>
      <div className={`lib-meta-head${stale ? " stale" : ""}`}>
        {multi && <div className="sub">操作の対象</div>}
        <div className="title selectable">{s.file_name}</div>
        <div className="sub">{[isVideo ? "動画" : "写真", vc].filter(Boolean).join("・")}</div>
        {status && (
          <div className={s.file_status === "missing" ? "status-warn" : "status-error"}>
            {status}
            {primary?.status_reason ? `（${primary.status_reason}）` : ""}
          </div>
        )}
        {!multi && <MarkControls summary={s} disabled={stale} />}
      </div>

      <Section id="lib.meta.info" title="情報">
        <dl className="lib-kv selectable">
          <dt>フォルダ</dt>
          <dd>{dirName(primary?.path) ?? "（場所が分かりません）"}</dd>
          <dt>撮影日時</dt>
          <dd>
            {formatCaptureTime(s.capture)}
            {note && <span className="note">{note}</span>}
          </dd>
          <dt>カメラ</dt>
          <dd>{s.camera ?? "—"}</dd>
          {!isVideo && (
            <>
              <dt>レンズ</dt>
              <dd>{s.lens ?? "—"}</dd>
              <dt>ISO 感度</dt>
              <dd>{formatIso(details.iso)}</dd>
              <dt>絞り</dt>
              <dd>{formatAperture(details.aperture)}</dd>
              <dt>シャッター速度</dt>
              <dd>{formatShutter(details.shutter_s)}</dd>
              <dt>焦点距離</dt>
              <dd>{formatFocal(details.focal_mm)}</dd>
            </>
          )}
          <dt>寸法</dt>
          <dd>{formatDimensions(s.width, s.height, s.orientation)}</dd>
          <dt>ファイルの大きさ</dt>
          <dd>{formatBytes(primary?.size)}</dd>
          <dt>GPS</dt>
          <dd>{formatGps(details.gps) ?? "なし"}</dd>
        </dl>
      </Section>

      {isVideo && (
        <Section id="lib.meta.video" title="動画">
          {hasVideoInfo(details.video) ? (
            <dl className="lib-kv selectable">
              {videoRows(details.video).map(([k, v]) => (
                <Row key={k} k={k} v={v} />
              ))}
            </dl>
          ) : (
            <div className="hint">
              {ffprobe === null ? (
                <>
                  <p>
                    FFmpeg（ffprobe）が見つからないため、動画の長さ・コーデックなどを読めません。FFmpeg
                    を入れて（見つからなければ設定で場所を指定して）アプリを再起動し、動画のフォルダを取り込み直してください。
                  </p>
                  <button
                    type="button"
                    className="btn small"
                    onClick={() => actions.openDialog({ kind: "settings", tab: "general" })}
                  >
                    設定を開く
                  </button>
                </>
              ) : (
                <p>動画の情報がありません。</p>
              )}
            </div>
          )}
        </Section>
      )}

      <Section id="lib.meta.caption" title={multi ? "キャプション（操作の対象の 1 枚）" : "キャプション"}>
        <CaptionEditor key={s.variant_id} id={s.variant_id} caption={details.caption} disabled={stale} />
      </Section>

      <Section id="lib.meta.files" title="ファイル">
        <ul className="lib-files">
          {details.files.map((f) => (
            <li key={f.file_id}>
              <div className="path selectable" title={f.path ?? undefined}>
                {f.path ?? "（場所が分かりません。ドライブが接続されていない可能性があります）"}
              </div>
              <div className="meta">
                {fileRoleLabel(f.role)}・{formatBytes(f.size)}・
                <span className={f.status === "missing" ? "status-warn" : f.status === "error" ? "status-error" : ""}>
                  {fileStatusText(f)}
                </span>
              </div>
            </li>
          ))}
        </ul>
        <FileActions summary={s} onReload={props.onReload} />
      </Section>
    </>
  );
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <>
      <dt>{k}</dt>
      <dd>{v}</dd>
    </>
  );
}

function FileActions({ summary: s, onReload }: { summary: VariantSummary; onReload: () => void }) {
  const [checking, setChecking] = useState(false);
  const check = async () => {
    setChecking(true);
    try {
      const r = await core.checkFiles([s.variant_id]);
      // 確かめ直した後の状態（場所が分からない・読めないファイルは、前の状態のまま残る）。
      const after = await core.variantDetails(s.variant_id).catch(() => null);
      const notice = fileCheckNotice(r, after ? primaryFile(after.files) : null);
      actions.notify(notice.level, notice.message, notice.detail);
    } catch (e) {
      actions.reportError(e, "元ファイルを確かめられません");
    } finally {
      setChecking(false);
      onReload();
    }
  };
  return (
    <div className="actions">
      <button
        type="button"
        className="btn small"
        onClick={() => appApi.revealVariantFile(s.variant_id).catch((e) => actions.reportError(e, "フォルダで表示できません"))}
      >
        フォルダで表示
      </button>
      <button
        type="button"
        className="btn small"
        title={s.kind === "video" ? "動画を OS の既定のアプリで再生する" : "OS の既定のアプリで開く"}
        onClick={() => appApi.openVariantFile(s.variant_id).catch((e) => actions.reportError(e, "開けません"))}
      >
        {s.kind === "video" ? "既定のアプリで再生" : "既定のアプリで開く"}
      </button>
      {s.file_status !== "ok" && (
        <button type="button" className="btn small" disabled={checking} onClick={() => void check()}>
          ファイルを確かめ直す
        </button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// キャプション
// ---------------------------------------------------------------------------

function CaptionEditor(props: { id: VariantId; caption: string | null; disabled: boolean }) {
  const [draft, setDraft] = useState(props.caption ?? "");
  const [saving, setSaving] = useState(false);
  const dirty = useRef(false);
  const composing = useRef(false);
  const latest = useRef({ id: props.id, draft, caption: props.caption });
  latest.current = { id: props.id, draft, caption: props.caption };
  /** 保存の列（保存が重なっても、後に保存した値が最後に書かれるように順に送る）。 */
  const chain = useRef<Promise<void>>(Promise.resolve());
  /** 最後の保存の失敗（成功したら null）。閉じる前の書き込みで、失敗を伝えるため。 */
  const failure = useRef<unknown>(null);

  // 保存した値（読み直した詳細）が届いたら、編集していなければ合わせる。
  useEffect(() => {
    if (!dirty.current) setDraft(props.caption ?? "");
  }, [props.caption]);

  /** 編集中の内容を保存の列に入れる（戻り値は、それまでの保存がすべて済んだら解決する）。 */
  const commit = (): Promise<void> => {
    if (dirty.current) {
      dirty.current = false;
      const { id, draft: text, caption } = latest.current;
      const value = text.trim() === "" ? null : text;
      if (value !== (caption ?? null)) {
        chain.current = chain.current.then(async () => {
          setSaving(true);
          try {
            await core.setCaption([id], value);
            failure.current = null;
          } catch (e) {
            dirty.current = true;
            failure.current = e;
            actions.reportError(e, "キャプションを保存できません");
          } finally {
            setSaving(false);
          }
        });
      }
    }
    return chain.current;
  };

  // 別の写真に移ったとき（この欄が消えるとき）も、編集中の内容を保存する。カタログを閉じる・終了する前にも
  // 保存を待つ（欄にフォーカスがあるまま ⌘Q などで終了すると、フォーカスが外れず保存されないため）。
  const commitRef = useRef(commit);
  commitRef.current = commit;
  useEffect(() => {
    // 閉じる前の書き込み: 保存を済ませ、保存できていなければ失敗を伝える（閉じるのをやめさせる）。
    const unregister = registerFlush(async () => {
      await commitRef.current();
      if (dirty.current && failure.current !== null) throw failure.current;
    });
    return () => {
      unregister();
      void commitRef.current();
    };
  }, []);

  return (
    <>
      <textarea
        className="lib-caption"
        value={draft}
        disabled={props.disabled}
        placeholder="キャプション（テキスト検索の対象になります）"
        aria-label="キャプション"
        onChange={(e) => {
          dirty.current = true;
          setDraft(e.target.value);
        }}
        onCompositionStart={() => {
          composing.current = true;
        }}
        onCompositionEnd={() => {
          composing.current = false;
        }}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (composing.current || e.nativeEvent.isComposing || e.keyCode === 229) return;
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            void commit();
          } else if (e.key === "Escape") {
            // 編集を取り消す。
            dirty.current = false;
            setDraft(props.caption ?? "");
            e.currentTarget.blur();
          }
        }}
      />
      <div className="hint">
        {saving ? "保存中…" : `フォーカスを外すか ${comboLabel("mod+Enter")} で保存。空にすると削除します。`}
      </div>
    </>
  );
}
