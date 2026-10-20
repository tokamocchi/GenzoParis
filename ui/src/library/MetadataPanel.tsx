// メタデータのパネル（LIB-14・VID-03）と、評価・フラグ・カラーラベル・キャプションの操作（LIB-04）。
//
// - 操作の対象（selection.current）の詳細を core.variantDetails で読む（対象・detailsVersion が変わったら
//   読み直す。古い応答は捨てる）。新しい詳細が届くまでは前の詳細を薄く出し、1 枚用の評価などの操作は
//   止める（前の写真の値で、新しい対象を変えないように）。
// - 評価・フラグ・ラベルは actions.setRating などで付ける（グリッドでは選択中のすべて、ルーペでは表示中の
//   1 枚。actions.targets）。グリッドで複数を選んでいるときは「N 件を選択」と、まとめて付ける操作を出す。
// - 説明のメタデータ（LIB-15。タイトル・キャプション・作成者・著作権・撮影場所など。MetadataEditor）:
//   1 枚のときは操作の対象の 1 枚に、グリッドで複数を選んでいるときは選択中のすべてに書く（まとめは
//   core.metadataSummary。写真によって値が異なる項目は「（複数の値）」）。保存の決まりは MetadataEditor の doc。
// - キーワード（LIB-09。KeywordEditor）: 1 枚のときは操作の対象の 1 枚、グリッドで複数を選んでいるときは
//   選択中のすべてに付ける・外す。1 枚のときは、入っているコレクション（LIB-10）も出す。
// - 仮想コピー・スタック（LIB-12・LIB-13。VersionsSection）: 同じ写真のマスターと仮想コピーの一覧（押すと選ぶ）、
//   仮想コピーを作る・名前を変える・元の写真にする、入っているスタックの位置と操作（展開・折りたたみ、一番上に
//   する、一緒に選んだ写真を足す、外す、解除）。
// - GPS の行（LIB-21）: 使う位置（後から付けた位置があればそれ、なければカメラの位置）。後から付けた位置なら注記に
//   カメラの位置も出す。座標のコピー（"35.681236, 139.767125" の形）と「位置を付ける…」（GeotagDialog）。

import { useEffect, useRef, useState } from "react";

import * as appApi from "../api/app";
import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ColorLabel, Flag, GpsCoord, MetadataSummary, VariantDetails, VariantId, VariantSummary } from "../api/types";
import { FlagMark, LabelDot, Stars } from "../components/Marks";
import { Section } from "../components/Section";
import { COLOR_LABEL_NAMES, formatAperture, formatBytes, formatCaptureTime, formatFocal, formatShutter } from "../format";
import { comboLabel } from "../keyboard";
import { actions, useApp } from "../state/app";
import { openCaptureTimeDialog, openGeotagDialog } from "../state/stacks";
import {
  addTargetsToStack,
  createVirtualCopyOf,
  makeMaster,
  removeTargetsFromStack,
  revealAndSelect,
  setStackTop,
  toggleStack,
  unstackTargets,
} from "../state/stacks";
import { gpsSourceNote } from "./geotagLogic";
import { KeywordEditor, PhotoCollections } from "./KeywordEditor";
import "./library.css";
import { MetadataEditor, MetadataHelp } from "./MetadataEditor";
import { variantBriefLabel } from "./stackLogic";
import { batchNote, idsKey, statesFromMetadata, statesFromSummary } from "./metadataEdit";
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
  gpsDecimal,
  hasVideoInfo,
  primaryFile,
  shortHash,
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
      {multi && <BatchKeywords />}
      {multi && <BatchMetadata version={detailsVersion} />}
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

/** 座標を "35.681236, 139.767125" の形でクリップボードに写す（書けなければ、文字を選んで写すよう案内する）。 */
async function copyCoordinates(g: GpsCoord): Promise<void> {
  try {
    if (typeof navigator === "undefined" || !navigator.clipboard) throw new Error("クリップボードを使えません");
    await navigator.clipboard.writeText(gpsDecimal(g));
    actions.notify("info", "座標をコピーしました");
  } catch {
    actions.notify("warning", "コピーできません。座標の文字を選んでコピーしてください");
  }
}

/**
 * GPS の行の値（LIB-21）: 使う位置、後から付けた位置ならその注記（カメラの位置も出す）、座標のコピーと「位置を
 * 付ける…」。動画 1 本だけのときは位置を付けない（まとめて選んだ中の動画はコアが除く）。
 */
