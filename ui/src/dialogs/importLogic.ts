// 取り込み（IMP-01・VID-01）のダイアログとドラッグ＆ドロップの判定、重複と思われるもの（IMP-04）・RAW と
// JPEG の扱い（IMP-06）の設定、コピー取り込み（IMP-02）・リネーム規則（IMP-03）・まとめて付けるキーワードと
// 説明のメタデータ（IMP-05）の入力と、結果の知らせ（DOM を使わない純粋な計算）。

import type {
  CopySettings,
  CoreSettings,
  DuplicateFile,
  ExternalTools,
  FolderRule,
  ImportMetadata,
  ImportOptions,
  ImportPreferences,
  ImportReport,
  MetadataField,
  RenameRule,
  SettingsUpdate,
} from "../api/types";
import { parseKeywordInput, validateKeywordName } from "../library/keywordLogic";
import { MAX_METADATA_CHARS, METADATA_FIELDS } from "../library/metadataEdit";
import { formatBytes } from "../format";
import { type Notice, baseName, formatCount } from "./dialogFormat";

/** パスの種類（アプリの path_kinds。api/app.ts の pathKinds）。 */
export type PathKind = "dir" | "file" | "missing";

/**
 * フォルダではないと分かる拡張子（小文字）。ドロップしたものがファイルなら取り込まない（取り込みはフォルダ
 * 単位）。genzo-api の import.rs の RAW_EXTENSIONS・IMAGE_EXTENSIONS・VIDEO_EXTENSIONS に、よく一緒に
 * 扱うファイル（カタログ・サイドカー・ほかの RAW・画像・動画・文書など）を加えたもの。
 *
 * ドロップしたものの種類は、アプリの path_kinds（api/app.ts の pathKinds）で確かめる（Dialogs.tsx）。これは
 * それを確かめられなかったときの、名前からの推定（splitDroppedPaths）にだけ使う。ここにない拡張子の名前
 * （"2024.05.01"・"2024.Kyoto" のようなフォルダもある）はフォルダとみなす。実際はファイルなら、取り込みの
 * ジョブが「フォルダではありません」で失敗する。
 */
const FILE_EXTENSIONS = new Set([
  // GenzoParis が取り込むもの
  "arw",
  "dng",
  "jpg",
  "jpeg",
  "tif",
  "tiff",
  "png",
  "mp4",
  "mov",
  // ほかの RAW
  "cr2",
  "cr3",
  "crw",
  "nef",
  "nrw",
  "raf",
  "orf",
  "rw2",
  "rwl",
  "pef",
  "srw",
  "sr2",
  "srf",
  "3fr",
  "iiq",
  "x3f",
  "erf",
  "mrw",
  "dcr",
  "kdc",
  "mos",
  // ほかの画像
  "heic",
  "heif",
  "hif",
  "gif",
  "webp",
  "avif",
  "jxl",
  "bmp",
  "psd",
  "psb",
  "svg",
  // ほかの動画・音声
  "mts",
  "m2ts",
  "mkv",
  "avi",
  "wmv",
  "m4v",
  "mpg",
  "mpeg",
  "3gp",
  "lrv",
  "thm",
  "mp3",
  "wav",
  "aac",
  "m4a",
  // カタログ・サイドカー・文書・圧縮ファイル
  "xmp",
  "db",
  "lrcat",
  "dop",
  "pp3",
  "json",
  "xml",
  "csv",
  "txt",
  "pdf",
  "doc",
  "docx",
  "xls",
  "xlsx",
  "ppt",
  "pptx",
  "zip",
  "7z",
  "rar",
  "gz",
  "tar",
  "dmg",
  "exe",
  "lnk",
]);

/** 名前の拡張子（小文字。なければ ""）。 */
function extensionOf(path: string): string {
  const name = baseName(path);
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(i + 1).toLowerCase() : "";
}

/** ドロップしたパスを、フォルダとみなすものとファイルに分ける。 */
export function splitDroppedPaths(paths: readonly string[]): { folders: string[]; files: string[] } {
  const folders: string[] = [];
  const files: string[] = [];
  for (const p of paths) {
    const ext = extensionOf(p);
    if (ext && FILE_EXTENSIONS.has(ext)) files.push(p);
    else folders.push(p);
  }
  return { folders, files };
}

