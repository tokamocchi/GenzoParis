// ファイルの内容による同一性（ORG-03）・重複（IMP-04）の設定の画面の表示（DOM を使わない純粋な計算）。
//
// 内容のハッシュ（ファイル全体の BLAKE3）は、カタログを開いている間にコアがバックグラウンドで計算する
// （genzo-api の identity.rs）。ここでは、その状況・計算の結果・内容が同じファイルの組を文にする。

import type { ContentFile, ContentHashStatus, DuplicateGroupList, HashReport } from "../api/types";
import { type Notice, formatCount } from "./dialogFormat";

/** 内容のハッシュの状況の文（設定の「カタログ」の「内容による同一性」）。 */
export function describeHashStatus(s: ContentHashStatus): string[] {
  const lines = [
    `${formatCount(s.hashed)} / ${formatCount(s.files)} 個のファイルの内容のハッシュを記録済みです${
      s.pending > 0 ? `（計算の対象 ${formatCount(s.pending)} 個）` : ""
    }。`,
  ];
  if (s.job_id !== null) lines.push("バックグラウンドで計算しています（上の帯のジョブから取り消せます）。");
  lines.push(
    s.duplicate_groups > 0
      ? `内容が同じファイルの組: ${formatCount(s.duplicate_groups)} 組（記録済みのファイルの中で）`
      : "内容が同じファイルはありません（記録済みのファイルの中で）。",
  );
  return lines;
}

/** 内容のハッシュの計算の結果の知らせ。 */
export function describeHashReport(r: HashReport): Notice {
  const detail: string[] = [];
  if (r.skipped_network > 0) {
    detail.push(
      `ネットワークのボリュームのため読まなかったファイル: ${formatCount(r.skipped_network)} 個（設定で含められます）`,
    );
  }
  if (r.skipped_unavailable > 0) {
    detail.push(`見つからない・ドライブが接続されていないファイル: ${formatCount(r.skipped_unavailable)} 個`);
  }
  if (r.changed > 0) {
    detail.push(
      `登録の後に内容が変わっていたファイル: ${formatCount(r.changed)} 個（「元ファイルを確かめ直す」で登録し直すと計算の対象になります）`,
    );
  }
  if (r.failed_count > 0) detail.push(`読めなかったファイル: ${formatCount(r.failed_count)} 個`);
  for (const f of r.failed.slice(0, 5)) detail.push(`・${f.path}: ${f.reason}`);
  const message = r.cancelled
    ? `内容のハッシュの計算を取り消しました（${formatCount(r.hashed)} 個は記録済み）`
    : `内容のハッシュを ${formatCount(r.hashed)} 個のファイルに記録しました`;
  return {
    level: r.failed_count > 0 ? "warning" : r.cancelled ? "info" : "success",
    message,
    detail: detail.length > 0 ? detail.join("\n") : undefined,
  };
}

/** 内容が同じファイルの 1 つの行（場所・役割・状態）。 */
export function contentFileLine(f: ContentFile): string {
  const where = f.path ?? `場所不明のファイル（ID ${f.file_id}）`;
  const role = f.role === "sidecar_jpeg" ? "（RAW と組の JPEG）" : "";
  const status = f.status === "missing" ? "（見つからない）" : f.status === "error" ? "（読めない）" : "";
  return `${where}${role}${status}`;
}

/** 内容が同じファイルの組の見出し（「1. 2 個のファイル（BLAKE3 aaaaaaaaaaaa…）」）。 */
export function duplicateGroupTitle(index: number, files: number, fullHash: string): string {
  return `${index + 1}. ${formatCount(files)} 個のファイル（BLAKE3 ${fullHash.slice(0, 12)}…）`;
}

/** 組の一覧の補足（総数が表示した数より多いとき）。 */
export function duplicateListNote(list: DuplicateGroupList): string | null {
  if (list.total <= list.groups.length) return null;
  return `全 ${formatCount(list.total)} 組のうち、最初の ${formatCount(list.groups.length)} 組を表示しています。`;
}
