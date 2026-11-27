// 動画のスクラブ表示（VID-05）と、動画からの静止画の切り出し（VID-07）の、画面の計算（DOM を使わない）。
//
// - スクラブ表示: グリッドの動画のサムネイルの上でカーソルを左右に動かすと、動画の長さを SCRUB_FRAMES 等分した
//   フレームのうち、カーソルの位置の番号のものを出す（genzo:// の /scrub/{動画}/{番号}。コアが利用者の ffmpeg で
//   作る。ffmpeg がなければ出ない）。
// - 静止画の切り出し: 再生している位置のフレームを JPEG にして、動画と同じフォルダに保存し、カタログに登録する
//   （コアのジョブ）。

import type { JobInfo, VideoFrameReport } from "../api/types";
import { type Notice, baseName } from "../dialogs/dialogFormat";

/** スクラブ表示のフレームの数（genzo-api の SCRUB_FRAMES と同じ）。 */
export const SCRUB_FRAMES = 10;

/** カーソルを載せてから、スクラブ表示を始めるまでの時間（ms。通り過ぎるだけのときに ffmpeg を動かさない。仮置き）。 */
export const SCRUB_HOVER_DELAY_MS = 250;

/**
 * カーソルの位置（要素の左端からの画素）から、スクラブ表示のフレームの番号を求める（幅が分からなければ 0）。
 */
export function scrubIndex(x: number, width: number, frames = SCRUB_FRAMES): number {
  if (!(width > 0) || !Number.isFinite(x)) return 0;
  return Math.min(frames - 1, Math.max(0, Math.floor((x / width) * frames)));
}

/** 切り出しの結果の通知。 */
export function describeFrameReport(r: VideoFrameReport): Notice {
  const name = baseName(r.path);
  if (r.registered === null) {
    return {
      level: "warning",
      message: `静止画を保存しましたが、カタログに登録できませんでした: ${name}`,
      detail: [`保存先: ${r.path}`, r.registration_error ?? ""].filter((l) => l !== "").join("\n"),
    };
  }
  return {
    level: r.registration_error ? "warning" : "success",
    message: `静止画を切り出して登録しました: ${name}（${r.width} × ${r.height}）`,
    detail: [`保存先: ${r.path}`, r.registration_error ?? ""].filter((l) => l !== "").join("\n"),
  };
}

/**
 * 切り出しのジョブの結果の説明。ジョブそのものの失敗（state が failed）は state/app.ts が job_finished のイベントで
 * 通知するので null。
 */
export function describeFrameJob(info: JobInfo): Notice | null {
  if (info.state === "failed") return null;
  const r = info.result;
  if (r && r.type === "video_frame") return describeFrameReport(r);
  if (info.state === "cancelled") return { level: "info", message: "静止画の切り出しを取り消しました" };
  return { level: "success", message: "静止画を切り出しました" };
}
