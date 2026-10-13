// テスト用の Bridge（invoke・listen の記録と応答の差し替え）と、画面のテストに使う応答の見本。

import type { Bridge, Unlisten } from "./bridge";
import type {
  AutoStackReport,
  BeforeInfo,
  CatalogSchema,
  ClearReport,
  CollectionInfo,
  ContentHashStatus,
  CoreSettings,
  CrashReportSummary,
  DuplicateGroupList,
  FrameInfo,
  ImportPreferences,
  ImportReport,
  IptcMetadata,
  KeywordInfo,
  KeywordUsage,
  LogFolderInfo,
  LogStatus,
  MetadataSummary,
  NamePreview,
  SmartRule,
  StackInfo,
  VariantDetails,
  VariantKeywords,
  VariantSummary,
} from "./types";

export interface MockCall {
  command: string;
  args?: Record<string, unknown>;
}

export class MockBridge implements Bridge {
  calls: MockCall[] = [];
  handlers = new Map<string, Set<(p: unknown) => void>>();
  /** コマンドごとの応答（関数なら引数から作る）。 */
  responses = new Map<string, unknown | ((args?: Record<string, unknown>) => unknown)>();
  /** 応答を遅らせる（ミリ秒。順序のテスト用）。 */
  delay: (call: MockCall) => number = () => 0;

  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    const call = { command, args };
    this.calls.push(call);
    const ms = this.delay(call);
    if (ms > 0) await new Promise((r) => setTimeout(r, ms));
    const r = this.responses.get(command);
    const v = typeof r === "function" ? (r as (a?: Record<string, unknown>) => unknown)(args) : r;
    if (v instanceof Error) throw v;
    return v as T;
  }

  async invokeBinary(command: string, args?: Record<string, unknown>): Promise<ArrayBuffer> {
    return this.invoke<ArrayBuffer>(command, args);
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> {
    const set = this.handlers.get(event) ?? new Set();
    set.add(handler as (p: unknown) => void);
    this.handlers.set(event, set);
    return () => set.delete(handler as (p: unknown) => void);
  }

  emit(event: string, payload: unknown): void {
    for (const h of this.handlers.get(event) ?? []) h(payload);
  }

  /** core_execute で送ったコマンドの type の並び。 */
  coreCommands(): string[] {
    return this.calls
      .filter((c) => c.command === "core_execute")
      .map((c) => (c.args?.command as { type: string }).type);
  }
}

// ---------------------------------------------------------------------------
// ログ・クラッシュレポート（SYS-07）とカタログの構造（ORG-07）の見本（画面のテスト用）
// ---------------------------------------------------------------------------

const MOCK_LOG_DIR = "/home/u/.local/share/io.github.tokamocchi.genzoparis/logs";

/** ログの状態の見本（app_status の logs）。 */
export const MOCK_LOG_STATUS: LogStatus = {
  dir: MOCK_LOG_DIR,
  file: `${MOCK_LOG_DIR}/app.log`,
  file_enabled: true,
  problems: [],
};

/** クラッシュレポートの見本。 */
export const MOCK_CRASH_REPORT: CrashReportSummary = {
  path: `${MOCK_LOG_DIR}/crash/crash-20261012T010203456Z-app-4242.txt`,
  file_name: "crash-20261012T010203456Z-app-4242.txt",
  created_at: "2026-10-12T01:02:03.456Z",
  program: "app",
  message: "index out of bounds: the len is 3 but the index is 7",
};

/** ログのフォルダの中身の見本。 */
export const MOCK_LOG_FOLDER: LogFolderInfo = {
  dir: MOCK_LOG_DIR,
  exists: true,
  files: [
    { name: "app.1.log", bytes: 5_242_880, modified: "2026-10-11T09:00:00Z" },
    { name: "app.log", bytes: 120_000, modified: "2026-10-12T01:02:03Z" },
  ],
  crash_reports: [MOCK_CRASH_REPORT],
  total_bytes: 5_242_880 + 120_000 + 8_000,
  current_file: `${MOCK_LOG_DIR}/app.log`,
};