export function GpsValue(props: { details: VariantDetails; stale: boolean; multi: boolean }) {
  const { details, stale, multi } = props;
  const gps = details.gps;
  const note = gpsSourceNote(details);
  const canEdit = details.summary.kind !== "video" || multi;
  return (
    <>
      {formatGps(gps) ?? "なし"}
      {note && <span className="note">{note}</span>}
      {(gps || canEdit) && (
        <span className="lib-gps-actions">
          {gps && (
            <button
              type="button"
              className="btn small"
              onClick={() => void copyCoordinates(gps)}
              disabled={stale}
              title="座標を「緯度, 経度」の 10 進の度（35.681236, 139.767125 の形）でコピーする"
            >
              コピー
            </button>
          )}
          {canEdit && (
            <button
              type="button"
              className="btn small"
              onClick={() => openGeotagDialog()}
              disabled={stale}
              title="座標を貼り付けて、選んだ写真にまとめて位置を付ける（元のファイルは変えません。動画は除きます）"
            >
              位置を付ける…
            </button>
          )}
        </span>
      )}
    </>
  );
}

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
            <button
              type="button"
              className="btn small lib-capture-edit"
              onClick={() => openCaptureTimeDialog()}
              disabled={stale}
              title="時計のずれ・タイムゾーンを、選んだ写真や機材ごとにまとめて直す（元のファイルは変えません）"
            >
              修正…
            </button>
          </dd>
          <dt>カメラ</dt>
          <dd>
            {s.camera ?? "—"}
            {details.camera_serial && <span className="note">シリアル番号 {details.camera_serial}</span>}
          </dd>
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
          <dd>
            <GpsValue details={details} stale={stale} multi={multi} />
          </dd>
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

      {!multi && <VersionsSection details={details} disabled={stale} />}

      {!multi && (
        <>
          <KeywordEditor key={`kw${s.variant_id}`} targets={[s.variant_id]} disabled={stale} />
          <PhotoCollections variantId={s.variant_id} />
          <MetadataEditor
            key={s.variant_id}
            targets={[s.variant_id]}
            states={statesFromMetadata(details.metadata, details.caption)}
            disabled={stale}
          />
          <MetadataHelp />
        </>
      )}

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
                {f.full_hash !== undefined && (
                  <span title={f.full_hash ? `内容のハッシュ（BLAKE3）: ${f.full_hash}` : "内容のハッシュはまだ計算していません（バックグラウンドで計算します）"}>
                    ・内容 {shortHash(f.full_hash)}
                  </span>
                )}
              </div>
            </li>
          ))}
        </ul>
        <FileActions summary={s} onReload={props.onReload} />
      </Section>
    </>
  );
}

/**
 * 仮想コピー・スタック（LIB-12・LIB-13）: 同じ写真のマスターと仮想コピーの一覧（押すとその写真を選ぶ）と、
 * 入っているスタックの位置・操作。操作は表示中の 1 枚（この詳細の写真）に対して行う。
 */
