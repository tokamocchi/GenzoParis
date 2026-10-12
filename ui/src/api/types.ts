// コア API（genzo-api）とアプリ（genzo-app）の型。Rust の serde の形をそのまま写したもの。
//
// - 正は Rust 側の定義（crates/genzo-api/src/types.rs・command.rs・events.rs・error.rs、
//   crates/genzo-model/src、crates/genzo-app/src）。変えたら両方を直す（型の自動生成は未導入。
//   genzo-api の lib.rs の doc「型の生成」）。
// - ID（VariantId など）は Rust の i64。JavaScript の number で扱える範囲（2^53 未満）を前提にする。
// - 日時（DateTime<Utc>）は RFC 3339 の文字列。

// ---------------------------------------------------------------------------
// ID・列挙
// ---------------------------------------------------------------------------

export type VariantId = number;
export type AssetId = number;
export type FileId = number;
export type FolderId = number;
export type VolumeId = number;
export type KeywordId = number;
export type HistoryEntryId = number;

/** 評価（0〜5）。 */
export type Rating = 0 | 1 | 2 | 3 | 4 | 5;
/** フラグ（-1: 不採用、0: なし、1: 採用）。 */
export type Flag = -1 | 0 | 1;
export type ColorLabel = "red" | "yellow" | "green" | "blue" | "purple";
export type AssetKind = "photo" | "video";
export type FileStatus = "ok" | "missing" | "error";
export type FileRole = "primary" | "sidecar_jpeg";
export type TzSource = "exif" | "utc_only" | "user_default" | "user_set";
/** EXIF の向き（1〜8）。 */
export type Orientation = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8;
export type RenderQuality = "draft" | "final";
export type RenderBackend = "gpu" | "cpu";
export type OutputColorSpace = "srgb" | "display_p3" | "adobe_rgb";

// ---------------------------------------------------------------------------
// エラー（genzo-api の ErrorInfo）
// ---------------------------------------------------------------------------

export type ErrorKind =
  | "invalid_input"
  | "not_found"
  | "stale"
  | "cancelled"
  | "file_access"
  | "unsupported_file"
  | "corrupt_file"
  | "protected_destination"
  | "conflict"
  | "trash_unavailable"
  | "storage"
  | "catalog_corrupt"
  | "incompatible_version"
  | "worker"
  | "gpu"
  | "closed"
  | "internal";

export interface ErrorInfo {
  kind: ErrorKind;
  /** 表示用の説明（日本語）。 */
  message: string;
  user_actionable: boolean;
  retryable: boolean;
  hint: string | null;
}

// ---------------------------------------------------------------------------
// メタデータ
// ---------------------------------------------------------------------------

export interface GpsCoord {
  lat: number;
  lon: number;
}

export interface CaptureTime {
  /** 元の日時の文字列。 */
  raw: string | null;
  /** ファイルに記録されていたオフセット（"+09:00"。UTC の印だけなら "Z"）。 */
  offset: string | null;
  tz_source: TzSource;
  /** UTC の推定に使ったオフセット（"+09:00"）。 */
  tz_assumed: string | null;
  correction_s: number;
  /** 補正後の UTC（RFC 3339）。日時が不明なら null。 */
  utc: string | null;
}

export interface VideoMetadata {
  duration_s: number | null;
  fps: number | null;
  codec: string | null;
  bit_depth: number | null;
  color_transfer: string | null;
  color_primaries: string | null;
  width: number | null;
  height: number | null;
  creation_time: string | null;
}

// ---------------------------------------------------------------------------
// 現像設定（genzo-model の DevelopSettings。2.5 節）
// ---------------------------------------------------------------------------

export type WbPreset = "daylight" | "cloudy" | "shade" | "tungsten" | "fluorescent" | "flash";

/** "as_shot" / {preset} / {custom}。 */
export type WhiteBalance =
  | "as_shot"
  | { preset: WbPreset }
  | { custom: { temperature_k: number; tint: number } };

export interface ToneParams {
  highlights: number;
  shadows: number;
  whites: number;
  blacks: number;
}

export interface ParametricCurve {
  highlights: number;
  lights: number;
  darks: number;
  shadows: number;
}

export interface CurvePoint {
  x: number;
  y: number;
}

export interface ToneCurve {
  parametric: ParametricCurve;
  luma: CurvePoint[];
  red: CurvePoint[];
  green: CurvePoint[];
  blue: CurvePoint[];
}

