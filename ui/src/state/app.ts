// アプリ全体の状態（表示モード・検索の条件・選択・ダイアログ・通知・ジョブ）と操作。
//
// 画面の各部（library・viewer・develop・dialogs）は、ここの `appStore` を購読し、`actions` で変える。

import * as app from "../api/app";
import * as core from "../api/core";
import { describeError, isStale, toApiError } from "../api/errors";
import { onCoreEvent } from "../api/events";
import type {
  AppStatus,
  CatalogStatus,
  CloseFailure,
  ColorLabel,
  CoreEvent,
  CoreSettings,
  DeleteKind,
  DevelopSettings,
  ErrorInfo,
  Flag,
  JobInfo,
  JobKind,
  JobState,
  Rating,
  SearchFilter,
  SearchSort,
  VariantId,
} from "../api/types";
import { searchCache } from "./search";
import { createStore, useStore } from "./store";

/** 表示モード（G: グリッド、E: ルーペ、D: 現像）。 */
export type Mode = "grid" | "loupe" | "develop";

/** ダイアログ（同時に 1 つ）。 */
export type DialogState =
  | { kind: "import" }
  | { kind: "export"; variantIds: VariantId[] }
  | { kind: "delete"; deleteKind: DeleteKind; variantIds: VariantId[] }
  | { kind: "paste_settings"; targets: VariantId[] }
  | { kind: "settings"; tab?: "general" | "catalog" | "backup" }
  | { kind: "shortcuts" }
  | { kind: "close_failed"; failure: CloseFailure; quitting: boolean };

export interface Toast {
  id: number;
  level: "info" | "success" | "warning" | "error";
  message: string;
  detail?: string;
}

export interface JobView {
  jobId: number;
  kind: JobKind;
  state: JobState;
  done: number;
  total: number;
  message: string | null;
  /** 画面の表示名（「取り込み: 2024-05-01」など）。 */
  label: string;
}

export interface Selection {
  /** 選択中の variant（複数選択）。 */
  ids: VariantId[];
  /** 操作の対象（ルーペ・現像で表示する写真）。 */
  current: VariantId | null;
  /** current の検索結果の中の位置（不明なら null）。 */
  currentIndex: number | null;
  /** Shift で範囲を選ぶときの起点。 */
  anchorIndex: number | null;
}

export interface AppState {
  /** 起動時のアプリの状態。 */
  status: AppStatus | null;
  /** 開いているカタログ。 */
  catalog: CatalogStatus | null;
  /** カタログを開けなかったとき（壊れている・他のアプリが使っているなど）。 */
  catalogError: { path: string; error: ErrorInfo } | null;
  /** 開いている途中のカタログ（起動時に前回のカタログを開く間など。画面は最初の画面を出さずに待つ）。 */
  catalogOpening: string | null;
  mode: Mode;
  filter: SearchFilter;
  sort: SearchSort;
  /** 今の検索結果（世代と件数）。 */
  search: { generation: number; count: number } | null;
  searching: boolean;
  selection: Selection;
  /** 評価・フラグを付けたら次の写真へ進む（LIB-05）。 */
  autoAdvance: boolean;
  dialog: DialogState | null;
  toasts: Toast[];
  jobs: Record<number, JobView>;
  /** サムネイルの新しい rev（thumbnail_updated のイベント）。 */
  thumbRevs: Record<number, string>;
  /** 現像設定のコピー（DEV-30）。 */
  clipboard: { settings: DevelopSettings; from: VariantId } | null;
  /** フォルダの一覧を読み直す合図（取り込み・削除の後に増える）。 */
  foldersVersion: number;
  /** variant の詳細を読み直す合図（評価・キャプション・現像設定の変更）。 */
  detailsVersion: number;
}

export const DEFAULT_SORT: SearchSort = { key: "capture_time", direction: "ascending" };

const AUTO_ADVANCE_KEY = "genzo.autoAdvance";

function loadAutoAdvance(): boolean {
  try {
    return localStorage.getItem(AUTO_ADVANCE_KEY) === "1";
  } catch {
    return false;
  }
}

export const appStore = createStore<AppState>({
  status: null,
  catalog: null,
  catalogError: null,
  catalogOpening: null,
  mode: "grid",
  filter: {},
  sort: DEFAULT_SORT,
  search: null,
  searching: false,
  selection: { ids: [], current: null, currentIndex: null, anchorIndex: null },
  autoAdvance: typeof localStorage === "undefined" ? false : loadAutoAdvance(),
  dialog: null,
  toasts: [],
  jobs: {},
  thumbRevs: {},
  clipboard: null,
  foldersVersion: 0,
  detailsVersion: 0,
});

