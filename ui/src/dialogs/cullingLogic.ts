// 選別支援（ORG-06。写真の右クリックの「選別支援…」）の、結果の文と、選ぶ・まとめる対象の計算（DOM を使わない）。
//
// コア（culling_assist）が、カタログのサムネイルから、似た写真のグループ（撮影日時の順に続く、時刻が近くて
// 見た目の近い写真）と、その中でピントの甘い見込みの写真（いちばんシャープな写真より明らかに甘いもの）を返す。
// 何も変えない。画面は、甘い見込みの写真を選ぶ・グループをスタックにまとめる操作を出す（決めるのは利用者）。
// サムネイル（長辺 320）での判定なので粗い。

import type { CullingOptions, CullingReport, VariantId } from "../api/types";

/** 既定の設定（genzo-api の CullingOptions の既定と同じ）。 */
export const DEFAULT_CULLING_OPTIONS: CullingOptions = { max_gap_seconds: 10, max_distance: 10, soft_ratio: 0.7 };

const n = (v: number) => v.toLocaleString("ja-JP");

/** 結果の見出しの文。 */
export function describeCulling(r: CullingReport): string {
  if (r.groups.length === 0) return `${n(r.analyzed)} 枚を調べました。似た写真のグループは見つかりませんでした`;
  return `${n(r.analyzed)} 枚を調べました: 似た写真のグループ ${n(r.groups.length)} 組（${n(r.grouped)} 枚）、ピントの甘い見込み ${n(r.soft)} 枚`;
}

/** 調べなかった写真の説明（数が 0 のものは出さない）。 */
export function cullingSkips(r: CullingReport): string[] {
  const lines: string[] = [];
  if (r.skipped_videos > 0) lines.push(`動画 ${n(r.skipped_videos)} 本は調べていません`);
  if (r.without_time > 0) lines.push(`撮影日時のない ${n(r.without_time)} 枚は調べていません`);
  if (r.without_thumbnail > 0) lines.push(`サムネイルのない ${n(r.without_thumbnail)} 枚は調べていません`);
  if (r.missing > 0) lines.push(`${n(r.missing)} 枚は見つかりませんでした`);
  if (r.cancelled) lines.push("途中で取り消されました");
  return lines;
}

/** 甘い見込みの写真（グループの順。重ならない）。 */
export function softVariants(r: CullingReport): VariantId[] {
  return [...new Set(r.groups.flatMap((g) => g.soft))];
}

/** グループの 1 行の文（"3 枚: いちばんシャープな見込み 1 枚、甘い見込み 1 枚"）。 */
export function describeGroup(g: CullingReport["groups"][number]): string {
  const soft = g.soft.length > 0 ? `、甘い見込み ${n(g.soft.length)} 枚` : "";
  return `${n(g.variant_ids.length)} 枚${soft}`;
}