/**
 * 取り込みのダイアログで注意を出すもの: アプリの path_kinds で確かめて、フォルダではなかったもの（`files`）と
 * 見つからなかったもの（`missing`）。`kinds` にないもの（「参照…」で OS のフォルダの選択から選んだもの・
 * 確かめている途中のもの・確かめられなかったもの）には出さない（名前からは推定しない。"2024.Kyoto" のような
 * フォルダもあるため）。
 */
export function nonFolderPaths(
  folders: readonly string[],
  kinds: ReadonlyMap<string, PathKind>,
): { files: string[]; missing: string[] } {
  return {
    files: folders.filter((p) => kinds.get(p) === "file"),
    missing: folders.filter((p) => kinds.get(p) === "missing"),
  };
}

/** ffmpeg・ffprobe が見つからないときの注意（見つかっていれば null）。決定事項ログ No.62。 */
export function videoToolsWarning(tools: ExternalTools | null | undefined): string | null {
  if (!tools) return null;
  const missing: string[] = [];
  if (!tools.ffprobe) missing.push("ffprobe（動画の長さ・fps・コーデック・作成日時などの情報）");
  if (!tools.ffmpeg) missing.push("ffmpeg（動画のサムネイル）");
  if (missing.length === 0) return null;
  return `次のものが見つかりません: ${missing.join("、")}。動画は登録できますが、これらは取れません。`;
}

// ---------------------------------------------------------------------------
// 重複と思われるもの（IMP-04）・RAW と JPEG の扱い（IMP-06）
// ---------------------------------------------------------------------------

/** 取り込みのダイアログの選択（カタログの設定が既定。変えたら既定として覚える）。 */
export interface ImportChoice {
  /** 重複と思われるもの（内容が同じファイルが登録済みのもの）を取り込まない。 */
  skipDuplicates: boolean;
  /** RAW と同時記録の JPEG を別々の写真として取り込む。 */
  separateRawJpeg: boolean;
}

/** カタログの設定から、ダイアログの選択の初めの値を作る（古いコアで値がなければ genzo-api の既定）。 */
export function importChoiceFrom(settings: Partial<CoreSettings> | null | undefined): ImportChoice {
  return {
    skipDuplicates: settings?.import_skip_duplicates ?? true,
    separateRawJpeg: settings?.import_separate_raw_jpeg ?? false,
  };
}

/**
 * 選んだ設定を、取り込みの指定（コアの ImportOptions。省かずに渡す）と、カタログの設定の更新（既定と違えば。
 * 次に開いたときの既定にする）にする。
 */
export function importChoiceRequest(
  settings: Partial<CoreSettings> | null | undefined,
  choice: ImportChoice,
): { options: ImportOptions; update: SettingsUpdate | null } {
  const current = importChoiceFrom(settings);
  const update: SettingsUpdate = {};
  if (choice.skipDuplicates !== current.skipDuplicates) update.import_skip_duplicates = choice.skipDuplicates;
  if (choice.separateRawJpeg !== current.separateRawJpeg) update.import_separate_raw_jpeg = choice.separateRawJpeg;
  return {
    options: { skip_duplicates: choice.skipDuplicates, separate_raw_jpeg: choice.separateRawJpeg },
    update: Object.keys(update).length > 0 ? update : null,
  };
}

/**
 * 重複のファイルの行（「/card/B.jpg（同じ内容: /photos/A.jpg）」の形。どちらも場所のすべて）。すべてのファイルを
 * 挙げる（知らせは短くして出し、全体は結果の記録に残す。resultLog.ts）。
 */
export function duplicateLines(list: readonly DuplicateFile[]): string[] {
  return list.map((d) => `・${d.path}（同じ内容: ${d.existing_path ?? "場所不明"}）`);
}

/** 知らせの詳細に挙げるコピーしたファイルの例の数（仮置き: 5。全体は結果の記録に残す）。 */
export const COPY_EXAMPLES = 5;

/**
 * 取り込みの結果の知らせ（件数・コピー・重複のため取り込まなかったもの・設定により登録した重複・RAW と組にした
 * JPEG・まとめて付けたもの・登録できなかったもの）。登録できなかったファイルがあれば注意（warning）にする。
 * 詳細は結果の記録（resultLog.ts）にも残すので、コピーしたファイル・重複のファイル・登録できなかったファイルは
 * すべて挙げる。
 */
