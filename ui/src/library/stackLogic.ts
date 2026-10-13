// スタック（LIB-12）・仮想コピー（LIB-13）の画面の計算（DOM を使わない純粋な計算。docs/04 の 3.10 節）。
//
// - グリッド・フィルムストリップのスタックの印: 折りたたんだスタックは写真の数、展開したスタックは「何枚目 /
//   写真の数」（stackBadgeLabel）。印のクリックと S で折りたたみを切り替える。
// - 選択とスタック（決定事項ログ No.86）: 操作は表示している写真（折りたたんだスタックは代表の 1 枚）だけに
//   付く。削除のダイアログでは、折りたたんだスタックの中に隠れた写真も含めるかを選べる（hiddenStackMembers。
//   既定は含めない）。
// - 写真の右クリックのメニュー（photoMenuEntries）は、項目の種類・文言・使えるかだけを決める（実行は
//   state/stacks.ts）。撮影日時の修正（LIB-16）・処理バージョンの更新（DATA-09）・フォルダへの移動と名前の変更
//   （FILE-02）・見つからない写真の再リンク（FILE-04）のダイアログも、このメニューから開ける。

import type {
  AutoStackReport,
  DeleteKind,
  FileStatus,
  StackBadge,
  StackInfo,
  VariantBrief,
  VariantId,
  VariantSummary,
} from "../api/types";
import { parseRangeInput } from "./filterLogic";

/** 仮想コピーの名前の文字数の上限（genzo-catalog の MAX_VARIANT_NAME_CHARS と同じ）。 */
export const MAX_COPY_NAME_CHARS = 200;

/** 自動スタックの間隔の上限（秒。genzo-api の MAX_AUTO_STACK_GAP_SECONDS と同じ）。 */
export const MAX_AUTO_STACK_GAP_SECONDS = 3600;

/** 自動スタックの間隔の既定値（秒。**仮置き**: 連写の 1 回分がまとまる程度）。 */
export const DEFAULT_AUTO_STACK_GAP_SECONDS = 2;

/** スタックの印の文字（折りたたみ: 写真の数、展開: 何枚目 / 写真の数）。`wide` なら言葉を足す。 */
export function stackBadgeLabel(b: StackBadge, wide: boolean): string {
  if (b.collapsed) return wide ? `スタック ${b.count}` : String(b.count);
  return `${b.position}/${b.count}`;
}

/** スタックの印の説明（ツールチップ）。 */
export function stackBadgeTitle(b: StackBadge): string {
  if (b.collapsed) return `スタック（${b.count} 枚を折りたたみ中）。クリックか S で展開`;
  const top = b.position === 1 ? "。一番上の写真" : "";
  return `スタックの ${b.position} 枚目（${b.count} 枚）${top}。クリックか S で折りたたむ`;
}

/** 仮想コピーの名前を確かめる（問題がなければ null）。コアと同じ規則（空でない・200 文字まで・制御文字なし）。 */
export function validateCopyName(name: string): string | null {
  const t = name.trim();
  if (t === "") return "名前を入れてください";
  if ([...t].length > MAX_COPY_NAME_CHARS) return `名前は ${MAX_COPY_NAME_CHARS} 文字までにしてください`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(t)) return "改行・タブなどの制御文字は使えません";
  return null;
}

/** 自動スタックの間隔の入力（秒。小数・全角の数字も読む）を読む。読めない・範囲の外なら null。 */
export function parseGapSeconds(text: string): number | null {
  const r = parseRangeInput(text, false, MAX_AUTO_STACK_GAP_SECONDS);
  return r.ok && r.value !== null ? r.value : null;
}

/** 自動スタックの結果（見込み・実行）の文。 */
export function describeAutoStack(r: AutoStackReport): string[] {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const lines: string[] = [];
  if (r.stacks === 0) {
    lines.push(`この間隔でまとめられる写真はありません（対象 ${n(r.considered)} 枚）。`);
  } else if (r.dry_run) {
    lines.push(`${n(r.stacks)} 個のスタックを作ります（${n(r.photos)} 枚。対象 ${n(r.considered)} 枚）。`);
  } else {
    lines.push(`${n(r.stacks)} 個のスタックを作りました（${n(r.photos)} 枚）。`);
  }
  if (r.skipped_in_stacks > 0) lines.push(`すでにスタックに入っている ${n(r.skipped_in_stacks)} 枚は対象にしません。`);
  if (r.skipped_without_time > 0) lines.push(`撮影日時の分からない ${n(r.skipped_without_time)} 枚は対象にしません。`);
  return lines;
}

/**
 * 選んだ写真のうち、折りたたんだスタックの代表のものについて、そのスタックの中に隠れている写真（選んでいない
 * もの）。展開したスタックの写真は表示されているので含めない。重なりは除き、スタックの順に並べる。
 */
