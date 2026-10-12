// 削除のダイアログ（FILE-01）の説明・計画の要約・結果のまとめ（DOM を使わない純粋な計算）。
//
// 削除は 3 つの別々の操作（genzo-api の delete.rs）。どれも計画（影響の一覧）を見せてから実行し、計画の後に
// カタログが変わっていれば、コアは実行せずに PlanOutdated（kind は conflict）を返す。

import type { DeleteKind, DeletePlan, DeleteReport, ErrorInfo } from "../api/types";
import { type Notice, baseName, formatCount, groupReasons } from "./dialogFormat";

export interface DeleteKindOption {
  kind: DeleteKind;
  label: string;
  description: string;
  /** 元のファイルに触れるか（危険の色で示す）。 */
  danger: boolean;
}

export const DELETE_KIND_OPTIONS: DeleteKindOption[] = [
  {
    kind: "remove_from_catalog",
    label: "カタログから除去",
    description:
      "カタログの登録だけを消します。元のファイルは残ります。仮想コピー・現像設定・履歴も消えます（同じフォルダを取り込み直すと、現像していない状態で登録し直します）。",
    danger: false,
  },
  {
    kind: "trash",
    label: "ゴミ箱に移す",
    description:
      "元のファイルを OS のゴミ箱へ移し、カタログからも除去します。ゴミ箱から元に戻せます。ゴミ箱のないドライブ（ネットワークの共有フォルダなど）では移さずに失敗にします（直接は削除しません）。",
    danger: true,
  },
  {
    kind: "virtual_copies",
    label: "仮想コピーだけを削除",
    description: "選んだ仮想コピーを削除します。マスター（元の写真）と元のファイルはそのままです。",
    danger: false,
  },
];

export function deleteKindLabel(kind: DeleteKind): string {
  return DELETE_KIND_OPTIONS.find((o) => o.kind === kind)?.label ?? kind;
}

export interface DeletePlanSummary {
  /** 対象の variant（マスターと仮想コピー）。 */
  variants: number;
  masters: number;
  virtualCopies: number;
  /** 対象の写真・動画（asset）。 */
  assets: number;
  /** 影響を受けるファイル（同時記録の JPEG を含む）。 */
  files: number;
  /** 場所の分からないファイル（ボリュームが接続されていない）。 */
  filesWithoutPath: number;
  /** 選んだもの以外も対象になったか（同じ写真のマスター・仮想コピー）。 */
  includesUnselected: boolean;
}

export function summarizeDeletePlan(plan: DeletePlan, selectedIds: readonly number[]): DeletePlanSummary {
  const selected = new Set(selectedIds);
  const masters = plan.variants.filter((v) => v.is_master).length;
  return {
    variants: plan.variants.length,
    masters,
    virtualCopies: plan.variants.length - masters,
    assets: plan.assets.length,
    files: plan.files.length,
    filesWithoutPath: plan.files.filter((f) => f.path === null).length,
    includesUnselected: plan.variants.some((v) => !selected.has(v.variant_id)),
  };
}

/** 計画の説明の文（画面の確認の文）。 */
export function describeDeletePlan(kind: DeleteKind, s: DeletePlanSummary): string[] {
  const lines: string[] = [];
  const variantsNote =
    s.virtualCopies > 0 ? `（仮想コピー ${formatCount(s.virtualCopies)} 件を含む ${formatCount(s.variants)} 件）` : "";
  switch (kind) {
    case "remove_from_catalog":
      lines.push(`${formatCount(s.assets)} 枚の写真・動画をカタログから除去します${variantsNote}。`);
      lines.push(`元のファイル ${formatCount(s.files)} 個は残ります。`);
      break;
    case "trash":
      lines.push(`${formatCount(s.files)} 個のファイルを OS のゴミ箱へ移します。`);
      lines.push(`${formatCount(s.assets)} 枚の写真・動画をカタログから除去します${variantsNote}。`);
      break;
    case "virtual_copies":
      lines.push(`${formatCount(s.variants)} 件の仮想コピーを削除します。元のファイルは変わりません。`);
      break;
  }
  if (s.includesUnselected && kind !== "virtual_copies") {
    lines.push("選んだ写真と同じ元ファイルのマスター・仮想コピーも、まとめて対象になります。");
  }
  if (s.filesWithoutPath > 0 && kind === "trash") {
    lines.push(
      `${formatCount(s.filesWithoutPath)} 個のファイルは場所が分かりません（ドライブが接続されていない可能性があります）。`,
    );
  }
  return lines;
}