export interface ColorParams {
  saturation: number;
  vibrance: number;
}

/** 切り抜き（G の外接矩形の各軸の割合。0〜1）。 */
export interface CropRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export type AspectRatio = "original" | { ratio: { w: number; h: number } };

export interface Geometry {
  crop: CropRect;
  /** 角度（度。正で反時計回り。-45〜45）。 */
  angle_deg: number;
  aspect: AspectRatio | null;
}

export interface LensCorrection {
  profile_enabled: boolean;
  distortion_scale: number;
  vignetting_scale: number;
  remove_chromatic_aberration: boolean;
}

export interface DetailParams {
  sharpening: { amount: number; radius: number; detail: number; masking: number };
  noise_reduction: { luminance: number; color: number };
}

export interface RenderDeps {
  camera_profile: { id: string; sha256: string };
  lens_profile: { id: string; sha256: string } | null;
  raw_decoder: string;
}

export interface DevelopSettings {
  schema_version: number;
  process_version: number;
  white_balance: WhiteBalance;
  /** 露光量（EV。-5〜5）。 */
  exposure_ev: number;
  /** コントラスト（-100〜100）。 */
  contrast: number;
  tone: ToneParams;
  tone_curve: ToneCurve;
  color: ColorParams;
  geometry: Geometry;
  lens: LensCorrection;
  detail: DetailParams;
  /** 部分補正（v1。MVP の画面では扱わず、そのまま保つ）。 */
  masks: unknown[];
  render_deps: RenderDeps;
}

/** 値の範囲（genzo-model の develop::ranges）。 */
export const RANGES = {
  exposureEv: { min: -5, max: 5 },
  slider: { min: -100, max: 100 },
  amount: { min: 0, max: 100 },
  temperatureK: { min: 2000, max: 50000 },
  tint: { min: -150, max: 150 },
  angleDeg: { min: -45, max: 45 },
} as const;

/** WB のプリセットの色温度と tint（genzo-model の WbPreset::temperature_tint）。 */
export const WB_PRESETS: Record<WbPreset, { temperature_k: number; tint: number; label: string }> = {
  daylight: { temperature_k: 5500, tint: 10, label: "太陽光" },
  cloudy: { temperature_k: 6500, tint: 10, label: "曇天" },
  shade: { temperature_k: 7500, tint: 10, label: "日陰" },
  tungsten: { temperature_k: 2850, tint: 0, label: "白熱灯" },
  fluorescent: { temperature_k: 3800, tint: 21, label: "蛍光灯" },
  flash: { temperature_k: 5500, tint: 0, label: "フラッシュ" },
};

/** 一括適用で写す項目（genzo-api の SettingGroups）。 */
export interface SettingGroups {
  white_balance: boolean;
  basic_tone: boolean;
  tone_curve: boolean;
  color: boolean;
  geometry: boolean;
  lens: boolean;
  detail: boolean;
  masks: boolean;
}

// ---------------------------------------------------------------------------
// 書き出し（genzo-model の ExportSettings）
// ---------------------------------------------------------------------------

export type ExportFormat = { jpeg: { quality: number } } | "tiff16" | "png8" | "png16";
export type ExportSize = "original" | { long_edge: number };
export type ConflictPolicy = "sequence" | "overwrite" | "skip";

export interface ExportSettings {
  format: ExportFormat;
  color_space: OutputColorSpace;
  size: ExportSize;
  remove_gps: boolean;
  on_conflict: ConflictPolicy;
}

// ---------------------------------------------------------------------------
// ジョブ
// ---------------------------------------------------------------------------

export type JobKind =
  | "import"
  | "export"
  | "regenerate_previews"
  | "integrity_check"
  | "reresolve_capture_times"
  | "collect_garbage";
export type JobState = "running" | "succeeded" | "failed" | "cancelled";

export interface FileIssue {
  path: string;
  reason: string;
}

export interface VariantIssue {
  variant_id: VariantId;
  reason: string;
}

export interface ImportReport {
  root: string;
  files_found: number;
  added: number;
  updated: number;
  unchanged: number;
  errors: FileIssue[];
  not_registered: FileIssue[];
  skipped_links: number;
  thumbnails: number;
  thumbnail_failures: FileIssue[];
  cancelled: boolean;
}