/** アプリの状態の一部を購読する。 */
export function useApp<T>(selector: (s: AppState) => T, equal?: (a: T, b: T) => boolean): T {
  return useStore(appStore, selector, equal);
}

// ---------------------------------------------------------------------------
// 通知
// ---------------------------------------------------------------------------

let toastSeq = 0;
const TOAST_MS: Record<Toast["level"], number> = {
  info: 4000,
  success: 4000,
  warning: 8000,
  error: 12000,
};

function notify(level: Toast["level"], message: string, detail?: string): void {
  const id = ++toastSeq;
  appStore.set((s) => ({ toasts: [...s.toasts.slice(-4), { id, level, message, detail }] }));
  setTimeout(() => dismissToast(id), TOAST_MS[level]);
}

function dismissToast(id: number): void {
  appStore.set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
}

/** 失敗を通知する（取り消し・古い世代は通知しない）。 */
function reportError(e: unknown, context?: string): void {
  const err = toApiError(e);
  if (err.kind === "cancelled" || err.kind === "stale") return;
  console.error(context ?? "エラー", err.info);
  notify("error", context ? `${context}: ${err.info.message}` : err.info.message, err.info.hint ?? undefined);
}

// ---------------------------------------------------------------------------
// ジョブ
// ---------------------------------------------------------------------------

export const JOB_LABEL: Record<JobKind, string> = {
  import: "取り込み",
  export: "書き出し",
  regenerate_previews: "プレビューの作り直し",
  integrity_check: "カタログの詳細チェック",
  reresolve_capture_times: "撮影日時の推定し直し",
  collect_garbage: "キャッシュの整理",
};

const jobWaiters = new Map<number, ((info: JobInfo) => void)[]>();

/** ジョブを登録する（進捗の表示に使う）。 */
function trackJob(jobId: number, kind: JobKind, label?: string): void {
  appStore.set((s) => ({
    jobs: {
      ...s.jobs,
      [jobId]: s.jobs[jobId] ?? {
        jobId,
        kind,
        state: "running",
        done: 0,
        total: 0,
        message: null,
        label: label ?? JOB_LABEL[kind],
      },
    },
  }));
}

/** ジョブが終わるのを待って、結果を返す。 */
function waitJob(jobId: number): Promise<JobInfo> {
  return new Promise((resolve, reject) => {
    const list = jobWaiters.get(jobId) ?? [];
    list.push(resolve);
    jobWaiters.set(jobId, list);
    // 既に終わっていたら（イベントを取りこぼした場合も）、状態を問い合わせる。
    core
      .job(jobId)
      .then((info) => {
        if (info.state !== "running") finishJob(info);
      })
      .catch(reject);
  });
}

function finishJob(info: JobInfo): void {
  const waiters = jobWaiters.get(info.job_id);
  jobWaiters.delete(info.job_id);
  appStore.set((s) => {
    const cur = s.jobs[info.job_id];
    if (!cur) return {};
    return {
      jobs: {
        ...s.jobs,
        [info.job_id]: { ...cur, state: info.state, done: info.done, total: info.total },
      },
    };
  });
  for (const w of waiters ?? []) w(info);
  // 終わったジョブは少し後に一覧から消す。
  setTimeout(() => {
    appStore.set((s) => {
      const { [info.job_id]: _removed, ...rest } = s.jobs;
      return { jobs: rest };
    });
  }, 6000);
}

// ---------------------------------------------------------------------------
// 検索・選択
// ---------------------------------------------------------------------------

let searchSeq = 0;

/** Shift で範囲を選ぶときの件数の上限（**仮置き**: 10,000。コマンドの大きさと応答の時間のため）。 */
export const MAX_RANGE_SELECTION = 10_000;

/** 今の条件で検索し直す（選択中の写真の位置を求め直す）。 */
async function runSearch(): Promise<void> {
  const seq = ++searchSeq;
  const { filter, sort } = appStore.get();
  appStore.set({ searching: true });
  try {
    const r = await core.search(filter, sort);
    if (seq !== searchSeq) return;
    searchCache.reset(r.generation, r.count);
    appStore.set({ search: r, searching: false });
    await relocateSelection();
  } catch (e) {
    if (seq === searchSeq) appStore.set({ searching: false });
    reportError(e, "検索できません");
  }
}

