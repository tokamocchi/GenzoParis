// テスト用の Bridge（invoke・listen の記録と応答の差し替え）と、画面のテストに使う応答の見本。

import type { Bridge, Unlisten } from "./bridge";
import type {
  AutoStackReport,
  BeforeInfo,
  CaptureDevice,
  CaptureTimeEditInfo,
  CaptureTimePlan,
  CatalogSchema,
  ClearReport,
  CollectionInfo,
  ContentHashStatus,
  CoreSettings,
  CrashReportSummary,
  DevelopSettings,
  DevelopState,
  DuplicateGroupList,
  FileMovePlan,
  FileMoveReport,
  FileRenamePlan,
  FolderInfo,
  MissingCheckReport,
  RelinkSearch,
  RelocatePlan,
  VolumeInfo,
  FrameInfo,
  ImportPreferences,
  ImportReport,
  IptcMetadata,
  KeywordInfo,
  KeywordUsage,
  LensCorrectionInfo,
  LogFolderInfo,
  LogStatus,
  MetadataSummary,
  NamePreview,
  RegionInfo,
  RegionRequest,
  SmartRule,
  SourceInfo,
  StackInfo,
  Timeline,
  TimelineGroup,
  VariantDetails,
  VariantKeywords,
  VariantSummary,
  VideoPlayback,
  VideoProxyCacheStatus,
  ViewOutputSize,
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
  schema_version: 12,
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
  process_version: 2,
  generation: 7,
};

/** 埋め込みのレンズ補正の情報の見本（DEV-14。Sony の ARW）。 */
export const MOCK_LENS_CORRECTION_INFO: LensCorrectionInfo = {
  source: "sony_arw",
  description: "Sony の ARW（歪曲・周辺減光・色収差）",
  distortion: true,
  vignetting: true,
  chromatic_aberration: true,
};

/** 現像設定の見本（genzo-model の DevelopSettings::default の JSON。スキーマ版 2・処理バージョン 2）。 */
export function mockDevelopSettings(over: Partial<DevelopSettings> = {}): DevelopSettings {
  return {
    schema_version: 2,
    process_version: 2,
    white_balance: "as_shot",
    highlight_recovery: { method: "clip", amount: 100 },
    exposure_ev: 0,
    contrast: 0,
    tone: { highlights: 0, shadows: 0, whites: 0, blacks: 0 },
    tone_curve: {
      parametric: { highlights: 0, lights: 0, darks: 0, shadows: 0 },
      luma: [],
      red: [],
      green: [],
      blue: [],
    },
    color: { saturation: 0, vibrance: 0 },
    geometry: { crop: { left: 0, top: 0, right: 1, bottom: 1 }, angle_deg: 0, aspect: null },
    lens: {
      profile_enabled: false,
      distortion_scale: 100,
      vignetting_scale: 100,
      remove_chromatic_aberration: false,
      distortion: 0,
      vignetting_amount: 0,
      vignetting_midpoint: 50,
      ca_red_cyan: 0,
      ca_blue_yellow: 0,
    },
    detail: {
      sharpening: { amount: 0, radius: 1, detail: 0, masking: 0 },
      noise_reduction: { luminance: 0, luminance_detail: 50, luminance_contrast: 0, color: 0, color_detail: 50 },
    },
    masks: [],
    render_deps: { camera_profile: { id: "", sha256: "" }, lens_profile: null, raw_decoder: "" },
    ...over,
  };
}

/**
 * 設定の貼り付け（paste_settings）・処理バージョンのまとめての更新（upgrade_process_versions）の応答を足す（DEV-30・
 * DATA-09）。`versions` は写真ごとの処理バージョン（無い写真は 2）。コアと同じく、貼り付けは版を変えず
 * （process_version が "upgrade" なら貼り付け元の版まで上げる）、版 1 の写真に v1 の機能の項目（レンズ補正の
 * プロファイル・手動の歪曲・ノイズ軽減の輝度）を写したら needs_upgrade に数える。送ったコマンドは `sent` に残す。
 */