export type ExportOutcome =
  | {
      status: "written";
      path: string;
      replaced: boolean;
      backend: RenderBackend;
      warnings: string[];
      render_deps: RenderDeps | null;
    }
  | { status: "skipped"; reason: string; existing: string | null }
  | { status: "failed"; error: ErrorInfo };

export interface ExportItem {
  variant_id: VariantId;
  outcome: ExportOutcome;
}

export interface ExportReport {
  dest_dir: string;
  items: ExportItem[];
  written: number;
  skipped: number;
  failed: number;
  cancelled: boolean;
}

export interface RegenerateReport {
  rendered: number;
  skipped: number;
  failed: VariantIssue[];
  warnings: VariantIssue[];
}

export interface IntegrityReport {
  ok: boolean;
  integrity_errors: string[];
  foreign_key_violations: string[];
  fts_error: string | null;
}

export type JobResult =
  | ({ type: "import" } & ImportReport)
  | ({ type: "export" } & ExportReport)
  | ({ type: "regenerate_previews" } & RegenerateReport)
  | ({ type: "integrity_check" } & IntegrityReport)
  | { type: "reresolve_capture_times"; updated: number }
  | {
      type: "collect_garbage";
      thumbnails_removed: number;
      previews_removed: number;
      temp_files_removed: number;
    };

export interface JobInfo {
  job_id: number;
  kind: JobKind;
  state: JobState;
  done: number;
  total: number;
  result: JobResult | null;
  error: ErrorInfo | null;
}

// ---------------------------------------------------------------------------
// 検索
// ---------------------------------------------------------------------------

export interface SearchFilter {
  rating_min?: Rating | null;
  rating_max?: Rating | null;
  flags?: Flag[] | null;
  /** null の要素は「ラベルなし」。 */
  color_labels?: (ColorLabel | null)[] | null;
  /** RFC 3339（UTC）。 */
  captured_from?: string | null;
  captured_until?: string | null;
  cameras?: string[] | null;
  lenses?: string[] | null;
  kind?: AssetKind | null;
  long_edge_min?: number | null;
  long_edge_max?: number | null;
  duration_min_s?: number | null;
  duration_max_s?: number | null;
  fps_min?: number | null;
  fps_max?: number | null;
  codecs?: string[] | null;
  text?: string | null;
  folder_id?: FolderId | null;
  include_subfolders?: boolean;
  keywords?: KeywordId[];
  file_statuses?: FileStatus[] | null;
  masters_only?: boolean;
}

export type SortKey = "capture_time" | "file_name" | "rating" | "import_order";
export type SortDirection = "ascending" | "descending";

export interface SearchSort {
  key: SortKey;
  direction: SortDirection;
}

export interface SearchResult {
  generation: number;
  count: number;
}

export interface VariantSummary {
  variant_id: VariantId;
  asset_id: AssetId;
  is_master: boolean;
  variant_name: string | null;
  kind: AssetKind;
  file_id: FileId;
  file_name: string;
  folder_id: FolderId;
  file_status: FileStatus;
  file_revision: number;
  has_sidecar_jpeg: boolean;
  rating: Rating;
  flag: Flag;
  color_label: ColorLabel | null;
  capture: CaptureTime;
  camera: string | null;
  lens: string | null;
  width: number | null;
  height: number | null;
  orientation: Orientation;
  develop_hash: string;
  /** サムネイルの rev（`genzo://thumb/{id}?rev=`）。まだなければ null。 */
  thumb_rev: string | null;
}

export interface RangeResult {
  generation: number;
  start: number;
  items: VariantSummary[];
}

export interface IndexOf {
  generation: number;
  index: number | null;
}

export interface FileInfo {
  file_id: FileId;
  role: FileRole;
  path: string | null;
  size: number;
  status: FileStatus;
  status_reason: string | null;
}

export interface VariantDetails {
  summary: VariantSummary;
  iso: number | null;
  aperture: number | null;
  shutter_s: number | null;
  focal_mm: number | null;
  gps: GpsCoord | null;
  caption: string | null;
  video: VideoMetadata | null;
  files: FileInfo[];
}

export interface FolderInfo {
  folder_id: FolderId;
  volume_id: VolumeId;
  parent_id: FolderId | null;
  /** ボリュームの中の相対パス（"/" 区切り）。 */
  rel_path: string;
  /** 絶対パス（ボリュームが分かれば）。 */
  path: string | null;
}