/** 選択中の写真の位置を、今の世代で求め直す（見つからなければ選択を外す）。 */
async function relocateSelection(): Promise<void> {
  const sel = appStore.get().selection;
  if (sel.current === null) {
    // 選択がなければ先頭を選ぶ（ルーペ・現像で写真がない状態を避ける）。
    if (searchCache.count > 0 && appStore.get().mode !== "grid") await selectIndex(0);
    return;
  }
  try {
    const r = await core.indexOf(sel.current);
    if (r.generation !== searchCache.generation) return;
    if (r.index === null) {
      // 絞り込みで外れた。ルーペ・現像では近い位置の写真に移る。
      const fallback =
        sel.currentIndex !== null ? Math.min(sel.currentIndex, searchCache.count - 1) : -1;
      if (fallback >= 0 && appStore.get().mode !== "grid") await selectIndex(fallback);
      else
        appStore.set((s) => ({
          selection: { ...s.selection, current: null, currentIndex: null, ids: [] },
        }));
      return;
    }
    appStore.set((s) => ({ selection: { ...s.selection, currentIndex: r.index } }));
  } catch (e) {
    if (!isStale(e)) reportError(e);
  }
}

/** 位置で選ぶ（`extend`: Shift で範囲、`toggle`: Ctrl / ⌘ で追加・解除）。 */
async function selectIndex(
  index: number,
  opts: { extend?: boolean; toggle?: boolean } = {},
): Promise<void> {
  if (index < 0 || index >= searchCache.count) return;
  const item = await searchCache.fetch(index);
  if (!item) return;
  const id = item.variant_id;
  const sel = appStore.get().selection;
  if (opts.extend && sel.anchorIndex !== null) {
    // 範囲の id を集める（大きすぎる範囲は、操作の対象（index）の側から 10,000 件まで。対象は必ず含める）。
    const [a, end] =
      sel.anchorIndex <= index
        ? [Math.max(sel.anchorIndex, index - (MAX_RANGE_SELECTION - 1)), index]
        : [index, Math.min(sel.anchorIndex, index + (MAX_RANGE_SELECTION - 1))];
    await searchCache.ensure(a, end + 1);
    const ids: VariantId[] = [];
    for (let i = a; i <= end; i++) {
      const v = searchCache.get(i);
      if (v) ids.push(v.variant_id);
    }
    appStore.set({ selection: { ids, current: id, currentIndex: index, anchorIndex: sel.anchorIndex } });
    return;
  }
  if (opts.toggle) {
    const has = sel.ids.includes(id);
    const ids = has ? sel.ids.filter((x) => x !== id) : [...sel.ids, id];
    appStore.set({
      selection: { ids, current: has ? (ids[ids.length - 1] ?? null) : id, currentIndex: has ? null : index, anchorIndex: index },
    });
    if (has) await relocateSelection();
    return;
  }
  appStore.set({ selection: { ids: [id], current: id, currentIndex: index, anchorIndex: index } });
}

/** 前後の写真へ（`delta` は -1 / +1 など）。 */
async function moveSelection(delta: number): Promise<void> {
  const { currentIndex } = appStore.get().selection;
  const base = currentIndex ?? (delta > 0 ? -1 : searchCache.count);
  const next = Math.max(0, Math.min(searchCache.count - 1, base + delta));
  if (next !== currentIndex) await selectIndex(next);
}

/** 操作の対象: グリッドでは選択中のすべて、ルーペ・現像では表示中の 1 枚。 */
function targets(): VariantId[] {
  const { mode, selection } = appStore.get();
  if (mode === "grid") return selection.ids.length > 0 ? selection.ids : selection.current !== null ? [selection.current] : [];
  return selection.current !== null ? [selection.current] : [];
}

async function afterMark(): Promise<void> {
  const s = appStore.get();
  if (s.autoAdvance && s.mode !== "grid") await moveSelection(1);
  else if (s.autoAdvance && s.mode === "grid" && s.selection.ids.length <= 1) await moveSelection(1);
}

// ---------------------------------------------------------------------------
// イベント
// ---------------------------------------------------------------------------

/** 検索結果のキャッシュの読み直しをまとめる時間（**仮置き**: 100 ms）。 */
const INVALIDATE_DELAY_MS = 100;
let invalidateTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleInvalidate(): void {
  if (invalidateTimer !== null) return;
  invalidateTimer = setTimeout(() => {
    invalidateTimer = null;
    searchCache.invalidate();
  }, INVALIDATE_DELAY_MS);
}