function VersionsSection({ details, disabled }: { details: VariantDetails; disabled: boolean }) {
  const s = details.summary;
  const variants = details.variants ?? [];
  const stack = details.stack ?? null;
  const isCopy = !s.is_master;
  const position = stack ? stack.members.indexOf(s.variant_id) + 1 : 0;
  // 一緒に選んでいる、このスタックに入っていない写真の数（「選んだ写真を足す」）。
  const selectedIds = useApp((st) => st.selection.ids);
  const addable = stack ? selectedIds.filter((id) => !stack.members.includes(id)).length : 0;
  return (
    <Section id="lib.meta.versions" title="仮想コピー・スタック">
      {variants.length > 1 && (
        <ul className="lib-versions" aria-label="同じ写真のマスターと仮想コピー">
          {variants.map((v) => (
            <li key={v.variant_id}>
              <button
                type="button"
                className={`lib-version${v.variant_id === s.variant_id ? " on" : ""}`}
                disabled={disabled || v.variant_id === s.variant_id}
                onClick={() => void revealAndSelect(v.variant_id)}
                title={v.variant_id === s.variant_id ? "表示中の写真" : "この写真を選ぶ（今の絞り込みで表示されていなければ選べません）"}
              >
                {variantBriefLabel(v)}
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="actions">
        <button
          type="button"
          className="btn small"
          disabled={disabled || s.kind === "video"}
          onClick={() => void createVirtualCopyOf(s.variant_id)}
          title={`元のファイルを複製せずに、別の現像設定を持てる写しを作る（${comboLabel("mod+'")}）`}
        >
          仮想コピーを作る
        </button>
        {isCopy && (
          <>
            <button
              type="button"
              className="btn small"
              disabled={disabled}
              onClick={() => actions.openDialog({ kind: "virtual_copy_name", variantId: s.variant_id })}
            >
              名前を変える…
            </button>
            <button
              type="button"
              className="btn small"
              disabled={disabled}
              onClick={() => void makeMaster(s.variant_id)}
              title="この仮想コピーを元の写真（マスター）にする（前のマスターは仮想コピーになる。現像設定・評価などはそれぞれのまま）"
            >
              元の写真にする
            </button>
          </>
        )}
      </div>
      {stack ? (
        <>
          <p className="sub">
            スタック: {stack.members.length.toLocaleString("ja-JP")} 枚中 {position.toLocaleString("ja-JP")} 枚目
            {position === 1 ? "（一番上）" : ""}・{stack.collapsed ? "折りたたみ中" : "展開中"}
          </p>
          <div className="actions">
            <button
              type="button"
              className="btn small"
              disabled={disabled}
              onClick={() => void toggleStack(s.variant_id)}
              title={`グリッド・フィルムストリップでの表示を切り替える（${comboLabel("s")}）`}
            >
              {stack.collapsed ? "展開" : "折りたたむ"}
            </button>
            <button
              type="button"
              className="btn small"
              disabled={disabled || position === 1}
              onClick={() => void setStackTop(s.variant_id)}
              title={`この写真をスタックの一番上にする（${comboLabel("shift+s")}）`}
            >
              一番上にする
            </button>
            <button
              type="button"
              className="btn small"
              disabled={disabled || addable === 0}
              onClick={() => void addTargetsToStack()}
              title="グリッド・フィルムストリップで一緒に選んだ写真を、このスタックの最後に足す（一番上と折りたたみは変えない。ほかのスタックの写真は移す）"
            >
              {addable > 0 ? `選んだ ${addable.toLocaleString("ja-JP")} 枚を足す` : "選んだ写真を足す"}
            </button>
            <button type="button" className="btn small" disabled={disabled} onClick={() => void removeTargetsFromStack()}>
              スタックから外す
            </button>
            <button
              type="button"
              className="btn small"
              disabled={disabled}
              onClick={() => void unstackTargets()}
              title={`スタックを解除する（写真は消えない。${comboLabel("mod+shift+g")}）`}
            >
              解除
            </button>
          </div>
        </>
      ) : (
        <p className="hint">
          スタックに入っていません。グリッドで 2 枚以上を選んで {comboLabel("mod+g")} でまとめられます。
        </p>
      )}
    </Section>
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
// 説明のメタデータの一括編集（グリッドで複数を選んでいるとき）
// ---------------------------------------------------------------------------

/** 選択中のすべての写真の説明のメタデータ（まとめを読み、入力した項目だけをすべてに書く）。 */
function BatchMetadata({ version }: { version: number }) {
  const ids = useApp((s) => s.selection.ids);
  const key = idsKey(ids);
  const [state, setState] = useState<{ key: string; summary: MetadataSummary } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);
  useEffect(() => {
    const my = ++seq.current;
    setError(null);
    const t = setTimeout(() => {
      core
        .metadataSummary(ids)
        .then((summary) => {
          if (seq.current === my) setState({ key, summary });
        })
        .catch((e: unknown) => {
          if (seq.current !== my) return;
          const err = toApiError(e);
          if (err.kind === "cancelled" || err.kind === "stale") return;
          setError(err.info.message);
        });
    }, DETAILS_DELAY_MS);
    return () => clearTimeout(t);
    // ids は key で比べる（選択のたびに配列が作り直されても、同じ並びなら読み直さない）。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, version]);
  if (error && !state) return <p className="error-text lib-meta-note">メタデータを読めません: {error}</p>;
  if (!state) return null;
  const stale = state.key !== key;
  return (
    <div className={stale ? "lib-meta-batch stale" : "lib-meta-batch"}>
      <MetadataEditor
        key={key}
        targets={ids}
        states={statesFromSummary(state.summary)}
        disabled={stale}
        note={batchNote(ids.length, state.summary.assets)}
      />
      <MetadataHelp />
    </div>
  );
}

// ---------------------------------------------------------------------------
// キーワード（グリッドで複数を選んでいるとき）
// ---------------------------------------------------------------------------

/** 選択中のすべての写真のキーワード（付ける・外すは選択中のすべてに）。 */
function BatchKeywords() {
  const ids = useApp((s) => s.selection.ids);
  return <KeywordEditor key={`kw-batch`} targets={ids} />;
}