export function describeImportReport(r: ImportReport): Notice {
  const duplicates = r.duplicates ?? [];
  const skipped = duplicates.filter((d) => d.skipped);
  const registered = duplicates.filter((d) => !d.skipped);
  const problems = r.errors.length + r.not_registered.length;
  const copy = r.copy ?? null;
  let message: string;
  if (copy?.dry_run) {
    message = `コピーの確認: ${formatCount(copy.copied.length)} 件のコピー先を決めました（コピーしていません）`;
  } else if (copy) {
    message = `コピー取り込み: ${formatCount(copy.copied.length)} 件をコピーし、${formatCount(r.added)} 件を追加`;
  } else {
    message = `取り込み: ${formatCount(r.added)} 件を追加、${formatCount(r.updated)} 件を更新`;
    if (r.unchanged) message += `、${formatCount(r.unchanged)} 件は変更なし`;
  }
  if (skipped.length > 0) message += `、重複の ${formatCount(skipped.length)} 件は取り込まず`;
  if (r.cancelled) message += "（途中で取り消しました）";
  const detail: string[] = [];
  if (problems > 0) detail.push(`${formatCount(problems)} 件のファイルを登録できませんでした`);
  if (copy) {
    detail.push(`コピー先: ${copy.destination}`);
    if (copy.bytes_copied > 0) detail.push(`コピーした大きさ: ${formatBytes(copy.bytes_copied)}`);
    const existing = copy.copied.filter((c) => c.existing).length;
    if (existing > 0) {
      detail.push(
        `コピー先に同じ内容のファイルが既にあったため、コピーせずに登録したファイル: ${formatCount(existing)} 件`,
      );
    }
    if (copy.dated_by_file_time > 0) {
      detail.push(
        `撮影日時が分からず、ファイルの更新日時で日付を決めたファイル: ${formatCount(copy.dated_by_file_time)} 件`,
      );
    }
    if (copy.copied.length > 0) {
      detail.push(copy.dry_run ? "コピー先（予定）:" : "コピーしたファイル（元 → コピー先）:");
      for (const c of copy.copied) detail.push(`・${baseName(c.source)} → ${c.destination}`);
    }
  }
  for (const f of r.not_registered) detail.push(`・登録できなかった: ${f.path}（${f.reason}）`);
  if (skipped.length > 0) {
    detail.push(`内容が同じファイルが登録済みのため、取り込まなかったファイル（${formatCount(skipped.length)} 件）:`);
    detail.push(...duplicateLines(skipped));
  }
  if (registered.length > 0) {
    detail.push(
      `内容が同じファイルが登録済みですが、設定により別の写真として登録したファイル（${formatCount(registered.length)} 件）:`,
    );
    detail.push(...duplicateLines(registered));
  }
  if (r.paired) detail.push(`RAW と同時記録の JPEG ${formatCount(r.paired)} 件を、RAW と 1 枚の写真にまとめました`);
  if (r.tagged) detail.push(`${formatCount(r.tagged)} 枚の写真に、取り込み時のキーワード・メタデータを付けました`);
  return {
    level: problems > 0 ? "warning" : "success",
    message,
    detail: detail.length > 0 ? detail.join("\n") : undefined,
  };
}

// ---------------------------------------------------------------------------
// コピー取り込み（IMP-02）・リネーム規則（IMP-03）・まとめて付けるもの（IMP-05）
// ---------------------------------------------------------------------------

/** フォルダの規則の選択肢（genzo-api の FolderRule::ALL と同じ順。例は 2026-10-13 に撮った写真）。 */
export const FOLDER_RULE_OPTIONS: { value: FolderRule; label: string }[] = [
  { value: "year_date", label: "撮影日別（2026/2026-10-13）" },
  { value: "year_month_day", label: "撮影日別（2026/10/13）" },
  { value: "year_month", label: "撮影月別（2026/2026-10）" },
  { value: "date", label: "撮影日別（2026-10-13）" },
  { value: "date_compact", label: "撮影日別（20261013）" },
  { value: "single", label: "1 つのフォルダ（コピー先にそのまま）" },
  { value: "source", label: "元のフォルダの構成のまま" },
];

/** テンプレートの項目（genzo-api の TemplateToken。ボタンで入れる）。 */
export const TEMPLATE_TOKENS: { token: string; label: string }[] = [
  { token: "{date}", label: "撮影日" },
  { token: "{time}", label: "撮影時刻" },
  { token: "{year}", label: "年" },
  { token: "{month}", label: "月" },
  { token: "{day}", label: "日" },
  { token: "{hour}", label: "時" },
  { token: "{minute}", label: "分" },
  { token: "{second}", label: "秒" },
  { token: "{seq}", label: "連番" },
  { token: "{name}", label: "元の名前" },
  { token: "{camera}", label: "カメラ" },
  { token: "{text}", label: "任意の文字" },
];

