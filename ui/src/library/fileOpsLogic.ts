// アプリの中でのファイル操作（FILE-02）・ボリューム（FILE-03）・見つからない写真と再リンク（FILE-04）の、
// DOM を使わない計算（文言・既定の選択・名前の確かめ）。
//
// - 名前の規則は、コア（genzo-api の fileops.rs の validate_name。取り込みのリネームと同じ）と同じ: どの OS でも
//   使える名前だけ（使えない文字 < > : " / \ | ? * と制御文字、Windows の予約名、先頭の「.」、末尾の空白と「.」、
//   UTF-8 で 200 バイトを超える長さはエラー）。画面で先に知らせ、最後はコアが確かめる。
// - 再リンクの候補の既定の選択: 確からしい候補（内容が同じ・大きさと先頭と末尾が同じ）が 1 つだけなら選ぶ。
//   名前と撮影日時だけが同じ（内容が違う）候補は、利用者が選んだときだけ結び付ける（accept_changed）。

import { formatBytes } from "../format";
import type {
  FileIssue,
  FileMovePlan,
  FileMoveReport,
  FileRenamePlan,
  FolderId,
  FolderInfo,
  JobInfo,
  MissingCheckReport,
  RelinkMatch,
  RelinkReport,
  RelinkRequest,
  RelinkTarget,
  RelocatePlan,
  SearchFilter,
  VolumeId,
  VolumeInfo,
} from "../api/types";
import { formatCount, groupReasons, type Notice } from "../dialogs/dialogFormat";

