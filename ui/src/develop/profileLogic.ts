// カメラプロファイル・ルック（DEV-17）の画面の計算（DOM を使わない純粋な計算）。
//
// - カメラプロファイル（DCP）は RAW だけで、処理バージョン 2 から使われる。ルック（3D LUT の .cube）は RAW 以外の
//   写真にも使える（処理バージョン 2 から）。
// - 選べるカメラプロファイルは「標準」（アプリのカメラ行列）と、取り込んだもののうちこの写真の機種用のもの。
//   「ほかの機種用も表示」で、ほかの機種用・機種の分からないものも出す（使えるが、色が合わないことがある）。
// - 設定が指している ID がライブラリにない（除いた・別のパソコンで付けた）ときも、選択肢に「見つかりません」として
//   残す（勝手に標準に戻さない。描画は標準の色で、コアが警告を出す）。

import type {
  DevelopSettings,
  ProfileEntry,
  ProfileImportReport,
  ProfileSettings,
  SourceInfo,
} from "../api/types";

/** カメラプロファイル・ルックを使える最初の処理バージョン（genzo-pipeline の ProcessVersion::V2）。 */
export const PROFILE_PROCESS_VERSION = 2;

/** 既定（標準の色・ルックなし・適用量 100）。 */
export function defaultProfile(): ProfileSettings {
  return { camera: null, look: null, look_amount: 100 };
}

/** 欠けた項目を既定で補ったもの（古いコアの応答）。 */
export function fullProfile(p: Partial<ProfileSettings> | null | undefined): ProfileSettings {
  return { ...defaultProfile(), ...(p ?? {}) };
}

/** 既定のままか。 */
export function profileIsDefault(p: Partial<ProfileSettings> | null | undefined): boolean {
  const f = fullProfile(p);
  return f.camera === null && f.look === null && f.look_amount === 100;
}

/** `profile` の一部を変えた設定。 */
export function withProfile(s: DevelopSettings, patch: Partial<ProfileSettings>): DevelopSettings {
  return { ...s, profile: { ...fullProfile(s.profile), ...patch } };
}

/** パネルの操作を使えるか。 */
export interface ProfileControls {
  /** 処理バージョンが古く、使われない（更新の案内を出す）。 */
  needsUpgrade: boolean;
  /** RAW 以外の写真（カメラプロファイルは使われない）。 */
  notRaw: boolean;
  /** カメラプロファイルを選べる。 */
  camera: boolean;
  /** ルックを選べる。 */
  look: boolean;
  /** ルックの適用量を変えられる（ルックを選んでいる）。 */
  lookAmount: boolean;
}

export function profileControls(
  settings: DevelopSettings,
  source: SourceInfo | null,
  disabled: boolean,
): ProfileControls {
  const needsUpgrade = settings.process_version < PROFILE_PROCESS_VERSION;
  const on = !disabled && !needsUpgrade;
  const notRaw = source !== null && !source.is_raw;
  const p = fullProfile(settings.profile);
  return {
    needsUpgrade,
    notRaw,
    camera: on && !notRaw,
    look: on,
    lookAmount: on && p.look !== null,
  };
}

/** 選択肢の 1 つ。 */
export interface ProfileChoice {
  /** 値（null は「標準」・「なし」）。 */
  id: string | null;
  label: string;
  /** 選べない（読めない写し）。 */
  disabled?: boolean;
}

function entryLabel(e: ProfileEntry): string {
  const base = e.name === e.id ? e.name : `${e.name}（${e.id}）`;
  return e.usable ? base : `${base}（読めません）`;
}

/**
 * カメラプロファイルの選択肢: 「標準」・この機種用（matches_camera が true）・（showAll なら）ほかの機種用。設定が
 * 指している ID が一覧になければ「見つかりません」として加える。
 */
export function cameraChoices(
  entries: readonly ProfileEntry[],
  current: string | null,
  showAll: boolean,
): ProfileChoice[] {
  const cams = entries.filter((e) => e.kind === "camera");
  const matching = cams.filter((e) => e.matches_camera === true);
  const others = cams.filter((e) => e.matches_camera !== true);
  const shown = showAll ? [...matching, ...others] : matching;
  const out: ProfileChoice[] = [{ id: null, label: "標準（アプリのカメラ行列）" }];
  for (const e of shown) {
    const other = e.matches_camera !== true;
    const model = other && e.camera_model ? ` — ${e.camera_model} 用` : "";
    out.push({ id: e.id, label: `${entryLabel(e)}${model}`, disabled: !e.usable });
  }
  if (current !== null && !out.some((c) => c.id === current)) {
    const known = cams.find((e) => e.id === current);
    out.push(
      known
        ? { id: current, label: `${entryLabel(known)}${known.camera_model ? ` — ${known.camera_model} 用` : ""}` }
        : { id: current, label: `${current}（見つかりません）` },
    );
  }
  return out;
}

/** ルックの選択肢: 「なし」・取り込んだルック。設定が指している ID が一覧になければ「見つかりません」として加える。 */
export function lookChoices(entries: readonly ProfileEntry[], current: string | null): ProfileChoice[] {
  const out: ProfileChoice[] = [{ id: null, label: "なし" }];
  for (const e of entries.filter((x) => x.kind === "look")) {
    out.push({ id: e.id, label: entryLabel(e), disabled: !e.usable });
  }
  if (current !== null && !out.some((c) => c.id === current)) {
    out.push({ id: current, label: `${current}（見つかりません）` });
  }
  return out;
}

/** ほかの機種用のカメラプロファイルを選んでいるか（「ほかの機種用も表示」を最初から開くため）。 */
export function selectsOtherCamera(entries: readonly ProfileEntry[], current: string | null): boolean {
  if (current === null) return false;
  const e = entries.find((x) => x.kind === "camera" && x.id === current);
  return e !== undefined && e.matches_camera !== true;
}

/** 取り込みの結果の知らせ（level と本文・詳細）。 */
export function describeProfileImport(r: ProfileImportReport): {
  level: "success" | "warning" | "info";
  message: string;
  detail?: string;
} {
  const parts: string[] = [];
  if (r.imported.length > 0) parts.push(`${r.imported.length} 個を取り込みました`);
  if (r.replaced.length > 0) parts.push(`${r.replaced.length} 個を置き換えました`);
  if (r.repaired.length > 0) parts.push(`${r.repaired.length} 個は消えた・壊れた写しを書き直しました`);
  if (r.unchanged.length > 0) parts.push(`${r.unchanged.length} 個は同じ内容のものがありました`);
  if (r.failed.length > 0) parts.push(`${r.failed.length} 個は取り込めませんでした`);
  const message = parts.length > 0 ? parts.join("・") : "取り込むものがありませんでした";
  const details: string[] = r.failed.map((f) => `${f.path}: ${f.message}`);
  if (r.replaced.length > 0) {
    details.push("置き換えたものを使っている写真は、次に開いたときに「内容が違う」と知らせます（色が変わる可能性があります）。");
  }
  return {
    level:
      r.failed.length > 0
        ? "warning"
        : r.imported.length + r.replaced.length + r.repaired.length > 0
          ? "success"
          : "info",
    message,
    detail: details.length > 0 ? details.join("\n") : undefined,
  };
}

/** 同じ ID のものがあって取り込めなかった（置き換えれば取り込める）ファイルがあるか。 */
export function hasReplaceableFailures(r: ProfileImportReport): boolean {
  return r.failed.some((f) => f.message.includes("すでにあります"));
}