/** テンプレートのひな形（選ぶと入力欄に入れる）。 */
export const TEMPLATE_PRESETS: { label: string; template: string }[] = [
  { label: "撮影日-連番", template: "{date}-{seq}" },
  { label: "撮影日_撮影時刻", template: "{date}_{time}" },
  { label: "任意の文字-連番", template: "{text}-{seq}" },
  { label: "撮影日_元の名前", template: "{date}_{name}" },
  { label: "カメラ_元の名前", template: "{camera}_{name}" },
];

/** ファイル名に使えない文字（genzo-api の FORBIDDEN_NAME_CHARS と同じ）。 */
export const FORBIDDEN_NAME_CHARS = ["<", ">", ":", '"', "/", "\\", "|", "?", "*"];

/** テンプレートの文字数の上限（genzo-api の MAX_TEMPLATE_CHARS と同じ）。 */
export const MAX_TEMPLATE_CHARS = 200;
/** 任意の文字の文字数の上限（genzo-api の MAX_CUSTOM_TEXT_CHARS と同じ）。 */
export const MAX_CUSTOM_TEXT_CHARS = 100;
/** 連番の開始番号の上限（genzo-api の MAX_SEQUENCE_START と同じ）。 */
export const MAX_SEQUENCE_START = 999_999_999;

/** アプリの既定の取り込みの設定（genzo-app の ImportPreferences の既定と同じ）。 */
export const DEFAULT_IMPORT_PREFERENCES: ImportPreferences = {
  copy: false,
  recursive: true,
  destination: null,
  folder_rule: "year_date",
  rename: false,
  rename_rule: { template: "{date}-{seq}", custom_text: "", sequence_start: 1, sequence_digits: 4 },
  metadata: { keywords: [], fields: {} },
};

/** 取り込みのダイアログの入力（文字列のまま持ち、取り込むときに確かめる）。 */
export interface ImportForm {
  /** コピーして取り込む（false はその場で登録）。 */
  copy: boolean;
  recursive: boolean;
  destination: string;
  folderRule: FolderRule;
  rename: boolean;
  template: string;
  customText: string;
  sequenceStart: string;
  sequenceDigits: number;
  /** キーワードの入力欄（「場所 > 京都, 家族」）。 */
  keywords: string;
  /** 説明のメタデータ（空は付けない）。 */
  fields: Partial<Record<MetadataField, string>>;
}

/** アプリに保存した設定から、ダイアログの入力の初めの値を作る（古いアプリで無ければ既定）。 */
export function importFormFrom(prefs: ImportPreferences | null | undefined): ImportForm {
  const p = prefs ?? DEFAULT_IMPORT_PREFERENCES;
  const rule = p.rename_rule ?? DEFAULT_IMPORT_PREFERENCES.rename_rule;
  return {
    copy: !!p.copy,
    recursive: p.recursive ?? true,
    destination: p.destination ?? "",
    folderRule: p.folder_rule ?? "year_date",
    rename: !!p.rename,
    template: rule.template,
    customText: rule.custom_text,
    sequenceStart: String(rule.sequence_start),
    sequenceDigits: rule.sequence_digits,
    keywords: (p.metadata?.keywords ?? []).map((k) => k.join(" > ")).join(", "),
    fields: { ...(p.metadata?.fields ?? {}) },
  };
}

/** 入力のリネームの規則（開始番号が数でなければ 1）。 */
export function renameRuleOf(form: ImportForm): RenameRule {
  const start = Number.parseInt(form.sequenceStart.trim(), 10);
  return {
    template: form.template,
    custom_text: form.customText,
    sequence_start: Number.isFinite(start) && start >= 0 ? start : 1,
    sequence_digits: form.sequenceDigits,
  };
}

/** 入力のまとめて付けるもの（空の値・空のキーワードは除く）。 */
export function importMetadataOf(form: ImportForm): ImportMetadata {
  const fields: Partial<Record<MetadataField, string>> = {};
  for (const d of METADATA_FIELDS) {
    const v = form.fields[d.field]?.trim();
    if (v) fields[d.field] = v;
  }
  return { keywords: parseKeywordInput(form.keywords), fields };
}

/** 入力のコピーの設定（コピーしないときは null）。 */
export function copySettingsOf(form: ImportForm): CopySettings | null {
  if (!form.copy) return null;
  return {
    destination: form.destination.trim(),
    folder_rule: form.folderRule,
    rename: form.rename ? renameRuleOf(form) : null,
  };
}

