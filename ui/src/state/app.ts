// アプリ全体の状態（表示モード・検索の条件・選択・ダイアログ・通知・ジョブ）と操作。
//
// 画面の各部（library・viewer・develop・dialogs）は、ここの `appStore` を購読し、`actions` で変える。

import * as app from "../api/app";
import * as core from "../api/core";
import { describeError, isStale, toApiError } from "../api/errors";
import { onCoreEvent } from "../api/events";
import type {
  AppStatus,
  AssetKind,
  CatalogStatus,
  CloseFailure,
  CollectionId,
  CollectionKind,
  ColorLabel,
  CoreEvent,
  CoreSettings,
  CrashReportSummary,
  DeleteKind,
  DevelopSettings,
  ErrorInfo,
  Flag,
  JobInfo,
  JobKind,
  JobState,
  KeywordId,
  Rating,
  SearchFilter,
  SearchSort,
  VariantId,
  VariantSummary,
} from "../api/types";
import { type CatalogChangeSet, searchCache, thumbRevs } from "./search";
import { createStore, useStore } from "./store";

/** 表示モード（G: グリッド、E: ルーペ、D: 現像）。 */
export type Mode = "grid" | "loupe" | "develop";

/** ダイアログ（同時に 1 つ）。 */
export type DialogState =
  | { kind: "import" }
  | { kind: "export"; variantIds: VariantId[] }
  | { kind: "delete"; deleteKind: DeleteKind; variantIds: VariantId[] }
  | { kind: "paste_settings"; targets: VariantId[] }
  | { kind: "settings"; tab?: "general" | "view" | "catalog" | "backup" }
  | { kind: "shortcuts" }
  | { kind: "close_failed"; failure: CloseFailure; quitting: boolean }
  | { kind: "crash_reports"; reports: CrashReportSummary[] }
  // キーワード（LIB-09）: 作る（parentId の下）・名前の変更・移動・削除の確認。
  | { kind: "keyword"; mode: "create" | "rename" | "move"; keywordId?: KeywordId; parentId?: KeywordId | null }
  | { kind: "keyword_delete"; keywordId: KeywordId }
  // コレクション（LIB-10・LIB-11）: 作る（種類・親のセット・入れる写真）・名前の変更・移動・削除の確認・
  // スマートコレクションの条件の編集（collectionId がなければ新しく作る）・写真を入れる先を選ぶ。
  | {
      kind: "collection";
      mode: "create" | "rename" | "move";
      collectionKind?: CollectionKind;
      collectionId?: CollectionId;
      parentId?: CollectionId | null;
      variantIds?: VariantId[];
    }
  | { kind: "collection_delete"; collectionId: CollectionId }
  | { kind: "smart_collection"; collectionId?: CollectionId; parentId?: CollectionId | null }
  | { kind: "add_to_collection"; variantIds: VariantId[] };

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
  /** Shift で範囲を選ぶときの起点（位置。世代が変わると古くなりうるので、anchorId で確かめる）。 */
  anchorIndex: number | null;
  /** Shift で範囲を選ぶときの起点の写真（選ぶときに、今の世代の位置を求め直す）。 */
  anchorId: VariantId | null;
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
  /** 評価・フラグ・カラーラベルを付けたら次の写真へ進む（LIB-05）。 */
  autoAdvance: boolean;
  dialog: DialogState | null;
  toasts: Toast[];
  jobs: Record<number, JobView>;
  /**
   * サムネイルの新しい rev（thumbnail_updated のイベント）を id で引く、読み取り専用の見え方。中身は
   * state/search.ts の thumbRevs（Map）で、受け取るたびに複製しない（50 万件の取り込みで件数の 2 乗の手間に
   * しないため）。ルーペ（viewer/Loupe.tsx）が `s.thumbRevs[id]` で読む。
   */
  thumbRevs: Readonly<Record<VariantId, string | undefined>>;
  /** サムネイルの新しい rev の変化の通し番号（まとめて知らせるたびに増える。Grid・Filmstrip が描き直す）。 */
  thumbRevsVersion: number;
  /** 現像設定のコピー（DEV-30）。 */
  clipboard: { settings: DevelopSettings; from: VariantId } | null;
  /** フォルダの一覧を読み直す合図（取り込み・削除の後に増える）。 */
  foldersVersion: number;
  /** variant の詳細を読み直す合図（評価・キャプション・現像設定の変更）。 */
  detailsVersion: number;
  /** キーワード・コレクションの一覧と、写真のキーワードを読み直す合図（LIB-09〜11。それらの変更・削除の後に増える）。 */
  organizeVersion: number;
  /**
   * ターゲットコレクション（B キーで選んだ写真を入れる・外す手動のコレクション。LIB-10）。カタログごとに
   * ブラウザに保存する（state/organize.ts）。
   */
  targetCollection: CollectionId | null;
}

export const DEFAULT_SORT: SearchSort = { key: "capture_time", direction: "ascending" };

/** ターゲットコレクションを保存する鍵（カタログごと。コレクションの ID はカタログの中だけで意味を持つ）。 */
export function targetCollectionKey(catalogPath: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < catalogPath.length; i++) {
    h ^= catalogPath.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return `genzo.library.targetCollection.${(h >>> 0).toString(16).padStart(8, "0")}`;
}

