// テスト用の Bridge（invoke・listen の記録と応答の差し替え）と、画面のテストに使う応答の見本。

import type { Bridge, Unlisten } from "./bridge";
import type {
  BeforeInfo,
  CatalogSchema,
  ClearReport,
  CollectionInfo,
  CrashReportSummary,
  FrameInfo,
  IptcMetadata,
  KeywordInfo,
  KeywordUsage,
  LogFolderInfo,
  LogStatus,
  MetadataSummary,
  SmartRule,
  VariantKeywords,
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
  schema_version: 8,
  application_id: 0x475a4354,
  public_views: ["photos_v1", "photo_keywords_v1", "photo_collections_v1"],
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
