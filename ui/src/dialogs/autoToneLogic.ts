// 複数の写真への一括の自動補正（DEV-32。写真の右クリックのメニューの「自動補正を当てる…」）の、結果の文。
//
// コアは 1 枚ずつ値を求めて保存する（ジョブ。1 枚ごとに履歴に「自動補正」を 1 件）。動画は飛ばし、見つからない・
// 読めない写真は失敗に数えて続ける。値が今と同じ写真は保存しない。

import type { AutoToneBatchReport, JobInfo } from "../api/types";
import { type Notice, formatCount } from "./dialogFormat";

/** 失敗の理由を出す件数の上限（残りは件数だけ）。 */
export const AUTO_TONE_FAILURES_SHOWN = 5;

/** 一括の自動補正の結果の通知。 */
export function describeAutoToneReport(r: AutoToneBatchReport): Notice {
  const lines: string[] = [];
  if (r.unchanged > 0) lines.push(`値が今と同じでそのままの写真: ${formatCount(r.unchanged)} 枚`);
  if (r.skipped > 0) lines.push(`飛ばした動画: ${formatCount(r.skipped)} 本`);
  if (r.failed.length > 0) {
    lines.push(`失敗した写真: ${formatCount(r.failed.length)} 枚`);
    lines.push(...r.failed.slice(0, AUTO_TONE_FAILURES_SHOWN).map((f) => `- ${f.reason}`));
    if (r.failed.length > AUTO_TONE_FAILURES_SHOWN) {
      lines.push(`ほか ${formatCount(r.failed.length - AUTO_TONE_FAILURES_SHOWN)} 件`);
    }
  }
  const detail = lines.length > 0 ? lines.join("\n") : undefined;
  if (r.cancelled) {
    return {
      level: "info",
      message: `自動補正を取り消しました（${formatCount(r.applied)} 枚には当てました。現像の画面の履歴から戻せます）`,
      detail,
    };
  }
  if (r.applied === 0) {
    return {
      level: r.failed.length > 0 ? "warning" : "info",
      message:
        r.failed.length > 0 ? "自動補正を当てられた写真はありませんでした" : "自動補正で値が変わる写真はありませんでした",
      detail,
    };
  }
  return {
    level: r.failed.length > 0 ? "warning" : "success",
    message: `${formatCount(r.applied)} 枚に自動補正を当てました`,
    detail,
  };
}

/**
 * 一括の自動補正のジョブの結果の説明。ジョブそのものの失敗（state が failed）は state/app.ts が job_finished の
 * イベントで通知するので null（二重に知らせない）。
 */
export function describeAutoToneJob(info: JobInfo): Notice | null {
  if (info.state === "failed") return null;
  const r = info.result;
  if (r && r.type === "auto_tone") return describeAutoToneReport(r);
  if (info.state === "cancelled") return { level: "info", message: "自動補正を取り消しました" };
  return { level: "success", message: "自動補正が終わりました" };
}