export interface FileCheckReport {
  checked: number;
  ok: number;
  missing: FileIssue[];
  changed: string[];
}

/** 絞り込みの候補（カメラ・レンズ・コーデック。件数つき）。genzo-api の `Facets`。 */
export interface FacetValue {
  value: string;
  count: number;
}

export interface Facets {
  cameras: FacetValue[];
  lenses: FacetValue[];
  codecs: FacetValue[];
}

// ---------------------------------------------------------------------------
// 現像
// ---------------------------------------------------------------------------

export interface AsShotWhiteBalance {
  temperature_k: number;
  tint: number;
}

export interface SourceInfo {
  is_raw: boolean;
  /** 見える範囲（RAW の既定の切り抜き）の幅。 */
  width: number;
  height: number;
  decoder: string | null;
  as_shot_white_balance: AsShotWhiteBalance | null;
}

export interface DevelopState {
  variant_id: VariantId;
  settings: DevelopSettings;
  generation: number;
  can_undo: boolean;
  can_redo: boolean;
  dirty: boolean;
  dragging: boolean;
  source: SourceInfo;
}

export interface HistoryItem {
  id: HistoryEntryId;
  created_at: string;
  label: string;
  is_current: boolean;
}

export interface ClipCounts {
  red: number;
  green: number;
  blue: number;
  any: number;
  all: number;
}

export interface HistogramData {
  red: number[];
  green: number[];
  blue: number[];
  luma: number[];
  pixel_count: number;
  clipped_high: ClipCounts;
  clipped_low: ClipCounts;
}

export interface FrameInfo {
  variant_id: VariantId;
  generation: number;
  quality: RenderQuality;
  width: number;
  height: number;
  backend: RenderBackend;
  histogram: HistogramData;
  display_assumed_srgb: boolean;
  warnings: string[];
  /**
   * 切り抜く前の全体（G）を描いたものか（切り抜きの編集中。`set_develop_view` の `uncropped`）。
   * 古いコアでは無い（false とみなす）。
   */
  uncropped?: boolean;
}

export type RegionSpace = "output" | "geometry";

export interface RegionRequest {
  variant_id: VariantId;
  space: RegionSpace;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
}

export interface RegionInfo {
  variant_id: VariantId;
  generation: number;
  request: number;
  x: number;
  y: number;
  region_width: number;
  region_height: number;
  full_width: number;
  full_height: number;
  scale: number;
  width: number;
  height: number;
  tiles: number;
  display_assumed_srgb: boolean;
  warnings: string[];
}

export interface DisplayInfo {
  assumed_srgb: boolean;
  reason: string | null;
  description: string | null;
}

// ---------------------------------------------------------------------------
// 削除
// ---------------------------------------------------------------------------

export type DeleteKind = "virtual_copies" | "remove_from_catalog" | "trash";

export interface PlannedVariant {
  variant_id: VariantId;
  asset_id: AssetId;
  is_master: boolean;
  name: string | null;
}

export interface PlannedFile {
  file_id: FileId;
  asset_id: AssetId;
  role: FileRole;
  path: string | null;
}

export interface DeletePlan {
  plan_id: number;
  kind: DeleteKind;
  assets: AssetId[];
  variants: PlannedVariant[];
  files: PlannedFile[];
}

export interface DeleteReport {
  removed_variants: VariantId[];
  removed_assets: AssetId[];
  trashed_files: string[];
  skipped_missing: string[];
  failed: FileIssue[];
}

// ---------------------------------------------------------------------------
// 設定・保守
// ---------------------------------------------------------------------------

export interface CoreSettings {
  preview_cache_dir: string;
  preview_cache_bytes: number;
  default_export_color_space: OutputColorSpace;
  default_utc_offset_minutes: number;
}

export interface SettingsUpdate {
  preview_cache_dir?: string | null;
  preview_cache_bytes?: number | null;
  default_export_color_space?: OutputColorSpace | null;
  default_utc_offset_minutes?: number | null;
}

export interface BackupEntry {
  path: string;
  created_at: string;
}

export interface CatalogStats {
  schema_version: number;
  assets: number;
  variants: number;
  files: number;
  history_entries: number;
}

export type PreviousShutdown = "first_open" | "clean" | "unclean";

export interface StartupReport {
  previous_shutdown: PreviousShutdown;
  migrated_from: number | null;
  recovered_file_ops: number;
  backup: BackupEntry | null;
}