export function mockPasteCommands(
  b: MockBridge,
  versions: Map<number, number> = new Map(),
): { sent: Record<string, unknown>[] } {
  const sent: Record<string, unknown>[] = [];
  const base = b.responses.get("core_execute");
  const versionOf = (id: number) => versions.get(id) ?? 2;
  b.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    switch (c.type) {
      case "paste_settings": {
        sent.push(c);
        const settings = c.settings as DevelopSettings;
        const ids = [...new Set(c.targets as number[])];
        const usesV1Features =
          settings.lens.profile_enabled || settings.lens.distortion !== 0 || settings.detail.noise_reduction.luminance !== 0;
        let upgraded = 0;
        let needs = 0;
        for (const id of ids) {
          const before = versionOf(id);
          const after = c.process_version === "upgrade" ? Math.max(before, settings.process_version) : before;
          if (after > before) upgraded++;
          versions.set(id, after);
          if (after < 2 && usesV1Features) needs++;
        }
        return { type: "pasted", count: ids.length, upgraded, needs_upgrade: needs };
      }
      case "upgrade_process_versions": {
        sent.push(c);
        let count = 0;
        for (const id of new Set(c.variant_ids as number[])) {
          if (versionOf(id) < 2) {
            versions.set(id, 2);
            count++;
          }
        }
        return { type: "updated", count };
      }
      default:
        return typeof base === "function" ? (base as (a?: Record<string, unknown>) => unknown)(args) : base;
    }
  });
  return { sent };
}

/** 現像する写真の元の画像の情報の見本（RAW。埋め込みのレンズ補正の情報つき）。 */
export function mockSourceInfo(over: Partial<SourceInfo> = {}): SourceInfo {
  return {
    is_raw: true,
    width: 6000,
    height: 4000,
    decoder: "libraw-0.21.2",
    as_shot_white_balance: { temperature_k: 5100, tint: 8 },
    lens_correction: MOCK_LENS_CORRECTION_INFO,
    lens_correction_note: null,
    ...over,
  };
}

/** 現像の状態の見本（genzo-api の DevelopState）。 */
export function mockDevelopState(over: Partial<DevelopState> = {}): DevelopState {
  return {
    variant_id: 1,
    settings: mockDevelopSettings(),
    generation: 1,
    can_undo: false,
    can_redo: false,
    dirty: false,
    dragging: false,
    source: mockSourceInfo(),
    before: null,
    latest_process_version: 2,
    ...over,
  };
}

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
  video_proxy_cache_bytes: 5_000_000_000,
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

// ---------------------------------------------------------------------------
// v1 の time: 撮影日時の一括修正（LIB-16）・タイムライン（ORG-01）の見本
// ---------------------------------------------------------------------------

/** 機材の一覧の見本（α7 IV（シリアルあり）・α7C の写真と、機材の情報のない動画（DJI など））。 */
export const MOCK_CAPTURE_DEVICES: CaptureDevice[] = [
  {
    camera: "SONY ILCE-7C",
    serial: null,
    kind: "photo",
    assets: 320,
    with_time: 320,
    adjusted: 0,
    first: "2024-05-01T09:00:00+09:00",
    last: "2024-05-03T18:20:00+09:00",
  },
  {
    camera: "SONY ILCE-7M4",
    serial: "4012345",
    kind: "photo",
    assets: 1200,
    with_time: 1198,
    adjusted: 0,
    first: "2024-05-01T08:58:10+09:00",
    last: "2024-05-03T19:02:00+09:00",
  },
  {
    camera: null,
    serial: null,
    kind: "video",
    assets: 42,
    with_time: 42,
    adjusted: 0,
    first: "2024-05-01T18:10:00+09:00",
    last: "2024-05-04T03:30:00+09:00",
  },
];

/** 基準の写真に合わせる修正の見込みの見本（α7 IV の 1,200 枚を +1 時間 2 分 3 秒ずらす）。 */
export const MOCK_CAPTURE_PLAN: CaptureTimePlan = {
  assets: 1200,
  changed: 1198,
  unchanged: 0,
  without_time: 2,
  sources: { exif: 1198, utc_only: 0, user_default: 0, user_set: 0 },
  shift_seconds: 3723,
  examples: [
    {
      asset_id: 1,
      file_name: "DSC00001.ARW",
      before: {
        raw: "2024:05:01 08:58:10",
        offset: "+09:00",
        tz_source: "exif",
        tz_assumed: "+09:00",
        correction_s: 0,
        utc: "2024-04-30T23:58:10.000Z",
      },
      after: {
        raw: "2024:05:01 08:58:10",
        offset: "+09:00",
        tz_source: "exif",
        tz_assumed: "+09:00",
        correction_s: 3723,
        utc: "2024-05-01T01:00:13.000Z",
      },
    },
  ],
  edit_id: null,
};