/** 計画の一覧に出す行の上限（仮置き: 200。残りは件数だけ出す）。 */
export const PLAN_LIST_LIMIT = 200;

/** 計画のファイルの一覧（パスが分からないものは「場所不明」）。 */
export function planFileLines(plan: DeletePlan, limit = PLAN_LIST_LIMIT): { lines: string[]; rest: number } {
  const lines = plan.files.slice(0, limit).map((f) => {
    const role = f.role === "sidecar_jpeg" ? "（同時記録の JPEG）" : "";
    return f.path ? `${f.path}${role}` : `場所不明のファイル（ID ${f.file_id}）${role}`;
  });
  return { lines, rest: Math.max(0, plan.files.length - limit) };
}

/** 計画の variant の一覧（仮想コピーの削除の確認で出す）。 */
export function planVariantLines(plan: DeletePlan, limit = PLAN_LIST_LIMIT): { lines: string[]; rest: number } {
  const lines = plan.variants.slice(0, limit).map((v) => {
    const kind = v.is_master ? "マスター" : "仮想コピー";
    return v.name ? `${kind}「${v.name}」（ID ${v.variant_id}）` : `${kind}（ID ${v.variant_id}）`;
  });
  return { lines, rest: Math.max(0, plan.variants.length - limit) };
}

/** 計画が古くなった（計画し直して確認し直す）エラーか。 */
export function isOutdatedPlanError(e: ErrorInfo): boolean {
  if (e.kind === "conflict") return true;
  // 確認待ちの計画は古いものから捨てる（genzo-api の PlanStore）。見つからなければ計画し直す。
  return e.kind === "not_found" && e.message.includes("削除の計画");
}

/** 実行の結果の説明。 */
export function describeDeleteReport(kind: DeleteKind, r: DeleteReport): Notice {
  const lines: string[] = [];
  let message: string;
  switch (kind) {
    case "virtual_copies":
      message = `${formatCount(r.removed_variants.length)} 件の仮想コピーを削除しました`;
      break;
    case "remove_from_catalog":
      message = `${formatCount(r.removed_assets.length)} 枚をカタログから除去しました（元のファイルは残っています）`;
      break;
    case "trash":
      message = `${formatCount(r.trashed_files.length)} 個のファイルをゴミ箱へ移し、${formatCount(
        r.removed_assets.length,
      )} 枚をカタログから除去しました`;
      break;
  }
  if (r.skipped_missing.length > 0) {
    lines.push(
      `見つからなかったファイル ${formatCount(r.skipped_missing.length)} 個は移さずに、カタログから除去しました:`,
      ...r.skipped_missing.slice(0, 3).map((p) => `・${baseName(p)}`),
    );
    if (r.skipped_missing.length > 3) lines.push(`・ほか ${formatCount(r.skipped_missing.length - 3)} 個`);
  }
  if (r.failed.length > 0) {
    lines.push(
      `${formatCount(r.failed.length)} 枚はゴミ箱へ移せなかったため、カタログに残しています:`,
      ...groupReasons(
        r.failed.map((f) => f.reason),
        "枚",
        3,
      ),
    );
  }
  let level: Notice["level"] = "success";
  if (r.failed.length > 0) level = r.removed_variants.length > 0 ? "warning" : "error";
  else if (r.skipped_missing.length > 0) level = "warning";
  return { level, message, detail: lines.length > 0 ? lines.join("\n") : undefined };
}