export interface Capabilities {
  raw_decoder: string | null;
  worker_version: string;
  gpu: string | null;
}

export interface RestorePlan {
  backup: string;
  catalog_path: string;
  restored_path: string;
  displaced_path: string;
  steps: string[];
}

// ---------------------------------------------------------------------------
// コマンド（UI → コア。genzo-api の Command）
// ---------------------------------------------------------------------------

export type Command =
  | { type: "import_folder"; path: string; recursive: boolean }
  | { type: "job"; job_id: number }
  | { type: "wait_job"; job_id: number; timeout_ms: number | null }
  | { type: "cancel_job"; job_id: number }
  | { type: "search"; filter: SearchFilter; sort: SearchSort }
  | { type: "range"; generation: number; start: number; len: number }
  | { type: "index_of"; variant_id: VariantId }
  | { type: "set_rating"; variant_ids: VariantId[]; rating: Rating }
  | { type: "set_flag"; variant_ids: VariantId[]; flag: Flag }
  | { type: "set_color_label"; variant_ids: VariantId[]; label: ColorLabel | null }
  | { type: "set_caption"; variant_ids: VariantId[]; caption: string | null }
  | { type: "variant_details"; variant_id: VariantId }
  | { type: "folders" }
  | { type: "facets" }
  | { type: "check_files"; variant_ids: VariantId[] }
  | { type: "thumbnail"; variant_id: VariantId }
  | { type: "preview_path"; variant_id: VariantId }
  | { type: "request_preview"; variant_id: VariantId }
  | { type: "regenerate_previews"; variant_ids: VariantId[] }
  | { type: "open_develop"; variant_id: VariantId }
  | { type: "develop_state" }
  | { type: "set_settings"; settings: DevelopSettings; dragging: boolean }
  | { type: "end_drag" }
  | { type: "render_develop" }
  | { type: "render_region"; request: RegionRequest }
  | { type: "cancel_region" }
  | { type: "wait_for_frame"; generation: number; timeout_ms: number }
  | { type: "flush_develop" }
  | { type: "close_develop" }
  | { type: "undo"; variant_id: VariantId }
  | { type: "redo"; variant_id: VariantId }
  | { type: "history"; variant_id: VariantId }
  | { type: "copy_settings"; variant_id: VariantId }
  | {
      type: "paste_settings";
      settings: DevelopSettings;
      targets: VariantId[];
      groups: SettingGroups;
    }
  | { type: "create_virtual_copy"; variant_id: VariantId; name: string | null }
  | { type: "white_balance_at"; variant_id: VariantId; x: number; y: number }
  | { type: "set_develop_view"; uncropped: boolean }
  | {
      type: "constrain_crop";
      variant_id: VariantId;
      crop: CropRect;
      angle_deg: number;
    }
  | { type: "export"; variant_ids: VariantId[]; settings: ExportSettings; dest_dir: string }
  | { type: "plan_delete"; kind: DeleteKind; variant_ids: VariantId[] }
  | { type: "execute_delete"; plan_id: number }
  | { type: "get_settings" }
  | { type: "update_settings"; update: SettingsUpdate }
  | { type: "backup_now" }
  | { type: "list_backups" }
  | { type: "check_integrity" }
  | { type: "collect_cache_garbage" }
  | { type: "prepare_restore"; backup: string }
  | { type: "startup_report" }
  | { type: "catalog_stats" }
  | { type: "capabilities" };