/** 撮影日時の修正の履歴の見本（新しい順）。 */
export const MOCK_CAPTURE_EDITS: CaptureTimeEditInfo[] = [
  {
    id: 2,
    kind: "set_zone",
    shift_seconds: null,
    zone: "+09:00",
    zone_mode: "keep_local",
    asset_count: 42,
    created_at: "2026-10-13T01:00:00.000Z",
  },
  {
    id: 1,
    kind: "sync",
    shift_seconds: 3723,
    zone: null,
    zone_mode: null,
    asset_count: 1198,
    created_at: "2026-10-13T00:50:00.000Z",
  },
];

/** まとまりの見本を作る。 */
export function mockTimelineGroup(over: Partial<TimelineGroup> = {}): TimelineGroup {
  return {
    start: 0,
    count: 1,
    day: "2024-05-01",
    new_day: true,
    first: "2024-05-01T09:00:00",
    last: "2024-05-01T09:00:00",
    photos: 1,
    videos: 0,
    ...over,
  };
}

/**
 * タイムラインの見本（10 件: 5/1 の朝のイベント 4 枚と夕方 3 枚（動画 1）、5/2 の 2 枚、撮影日時なし 1 枚）。
 */
export const MOCK_TIMELINE: Timeline = {
  generation: 1,
  gap_seconds: 7200,
  groups: [
    mockTimelineGroup({ start: 0, count: 4, first: "2024-05-01T09:00:00", last: "2024-05-01T09:40:00", photos: 4 }),
    mockTimelineGroup({
      start: 4,
      count: 3,
      new_day: false,
      first: "2024-05-01T17:10:00",
      last: "2024-05-01T18:05:00",
      photos: 2,
      videos: 1,
    }),
    mockTimelineGroup({
      start: 7,
      count: 2,
      day: "2024-05-02",
      first: "2024-05-02T10:00:00",
      last: "2024-05-02T10:30:00",
      photos: 2,
    }),
    mockTimelineGroup({ start: 9, count: 1, day: null, new_day: false, first: null, last: null, photos: 1 }),
  ],
  events_merged: false,
  days: 2,
  without_time: 1,
};

// ---------------------------------------------------------------------------
// v1 の files: ボリューム（FILE-03）・ファイル操作（FILE-02）・見つからない写真と再リンク（FILE-04）の見本
// ---------------------------------------------------------------------------

/** ボリュームの見本（内蔵はつながっている、SD カードはつながっていない）。 */
export const MOCK_VOLUMES: VolumeInfo[] = [
  {
    volume_id: 1,
    uuid: "uuid:1b2c3d4e-0000-4000-8000-000000000001",
    label: null,
    mount_path: "/",
    online: true,
    identified: true,
    network: false,
    fs_type: "ext4",
    last_seen_at: "2026-10-13T01:00:00.000Z",
  },
  {
    volume_id: 2,
    uuid: "uuid:1234-ABCD",
    label: "SD カード",
    mount_path: "/media/me/SD",
    online: false,
    identified: true,
    network: false,
    fs_type: "exfat",
    last_seen_at: "2026-10-01T09:00:00.000Z",
  },
];

/** フォルダの見本（つながっている・つながっていない・見つからない）。 */
export const MOCK_FOLDERS_FILES: FolderInfo[] = [
  { folder_id: 11, volume_id: 1, parent_id: 10, rel_path: "home/me/写真", path: "/home/me/写真", online: true, missing: false, missing_files: 2 },
  { folder_id: 21, volume_id: 2, parent_id: 20, rel_path: "DCIM", path: "/media/me/SD/DCIM", online: false, missing: false, missing_files: 0 },
  { folder_id: 12, volume_id: 1, parent_id: 11, rel_path: "home/me/写真/旅行", path: "/home/me/写真/旅行", online: true, missing: true, missing_files: 5 },
];

/** 写真の移動の予定の見本（1 枚は別のドライブへコピー、1 枚は名前が重なって移せない）。 */
export const MOCK_FILE_MOVE_PLAN: FileMovePlan = {
  dest_folder: 31,
  dest_path: "/mnt/外付け/選別済み",
  movable: 1,
  copy_bytes: 52_428_800,
  photos: [
    {
      variant_id: 1,
      asset_id: 1,
      problem: null,
      files: [
        { file_id: 1, role: "primary", from: "/p/A.ARW", to: "/mnt/外付け/選別済み/A.ARW", method: "copy", size: 50_000_000 },
        { file_id: 2, role: "sidecar_jpeg", from: "/p/A.JPG", to: "/mnt/外付け/選別済み/A.JPG", method: "copy", size: 2_428_800 },
      ],
    },
    {
      variant_id: 2,
      asset_id: 2,
      problem: "移動先に同じ名前のもの（B.jpg）があります",
      files: [{ file_id: 3, role: "primary", from: "/p/B.jpg", to: "/mnt/外付け/選別済み/B.jpg", method: "copy", size: 1_000 }],
    },
  ],
};