/** カタログの構造の見本（一部の表・ビューだけ）。 */
export const MOCK_CATALOG_SCHEMA: CatalogSchema = {
  schema_version: 10,
  application_id: 0x475a4354,
  public_views: ["photos_v1", "photo_keywords_v1", "photo_collections_v1", "photo_stacks_v1", "photo_files_v1"],
  tables: [
    {
      name: "asset",
      kind: "table",
      columns: [
        { name: "id", decl_type: "INTEGER", not_null: false, default_value: null, primary_key: 1 },
        { name: "kind", decl_type: "TEXT", not_null: true, default_value: null, primary_key: 0 },
      ],
    },
    {
      name: "photo_keywords_v1",
      kind: "view",
      columns: [
        { name: "variant_id", decl_type: "INTEGER", not_null: false, default_value: null, primary_key: 0 },
        { name: "keyword_path", decl_type: "TEXT", not_null: false, default_value: null, primary_key: 0 },
      ],
    },
    {
      name: "photos_v1",
      kind: "view",
      columns: [
        { name: "variant_id", decl_type: "INTEGER", not_null: false, default_value: null, primary_key: 0 },
        { name: "file_name", decl_type: "TEXT", not_null: false, default_value: null, primary_key: 0 },
      ],
    },
  ],
  objects: [
    { kind: "table", name: "asset", table: "asset", sql: "CREATE TABLE asset (id INTEGER PRIMARY KEY, kind TEXT NOT NULL)" },
    { kind: "view", name: "photos_v1", table: "photos_v1", sql: "CREATE VIEW photos_v1 AS SELECT 1" },
    { kind: "view", name: "photo_keywords_v1", table: "photo_keywords_v1", sql: "CREATE VIEW photo_keywords_v1 AS SELECT 1" },
    { kind: "index", name: "asset_captured_at", table: "asset", sql: "CREATE INDEX asset_captured_at ON asset(captured_at_utc)" },
  ],
};

/** ログのコマンド（log_folder_info・open_log_folder・clear_logs・report_ui_error）の応答を入れる。 */
export function mockLogCommands(b: MockBridge): void {
  b.responses.set("log_folder_info", MOCK_LOG_FOLDER);
  b.responses.set("open_log_folder", undefined);
  b.responses.set("clear_logs", { removed: 2, truncated: 1, failed: [] } satisfies ClearReport);
  b.responses.set("report_ui_error", undefined);
}

// ---------------------------------------------------------------------------
// v1 の view: 説明のメタデータ（LIB-15）とビフォー / アフター（DEV-31）の見本（画面のテスト用）
// ---------------------------------------------------------------------------

/** 説明のメタデータの見本（詳細の metadata）。 */
export const MOCK_IPTC: IptcMetadata = {
  title: "清水寺の朝",
  headline: null,
  caption: "朝の清水の舞台\n京都",
  creator: "山田 太郎",
  creator_email: "taro@example.com",
  copyright: "© 2024 山田 太郎",
  copyright_status: "copyrighted",
  city: "京都市",
  country: "日本",
};

/** 複数の写真のまとめの見本（作成者は共通、タイトルは写真によって違う）。 */
export const MOCK_METADATA_SUMMARY: MetadataSummary = {
  assets: 3,
  fields: [
    { field: "title", value: null, mixed: true },
    { field: "headline", value: null, mixed: false },
    { field: "caption", value: null, mixed: true },
    { field: "creator", value: "山田 太郎", mixed: false },
    { field: "copyright", value: "© 2024 山田 太郎", mixed: false },
    { field: "copyright_status", value: null, mixed: true },
    { field: "city", value: "京都市", mixed: false },
  ],
};

/** ビフォーの状態の見本（取り込んだ直後の設定）。 */
export const MOCK_BEFORE_INFO: BeforeInfo = {
  variant_id: 1,
  source: { kind: "initial" },
  label: "読み込み",
  process_version: 1,
  generation: 7,
};

/** 画素のバイト列（frames.ts の形）を作る（ヘッダーの JSON と、幅 × 高さの RGBA）。 */
export function mockFrameBytes(header: Partial<FrameInfo> & { width: number; height: number }): ArrayBuffer {
  const json = new TextEncoder().encode(JSON.stringify(header));
  const out = new Uint8Array(4 + json.length + header.width * header.height * 4);
  new DataView(out.buffer).setUint32(0, json.length, true);
  out.set(json, 4);
  out.fill(255, 4 + json.length);
  return out.buffer;
}