function handleEvent(e: CoreEvent): void {
  switch (e.type) {
    case "search_updated":
      if (e.generation > searchCache.generation) {
        searchCache.reset(e.generation, e.count);
        appStore.set({ search: { generation: e.generation, count: e.count } });
        void relocateSelection();
      }
      break;
    case "catalog_changed":
      // 評価・キャプション・現像設定など、同じ世代のまま内容が変わるもの。続けて来る変更（履歴の移動・
      // 一括適用など）は、短い間まとめて 1 回だけ読み直す。
      scheduleInvalidate();
      appStore.set((s) => ({
        detailsVersion: s.detailsVersion + 1,
        foldersVersion:
          e.change === "imported" || e.change === "removed" ? s.foldersVersion + 1 : s.foldersVersion,
      }));
      break;
    case "thumbnail_updated":
      appStore.set((s) => ({ thumbRevs: { ...s.thumbRevs, [e.variant_id]: e.rev } }));
      break;
    case "job_progress":
      appStore.set((s) => {
        const cur = s.jobs[e.job_id] ?? {
          jobId: e.job_id,
          kind: e.kind,
          state: "running" as const,
          done: 0,
          total: 0,
          message: null,
          label: JOB_LABEL[e.kind],
        };
        return { jobs: { ...s.jobs, [e.job_id]: { ...cur, done: e.done, total: e.total } } };
      });
      break;
    case "job_finished": {
      appStore.set((s) => {
        const cur = s.jobs[e.job_id];
        if (!cur) return {};
        return { jobs: { ...s.jobs, [e.job_id]: { ...cur, state: e.state, message: e.message } } };
      });
      if (e.state === "failed") notify("error", `${JOB_LABEL[e.kind]}に失敗しました`, e.message ?? undefined);
      void core
        .job(e.job_id)
        .then(finishJob)
        .catch(() => undefined);
      break;
    }
    case "warning":
      // 現像の描画の警告は現像の画面に出すので、通知にはしない。
      if (e.code !== "render") notify("warning", e.message);
      break;
    case "preview_updated":
      break;
  }
}

let eventsInstalled = false;

function installEventHandlers(): void {
  if (eventsInstalled) return;
  eventsInstalled = true;
  onCoreEvent(handleEvent);
}

// ---------------------------------------------------------------------------
// カタログ
// ---------------------------------------------------------------------------

/** カタログを開いた後の状態にする。 */
async function catalogOpened(status: CatalogStatus): Promise<void> {
  // 新しく開いたコアの検索の世代は 1 から数え直すので、前のカタログの世代を残さない（残すと、新しい
  // カタログの search_updated を古いものとして捨てる）。
  searchCache.reset(0, 0);
  appStore.set({
    catalog: status,
    catalogError: null,
    catalogOpening: null,
    search: null,
    selection: { ids: [], current: null, currentIndex: null, anchorIndex: null },
    filter: {},
    sort: DEFAULT_SORT,
    mode: "grid",
    dialog: null,
    thumbRevs: {},
    clipboard: null,
  });
  appStore.set((s) => ({ foldersVersion: s.foldersVersion + 1 }));
  for (const w of status.warnings) handleEvent(w);
  await runSearch();
}

/**
 * カタログを閉じた後の状態にする（カタログの選択の画面に戻る）。
 *
 * コアは閉じるときに実行中のジョブを取り消すが、閉じた後はイベントを送らず、ジョブの状態も問い合わせられない。
 * そのため、待っているもの（actions.waitJob）には取り消しとして返す。
 */
function catalogClosed(): void {
  searchCache.reset(0, 0);
  const jobs = appStore.get().jobs;
  appStore.set({
    catalog: null,
    catalogOpening: null,
    search: null,
    searching: false,
    selection: { ids: [], current: null, currentIndex: null, anchorIndex: null },
    mode: "grid",
    dialog: null,
    thumbRevs: {},
    clipboard: null,
    jobs: {},
  });
  for (const [id, waiters] of [...jobWaiters]) {
    jobWaiters.delete(id);
    const job = jobs[id];
    const info: JobInfo = {
      job_id: id,
      kind: job?.kind ?? "import",
      state: "cancelled",
      done: job?.done ?? 0,
      total: job?.total ?? 0,
      result: null,
      error: null,
    };
    for (const w of waiters) w(info);
  }
}

/** カタログを開く（失敗は catalogError に入れる）。 */
async function openCatalog(path: string, create: boolean): Promise<boolean> {
  appStore.set({ catalogOpening: path });
  try {
    const status = await app.openCatalog(path, create);
    await catalogOpened(status);
    return true;
  } catch (e) {
    const error = toApiError(e).info;
    // アプリは別のカタログを開く前に今のカタログを閉じる。開けなかったときは、今のカタログが閉じたか
    // （検証の段階での失敗なら開いたまま）をアプリに確かめて、画面の状態を合わせる。
    if (appStore.get().catalog) {
      const still = await app
        .appStatus()
        .then((st) => st.catalog)
        .catch(() => null);
      if (still) {
        appStore.set({ catalogOpening: null });
        reportError(e, "カタログを開けません");
        return false;
      }
      catalogClosed();
    }
    appStore.set({ catalogOpening: null, catalogError: { path, error } });
    return false;
  }
}