function loadTargetCollection(catalogPath: string): CollectionId | null {
  try {
    const v = localStorage.getItem(targetCollectionKey(catalogPath));
    const n = v === null ? NaN : Number(v);
    return Number.isInteger(n) && n > 0 ? n : null;
  } catch {
    return null;
  }
}

const AUTO_ADVANCE_KEY = "genzo.autoAdvance";

function loadAutoAdvance(): boolean {
  try {
    return localStorage.getItem(AUTO_ADVANCE_KEY) === "1";
  } catch {
    return false;
  }
}

/**
 * サムネイルの新しい rev の見え方（AppState.thumbRevs）。`[id]` で thumbRevs（Map）を引く（複製しない。
 * 書き込みはしない）。
 */
const thumbRevsView: Readonly<Record<VariantId, string | undefined>> = new Proxy(
  Object.create(null) as Record<VariantId, string | undefined>,
  {
    get: (_target, key) => (typeof key === "string" ? thumbRevs.get(Number(key)) : undefined),
  },
);

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
  selection: { ids: [], current: null, currentIndex: null, anchorIndex: null, anchorId: null },
  autoAdvance: typeof localStorage === "undefined" ? false : loadAutoAdvance(),
  dialog: null,
  toasts: [],
  jobs: {},
  thumbRevs: thumbRevsView,
  thumbRevsVersion: 0,
  clipboard: null,
  foldersVersion: 0,
  detailsVersion: 0,
  organizeVersion: 0,
  targetCollection: null,
});