/** ビフォーの最新のプレビューの見本（before_frame の応答）。 */
export function mockBeforeFrame(variantId: number, generation: number): ArrayBuffer {
  return mockFrameBytes({
    variant_id: variantId,
    generation,
    width: 4,
    height: 3,
    output_width: 40,
    output_height: 30,
  });
}

// ---------------------------------------------------------------------------
// v1 の organize-a: キーワード（LIB-09）・コレクション（LIB-10）・スマートコレクション（LIB-11）の見本
// ---------------------------------------------------------------------------

/** キーワードの一覧の見本（場所 > 京都 > 祇園、場所 > 東京、人物 > 家族。家族は書き出さない）。 */
export const MOCK_KEYWORDS: KeywordInfo[] = [
  { keyword_id: 1, parent_id: null, name: "場所", include_on_export: true, photo_count: 0 },
  { keyword_id: 2, parent_id: 1, name: "京都", include_on_export: true, photo_count: 12 },
  { keyword_id: 3, parent_id: 2, name: "祇園", include_on_export: true, photo_count: 4 },
  { keyword_id: 4, parent_id: 1, name: "東京", include_on_export: true, photo_count: 7 },
  { keyword_id: 5, parent_id: null, name: "人物", include_on_export: true, photo_count: 0 },
  { keyword_id: 6, parent_id: 5, name: "家族", include_on_export: false, photo_count: 20 },
];

/** 選択中の 3 枚のキーワードの見本（京都はすべて、家族は 1 枚だけ）。 */
export const MOCK_VARIANT_KEYWORDS: VariantKeywords = {
  variants: 3,
  keywords: [
    { keyword_id: 2, name: "京都", path: ["場所", "京都"], count: 3 },
    { keyword_id: 6, name: "家族", path: ["人物", "家族"], count: 1 },
  ],
};

/** スマートコレクションの条件の見本（★3 以上 かつ「場所」を含む）。 */
export const MOCK_SMART_RULE: SmartRule = {
  version: 1,
  match: "all",
  rules: [
    { type: "rating", op: "gte", value: 3 },
    { type: "keyword", op: "has", keyword_id: 1 },
  ],
};

/** コレクションの一覧の見本（旅行 > 京都 2024（手動）・旅行 > ★3 以上の場所（スマート）・お気に入り（手動））。 */
export const MOCK_COLLECTIONS: CollectionInfo[] = [
  { collection_id: 10, parent_id: null, name: "旅行", kind: "set", rule: null, rule_error: null, count: 30 },
  { collection_id: 11, parent_id: 10, name: "京都 2024", kind: "manual", rule: null, rule_error: null, count: 25 },
  { collection_id: 12, parent_id: 10, name: "★3 以上の場所", kind: "smart", rule: MOCK_SMART_RULE, rule_error: null, count: 8 },
  { collection_id: 13, parent_id: null, name: "お気に入り", kind: "manual", rule: null, rule_error: null, count: 3 },
];

/** キーワードの使われ方の見本（削除の確認）。 */
export const MOCK_KEYWORD_USAGE: KeywordUsage = {
  photos: 0,
  photos_with_descendants: 23,
  descendants: 3,
  smart_collections: [{ collection_id: 12, name: "★3 以上の場所" }],
};

// ---------------------------------------------------------------------------
// v1 の organize-b: スタック（LIB-12）・仮想コピー（LIB-13）の見本
// ---------------------------------------------------------------------------

/** グリッドのセルの見本（写真 1 枚。stack・is_master などを上書きして使う）。 */
export function mockSummary(over: Partial<VariantSummary> = {}): VariantSummary {
  return {
    variant_id: 1,
    asset_id: 1,
    is_master: true,
    variant_name: null,
    kind: "photo",
    file_id: 1,
    file_name: "DSC00001.ARW",
    folder_id: 1,
    file_status: "ok",
    file_revision: 1,
    has_sidecar_jpeg: false,
    rating: 0,
    flag: 0,
    color_label: null,
    capture: {
      raw: "2024:05:01 12:00:00",
      offset: "+09:00",
      tz_source: "exif",
      tz_assumed: "+09:00",
      correction_s: 0,
      utc: "2024-05-01T03:00:00.000Z",
    },
    camera: "SONY ILCE-7M4",
    lens: null,
    width: 7008,
    height: 4672,
    orientation: 1,
    develop_hash: "0".repeat(64),
    thumb_rev: null,
    stack: null,
    ...over,
  };
}

