// 選別表示（LIB-03。N）: 選択中の写真を並べ、作業中の写真（枠で強調）に評価・フラグ・ラベルを付けながら選ぶ。
//
// - 並べるのは選択中の写真（選んだ順。表示中の写真を含める）を MAX_SURVEY_PHOTOS 枚まで（compareLogic の
//   surveyPhotos。超えた分は数だけを出す）。並べ方は、枠に写真を収めたときの面積の和が最も大きい列の数
//   （surveyLayout）。
// - 枠が小さいときはサムネイル（L0）だけを、写真を出す長辺が SURVEY_L1_MIN_PX（物理画素）を超えたら標準プレビュー
//   （L1）を読む（同時に読む高解像度の画像の数とメモリを抑える）。拡大はしない（比較表示・ルーペで行う）。
// - 枠のクリック・←・→ で作業中の写真を移す。× と「/」で表示から外す（選択から外す。写真は消えない。1 枚だけに
//   なったら外さない）。フィルムストリップの Ctrl / ⌘ + クリックで足す・外す（普通のクリックは、その 1 枚だけに
//   する）。評価などのキーは作業中の写真に付く（「自動で次へ」は働かない）。枠の下の ★ のクリックでも評価できる。

import { useRef } from "react";

import * as core from "../api/core";
import type { Rating, VariantId, VariantSummary } from "../api/types";
import { FlagMark, LabelDot, Stars } from "../components/Marks";
import { comboLabel, useHotkeys } from "../keyboard";
import { actions, useApp } from "../state/app";
import { focusSurvey, removeSurveyPhoto, stepSurvey, surveyOf } from "../state/compare";
import { searchCache, thumbRevs, useSearchVersion } from "../state/search";
import {
  MAX_SURVEY_PHOTOS,
  SURVEY_CAPTION_PX,
  SURVEY_GAP_PX,
  SURVEY_L1_MIN_PX,
  surveyLayout,
} from "./compareLogic";
import { useViewRect } from "./hooks";
import { SlotImages, usePreviewImages } from "./previewImages";
import { useVariantSummary } from "./useSummary";
import { type Rect, fitRect, orientedSize } from "./viewerMath";
import "./viewer.css";

/** 写真の縦横比（幅 / 高さ。向きを反映。分からなければ 3:2）。 */
function aspectOf(id: VariantId): number {
  const v = searchCache.getLoadedById(id);
  const o = v ? orientedSize(v.width, v.height, v.orientation) : null;
  return o ? o.width / o.height : 1.5;
}

export function SurveyView() {
  const ids = useApp((s) => s.selection.ids);
  const current = useApp((s) => s.selection.current);
  const hotkeysEnabled = useApp((s) => s.dialog === null && s.mode === "survey");
  useSearchVersion();
  const { shown, hidden } = surveyOf({ ids, current });
  const areaRef = useRef<HTMLDivElement>(null);
  const area = useViewRect(areaRef);
  const layout = surveyLayout(shown.map(aspectOf), area.width, area.height, SURVEY_GAP_PX, SURVEY_CAPTION_PX);

  const removeCurrent = () => {
    if (current === null) return;
    if (!removeSurveyPhoto(current)) actions.notify("info", "最後の 1 枚は表示から外せません");
  };
  useHotkeys(
    {
      ArrowLeft: () => stepSurvey(-1),
      ArrowRight: () => stepSurvey(1),
      "/": removeCurrent,
    },
    hotkeysEnabled,
  );

  const total = shown.length + hidden;
  return (
    <div className="survey">
      <div ref={areaRef} className="survey-area">
        {shown.map((id, i) =>
          layout.cells[i] ? (
            <SurveyCell
              key={id}
              id={id}
              rect={layout.cells[i]}
              active={id === current}
              dpr={area.dpr}
              removable={shown.length > 1}
            />
          ) : null,
        )}
        {shown.length === 0 && <div className="empty-state">写真が選ばれていません</div>}
      </div>
      <div className="compare-bar">
        <span>
          {hidden > 0
            ? `選んだ ${total.toLocaleString()} 枚のうち、${MAX_SURVEY_PHOTOS} 枚を表示しています`
            : `${shown.length.toLocaleString()} 枚を表示しています`}
        </span>
        <span className="hint">
          フィルムストリップの {comboLabel("mod")} + クリックで足す・外す。× か {comboLabel("/")} で表示から外します（写真は消えません）
        </span>
        <span className="loupe-video-spacer" />
        <button className="btn small" disabled={current === null || shown.length <= 1} onClick={removeCurrent} title={`作業中の写真を表示から外す（${comboLabel("/")}）`}>
          表示から外す
        </button>
      </div>
    </div>
  );
}