/** 保存する設定（取り込みを始めたとき。次に開いたときの初めの値）。 */
export function importPreferencesOf(form: ImportForm): ImportPreferences {
  const dest = form.destination.trim();
  return {
    copy: form.copy,
    recursive: form.recursive,
    destination: dest === "" ? null : dest,
    folder_rule: form.folderRule,
    rename: form.rename,
    rename_rule: renameRuleOf(form),
    metadata: importMetadataOf(form),
  };
}

/** テンプレート・任意の文字に使えない文字の説明（なければ null）。 */
function forbiddenCharMessage(text: string): string | null {
  const c = FORBIDDEN_NAME_CHARS.find((ch) => text.includes(ch));
  if (c !== undefined) return `「${c}」はファイル名に使えません`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(text)) return "改行・タブなどの制御文字はファイル名に使えません";
  return null;
}

/**
 * 入力の誤り（取り込みのボタンを押せない理由。なければ空）。テンプレートの項目の名前・波かっこの対応はコアの
 * 名前の例（core.importNamePreview）が確かめる。
 */
export function validateImportForm(form: ImportForm): string[] {
  const errors: string[] = [];
  if (form.copy) {
    if (form.destination.trim() === "") errors.push("コピー先のフォルダを選んでください");
    if (form.rename) {
      if (form.template.trim() === "") errors.push("ファイル名のテンプレートを入れてください");
      else if ([...form.template].length > MAX_TEMPLATE_CHARS)
        errors.push(`テンプレートは ${MAX_TEMPLATE_CHARS} 文字までです`);
      // 波かっこの中（項目の名前）は除いて確かめる。
      const literal = form.template.replace(/\{[^{}]*\}/g, "");
      const m = forbiddenCharMessage(literal);
      if (m) errors.push(`テンプレート: ${m}`);
      const t = forbiddenCharMessage(form.customText);
      if (t) errors.push(`任意の文字: ${t}`);
      if ([...form.customText].length > MAX_CUSTOM_TEXT_CHARS)
        errors.push(`任意の文字は ${MAX_CUSTOM_TEXT_CHARS} 文字までです`);
      const start = form.sequenceStart.trim();
      const n = Number(start);
      if (!/^\d+$/.test(start) || n > MAX_SEQUENCE_START)
        errors.push(`連番の開始番号は 0〜${MAX_SEQUENCE_START.toLocaleString("ja-JP")} の整数です`);
      if (!(form.sequenceDigits >= 1 && form.sequenceDigits <= 9)) errors.push("連番の桁数は 1〜9 です");
    }
  }
  for (const path of parseKeywordInput(form.keywords)) {
    for (const name of path) {
      const e = validateKeywordName(name);
      if (e) {
        errors.push(`キーワード「${name}」: ${e}`);
        break;
      }
    }
  }
  for (const d of METADATA_FIELDS) {
    const v = form.fields[d.field];
    if (v && [...v.trim()].length > MAX_METADATA_CHARS) errors.push(`${d.label}は ${MAX_METADATA_CHARS} 文字までです`);
  }
  return errors;
}

/**
 * 取り込みの指定（コアの ImportOptions）: 重複・RAW と JPEG の選択（importChoiceRequest）に、コピーの設定と
 * まとめて付けるものを足す。
 */
export function importOptionsOf(form: ImportForm, choice: ImportOptions): ImportOptions {
  const metadata = importMetadataOf(form);
  const options: ImportOptions = { ...choice };
  const copy = copySettingsOf(form);
  if (copy) options.copy = copy;
  if (metadata.keywords.length > 0 || Object.keys(metadata.fields).length > 0) options.metadata = metadata;
  return options;
}

/** まとめて付けるものの数（節の見出しに出す。キーワードと値のある項目）。 */
export function metadataCount(form: ImportForm): number {
  const m = importMetadataOf(form);
  return m.keywords.length + Object.keys(m.fields).length;
}

/** テンプレートの入力欄の選択範囲に項目を入れる（新しい文字列と、その後のカーソルの位置）。 */
export function insertToken(
  template: string,
  token: string,
  start: number | null | undefined,
  end: number | null | undefined,
): { text: string; cursor: number } {
  const a = Math.max(0, Math.min(start ?? template.length, template.length));
  const b = Math.max(a, Math.min(end ?? a, template.length));
  const text = template.slice(0, a) + token + template.slice(b);
  return { text, cursor: a + token.length };
}