/** 名前に使えない文字（コアの naming.rs の FORBIDDEN_NAME_CHARS と同じ）。 */
const FORBIDDEN = /[<>:"/\\|?*\u0000-\u001f\u007f]/;

/** Windows の予約名（最初の「.」の前の部分で比べる。コアの naming.rs と同じ）。 */
const RESERVED = new Set([
  "con",
  "prn",
  "aux",
  "nul",
  "conin$",
  "conout$",
  ...Array.from({ length: 10 }, (_, i) => `com${i}`),
  ...Array.from({ length: 10 }, (_, i) => `lpt${i}`),
  "com¹",
  "com²",
  "com³",
  "lpt¹",
  "lpt²",
  "lpt³",
]);

/** 名前の長さの上限（UTF-8 のバイト数。コアの MAX_STEM_BYTES）。 */
export const MAX_NAME_BYTES = 200;

/** 名前（ファイル名の拡張子を除く部分・フォルダの名前）の問題（なければ null）。 */
export function validateName(name: string): string | null {
  const v = name.normalize("NFC");
  if (v.trim() === "") return "名前を入力してください";
  if (FORBIDDEN.test(v)) return "使えない文字（< > : \" / \\ | ? * と制御文字）があります";
  if (v.startsWith(".")) return "「.」で始まる名前は使えません（隠しファイルになるため）";
  if (/[. ]$/.test(v) || /^\s|\s$/.test(v)) return "前後の空白と、末尾の「.」は使えません";
  const first = v.split(".")[0].trimEnd().toLowerCase();
  if (RESERVED.has(first)) return `「${v.split(".")[0]}」は Windows の予約名のため使えません`;
  if (new TextEncoder().encode(v).length > MAX_NAME_BYTES) return `名前が長すぎます（${MAX_NAME_BYTES} バイトまで）`;
  return null;
}

/** ファイル名を、拡張子を除いた部分と拡張子に分ける（コアの split_extension と同じ。先頭の「.」は拡張子でない）。 */
export function splitExtension(name: string): { stem: string; ext: string } {
  const i = name.lastIndexOf(".");
  if (i > 0 && i < name.length - 1) return { stem: name.slice(0, i), ext: name.slice(i + 1) };
  return { stem: name, ext: "" };
}

// ---------------------------------------------------------------------------
// ボリューム・フォルダの印
// ---------------------------------------------------------------------------

/** フォルダの行の印（オフライン・見つからない・見つからないファイルの数）と、説明の文。 */
export interface FolderMarks {
  offline: boolean;
  missing: boolean;
  missingFiles: number;
  /** 写真を落とせる（移動先にできる）か。 */
  droppable: boolean;
  /** 行の説明（title）。 */
  title: string;
}

export function folderMarks(f: FolderInfo): FolderMarks {
  const offline = f.online === false;
  const missing = f.missing === true;
  const missingFiles = f.missing_files ?? 0;
  const lines = [f.path ?? f.rel_path];
  if (offline) lines.push("ドライブがつながっていません（つなぐと表示・現像・書き出しができます）");
  if (missing) lines.push("フォルダが見つかりません（右クリックの「場所を指定し直す…」で結び付け直すか、「カタログから除く…」で除けます）");
  if (missingFiles > 0) lines.push(`見つからない写真: ${formatCount(missingFiles)} 件`);
  return { offline, missing, missingFiles, droppable: !offline && !missing, title: lines.join("\n") };
}

/** つながっていないボリュームの数の案内（なければ null）。 */
export function offlineVolumesText(volumes: readonly VolumeInfo[]): string | null {
  const off = volumes.filter((v) => !v.online);
  if (off.length === 0) return null;
  const names = off.slice(0, 3).map(volumeName);
  const more = off.length > 3 ? ` ほか ${formatCount(off.length - 3)} 台` : "";
  return `つながっていないドライブ: ${names.join("、")}${more}`;
}

/** ボリュームの表示名（ラベル、なければマウント先）。 */
export function volumeName(v: VolumeInfo): string {
  return v.label ?? v.mount_path ?? `ボリューム ${v.volume_id}`;
}

const pathCollator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** 移動先に選べるフォルダの選択肢（パスの順）。 */
export interface FolderChoice {
  id: FolderId;
  label: string;
}

/**
 * 移動先に選べるフォルダ（つながっていて見つかるもの）。`exclude` は選べないフォルダ（その配下も。フォルダの
 * 移動で、自分の中へは移せないため）、`volume` は同じボリュームのものだけにするとき（フォルダの移動は同じ
 * ドライブの中だけ）。
 */
export function folderChoices(
  folders: readonly FolderInfo[],
  opts: { exclude?: FolderId | null; volume?: VolumeId | null } = {},
): FolderChoice[] {
  const byId = new Map<FolderId, FolderInfo>();
  for (const f of folders) byId.set(f.folder_id, f);
  const inExcluded = (f: FolderInfo): boolean => {
    if (opts.exclude === undefined || opts.exclude === null) return false;
    // 親をたどる（壊れたデータの輪で止まらないように、たどる数を限る）。
    let cur: FolderInfo | undefined = f;
    for (let i = 0; cur && i <= folders.length; i++) {
      if (cur.folder_id === opts.exclude) return true;
      cur = cur.parent_id === null ? undefined : byId.get(cur.parent_id);
    }
    return false;
  };
  return folders
    .filter((f) => folderMarks(f).droppable && !inExcluded(f))
    .filter((f) => opts.volume === undefined || opts.volume === null || f.volume_id === opts.volume)
    .map((f) => ({ id: f.folder_id, label: f.path ?? f.rel_path }))
    .sort((a, b) => pathCollator.compare(a.label, b.label) || a.id - b.id);
}

/**
 * フォルダと配下のフォルダの ID（最初が自分。親をたどって判定する。カタログから除くときの、表示中の
 * フォルダの付け替えと、配下の数の案内に使う）。
 */
export function folderSubtreeIds(folders: readonly FolderInfo[], id: FolderId): FolderId[] {
  const byId = new Map<FolderId, FolderInfo>();
  for (const f of folders) byId.set(f.folder_id, f);
  const inside = (f: FolderInfo): boolean => {
    // 親をたどる（壊れたデータの輪で止まらないように、たどる数を限る）。
    let cur: FolderInfo | undefined = f;
    for (let i = 0; cur && i <= folders.length; i++) {
      if (cur.folder_id === id) return true;
      cur = cur.parent_id === null ? undefined : byId.get(cur.parent_id);
    }
    return false;
  };
  const rest = folders.filter((f) => f.folder_id !== id && inside(f)).map((f) => f.folder_id);
  return byId.has(id) ? [id, ...rest] : rest;
}

/** フォルダをカタログから除くときの説明の文（配下のフォルダの数と、見つからない写真の印）。 */
export function describeFolderRemoval(folders: readonly FolderInfo[], id: FolderId): string {
  const ids = folderSubtreeIds(folders, id);
  const sub = ids.length > 1 ? `配下のフォルダ ${formatCount(ids.length - 1)} 個と一緒に、` : "";
  const missing = folders.filter((f) => ids.includes(f.folder_id)).reduce((n, f) => n + (f.missing_files ?? 0), 0);
  const warn =
    missing > 0
      ? `見つからない写真が ${formatCount(missing)} 件登録されているため、除けません（先に写真をカタログから除くか、再リンクしてください）。`
      : "写真が登録されていれば除けません（先に写真をカタログから除くか、再リンクしてください）。";
  return `${sub}フォルダをカタログから除きます（ディスクのフォルダとファイルには触れません）。${warn}`;
}

/** 「見つからない写真だけ」の絞り込みか。 */
export function isMissingOnly(f: SearchFilter): boolean {
  return Array.isArray(f.file_statuses) && f.file_statuses.length === 1 && f.file_statuses[0] === "missing";
}

/** 「見つからない写真だけ」の絞り込みを切り替える条件（外すときは条件なし）。 */
export function missingOnlyPatch(on: boolean): Pick<SearchFilter, "file_statuses"> {
  return { file_statuses: on ? ["missing"] : null };
}

// ---------------------------------------------------------------------------
// 移動・名前の変更
// ---------------------------------------------------------------------------

/** 写真の移動の予定のまとめ（ダイアログの文）。 */
export function describeMovePlan(plan: FileMovePlan): { lines: string[]; problems: string[] } {
  const lines: string[] = [];
  const skipped = plan.photos.length - plan.movable;
  lines.push(`${formatCount(plan.movable)} 枚の写真を移します${skipped > 0 ? `（${formatCount(skipped)} 枚は移せません）` : ""}`);
  const files = plan.photos.filter((p) => p.problem === null).reduce((n, p) => n + p.files.length, 0);
  if (files > plan.movable) lines.push(`ファイルは ${formatCount(files)} 個（RAW と同時記録の JPEG は一緒に動きます）`);
  if (plan.copy_bytes > 0) {
    lines.push(
      `別のドライブへ ${formatBytes(plan.copy_bytes)} をコピーします。コピーを元と照合してから移動先を登録し、元のファイルはゴミ箱へ移します（直接は削除しません）`,
    );
  }
  const problems = groupReasons(
    plan.photos.flatMap((p) => (p.problem ? [p.problem] : [])),
    "枚",
  );
  return { lines, problems };
}

/** 写真の移動・名前の変更の結果の知らせ。`verb` は「移動」「名前の変更」。 */
export function describeMoveReport(r: FileMoveReport, verb: "移動" | "名前の変更"): Notice {
  const detail: string[] = [];
  const issues = (title: string, list: FileIssue[]) => {
    if (list.length === 0) return;
    detail.push(`${title}:`);
    for (const i of list.slice(0, 20)) detail.push(`・${i.path}: ${i.reason}`);
    if (list.length > 20) detail.push(`・ほか ${formatCount(list.length - 20)} 件`);
  };
  issues("飛ばした写真", r.skipped);
  issues("失敗した写真（元の状態に戻しました）", r.failed);
  issues("ゴミ箱へ移せず、元の場所に残ったファイル", r.left_sources);
  let level: Notice["level"] = "success";
  if (r.failed.length > 0 || r.left_sources.length > 0) level = "warning";
  else if (r.photos === 0) level = r.skipped.length > 0 ? "warning" : "info";
  let message = `${formatCount(r.photos)} 枚の写真の${verb}をしました`;
  if (r.bytes_copied > 0) message += `（${formatBytes(r.bytes_copied)} をコピー）`;
  if (r.cancelled) message += "（途中で取り消しました）";
  return { level, message, detail: detail.length > 0 ? detail.join("\n") : undefined };
}

/** 写真の移動のジョブの結果の知らせ（失敗はジョブの失敗として state/app.ts が知らせるので null）。 */
export function describeMoveJob(info: JobInfo): Notice | null {
  if (info.state === "failed") return null;
  const r = info.result;
  if (r && r.type === "move_files") return describeMoveReport(r, "移動");
  if (info.state === "cancelled") return { level: "info", message: "写真の移動を取り消しました" };
  return { level: "success", message: "写真の移動が終わりました" };
}

/** 名前の変更の予定の文（変えられる枚数と、変えられない理由）。 */
export function describeRenamePlan(plan: FileRenamePlan): { lines: string[]; problems: string[] } {
  const skipped = plan.photos.length - plan.renamable;
  const lines = [
    `${formatCount(plan.renamable)} 枚の写真の名前を変えます${skipped > 0 ? `（${formatCount(skipped)} 枚は変えられません）` : ""}`,
  ];
  const files = plan.photos.filter((p) => p.problem === null).reduce((n, p) => n + p.files.length, 0);
  if (files > plan.renamable) lines.push(`ファイルは ${formatCount(files)} 個（RAW と同時記録の JPEG は同じ名前にそろえます）`);
  const problems = groupReasons(
    plan.photos.flatMap((p) => (p.problem ? [p.problem] : [])),
    "枚",
  );
  return { lines, problems };
}

// ---------------------------------------------------------------------------
// 見つからない写真・再リンク・場所の指定し直し
// ---------------------------------------------------------------------------

/** 見つからない写真の確認の結果の知らせ（何も変わらず、見つからない写真もなければ null）。 */
export function describeCheckReport(r: MissingCheckReport, explicit: boolean): Notice | null {
  const parts: string[] = [];
  if (r.newly_missing > 0) parts.push(`新しく見つからなくなった写真 ${formatCount(r.newly_missing)} 件`);
  if (r.restored > 0) parts.push(`元の場所に戻っていた写真 ${formatCount(r.restored)} 件`);
  const detail: string[] = [];
  if (r.missing > 0) detail.push(`見つからない写真: ${formatCount(r.missing)} 件（絞り込みの「見つからない写真だけ」で表示できます）`);
  if (r.missing_folders > 0) detail.push(`見つからないフォルダ: ${formatCount(r.missing_folders)} 個（フォルダの「!」の印）`);
  if (r.skipped_offline > 0) detail.push(`ドライブがつながっていないため確かめなかった写真: ${formatCount(r.skipped_offline)} 件`);
  if (r.skipped_network > 0) detail.push(`ネットワークのドライブのため確かめなかった写真: ${formatCount(r.skipped_network)} 件`);
  if (!explicit && parts.length === 0) return null;
  const message =
    parts.length > 0
      ? `見つからない写真を確かめました: ${parts.join("、")}`
      : `見つからない写真を確かめました（${formatCount(r.checked)} 件。変わりはありません）`;
  return {
    level: r.newly_missing > 0 ? "warning" : "info",
    message: r.cancelled ? `${message}（途中で取り消しました）` : message,
    detail: detail.length > 0 ? detail.join("\n") : undefined,
  };
}

/** 見つからない写真の確認のジョブの結果の知らせ（`explicit` は利用者が始めたもの。describeCheckReport）。 */
export function describeCheckJob(info: JobInfo, explicit: boolean): Notice | null {
  if (info.state === "failed") return null;
  const r = info.result;
  if (r && r.type === "check_missing") return describeCheckReport(r, explicit);
  if (!explicit) return null;
  if (info.state === "cancelled") return { level: "info", message: "見つからない写真の確認を取り消しました" };
  return { level: "info", message: "見つからない写真を確かめました" };
}

/** 一致の仕方の説明。 */
export const RELINK_MATCH_LABEL: Record<RelinkMatch, string> = {
  content: "内容が同じ（ファイル全体のハッシュ）",
  quick: "大きさと先頭・末尾が同じ",
  name_and_time: "名前と撮影日時が同じ（内容は変わっています）",
};

/** 確からしい候補か（結び付けても内容が登録と同じ）。 */
export function isSafeMatch(m: RelinkMatch): boolean {
  return m === "content" || m === "quick";
}

/**
 * 既定で選ぶ候補（パス）。確からしい候補が 1 つだけなら、その候補。確からしい候補が複数あるか、名前と撮影日時
 * だけが同じ候補しかなければ選ばない（利用者が選ぶ）。
 */
export function defaultRelinkChoice(t: RelinkTarget): string | null {
  const safe = t.candidates.filter((c) => isSafeMatch(c.matched));
  if (safe.length === 1) return safe[0].path;
  // 内容が同じものが 1 つだけなら、それを選ぶ（大きさと先頭・末尾だけが同じものが他にあっても）。
  const content = t.candidates.filter((c) => c.matched === "content");
  return content.length === 1 ? content[0].path : null;
}

/** 選んだ候補から、再リンクの要求を作る（選んでいないものは除く。内容の違う候補は accept_changed を付ける）。 */
export function relinkRequests(targets: readonly RelinkTarget[], choices: ReadonlyMap<number, string | null>): RelinkRequest[] {
  const out: RelinkRequest[] = [];
  for (const t of targets) {
    const path = choices.get(t.file_id) ?? null;
    if (!path) continue;
    const c = t.candidates.find((x) => x.path === path);
    if (!c) continue;
    out.push(c.matched === "name_and_time" ? { file_id: t.file_id, path, accept_changed: true } : { file_id: t.file_id, path });
  }
  return out;
}

/** 再リンクの結果の知らせ。 */
export function describeRelinkReport(r: RelinkReport): Notice {
  const detail: string[] = [];
  if (r.reread.length > 0) detail.push(`内容が変わっていたので読み直したファイル: ${formatCount(r.reread.length)} 個`);
  for (const f of r.failed.slice(0, 20)) detail.push(`・${f.path}: ${f.reason}`);
  if (r.failed.length > 20) detail.push(`・ほか ${formatCount(r.failed.length - 20)} 件`);
  return {
    level: r.failed.length > 0 ? "warning" : "success",
    message: `${formatCount(r.linked.length)} 個のファイルを結び付け直しました${r.failed.length > 0 ? `（${formatCount(r.failed.length)} 個はできませんでした）` : ""}`,
    detail: detail.length > 0 ? detail.join("\n") : undefined,
  };
}

/** 場所の指定し直しの予定の文。 */
export function describeRelocatePlan(p: RelocatePlan): string {
  const sub = p.subfolders > 0 ? `（配下のフォルダ ${formatCount(p.subfolders)} 個を含む）` : "";
  return `登録した写真のファイル ${formatCount(p.files)} 個のうち、${formatCount(p.found)} 個が新しい場所に同じ名前・大きさで見つかりました${sub}`;
}