export function hiddenStackMembers(selected: readonly VariantId[], stacks: readonly StackInfo[]): VariantId[] {
  const chosen = new Set(selected);
  const out: VariantId[] = [];
  const seen = new Set<VariantId>();
  for (const s of stacks) {
    if (!s.collapsed || !s.members.some((m) => chosen.has(m))) continue;
    for (const m of s.members) {
      if (!chosen.has(m) && !seen.has(m)) {
        seen.add(m);
        out.push(m);
      }
    }
  }
  return out;
}

/**
 * Delete・Backspace で開く削除のダイアログの既定の種類: 対象がすべて仮想コピーと分かっていれば「仮想コピーだけを
 * 削除」（カタログからの除去を既定にすると、マスターと元の写真ごと除いてしまうため。Lightroom と同じ）。分から
 * ないもの・マスターが含まれていれば「カタログから除去」。
 */
export function defaultDeleteKind(
  targets: readonly VariantId[],
  lookup: (id: VariantId) => Pick<VariantSummary, "is_master"> | undefined,
): DeleteKind {
  if (targets.length === 0) return "remove_from_catalog";
  return targets.every((id) => lookup(id)?.is_master === false) ? "virtual_copies" : "remove_from_catalog";
}

/** 同じ写真のマスター・仮想コピーの一覧の 1 行の名前。 */
export function variantBriefLabel(v: VariantBrief): string {
  return v.is_master ? "マスター（元の写真）" : `仮想コピー: ${v.name ?? "（名前なし）"}`;
}

/** 写真の右クリックのメニューの項目の種類（実行は state/stacks.ts の runPhotoMenu）。 */
export type PhotoMenuAction =
  | "create_copy"
  | "rename_copy"
  | "make_master"
  | "group"
  | "add_to_stack"
  | "toggle_stack"
  | "set_top"
  | "remove_from_stack"
  | "unstack"
  | "expand_all"
  | "collapse_all"
  | "auto_stack"
  | "edit_capture_time"
  | "upgrade_process"
  | "move_to_folder"
  | "rename_files"
  | "relink";

export interface PhotoMenuEntry {
  action: PhotoMenuAction;
  label: string;
  /** ショートカット（keyboard.ts の書き方）。 */
  keys?: string;
  enabled: boolean;
}

/** 区切り線。 */
export const PHOTO_MENU_SEPARATOR = "separator" as const;

/**
 * 写真の右クリックのメニューの項目。`targets` は操作の対象の数、`current` は右クリックした（表示中の）写真の
 * 項目（読み込んでいなければ undefined）。
 */
export function photoMenuEntries(
  targets: number,
  current: (Pick<VariantSummary, "is_master" | "stack" | "kind"> & { file_status?: FileStatus }) | undefined,
): (PhotoMenuEntry | typeof PHOTO_MENU_SEPARATOR)[] {
  const stack = current?.stack ?? null;
  const any = targets >= 1 || current !== undefined;
  const isCopy = current?.is_master === false;
  const isVideo = current?.kind === "video";
  return [
    { action: "create_copy", label: "仮想コピーを作る", keys: "mod+'", enabled: current !== undefined && !isVideo },
    { action: "rename_copy", label: "仮想コピーの名前を変える…", enabled: isCopy },
    { action: "make_master", label: "元の写真（マスター）にする", enabled: isCopy },
    PHOTO_MENU_SEPARATOR,
    { action: "group", label: "スタックにまとめる", keys: "mod+g", enabled: targets >= 2 },
    { action: "add_to_stack", label: "選んだ写真をスタックに足す", enabled: targets >= 2 },
    {
      action: "toggle_stack",
      label: stack?.collapsed === false ? "スタックを折りたたむ" : "スタックを展開",
      keys: "s",
      enabled: stack !== null,
    },
    { action: "set_top", label: "スタックの一番上にする", keys: "shift+s", enabled: stack !== null && stack.position > 1 },
    { action: "remove_from_stack", label: "スタックから外す", enabled: stack !== null },
    { action: "unstack", label: "スタックを解除", keys: "mod+shift+g", enabled: stack !== null },
    PHOTO_MENU_SEPARATOR,
    { action: "expand_all", label: "すべてのスタックを展開", enabled: true },
    { action: "collapse_all", label: "すべてのスタックを折りたたむ", enabled: true },
    { action: "auto_stack", label: "撮影時刻でスタック…", enabled: true },
    PHOTO_MENU_SEPARATOR,
    { action: "edit_capture_time", label: "撮影日時を修正…", enabled: any },
    { action: "upgrade_process", label: "処理バージョンを最新に更新…", enabled: any && !(targets <= 1 && isVideo) },
    PHOTO_MENU_SEPARATOR,
    { action: "move_to_folder", label: "フォルダへ移動…", enabled: any },
    { action: "rename_files", label: "ファイルの名前を変える…", keys: "F2", enabled: any },
    { action: "relink", label: "見つからない写真を探す…", enabled: current?.file_status === "missing" },
  ];
}