/** 写真の移動の結果の見本（元のファイルの 1 つをゴミ箱へ移せなかった）。 */
export const MOCK_FILE_MOVE_REPORT: FileMoveReport = {
  moved: [
    { file_id: 1, from: "/p/A.ARW", to: "/mnt/外付け/選別済み/A.ARW" },
    { file_id: 2, from: "/p/A.JPG", to: "/mnt/外付け/選別済み/A.JPG" },
  ],
  photos: 1,
  skipped: [{ path: "/p/B.jpg", reason: "移動先に同じ名前のもの（B.jpg）があります" }],
  failed: [],
  left_sources: [{ path: "/p/A.JPG", reason: "ゴミ箱が使えません" }],
  bytes_copied: 52_428_800,
  cancelled: false,
};

/** 写真の名前の変更の予定の見本。 */
export const MOCK_FILE_RENAME_PLAN: FileRenamePlan = {
  renamable: 1,
  photos: [
    {
      variant_id: 1,
      asset_id: 1,
      problem: null,
      files: [
        { file_id: 1, role: "primary", from: "/p/DSC01.ARW", to: "/p/京都-0001.ARW", method: "rename", size: 50_000_000 },
        { file_id: 2, role: "sidecar_jpeg", from: "/p/DSC01.JPG", to: "/p/京都-0001.JPG", method: "rename", size: 2_000_000 },
      ],
    },
    {
      variant_id: 2,
      asset_id: 2,
      problem: "今と同じ名前です",
      files: [{ file_id: 3, role: "primary", from: "/p/京都-0002.jpg", to: "/p/京都-0002.jpg", method: "rename", size: 1_000 }],
    },
  ],
};

/** 見つからない写真の確認の結果の見本。 */
export const MOCK_MISSING_REPORT: MissingCheckReport = {
  checked: 120,
  newly_missing: 2,
  restored: 1,
  missing: 7,
  missing_folders: 1,
  skipped_offline: 30,
  skipped_network: 0,
  cancelled: false,
};

/** 再リンクの候補の見本（内容が同じ・名前と撮影日時だけが同じ・候補なし）。 */
export const MOCK_RELINK_SEARCH: RelinkSearch = {
  search_dir: "/mnt/新しい",
  scanned_files: 40,
  unreadable: [],
  cancelled: false,
  targets: [
    {
      file_id: 1,
      variant_id: 1,
      role: "primary",
      name: "A.ARW",
      last_path: "/p/A.ARW",
      size: 50_000_000,
      candidates: [{ path: "/mnt/新しい/A.ARW", matched: "content", size: 50_000_000, same_name: true }],
    },
    {
      file_id: 2,
      variant_id: 2,
      role: "primary",
      name: "B.jpg",
      last_path: "/p/B.jpg",
      size: 1_000,
      candidates: [{ path: "/mnt/新しい/B.jpg", matched: "name_and_time", size: 1_200, same_name: true }],
    },
    { file_id: 3, variant_id: 3, role: "primary", name: "C.jpg", last_path: "/p/C.jpg", size: 900, candidates: [] },
  ],
};

/** フォルダの場所の指定し直しの予定の見本。 */
export const MOCK_RELOCATE_PLAN: RelocatePlan = {
  folder_id: 12,
  from: "/home/me/写真/旅行",
  to: "/home/me/写真/2024 京都旅行",
  files: 12,
  found: 11,
  subfolders: 2,
  problem: null,
};

// ---------------------------------------------------------------------------
// v1 の compare: 比較表示・選別表示（LIB-03）の見本
// ---------------------------------------------------------------------------

/** 比較表示の面で使う写真の出力の大きさの見本（view_output_size の応答。id ごとに違う大きさ）。 */
export function mockViewSize(variantId: number): ViewOutputSize {
  return { variant_id: variantId, width: 600 + variantId * 10, height: 400 + variantId * 10 };
}

/** 比較表示の面の表示範囲の情報の見本（render_view_region の応答。要求の範囲をそのまま描いたもの）。 */
export function mockViewRegion(pane: number, request: number, req: RegionRequest): RegionInfo {
  const size = mockViewSize(req.variant_id);
  return {
    variant_id: req.variant_id,
    generation: 0,
    request,
    x: Math.floor(req.x),
    y: Math.floor(req.y),
    region_width: Math.ceil(req.width),
    region_height: Math.ceil(req.height),
    full_width: size.width,
    full_height: size.height,
    scale: req.scale ?? 1,
    width: Math.ceil(req.width),
    height: Math.ceil(req.height),
    tiles: 1,
    display_assumed_srgb: true,
    warnings: [],
    pane,
  };
}