/** スタックの見本（連写 3 枚。一番上は 2。折りたたみ中）。 */
export const MOCK_STACK: StackInfo = {
  stack_id: 5,
  top_variant_id: 2,
  collapsed: true,
  members: [2, 1, 3],
};

/** 折りたたんだスタックの代表のセルの見本。 */
export const MOCK_STACK_TOP: VariantSummary = mockSummary({
  variant_id: 2,
  file_name: "DSC00002.ARW",
  stack: { stack_id: 5, position: 1, count: 3, collapsed: true },
});

/** 仮想コピーのセルの見本（マスター 1 と同じスタックの 2 枚目。展開中）。 */
export const MOCK_VIRTUAL_COPY: VariantSummary = mockSummary({
  variant_id: 4,
  is_master: false,
  variant_name: "白黒",
  stack: { stack_id: 6, position: 2, count: 2, collapsed: false },
});

/** 仮想コピーの詳細の見本（同じ写真のマスターと仮想コピー、入っているスタック）。 */
export const MOCK_VIRTUAL_COPY_DETAILS: VariantDetails = {
  summary: MOCK_VIRTUAL_COPY,
  iso: 100,
  aperture: 2.8,
  shutter_s: 0.004,
  focal_mm: 35,
  gps: null,
  caption: null,
  video: null,
  files: [],
  variants: [
    { variant_id: 1, is_master: true, name: null },
    { variant_id: 4, is_master: false, name: "白黒" },
  ],
  stack: { stack_id: 6, top_variant_id: 1, collapsed: false, members: [1, 4] },
};

/** 自動スタックの見込みの見本（間隔 2 秒）。 */
export const MOCK_AUTO_STACK_PREVIEW: AutoStackReport = {
  stacks: 12,
  photos: 41,
  considered: 230,
  skipped_in_stacks: 6,
  skipped_without_time: 1,
  dry_run: true,
};

// ---------------------------------------------------------------------------
// v1 の identity: 内容による同一性（ORG-03）・重複（IMP-04）・RAW と JPEG のペア（IMP-06）の見本
// ---------------------------------------------------------------------------

/** カタログの設定の見本（genzo-api の既定の値）。 */
export const MOCK_CORE_SETTINGS: CoreSettings = {
  preview_cache_dir: "/home/u/Pictures/GenzoParis/catalog-data/previews",
  preview_cache_bytes: 20_000_000_000,
  default_export_color_space: "srgb",
  default_utc_offset_minutes: 540,
  import_skip_duplicates: true,
  import_separate_raw_jpeg: false,
  hash_in_background: true,
  hash_network_volumes: false,
};

/** RAW と同時記録の JPEG を 1 枚にまとめた写真のセルの見本。 */
export const MOCK_PAIR: VariantSummary = mockSummary({
  variant_id: 7,
  asset_id: 7,
  file_name: "DSC00007.ARW",
  has_sidecar_jpeg: true,
});

/** 内容のハッシュの状況の見本（計算の途中。重複の組が 2 つ）。 */
export const MOCK_CONTENT_HASH_STATUS: ContentHashStatus = {
  files: 1300,
  hashed: 1250,
  pending: 50,
  duplicate_groups: 2,
  job_id: null,
};

/** 内容が同じファイルの組の見本。 */
export const MOCK_DUPLICATE_GROUPS: DuplicateGroupList = {
  total: 2,
  groups: [
    {
      full_hash: "a".repeat(64),
      files: [
        { file_id: 1, asset_id: 1, variant_id: 1, role: "primary", path: "/p/2024/DSC00001.ARW", size: 25_000_000, status: "ok" },
        { file_id: 9, asset_id: 9, variant_id: 9, role: "primary", path: "/p/コピー/DSC00001.ARW", size: 25_000_000, status: "ok" },
      ],
    },
    {
      full_hash: "b".repeat(64),
      files: [
        { file_id: 2, asset_id: 2, variant_id: 2, role: "sidecar_jpeg", path: "/p/2024/DSC00002.JPG", size: 4_000_000, status: "ok" },
        { file_id: 12, asset_id: 12, variant_id: 12, role: "primary", path: "/p/書き出し前/DSC00002.JPG", size: 4_000_000, status: "missing" },
      ],
    },
  ],
};