function SurveyCell(props: { id: VariantId; rect: Rect; active: boolean; dpr: number; removable: boolean }) {
  const { id, rect } = props;
  const summary = useVariantSummary(id);
  const photoH = Math.max(0, rect.height - SURVEY_CAPTION_PX);
  const cls = `survey-cell${props.active ? " active" : ""}`;
  const setRating = (r: Rating) => {
    core.setRating([id], r).catch((e) => actions.reportError(e, "評価を変えられません"));
  };
  return (
    <div
      className={cls}
      style={{ left: rect.x, top: rect.y, width: rect.width, height: rect.height }}
      onClick={() => focusSurvey(id)}
      title={summary?.file_name}
    >
      <div className="survey-photo" style={{ height: photoH }}>
        {summary ? (
          <SurveyPhoto key={id} summary={summary} width={rect.width} height={photoH} dpr={props.dpr} />
        ) : (
          <div className="loupe-center-note">
            <div className="spinner" />
          </div>
        )}
      </div>
      <div className="survey-caption">
        {summary && (
          <>
            <Stars rating={summary.rating} size={12} onSet={setRating} />
            <FlagMark flag={summary.flag} />
            <LabelDot label={summary.color_label} />
            <span className="survey-name">{summary.file_name}</span>
          </>
        )}
      </div>
      {props.removable && (
        <button
          type="button"
          className="survey-remove"
          title={`表示から外す（${comboLabel("/")}。写真は消えません）`}
          aria-label="表示から外す"
          onClick={(e) => {
            e.stopPropagation();
            removeSurveyPhoto(id);
          }}
        >
          ×
        </button>
      )}
    </div>
  );
}

function SurveyPhoto(props: { summary: VariantSummary; width: number; height: number; dpr: number }) {
  const { summary } = props;
  useApp((s) => s.thumbRevsVersion);
  const thumbRev = thumbRevs.of(summary);
  const o = orientedSize(summary.width, summary.height, summary.orientation) ?? { width: 3, height: 2 };
  // 写真を出す長辺（物理画素）が L0 の長辺を超えたら L1 を読む（動画には L1 がない）。
  const approx = fitRect(o.width, o.height, props.width, props.height, 4);
  const wantL1 = summary.kind === "photo" && Math.max(approx.width, approx.height) * props.dpr > SURVEY_L1_MIN_PX;
  const { l0, l1, onL1Error } = usePreviewImages({ summary, thumbRev, waitMs: () => 0, wantL1 });
  const shown = l1.shown ?? l0.shown;
  const natural = shown ? { width: shown.width, height: shown.height } : o;
  const box = fitRect(natural.width, natural.height, props.width, props.height, 4);
  return (
    <>
      <div className="loupe-photo" style={{ left: box.x, top: box.y, width: box.width, height: box.height }}>
        {!l1.shown && <SlotImages slot={l0} />}
        <SlotImages slot={l1} onError={onL1Error} />
        {summary.kind === "video" && (
          <span className="loupe-video-mark small" aria-hidden="true">
            ▶
          </span>
        )}
      </div>
      {!shown && (l0.url === null || l0.failed) && l1.url === null && (
        <div className="loupe-center-note">
          <p>プレビューがありません</p>
        </div>
      )}
      {summary.file_status !== "ok" && (
        <span className="survey-missing" title={summary.file_status === "missing" ? "元ファイルが見つかりません" : "元ファイルを読めません"}>
          !
        </span>
      )}
    </>
  );
}