// サムネイルの新しい rev は、まとめて（1 フレームに 1 回）知らせる。ルーペの `s.thumbRevs[id]` の購読も
// これで読み直す。
thumbRevs.subscribe(() => appStore.set({ thumbRevsVersion: thumbRevs.version }));

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
  // 終わったジョブは少し後に一覧から消す。ジョブの番号はカタログごとに 1 から数え直すので、その間にカタログを
  // 切り替えていたら消さない（新しいカタログの同じ番号のジョブを消さないように。一覧はカタログの開閉で空になる）。
  const session = catalogSession;
  setTimeout(() => {
    if (session !== catalogSession) return;
    appStore.set((s) => {
      const cur = s.jobs[info.job_id];
      if (!cur || cur.state === "running") return {};
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

/**
 * 取り込み中に、検索結果（search_updated）・フォルダ・カタログの件数・絞り込みの候補を読み直す間隔（ms。
 * **仮置き**: 1 秒）。コアは取り込みの 64 件ごとに新しい世代とカタログの変更を知らせるので、そのたびに
 * 読み直すと、表示の作り直し（件数が変わると仮想スクロールの配置の計算もし直す）と、カタログの Mutex を
 * 取り込みと取り合う要求が増える。
 */
export const IMPORT_REFRESH_MS = 1000;

/** 間隔 `ms` に 1 回までにまとめて呼ぶ（間隔が空いていればすぐ、間隔の中なら間隔の終わりに 1 回）。 */
class Throttle {
  private last = Number.NEGATIVE_INFINITY;
  private timer: ReturnType<typeof setTimeout> | null = null;
  constructor(
    private readonly ms: number,
    private readonly run: () => void,
  ) {}

  request(): void {
    // 時計が戻っても、間隔より長くは待たない。
    const wait = Math.min(this.ms, this.last + this.ms - Date.now());
    if (wait <= 0) this.fire();
    else if (this.timer === null) this.timer = setTimeout(() => this.fire(), wait);
  }

  /** すぐ呼ぶ（待たせている分もまとめて）。 */
  fire(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.last = Date.now();
    this.run();
  }

  /** 待たせている分があれば、すぐ呼ぶ。 */
  flush(): void {
    if (this.timer !== null) this.fire();
  }

  /** 待たせている分を、ほかで済ませた（今呼んだことにして、次は間隔の後にする）。 */
  done(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.last = Date.now();
  }

  /** 待たせている分を捨て、間隔を数え直す（カタログを開いた・閉じたとき）。 */
  cancel(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.last = Number.NEGATIVE_INFINITY;
  }
}

/** 取り込みのジョブが実行中か。 */
function importRunning(): boolean {
  return Object.values(appStore.get().jobs).some((j) => j.kind === "import" && j.state === "running");
}

/** 使うのを待たせている新しい世代（取り込み中の search_updated。searchThrottle が使う）。 */
let pendingSearch: { generation: number; count: number } | null = null;

const searchThrottle = new Throttle(IMPORT_REFRESH_MS, () => {
  const p = pendingSearch;
  pendingSearch = null;
  if (p && applySearch(p)) void relocateSelection();
});

/** 新しい世代の検索結果にする（今より新しい世代のときだけ。したら true）。 */
function applySearch(r: { generation: number; count: number }): boolean {
  if (r.generation <= searchCache.generation) return false;
  searchCache.reset(r.generation, r.count);
  appStore.set({ search: { generation: r.generation, count: r.count } });
  return true;
}

/** 新しい世代の知らせ（search_updated）。取り込み中は IMPORT_REFRESH_MS に 1 回にまとめる。 */
function searchUpdated(generation: number, count: number): void {
  if (generation <= searchCache.generation || (pendingSearch && generation <= pendingSearch.generation)) return;
  pendingSearch = { generation, count };
  if (importRunning()) searchThrottle.request();
  else searchThrottle.fire();
}

// 待たせている間に今の世代の range が stale になった（コアは次の世代に進んでいる。選択などの操作が読もうと
// した）: 待たせている世代をすぐ使う（読み込みの続きは searchCache.items が新しい世代で読み直す）。
searchCache.onStale = () => {
  if (pendingSearch) searchThrottle.fire();
};

/** 今の条件で検索し直す（選択中の写真の位置を求め直す）。 */
async function runSearch(): Promise<void> {
  const seq = ++searchSeq;
  const { filter, sort } = appStore.get();
  appStore.set({ searching: true });
  try {
    const r = await core.search(filter, sort);
    if (seq !== searchSeq) return;
    // 応答より新しい世代（コアは search の直後に、次の世代の search_updated を送りうる。イベントとコマンドの
    // 応答は別の経路なので、イベントが先に届くことがある）を既に受け取っていれば、そちらのままにする。古い世代に
    // 戻すと、コアは今の世代以外の range を stale にするので、一覧が空のまま止まる。同じ世代なら、イベントで
    // 同じ結果にしてあるので、キャッシュを捨て直さない。コアの世代は 1 つのコアの中で増え続ける（別の
    // カタログを開いたときは catalogOpened が 0 に戻す）。取り込み中に待たせている世代が応答より新しければ、
    // そちらを使う（古ければ捨てる）。
    const pending = pendingSearch;
    pendingSearch = null;
    if (applySearch(pending && pending.generation > r.generation ? pending : r)) searchThrottle.done();
    appStore.set({ searching: false });
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
    // 問い合わせている間に選び直されていたら、その選択のままにする（前の写真の位置を、別の写真の位置として
    // 書かない。外れたときの選び直し・選択の解除もしない）。
    if (appStore.get().selection.current !== sel.current) return;
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
    const index = r.index;
    appStore.set((s) =>
      s.selection.current !== sel.current
        ? {}
        : {
            selection: {
              ...s.selection,
              currentIndex: index,
              // 範囲の起点が表示中の写真なら、その位置も合わせる。
              anchorIndex: s.selection.anchorId === sel.current ? index : s.selection.anchorIndex,
            },
          },
    );
  } catch (e) {
    if (!isStale(e)) reportError(e);
  }
}

/**
 * `from` から `to` までの範囲の項目を集める（大きすぎる範囲は `to` の側から 10,000 件まで。`to` は必ず
 * 含める）。
 */
async function rangeItems(from: number, to: number): Promise<VariantSummary[]> {
  const [a, end] =
    from <= to
      ? [Math.max(from, to - (MAX_RANGE_SELECTION - 1)), to]
      : [to, Math.min(from, to + (MAX_RANGE_SELECTION - 1))];
  // 今の世代の項目だけを使う（キャッシュのページの数には上限があるので、読んだ値をそのまま使う）。
  return searchCache.items(a, end + 1);
}

/**
 * 写真 `id` の今の世代の位置（`hint` の位置にその写真があればそこ。なければ index_of で求める）。検索結果に
 * ない・求められなければ null。コアが、待たせている新しい世代（取り込み中の間引き）に進んでいれば、その世代に
 * してから使う。
 */
async function locate(id: VariantId, hint: number | null): Promise<number | null> {
  if (hint !== null && hint >= 0 && hint < searchCache.count && (await searchCache.fetch(hint))?.variant_id === id) {
    return hint;
  }
  const r = await core.indexOf(id).catch(() => null);
  if (!r) return null;
  if (r.generation !== searchCache.generation && pendingSearch?.generation === r.generation) searchThrottle.fire();
  return r.generation === searchCache.generation ? r.index : null;
}

/**
 * 表示中の写真を変えずに、選択に足す・選択から外す（ルーペ・現像のフィルムストリップの Ctrl / ⌘・Shift +
 * クリック）。`extend` なら表示中の写真からその写真までの範囲を足し、そうでなければその写真を足す・外す
 * （表示中の写真は外さない。外すと、表示中の写真が選択に入っていない状態になる）。
 */
/**
 * 選ぶ操作（selectIndex・selectAll・selectNone）の通し番号。ページの読み込みを待つ間に新しい選ぶ操作が
 * あれば、古い操作の結果で選択を上書きしない（素早く送ったときに前の写真に戻らないように）。
 */
let selectSeq = 0;

async function changeSelectionKeepingCurrent(
  index: number,
  item: VariantSummary,
  extend: boolean,
  seq: number,
): Promise<boolean> {
  const id = item.variant_id;
  const sel = appStore.get().selection;
  if (!extend && id === sel.current) return false;
  let addedItems = [item];
  if (extend && sel.current !== null) {
    // 範囲の起点（表示中の写真）は、今の世代の位置で求める（currentIndex は世代が変わった直後は古い）。
    // 分からなければ（結果から外れた）、範囲ではなくその写真だけを足す。
    const generation = searchCache.generation;
    const start = await locate(sel.current, sel.currentIndex);
    if (start !== null) {
      // 起点を求める間に世代が変わったら、クリックした写真の位置も求め直す。
      const target = searchCache.generation === generation ? index : await locate(id, index);
      if (target === null) return false;
      addedItems = await rangeItems(start, target);
    }
  }
  if (seq !== selectSeq) return false;
  rememberSelected(addedItems);
  const added = addedItems.map((v) => v.variant_id);
  appStore.set((s) => {
    const cur = s.selection;
    // 表示中の写真は、常に選択に含める。
    const base = cur.current !== null && !cur.ids.includes(cur.current) ? [cur.current, ...cur.ids] : cur.ids;
    if (!extend && base.includes(id)) {
      if (id === cur.current) return {};
      return { selection: { ...cur, ids: base.filter((x) => x !== id) } };
    }
    const has = new Set(base);
    return { selection: { ...cur, ids: [...base, ...added.filter((x) => !has.has(x))] } };
  });
  return true;
}

/**
 * 位置で選ぶ（`extend`: Shift で範囲、`toggle`: Ctrl / ⌘ で追加・解除）。
 *
 * グリッドでは、操作の対象（current）もその写真に移る。`keepCurrent`（ルーペ・現像のフィルムストリップ）
 * なら、Ctrl / ⌘・Shift では選択だけを変え、表示中の写真（current）は変えない（Lightroom と同じ。Shift の
 * 範囲の起点は表示中の写真）。表示中の写真が移ると、現像の画面がその写真を開き直し、「選んだ…に同期」の
 * コピー元が最後にクリックした写真に入れ替わって、現像していた写真を上書きしてしまうため。修飾キーなしの
 * クリックは、どちらでもその写真を表示し、選択をその 1 枚にする。
 *
 * `expect` はクリックしたセルに出していた写真（グリッド・フィルムストリップ。Shift + クリックを含む）。セルは
 * 新しい世代のページが届くまで古い世代の項目を出すので、今の世代のその位置が別の写真なら、その写真の位置を
 * 求め直して選ぶ（見つからなければ選ばない）。範囲の起点も、起点の写真（anchorId・表示中の写真）の今の世代の
 * 位置で求める。
 *
 * 戻り値は選んだ（選択を変えた）か。選べなかったとき（写真が見つからない・読めない）は false。
 */
/** 最後に始めた選ぶ操作（表示の切り替えが、選んでいる途中の結果を待つため）。 */
let pendingSelect: Promise<unknown> = Promise.resolve();

function selectIndex(
  index: number,
  opts: { extend?: boolean; toggle?: boolean; keepCurrent?: boolean; expect?: VariantId } = {},
): Promise<boolean> {
  const p = selectIndexNow(index, opts);
  pendingSelect = p.catch(() => undefined);
  return p;
}

async function selectIndexNow(
  index: number,
  opts: { extend?: boolean; toggle?: boolean; keepCurrent?: boolean; expect?: VariantId },
): Promise<boolean> {
  if (index < 0 || index >= searchCache.count) return false;
  const seq = ++selectSeq;
  const superseded = () => seq !== selectSeq;
  let item = await searchCache.fetch(index);
  if (superseded()) return false;
  if (opts.expect !== undefined && item?.variant_id !== opts.expect) {
    const at = await locate(opts.expect, null);
    if (at === null || superseded()) return false;
    index = at;
    item = await searchCache.fetch(index);
    if (item?.variant_id !== opts.expect || superseded()) return false;
  }
  if (!item) return false;
  const id = item.variant_id;
  const sel = appStore.get().selection;
  if (opts.keepCurrent && sel.current !== null && (opts.extend || opts.toggle)) {
    return changeSelectionKeepingCurrent(index, item, !!opts.extend, seq);
  }
  if (opts.extend && sel.anchorIndex !== null) {
    // 範囲の起点は、起点の写真の今の世代の位置（結果から外れていれば、操作の対象の写真だけ）。
    const generation = searchCache.generation;
    const anchor = sel.anchorId !== null ? await locate(sel.anchorId, sel.anchorIndex) : sel.anchorIndex;
    if (searchCache.generation !== generation) {
      // 起点を求める間に世代が変わった: 操作の対象の位置も求め直す。
      const at = await locate(id, index);
      if (at === null) return false;
      index = at;
    }
    const from = anchor ?? index;
    // 範囲の項目を集める（大きすぎる範囲は、操作の対象（index）の側から 10,000 件まで。対象は必ず含める）。
    const items = await rangeItems(from, index);
    if (superseded()) return false;
    rememberSelected(items);
    appStore.set({
      selection: {
        ids: items.map((v) => v.variant_id),
        current: id,
        currentIndex: index,
        anchorIndex: from,
        anchorId: anchor === null ? id : sel.anchorId,
      },
    });
    return true;
  }
  rememberSelected([item]);
  if (opts.toggle) {
    const has = sel.ids.includes(id);
    const ids = has ? sel.ids.filter((x) => x !== id) : [...sel.ids, id];
    appStore.set({
      selection: {
        ids,
        current: has ? (ids[ids.length - 1] ?? null) : id,
        currentIndex: has ? null : index,
        anchorIndex: index,
        anchorId: id,
      },
    });
    if (has) await relocateSelection();
    return true;
  }
  appStore.set({ selection: { ids: [id], current: id, currentIndex: index, anchorIndex: index, anchorId: id } });
  return true;
}

// ---------------------------------------------------------------------------
// 選択中の写真の概要
// ---------------------------------------------------------------------------

/** 選択中の写真の概要のうち、覚えておくもの。 */
export type SelectedSummary = Pick<VariantSummary, "variant_id" | "kind" | "file_name">;

/**
 * 選択中の写真（選択の集合と表示中の写真）の概要（選んだときの項目から）。検索結果のキャッシュのページは上限で
 * 捨てるので、選んだ写真の種類（動画か）をページとは別に覚えておく（現像設定のコピー・貼り付けで、動画に
 * 既定の設定を写さないため）。選択から外れたものは、選択が変わったときに捨てる。
 */
const selectedSummaries = new Map<VariantId, SelectedSummary>();

function rememberSelected(items: Iterable<VariantSummary>): void {
  for (const v of items) {
    selectedSummaries.set(v.variant_id, { variant_id: v.variant_id, kind: v.kind, file_name: v.file_name });
  }
}

let lastSelection = appStore.get().selection;
appStore.subscribe(() => {
  const sel = appStore.get().selection;
  if (sel === lastSelection) return;
  lastSelection = sel;
  if (selectedSummaries.size === 0) return;
  const keep = new Set(sel.ids);
  if (sel.current !== null) keep.add(sel.current);
  for (const id of [...selectedSummaries.keys()]) if (!keep.has(id)) selectedSummaries.delete(id);
});

/**
 * 写真の種類（読み込み済みの検索結果・選んだときの項目から。どちらにもなければコアに問い合わせる）。読めなければ
 * 例外（呼び出し元が知らせる）。
 */
async function kindOf(id: VariantId): Promise<AssetKind> {
  const known = searchCache.getLoadedById(id)?.kind ?? selectedSummaries.get(id)?.kind;
  if (known) return known;
  return (await core.variantDetails(id)).summary.kind;
}

/**
 * 写真の現像設定をコピーする（Ctrl / ⌘ + Shift + C。DEV-30）。動画には現像設定がない（コアは既定の設定を返し、
 * 貼り付けると貼り付け先を既定に戻してしまう）ので、種類を確かめてからコピーする。`latest` は現像中の写真なら
 * 画面の最新の値（送信前を含む）。コピーしたら true。
 */
async function copySettingsOf(id: VariantId, latest: DevelopSettings | null): Promise<boolean> {
  try {
    if ((await kindOf(id)) === "video") {
      notify("info", "動画の設定はコピーできません");
      return false;
    }
    const settings = latest ?? (await core.copySettings(id));
    appStore.set({ clipboard: { settings, from: id } });
    notify("info", "現像設定をコピーしました");
    return true;
  } catch (e) {
    reportError(e, "現像設定をコピーできません");
    return false;
  }
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

/**
 * 現像設定の貼り付け（Ctrl / ⌘ + Shift + V・現像の画面の「貼り付け」）の対象: グリッドでは targets() と同じ。
 * ルーペ・現像では、表示中の写真とフィルムストリップで選んだ写真のすべて。
 *
 * 評価・フラグなどと違い、ルーペ・現像でも選択中のすべてにする: ショートカットの一覧（「現像設定を貼り付け
 * （選んだ写真へ）」）とグリッドでの動きに合わせ、Lightroom のように、フィルムストリップで選んだ写真へまとめて
 * 写せるようにするため（表示中の 1 枚だけにしたいときは、普通のクリックで選び直す）。どの写真に写すかは
 * 貼り付けのダイアログに枚数を出して確かめてもらい、コピー元の写真はダイアログで除く（pasteLogic.ts の
 * preparePasteTargets）。
 */
function pasteTargets(): VariantId[] {
  const { mode, selection } = appStore.get();
  if (mode === "grid" || selection.current === null) return targets();
  return selection.ids.includes(selection.current) ? selection.ids : [selection.current, ...selection.ids];
}

/** 評価・フラグ・カラーラベルを付けた後（「自動で次へ」がオンなら次の写真へ）。 */
/**
 * 表示の切り替え（setMode・openInLoupe・カタログの開閉）の通し番号。写真を選んでから切り替えるとき、選び
 * 終わるまでに新しい切り替えがあれば、古い切り替えを行わない。
 */
let modeSeq = 0;

/**
 * 選別の操作（評価・フラグ・カラーラベル）の列。前の操作と「自動で次へ」が済んでから、次の操作の対象を決める
 * （素早く続けて押したとき、同じ写真に付けて 2 枚進んだり、評価の順が入れ替わったりしないように）。
 */
let markQueue: Promise<void> = Promise.resolve();

/**
 * 選別の操作を列に入れる（`run` は自分の番が来てから対象を決める）。列に入れた後でカタログを切り替えたら
 * 行わない（新しいカタログの選択に、前のカタログで押した操作を付けないように）。
 */
function enqueueMark(run: () => Promise<void>): Promise<void> {
  const session = catalogSession;
  const guarded = () => (session === catalogSession ? run() : Promise.resolve());
  const next = markQueue.then(guarded, guarded);
  markQueue = next.catch(() => undefined);
  return next;
}

async function afterMark(): Promise<void> {
  const s = appStore.get();
  if (s.autoAdvance && s.mode !== "grid") await moveSelection(1);
  else if (s.autoAdvance && s.mode === "grid" && s.selection.ids.length <= 1) await moveSelection(1);
}

/**
 * カラーラベルのキーで付けるラベル: 対象がすべて `label` なら null（外す）、そうでなければ `label`。今の
 * ラベルが分からないもの（undefined。読み込んでいない）があれば付ける。
 */
export function nextColorLabel(
  label: ColorLabel,
  current: readonly (ColorLabel | null | undefined)[],
): ColorLabel | null {
  return current.length > 0 && current.every((c) => c === label) ? null : label;
}

/**
 * `ids` の今のカラーラベル（読み込み済みの検索結果から。読み込んでいないものは undefined）。読み込み済みの
 * 項目を 1 回だけ順に見る（写真ごとに探すと、選択の数 × 読み込み済みの件数になる）。
 */
/**
 * この画面で最後に書いたカラーラベル（写真ごと）。検索結果の読み直しは少し後なので、続けて同じキーを押したとき、
 * 読み込み済みの古い値ではなく、書いた値で外すか付けるかを決める。カタログの開閉で空にする（ラベルを変えるのは
 * この画面の操作だけ）。
 */
const writtenColorLabels = new Map<VariantId, ColorLabel | null>();

function loadedColorLabels(ids: readonly VariantId[]): (ColorLabel | null | undefined)[] {
  const wanted = new Set(ids.filter((id) => !writtenColorLabels.has(id)));
  const found = new Map<VariantId, ColorLabel | null>();
  for (const id of ids) {
    if (writtenColorLabels.has(id)) found.set(id, writtenColorLabels.get(id) ?? null);
  }
  if (wanted.size > 0) {
    let left = wanted.size;
    for (const v of searchCache.loaded()) {
      if (wanted.has(v.variant_id) && !found.has(v.variant_id)) {
        found.set(v.variant_id, v.color_label);
        if (--left === 0) break;
      }
    }
  }
  return ids.map((id) => found.get(id));
}

/** カラーラベルを付ける・外す（評価・フラグと同じく、「自動で次へ」がオンなら次の写真へ）。 */
async function applyColorLabel(ids: VariantId[], label: ColorLabel | null): Promise<void> {
  if (ids.length === 0) return;
  try {
    await core.setColorLabel(ids, label);
    for (const id of ids) writtenColorLabels.set(id, label);
    await afterMark();
  } catch (e) {
    reportError(e, "カラーラベルを変えられません");
  }
}

// ---------------------------------------------------------------------------
// イベント
// ---------------------------------------------------------------------------

/** 検索結果のキャッシュの読み直しをまとめる時間（**仮置き**: 100 ms）。 */
const INVALIDATE_DELAY_MS = 100;
let invalidateTimer: ReturnType<typeof setTimeout> | null = null;
/** まとめている変更（変わった variant・影響が広いか・最後の変更を受け取ったときの要求の通し番号）。 */
let pendingChange: { ids: Set<VariantId>; all: boolean; since: number } | null = null;

function scheduleInvalidate(e: Extract<CoreEvent, { type: "catalog_changed" }>): void {
  const change = pendingChange ?? { ids: new Set<VariantId>(), all: false, since: 0 };
  for (const id of e.variant_ids) change.ids.add(id);
  change.all ||= e.all;
  change.since = searchCache.requestMark();
  pendingChange = change;
  if (invalidateTimer !== null) return;
  invalidateTimer = setTimeout(() => {
    invalidateTimer = null;
    const c: CatalogChangeSet | null = pendingChange;
    pendingChange = null;
    // 新しい世代を待たせている（取り込み中）なら読み直さない: その世代にするときに表示中の範囲のページを
    // 読み直し、ほかのページは古い世代として、次に見たときに読み直す。
    if (c && !pendingSearch) searchCache.invalidate(c);
  }, INVALIDATE_DELAY_MS);
}

/** 取り込み中のカタログの変更で、フォルダ・件数・候補・詳細を読み直す合図（IMPORT_REFRESH_MS に 1 回）。 */
const importVersionsThrottle = new Throttle(IMPORT_REFRESH_MS, () =>
  appStore.set((s) => ({ detailsVersion: s.detailsVersion + 1, foldersVersion: s.foldersVersion + 1 })),
);

function handleEvent(e: CoreEvent): void {
  switch (e.type) {
    case "search_updated":
      searchUpdated(e.generation, e.count);
      break;
    case "catalog_changed":
      // 評価・キャプション・現像設定など、同じ世代のまま内容が変わるもの。続けて来る変更（履歴の移動・
      // 一括適用など）は、短い間まとめて 1 回だけ読み直す。
      scheduleInvalidate(e);
      if (e.change === "imported" && importRunning()) {
        // 取り込みの途中（64 件ごと）の変更は、フォルダ・件数・候補の読み直しを間引く。
        importVersionsThrottle.request();
        break;
      }
      appStore.set((s) => ({
        detailsVersion: s.detailsVersion + 1,
        foldersVersion:
          e.change === "imported" || e.change === "removed" ? s.foldersVersion + 1 : s.foldersVersion,
        // キーワード・コレクションの一覧（件数を含む）は、それらの変更と、写真の削除・評価などの変更
        // （スマートコレクションの件数）で読み直す。
        organizeVersion:
          e.change === "keywords" || e.change === "collections" || e.change === "removed" || e.change === "marks"
            ? s.organizeVersion + 1
            : s.organizeVersion,
      }));
      break;
    case "thumbnail_updated":
      // 受け取るたびに状態を作り直さない（知らせは 1 フレームにまとめる。search.ts の ThumbRevs）。
      thumbRevs.set(e.variant_id, e.rev);
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
      if (e.kind === "import") {
        // 取り込みが終わった: 間引いて待たせている読み直しを、すぐする。
        searchThrottle.flush();
        importVersionsThrottle.flush();
      }
      if (e.state === "failed") notify("error", `${JOB_LABEL[e.kind]}に失敗しました`, e.message ?? undefined);
      // 結果の問い合わせの間にカタログを切り替えたら、結果を捨てる（ジョブの番号はカタログごとに 1 から数え直す
      // ので、新しいカタログの同じ番号のジョブを終わったことにしないように）。
      const session = catalogSession;
      void core
        .job(e.job_id)
        .then((info) => {
          if (session === catalogSession) finishJob(info);
        })
        .catch((err) => {
          if (session !== catalogSession) return;
          // 結果を読めなくても、イベントの状態で終わらせる（待っている処理（取り込みの完了待ちなど）を
          // 止めたままにせず、一覧からも消す）。結果は null、読めなかった理由は error に入れる。
          const cur = appStore.get().jobs[e.job_id];
          finishJob({
            job_id: e.job_id,
            kind: e.kind,
            state: e.state,
            done: cur?.done ?? 0,
            total: cur?.total ?? 0,
            result: null,
            error: toApiError(err).info,
          });
        });
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

/**
 * カタログを開く・閉じるたびに増える番号。複数のフォルダの取り込みのような続きのある処理が、始めたときと
 * 同じカタログかを確かめる（閉じた・切り替えた・復元した後に、残りを別のカタログへ送らないため）。
 */
let catalogSession = 0;

/** 前のカタログの、待たせている読み直しとサムネイルの rev を捨てる（カタログを開いた・閉じたとき）。 */
function forgetCatalogState(): void {
  pendingSearch = null;
  searchThrottle.cancel();
  importVersionsThrottle.cancel();
  pendingChange = null;
  if (invalidateTimer !== null) clearTimeout(invalidateTimer);
  invalidateTimer = null;
  thumbRevs.clear();
}

/** カタログを開いた後の状態にする。 */
async function catalogOpened(status: CatalogStatus): Promise<void> {
  catalogSession++;
  // 新しく開いたコアの検索の世代は 1 から数え直すので、前のカタログの世代を残さない（残すと、新しい
  // カタログの search_updated を古いものとして捨てる）。
  forgetCatalogState();
  searchCache.reset(0, 0);
  appStore.set({
    catalog: status,
    catalogError: null,
    catalogOpening: null,
    search: null,
    selection: { ids: [], current: null, currentIndex: null, anchorIndex: null, anchorId: null },
    filter: {},
    sort: DEFAULT_SORT,
    mode: "grid",
    dialog: null,
    clipboard: null,
  });
  modeSeq++;
  writtenColorLabels.clear();
  appStore.set((s) => ({
    foldersVersion: s.foldersVersion + 1,
    organizeVersion: s.organizeVersion + 1,
    targetCollection: loadTargetCollection(status.catalog_path),
  }));
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
  // 待ちを返す前に増やす（返された待ちの続きが、閉じたことを分かるように）。
  catalogSession++;
  forgetCatalogState();
  searchCache.reset(0, 0);
  const jobs = appStore.get().jobs;
  appStore.set({
    catalog: null,
    catalogOpening: null,
    search: null,
    searching: false,
    selection: { ids: [], current: null, currentIndex: null, anchorIndex: null, anchorId: null },
    mode: "grid",
    dialog: null,
    clipboard: null,
    jobs: {},
    targetCollection: null,
  });
  modeSeq++;
  writtenColorLabels.clear();
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

// ---------------------------------------------------------------------------
// 取り込み
// ---------------------------------------------------------------------------

/**
 * フォルダを取り込み、終わったら結果を知らせて検索し直す（失敗は通知して null）。
 *
 * 待っている間にカタログを閉じた（切り替え・復元を含む）ときは、閉じたカタログの結果なので、知らせも検索の
 * し直しもせずに返す（catalogClosed が待ちを取り消しとして返す）。
 */
async function importFolder(path: string, recursive: boolean): Promise<JobInfo | null> {
  const session = catalogSession;
  try {
    const jobId = await core.importFolder(path, recursive);
    if (session !== catalogSession) return null;
    const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
    trackJob(jobId, "import", `取り込み: ${name}`);
    const info = await waitJob(jobId);
    if (session !== catalogSession) return info;
    const r = info.result;
    if (r && r.type === "import") {
      const problems = r.errors.length + r.not_registered.length;
      const text = `取り込み: ${r.added.toLocaleString()} 件を追加、${r.updated.toLocaleString()} 件を更新${
        r.unchanged ? `、${r.unchanged.toLocaleString()} 件は変更なし` : ""
      }`;
      notify(problems > 0 ? "warning" : "success", text, problems > 0 ? `${problems} 件のファイルを登録できませんでした` : undefined);
    } else if (info.state === "succeeded" && info.error) {
      // 終わったが結果を読めなかった（失敗は job_finished のイベントで知らせるので、ここでは知らせない）。
      notify("warning", "取り込みは終わりましたが、結果を読めません", info.error.message);
    }
    appStore.set((s) => ({ foldersVersion: s.foldersVersion + 1 }));
    await runSearch();
    return info;
  } catch (e) {
    if (session !== catalogSession) return null;
    reportError(e, "取り込めません");
    return null;
  }
}

/**
 * フォルダを順に取り込む（1 つずつ。結果は importFolder が知らせる）。
 *
 * 各フォルダの前と、取り込みが終わった後に、始めたときと同じカタログが開いているかを確かめ、違えば残りを
 * やめる（閉じた・切り替えた・復元した後に、残りを別のカタログへ送らないため）。取り込みが取り消された
 * ときも残りをやめる（利用者の取り消しのほか、カタログを閉じるときにコアが取り消して、閉じ終わる前に
 * 知らせてくることがあるため）。
 */
async function importFolders(folders: readonly string[], recursive: boolean): Promise<void> {
  const session = catalogSession;
  const stop = (reason: string, rest: number) => {
    if (rest > 0) notify("info", `${reason}ため、残りの ${rest.toLocaleString()} 個のフォルダは取り込みませんでした`);
  };
  for (let i = 0; i < folders.length; i++) {
    if (session !== catalogSession || appStore.get().catalog === null) {
      stop("カタログを閉じた", folders.length - i);
      return;
    }
    const info = await importFolder(folders[i], recursive);
    const rest = folders.length - i - 1;
    if (session !== catalogSession) {
      stop("カタログを閉じた", rest);
      return;
    }
    if (info?.state === "cancelled") {
      stop("取り込みが取り消された", rest);
      return;
    }
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
    const seq = ++modeSeq;
    const s = appStore.get();
    if (mode !== "grid" && s.selection.current === null && searchCache.count > 0) {
      // 写真を選んでいる途中（クリックの直後など）なら、その結果を待つ。まだ何も選ばれていなければ先頭の写真を
      // 選んでから切り替える（先に先頭を選び直すと、選んでいる途中の写真を打ち消してしまう）。選べなかった・
      // その間に別の切り替えがあったときは切り替えない。
      void (async () => {
        await pendingSelect;
        if (seq !== modeSeq) return;
        if (appStore.get().selection.current === null) {
          const ok = await selectIndex(0);
          if (!ok || seq !== modeSeq) return;
        }
        appStore.set({ mode });
      })();
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
  /**
   * どこを見るか（フォルダ・コレクション・すべての写真）を切り替える。フォルダとコレクションはどちらか
   * 1 つ（Lightroom の「ソース」と同じ）で、ほかの絞り込みの条件は残す。
   */
  showFolder(folderId: number, includeSubfolders: boolean) {
    actions.setFilter({ folder_id: folderId, include_subfolders: includeSubfolders, collection_id: null });
  },
  showCollection(collectionId: CollectionId) {
    actions.setFilter({ collection_id: collectionId, folder_id: null });
  },
  showAll() {
    actions.setFilter({ folder_id: null, collection_id: null });
  },
  setSort(sort: SearchSort) {
    appStore.set({ sort });
    void runSearch();
  },
  // 選択
  selectIndex,
  moveSelection,
  /** 位置の写真を選んでルーペで開く（グリッドのダブルクリック）。選べなかったら開かない（戻り値は開いたか）。 */
  async openInLoupe(index: number, expect?: VariantId): Promise<boolean> {
    const seq = ++modeSeq;
    const ok = await selectIndex(index, { expect });
    if (ok && seq === modeSeq) appStore.set({ mode: "loupe" });
    return ok && seq === modeSeq;
  },
  kindOf,
  /** 選択中の写真の概要（選んだときの項目。順は決まっていない）。 */
  selectedSummaries: (): Iterable<SelectedSummary> => selectedSummaries.values(),
  async selectAll() {
    const seq = ++selectSeq;
    const n = Math.min(searchCache.count, 10_000);
    // 今の世代の項目だけを使う（キャッシュのページの数には上限があるので、読んだ値をそのまま使う）。
    const items = await searchCache.items(0, n);
    if (seq !== selectSeq) return;
    rememberSelected(items);
    const ids = items.map((v) => v.variant_id);
    appStore.set((s) => ({ selection: { ...s.selection, ids } }));
    if (searchCache.count > n) notify("info", `先頭の ${n.toLocaleString()} 件を選びました`);
  },
  selectNone: () => {
    selectSeq++;
    appStore.set((s) => ({
      selection: { ...s.selection, ids: s.selection.current !== null ? [s.selection.current] : [] },
    }));
  },
  targets,
  pasteTargets,
  // 選別（LIB-04・05）
  setRating(rating: Rating) {
    return enqueueMark(async () => {
      const ids = targets();
      if (ids.length === 0) return;
      try {
        await core.setRating(ids, rating);
        await afterMark();
      } catch (e) {
        reportError(e, "評価を変えられません");
      }
    });
  },
  setFlag(flag: Flag) {
    return enqueueMark(async () => {
      const ids = targets();
      if (ids.length === 0) return;
      try {
        await core.setFlag(ids, flag);
        await afterMark();
      } catch (e) {
        reportError(e, "フラグを変えられません");
      }
    });
  },
  setColorLabel(label: ColorLabel | null) {
    return enqueueMark(() => applyColorLabel(targets(), label));
  },
  /**
   * カラーラベルのキー（6〜9）: 対象の写真がすべてそのラベルなら外し（なしにする）、そうでなければ付ける
   * （Lightroom と同じ。混在するときは付ける）。今のラベルは読み込み済みの検索結果で調べる（読み込んで
   * いない写真があれば、付ける）。
   */
  toggleColorLabel(label: ColorLabel) {
    return enqueueMark(() => {
      const ids = targets();
      return applyColorLabel(ids, nextColorLabel(label, loadedColorLabels(ids)));
    });
  },
  // 取り込み
  importFolder,
  importFolders,
  setClipboard: (clipboard: AppState["clipboard"]) => appStore.set({ clipboard }),
  copySettingsOf,
};