/** 取り込みの結果の見本（重複 2 件を取り込まず、RAW と JPEG を 3 組まとめた）。 */
export const MOCK_IMPORT_REPORT: ImportReport = {
  root: "/p/SD",
  files_found: 12,
  added: 10,
  updated: 0,
  unchanged: 0,
  errors: [],
  not_registered: [],
  skipped_links: 0,
  thumbnails: 7,
  thumbnail_failures: [],
  cancelled: false,
  duplicates: [
    { path: "/p/SD/DSC00001.ARW", existing_path: "/p/2024/DSC00001.ARW", existing_variant_id: 1, skipped: true },
    { path: "/p/SD/DSC00002.JPG", existing_path: "/p/2024/DSC00002.JPG", existing_variant_id: 2, skipped: true },
  ],
  paired: 3,
  skip_duplicates: true,
  separate_raw_jpeg: false,
};

// ---------------------------------------------------------------------------
// コピー取り込み（IMP-02）・リネーム規則（IMP-03）・まとめて付けるもの（IMP-05）の見本
// ---------------------------------------------------------------------------

/** アプリに保存した取り込みの設定の見本（コピー・撮影日別・リネーム・作成者とキーワード）。 */
export const MOCK_IMPORT_PREFERENCES: ImportPreferences = {
  copy: true,
  recursive: true,
  destination: "/home/u/写真",
  folder_rule: "year_date",
  rename: true,
  rename_rule: { template: "{date}_{text}-{seq}", custom_text: "京都", sequence_start: 1, sequence_digits: 4 },
  metadata: { keywords: [["場所", "京都"], ["家族"]], fields: { creator: "岡本", copyright: "© 2026 岡本" } },
};

/** 名前の例の見本（genzo-api の preview_names と同じ見本の写真。上の設定のとき）。 */
export const MOCK_NAME_PREVIEW: NamePreview = {
  examples: [
    { source: "DCIM/100MSDCF/DSC01234.ARW", destination: "2026/2026-10-13/20261013_京都-0001.ARW", captured: "2026-10-13 14:30:52" },
    { source: "DCIM/100MSDCF/DSC01234.JPG", destination: "2026/2026-10-13/20261013_京都-0001.JPG", captured: "2026-10-13 14:30:52" },
    { source: "DCIM/100MSDCF/DSC01235.ARW", destination: "2026/2026-10-13/20261013_京都-0002.ARW", captured: "2026-10-13 14:30:53" },
    { source: "DCIM/DJI_001/DJI_0001.MP4", destination: "2026/2026-10-14/20261014_京都-0003.MP4", captured: "2026-10-14 09:05:00" },
  ],
  error: null,
};

/** コピー取り込みの結果の見本（RAW と JPEG の組 1 つと JPEG 1 枚をコピーし、1 枚は既にあったものを登録）。 */
export const MOCK_COPY_IMPORT_REPORT: ImportReport = {
  root: "/media/u/SD",
  files_found: 3,
  added: 3,
  updated: 0,
  unchanged: 0,
  errors: [],
  not_registered: [],
  skipped_links: 0,
  thumbnails: 2,
  thumbnail_failures: [],
  cancelled: false,
  duplicates: [],
  paired: 1,
  skip_duplicates: true,
  separate_raw_jpeg: false,
  tagged: 2,
  copy: {
    destination: "/home/u/写真",
    copied: [
      {
        source: "/media/u/SD/DCIM/100MSDCF/DSC01234.ARW",
        destination: "/home/u/写真/2026/2026-10-13/20261013_京都-0001.ARW",
        existing: false,
      },
      {
        source: "/media/u/SD/DCIM/100MSDCF/DSC01234.JPG",
        destination: "/home/u/写真/2026/2026-10-13/20261013_京都-0001.JPG",
        existing: false,
      },
      {
        source: "/media/u/SD/DCIM/100MSDCF/DSC01235.JPG",
        destination: "/home/u/写真/2026/2026-10-13/20261013_京都-0002.JPG",
        existing: true,
      },
    ],
    bytes_copied: 31_457_280,
    dated_by_file_time: 0,
    dry_run: false,
    next_sequence: 3,
  },
};