export const actions = {
  // 通知
  notify,
  dismissToast,
  reportError,
  describeError,
  // ジョブ
  trackJob,
  waitJob,
  // カタログ
  installEventHandlers,
  openCatalog,
  catalogOpened,
  catalogClosed,
  clearCatalogError: () => appStore.set({ catalogError: null }),
  /** 開いているカタログの設定を、保存した値にする（書き出しの既定の色空間などを使う画面のため）。 */
  setCoreSettings: (settings: CoreSettings) =>
    appStore.set((s) => (s.catalog ? { catalog: { ...s.catalog, core_settings: settings } } : {})),
  setStatus: (status: AppStatus) => appStore.set({ status }),
  // 表示
  setMode(mode: Mode) {
    const s = appStore.get();
    if (mode !== "grid" && s.selection.current === null && searchCache.count > 0) {
      void selectIndex(0).then(() => appStore.set({ mode }));
      return;
    }
    appStore.set({ mode });
  },
  openDialog: (dialog: DialogState) => appStore.set({ dialog }),
  closeDialog: () => appStore.set({ dialog: null }),
  setAutoAdvance(v: boolean) {
    try {
      localStorage.setItem(AUTO_ADVANCE_KEY, v ? "1" : "0");
    } catch {
      // 保存できなくても、このセッションでは有効にする。
    }
    appStore.set({ autoAdvance: v });
  },
  // 検索
  runSearch,
  setFilter(patch: Partial<SearchFilter>) {
    appStore.set((s) => ({ filter: { ...s.filter, ...patch } }));
    void runSearch();
  },
  replaceFilter(filter: SearchFilter) {
    appStore.set({ filter });
    void runSearch();
  },
  setSort(sort: SearchSort) {
    appStore.set({ sort });
    void runSearch();
  },
  // 選択
  selectIndex,
  moveSelection,
  async selectAll() {
    const n = Math.min(searchCache.count, 10_000);
    await searchCache.ensure(0, n);
    const ids: VariantId[] = [];
    for (let i = 0; i < n; i++) {
      const v = searchCache.get(i);
      if (v) ids.push(v.variant_id);
    }
    appStore.set((s) => ({ selection: { ...s.selection, ids } }));
    if (searchCache.count > n) notify("info", `先頭の ${n.toLocaleString()} 件を選びました`);
  },
  selectNone: () =>
    appStore.set((s) => ({
      selection: { ...s.selection, ids: s.selection.current !== null ? [s.selection.current] : [] },
    })),
  targets,
  // 選別（LIB-04・05）
  async setRating(rating: Rating) {
    const ids = targets();
    if (ids.length === 0) return;
    try {
      await core.setRating(ids, rating);
      await afterMark();
    } catch (e) {
      reportError(e, "評価を変えられません");
    }
  },
  async setFlag(flag: Flag) {
    const ids = targets();
    if (ids.length === 0) return;
    try {
      await core.setFlag(ids, flag);
      await afterMark();
    } catch (e) {
      reportError(e, "フラグを変えられません");
    }
  },
  async setColorLabel(label: ColorLabel | null) {
    const ids = targets();
    if (ids.length === 0) return;
    try {
      await core.setColorLabel(ids, label);
    } catch (e) {
      reportError(e, "カラーラベルを変えられません");
    }
  },
  // 取り込み
  async importFolder(path: string, recursive: boolean) {
    try {
      const jobId = await core.importFolder(path, recursive);
      const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
      trackJob(jobId, "import", `取り込み: ${name}`);
      const info = await waitJob(jobId);
      const r = info.result;
      if (r && r.type === "import") {
        const problems = r.errors.length + r.not_registered.length;
        const text = `取り込み: ${r.added.toLocaleString()} 件を追加、${r.updated.toLocaleString()} 件を更新${
          r.unchanged ? `、${r.unchanged.toLocaleString()} 件は変更なし` : ""
        }`;
        notify(problems > 0 ? "warning" : "success", text, problems > 0 ? `${problems} 件のファイルを登録できませんでした` : undefined);
      }
      appStore.set((s) => ({ foldersVersion: s.foldersVersion + 1 }));
      await runSearch();
      return info;
    } catch (e) {
      reportError(e, "取り込めません");
      return null;
    }
  },
  setClipboard: (clipboard: AppState["clipboard"]) => appStore.set({ clipboard }),
};