/** 応答（genzo-api の Response）。 */
export type Response =
  | { type: "ok" }
  | { type: "job_started"; job_id: number }
  | ({ type: "job" } & JobInfo)
  | ({ type: "search" } & SearchResult)
  | ({ type: "range" } & RangeResult)
  | ({ type: "index_of" } & IndexOf)
  | { type: "updated"; count: number }
  | ({ type: "details" } & VariantDetails)
  | { type: "folders"; folders: FolderInfo[] }
  | ({ type: "facets" } & Facets)
  | ({ type: "file_check" } & FileCheckReport)
  | { type: "thumbnail"; thumbnail: { variant_id: VariantId; rev: string; jpeg: number[] } | null }
  | { type: "path"; path: string | null }
  | ({ type: "develop" } & DevelopState)
  | { type: "develop_or_none"; state: DevelopState | null }
  | { type: "generation"; generation: number }
  | ({ type: "frame" } & FrameInfo)
  | ({ type: "region" } & RegionInfo)
  | { type: "frame_or_none"; frame: FrameInfo | null }
  | { type: "saved"; saved: boolean }
  | { type: "settings"; settings: DevelopSettings }
  | { type: "settings_or_none"; settings: DevelopSettings | null }
  | { type: "history"; entries: HistoryItem[] }
  | { type: "variant_created"; variant_id: VariantId }
  | { type: "white_balance"; temperature_k: number; tint: number }
  | { type: "crop"; crop: CropRect }
  | ({ type: "display" } & DisplayInfo)
  | ({ type: "delete_plan" } & DeletePlan)
  | ({ type: "deleted" } & DeleteReport)
  | ({ type: "core_settings" } & CoreSettings)
  | ({ type: "backup" } & BackupEntry)
  | { type: "backups"; backups: BackupEntry[] }
  | ({ type: "restore" } & RestorePlan)
  | ({ type: "startup" } & StartupReport)
  | ({ type: "catalog_stats" } & CatalogStats)
  | ({ type: "capabilities" } & Capabilities);

// ---------------------------------------------------------------------------
// イベント（コア → UI。genzo-api の Event。Tauri のイベント名は "core-event"）
// ---------------------------------------------------------------------------

export type CatalogChange =
  | "imported"
  | "marks"
  | "caption"
  | "develop"
  | "variant_added"
  | "removed"
  | "file_status"
  | "capture_time";

export type WarningCode =
  | "unclean_shutdown"
  | "catalog_migrated"
  | "file_operation_recovered"
  | "backup_failed"
  | "gpu_fallback"
  | "gpu_disabled"
  | "display_profile_assumed_srgb"
  | "save_failed"
  | "preview_failed"
  | "render"
  | "file_missing"
  | "invalid_setting"
  | "default_time_zone_differs"
  | "cache_rebuilt"
  | "catalog_lock_unavailable"
  | "render_deps_changed";

export type CoreEvent =
  | { type: "job_progress"; job_id: number; kind: JobKind; done: number; total: number }
  | {
      type: "job_finished";
      job_id: number;
      kind: JobKind;
      state: JobState;
      message: string | null;
    }
  | { type: "catalog_changed"; change: CatalogChange; variant_ids: VariantId[]; all: boolean }
  | {
      type: "preview_updated";
      variant_id: VariantId;
      generation: number;
      quality: RenderQuality;
      width: number;
      height: number;
      backend: RenderBackend;
    }
  | { type: "thumbnail_updated"; variant_id: VariantId; rev: string }
  | { type: "search_updated"; generation: number; count: number }
  | {
      type: "warning";
      code: WarningCode;
      message: string;
      variant_id: VariantId | null;
      path: string | null;
    };

// ---------------------------------------------------------------------------
// アプリ（genzo-app のコマンド）
// ---------------------------------------------------------------------------

export type GpuMode = "auto" | "off";
export type DisplayTarget = "display_p3" | "srgb";
export type CatalogFileState = "missing" | "catalog" | "not_catalog";

export interface AppSettings {
  version: number;
  last_catalog: string | null;
  recent_catalogs: string[];
  ffmpeg_dir: string | null;
  gpu: GpuMode;
  backup_on_close: boolean;
}

export interface ExternalTools {
  ffmpeg: string | null;
  ffprobe: string | null;
}

export interface CatalogStatus {
  catalog_path: string;
  data_dir: string;
  startup: StartupReport;
  /** 起動時の警告（イベントの形）。 */
  warnings: CoreEvent[];
  stats: CatalogStats;
  capabilities: Capabilities | null;
  worker_error: ErrorInfo | null;
  display: DisplayInfo;
  tools: ExternalTools;
  core_settings: CoreSettings;
}

export interface AppStatus {
  version: string;
  platform: "windows" | "macos" | "linux";
  settings: AppSettings;
  notice: string | null;
  default_catalog: string | null;
  catalog: CatalogStatus | null;
  tools: ExternalTools;
}

export interface CloseFailure {
  error: ErrorInfo;
  still_open: boolean;
}

export interface RestoreOutcome {
  backup: string;
  displaced: string[];
  status: CatalogStatus | null;
  open_error: ErrorInfo | null;
}

export interface AppSettingsUpdate {
  ffmpeg_dir: string | null;
  gpu: GpuMode;
  backup_on_close: boolean;
}