/** 比較表示の面の表示範囲の画素の見本（view_region_frame の応答）。 */
export function mockViewRegionFrame(info: RegionInfo): ArrayBuffer {
  return mockFrameBytes(info as unknown as Partial<FrameInfo> & { width: number; height: number });
}

/** 比較表示のコマンドの応答を足す（面ごとの要求の番号・最後の結果を覚える）。 */
export function mockCompareCommands(b: MockBridge): void {
  const last = new Map<number, RegionInfo>();
  const seq = new Map<number, number>();
  const base = b.responses.get("core_execute");
  b.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    switch (c.type) {
      case "view_output_size":
        return { type: "view_size", ...mockViewSize(c.variant_id as number) };
      case "render_view_region": {
        const pane = c.pane as number;
        const n = (seq.get(pane) ?? 0) + 1;
        seq.set(pane, n);
        const info = mockViewRegion(pane, n, c.request as RegionRequest);
        last.set(pane, info);
        return { type: "region", ...info };
      }
      case "cancel_view_regions":
        return { type: "ok" };
      case "release_view":
        last.clear();
        return { type: "ok" };
      default:
        return typeof base === "function" ? (base as (a?: Record<string, unknown>) => unknown)(args) : base;
    }
  });
  b.responses.set("view_region_frame", (args?: Record<string, unknown>) => {
    const info = last.get(args?.pane as number);
    return info ? mockViewRegionFrame(info) : new ArrayBuffer(0);
  });
}

// ---------------------------------------------------------------------------
// v1 の video: 動画の再生（VID-04）の見本
// ---------------------------------------------------------------------------

/** 動画の再生の情報の見本（DJI の 4K・H.265 10 bit の MOV。再生用のファイルはまだなく、作れる）。 */
export function mockVideoPlayback(over: Partial<VideoPlayback> = {}): VideoPlayback {
  return {
    variant_id: 7,
    available: true,
    unavailable_reason: null,
    content_type: "video/quicktime",
    size: 412_345_678,
    video: {
      duration_s: 75.2,
      fps: 59.94,
      codec: "hevc",
      bit_depth: 10,
      color_transfer: "bt709",
      color_primaries: "bt709",
      width: 3840,
      height: 2160,
      creation_time: "2024-05-01T09:12:00.000000Z",
    },
    proxy: null,
    proxy_job: null,
    proxy_unavailable: null,
    proxy_encoder: "libx264",
    ...over,
  };
}

/** 再生用のファイルのキャッシュの状況の見本。 */
export const MOCK_VIDEO_PROXY_CACHE: VideoProxyCacheStatus = {
  dir: "/home/u/Pictures/GenzoParis/catalog-data/video-proxies",
  count: 3,
  total_bytes: 123_456_789,
  capacity_bytes: 5_000_000_000,
  removed: 0,
};

/**
 * 動画の再生のコマンドの応答を足す（video_playback・create_video_proxy・video_proxy_cache・clear_video_proxies）。
 * `playback` は variant ごとの再生の情報（なければ mockVideoPlayback）。作ったジョブの番号は `jobs` に残す。
 */
export function mockVideoCommands(
  b: MockBridge,
  playback: (id: number) => VideoPlayback = (id) => mockVideoPlayback({ variant_id: id }),
): { jobs: { variant_id: number; job_id: number }[] } {
  const jobs: { variant_id: number; job_id: number }[] = [];
  let cache = { ...MOCK_VIDEO_PROXY_CACHE };
  const base = b.responses.get("core_execute");
  b.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    switch (c.type) {
      case "video_playback":
        return { type: "video_playback", ...playback(c.variant_id as number) };
      case "create_video_proxy": {
        const job_id = 900 + jobs.length;
        jobs.push({ variant_id: c.variant_id as number, job_id });
        return { type: "job_started", job_id };
      }
      case "video_proxy_cache":
        return { type: "video_proxy_cache", ...cache };
      case "clear_video_proxies":
        cache = { ...cache, removed: cache.count, count: 0, total_bytes: 0 };
        return { type: "video_proxy_cache", ...cache };
      default:
        return typeof base === "function" ? (base as (a?: Record<string, unknown>) => unknown)(args) : base;
    }
  });
  return { jobs };
}
