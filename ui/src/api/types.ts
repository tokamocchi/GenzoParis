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
export type CollectionId = number;
export type HistoryEntryId = number;
export type SnapshotId = number;
export type StackId = number;

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
  /** 表示のオフセット（"+09:00"。basis がなければ UTC の推定にも使う）。 */
  tz_assumed: string | null;
  /**
   * UTC の計算に使ったオフセット（LIB-16。表示のオフセットと違う場合・UTC の印だけの値を現地時刻として読む場合
   * だけ。"+09:00"）。古いコアでは無い。
   */
  basis?: string | null;
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
  /** キーワード（XMP の dc:subject）を書かない（LIB-09。省略時は書く）。 */
  remove_keywords?: boolean;
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
  | "collect_garbage"
  | "hash_files"
  | "move_files"
  | "check_missing"
  | "find_relink_candidates";
export type JobState = "running" | "succeeded" | "failed" | "cancelled";

export interface FileIssue {
  path: string;
  reason: string;
}

export interface VariantIssue {
  variant_id: VariantId;
  reason: string;
}

/**
 * 取り込みの設定（IMP-02〜IMP-06。genzo-api の ImportOptions）。skip_duplicates・separate_raw_jpeg を省くと
 * カタログの設定に従う。copy があればコピー取り込み、なければその場で登録する。
 */
export interface ImportOptions {
  skip_duplicates?: boolean | null;
  separate_raw_jpeg?: boolean | null;
  /** コピー取り込み（IMP-02・IMP-03）。 */
  copy?: CopySettings | null;
  /** 新しく登録した写真にまとめて付けるキーワード・説明のメタデータ（IMP-05）。 */
  metadata?: ImportMetadata;
  /** コピー先の名前を決めるだけで、コピー・登録をしない（確認用）。 */
  dry_run?: boolean;
}

/** コピー先のフォルダの規則（genzo-api の FolderRule）。 */
export type FolderRule =
  | "year_date"
  | "year_month_day"
  | "year_month"
  | "date"
  | "date_compact"
  | "single"
  | "source";

/** リネームの規則（IMP-03。genzo-api の RenameRule）。 */
export interface RenameRule {
  /** テンプレート（{date}-{seq} など。{{・}} は波かっこそのもの）。 */
  template: string;
  /** 任意の文字（{text}）。 */
  custom_text: string;
  sequence_start: number;
  sequence_digits: number;
}

/** コピー取り込みの設定（IMP-02・IMP-03。genzo-api の CopySettings）。 */
export interface CopySettings {
  destination: string;
  folder_rule?: FolderRule;
  /** null なら元の名前のまま。 */
  rename?: RenameRule | null;
}

/** 取り込んだ写真にまとめて付けるもの（IMP-05。genzo-api の ImportMetadata）。 */
export interface ImportMetadata {
  /** キーワードの階層のパス（[["場所", "京都"], ["家族"]]）。 */
  keywords: string[][];
  /** 説明のメタデータ（空の値は付けない）。 */
  fields: Partial<Record<MetadataField, string>>;
}

/** コピーした（する）ファイル。 */
export interface CopiedFile {
  source: string;
  destination: string;
  /** コピーせず、コピー先に既にあった同じ内容のファイルを登録した。 */
  existing: boolean;
}

/** コピー取り込みの結果（genzo-api の CopyReport）。 */
export interface CopyReport {
  destination: string;
  copied: CopiedFile[];
  bytes_copied: number;
  /** 撮影日時が分からず、ファイルの更新日時で日付を決めたファイルの数。 */
  dated_by_file_time: number;
  dry_run: boolean;
  /** 次の連番（複数のフォルダを続けて取り込むときの、次のフォルダの開始番号）。 */
  next_sequence: number;
}

/** コピー先の名前の例（genzo-api の NamePreview）。 */
export interface NamePreview {
  examples: NameExample[];
  /** 設定の誤り（あれば例は空）。 */
  error: string | null;
}

export interface NameExample {
  /** 見本の元のファイル（取り込むフォルダからの相対パス）。 */
  source: string;
  /** コピー先のフォルダからの相対パス。 */
  destination: string;
  /** 見本の撮影日時。 */
  captured: string;
}

/** 取り込みで、内容が同じファイル（重複）として見つけたもの（IMP-04）。 */
export interface DuplicateFile {
  path: string;
  /** 同じ内容の登録済みのファイル（同じ取り込みで先に登録したものを含む）。 */
  existing_path: string | null;
  existing_variant_id: VariantId | null;
  /** 取り込まなかったか（false なら、設定により別の写真として登録した）。 */
  skipped: boolean;
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
  /** 内容が同じファイルが登録済みだったもの（IMP-04）。古いコアでは無い。 */
  duplicates?: DuplicateFile[];
  /** RAW と組にして 1 枚にまとめたファイルの数（IMP-06）。 */
  paired?: number;
  skip_duplicates?: boolean;
  separate_raw_jpeg?: boolean;
  /** コピー取り込みの結果（コピーしない取り込み・古いコアでは無い。IMP-02）。 */
  copy?: CopyReport | null;
  /** キーワード・説明のメタデータを付けた写真の数（IMP-05）。 */
  tagged?: number;
}

/** ファイルの内容のハッシュの計算の結果（ORG-03。genzo-api の HashReport）。 */
export interface HashReport {
  hashed: number;
  skipped_network: number;
  skipped_unavailable: number;
  changed: number;
  failed_count: number;
  failed: FileIssue[];
  cancelled: boolean;
}

/** ファイルの内容のハッシュの状況（genzo-api の ContentHashStatus）。 */
export interface ContentHashStatus {
  files: number;
  hashed: number;
  pending: number;
  duplicate_groups: number;
  job_id: number | null;
}

/** 内容が同じファイルの 1 つ（genzo-api の ContentFile）。 */
export interface ContentFile {
  file_id: FileId;
  asset_id: AssetId;
  variant_id: VariantId;
  role: FileRole;
  path: string | null;
  size: number;
  status: FileStatus;
}

export interface DuplicateGroup {
  full_hash: string;
  files: ContentFile[];
}

export interface DuplicateGroupList {
  total: number;
  groups: DuplicateGroup[];
}

export interface FileIdentity {
  path: string;
  size: number;
  full_hash: string;
  matches: ContentFile[];
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
    }
  | ({ type: "hash_files" } & HashReport)
  | ({ type: "move_files" } & FileMoveReport)
  | ({ type: "check_missing" } & MissingCheckReport)
  | ({ type: "relink_candidates" } & RelinkSearch);

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
  /** コレクション（LIB-10・LIB-11。手動はメンバー、スマートは条件、セットは中のいずれか）。 */
  collection_id?: CollectionId | null;
  /** 保存していないスマートコレクションの条件（編集中の件数など）。 */
  smart_rule?: SmartRule | null;
  /**
   * スタック（LIB-12）を、保存した折りたたみの状態によらず、すべて展開して並べる（結果のまとめ方だけに効く。
   * 一致する写真・件数は変わらない）。
   */
  expand_stacks?: boolean;
}

export type SortKey = "capture_time" | "file_name" | "rating" | "import_order";
export type SortDirection = "ascending" | "descending";

export interface SearchSort {
  key: SortKey;
  direction: SortDirection;
}

export interface SearchResult {
  generation: number;
  /** 表示する写真の数（折りたたんだスタックは 1 枚に数える）。 */
  count: number;
  /** 条件に一致したが、折りたたんだスタックの中に隠した写真の数（LIB-12。0 なら省略）。 */
  hidden?: number;
}

/** グリッドのセルに出すスタックの印（LIB-12。genzo-api の StackBadge）。 */
export interface StackBadge {
  stack_id: StackId;
  /** スタックの中の位置（1 から。1 が一番上）。 */
  position: number;
  /** スタックの写真の数（2 以上）。 */
  count: number;
  collapsed: boolean;
}

/** スタック（LIB-12。genzo-api の StackInfo）。 */
export interface StackInfo {
  stack_id: StackId;
  /** 一番上の写真（members の最初）。 */
  top_variant_id: VariantId;
  collapsed: boolean;
  /** 写真（スタックの中の順。最初が一番上）。 */
  members: VariantId[];
}

/** 撮影時刻の間隔による自動スタックの結果（LIB-12。genzo-api の AutoStackReport）。 */
export interface AutoStackReport {
  stacks: number;
  photos: number;
  considered: number;
  skipped_in_stacks: number;
  skipped_without_time: number;
  dry_run: boolean;
}

/** 同じ写真のマスターと仮想コピーの 1 件（LIB-13。genzo-api の VariantBrief）。 */
export interface VariantBrief {
  variant_id: VariantId;
  is_master: boolean;
  name: string | null;
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
  /** スタック（LIB-12）に入っていれば、その印。古いコアでは無い。 */
  stack?: StackBadge | null;
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

/** 表示中の検索結果に出ている写真（shown_variants。問い合わせた順。隠れた写真・絞り込みに合わない写真は含まない）。 */
export interface ShownVariants {
  generation: number;
  variant_ids: VariantId[];
}

export interface FileInfo {
  file_id: FileId;
  role: FileRole;
  path: string | null;
  size: number;
  status: FileStatus;
  status_reason: string | null;
  /** ファイル全体の BLAKE3（ORG-03。まだ計算していなければ null。古いコアでは無い）。 */
  full_hash?: string | null;
}

export interface VariantDetails {
  summary: VariantSummary;
  /** カメラの本体のシリアル番号（LIB-16 の機材の区別）。古いコアでは無い。 */
  camera_serial?: string | null;
  iso: number | null;
  aperture: number | null;
  shutter_s: number | null;
  focal_mm: number | null;
  gps: GpsCoord | null;
  /** キャプション（metadata.caption と同じ）。 */
  caption: string | null;
  /** カタログの中で編集した説明のメタデータ（LIB-15。asset に付き、仮想コピーも同じ値）。古いコアでは無い。 */
  metadata?: IptcMetadata;
  video: VideoMetadata | null;
  files: FileInfo[];
  /** 同じ写真のマスターと仮想コピー（LIB-13。マスターが先。自分を含む）。古いコアでは無い。 */
  variants?: VariantBrief[];
  /** 入っているスタック（LIB-12）。古いコアでは無い。 */
  stack?: StackInfo | null;
}

/**
 * 説明のメタデータの項目（LIB-15。IPTC Core の主要な項目。genzo-model の MetadataField。JSON のキーと
 * カタログの列の名前）。並びは genzo-model の MetadataField::ALL と同じ。
 */
export type MetadataField =
  | "title"
  | "headline"
  | "caption"
  | "creator"
  | "creator_job_title"
  | "creator_address"
  | "creator_city"
  | "creator_region"
  | "creator_postal_code"
  | "creator_country"
  | "creator_phone"
  | "creator_email"
  | "creator_url"
  | "copyright"
  | "copyright_status"
  | "usage_terms"
  | "copyright_url"
  | "sublocation"
  | "city"
  | "province_state"
  | "country"
  | "country_code"
  | "credit"
  | "source"
  | "instructions";

/** 説明のメタデータ（genzo-model の IptcMetadata。値がなければ null か、キーがない）。 */
export type IptcMetadata = Partial<Record<MetadataField, string | null>>;

/** 著作権の状態（copyright_status の値。null は不明）。 */
export type CopyrightStatus = "copyrighted" | "public_domain";

/** 複数の写真の説明のメタデータのまとめ（genzo-api の MetadataSummary）。 */
export interface MetadataSummary {
  /** 対象の元の写真（asset）の数。 */
  assets: number;
  fields: MetadataFieldState[];
}

export interface MetadataFieldState {
  field: MetadataField;
  /** すべての写真で同じ値ならその値（値が異なれば null）。 */
  value: string | null;
  /** 写真によって値が異なるか（「複数の値」）。 */
  mixed: boolean;
}

export interface FolderInfo {
  folder_id: FolderId;
  volume_id: VolumeId;
  parent_id: FolderId | null;
  /** ボリュームの中の相対パス（"/" 区切り）。 */
  rel_path: string;
  /** 絶対パス（ボリュームが分かれば。つながっていないボリュームは最後のマウント先からのパスで、表示用）。 */
  path: string | null;
  /** ボリュームがつながっているか（FILE-03。省けば true）。 */
  online?: boolean;
  /** フォルダが見つからない（ボリュームはつながっているのに、フォルダがない。FILE-04）。 */
  missing?: boolean;
  /** このフォルダ（配下は含まない）の見つからないファイルの数。 */
  missing_files?: number;
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
  /** ビフォー / アフターの表示のビフォー（DEV-31。設定していなければ null。古いコアでは無い）。 */
  before?: BeforeInfo | null;
}

/** ビフォーの設定の出どころ（DEV-31。genzo-api の BeforeSource）。切り抜き・角度は常にアフターと同じ。 */
export type BeforeSource =
  | { kind: "initial" }
  | { kind: "history"; entry_id: HistoryEntryId }
  | { kind: "snapshot"; snapshot_id: SnapshotId }
  | { kind: "current" };

export interface BeforeInfo {
  variant_id: VariantId;
  source: BeforeSource;
  /** 表示名（「読み込み」「履歴: 露光量 +0.30」など）。 */
  label: string;
  process_version: number;
  /** 最後に要求したビフォーの描画の世代。 */
  generation: number;
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
   * 回転で画像の外になった四隅は黒（0）で塗られ、ヒストグラムには数えない。古いコアでは無い（false とみなす）。
   */
  uncropped?: boolean;
  /**
   * この描画の設定での出力の画像（C。切り抜いた後）のフル解像度の幅と高さ（表示範囲の描画の座標に使う）。
   * `uncropped` のときは切り抜く前の全体（G の外接矩形）のフル解像度の大きさ。古いコアでは無い。
   */
  output_width?: number;
  output_height?: number;
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
  /** ビフォーを描いたものか（DEV-31。render_before_region）。古いコアでは無い。 */
  before?: boolean;
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
  /** 取り込みの既定: 重複と思われるものを取り込まない（IMP-04）。 */
  import_skip_duplicates: boolean;
  /** 取り込みの既定: RAW と同時記録の JPEG を別々の写真として取り込む（IMP-06）。 */
  import_separate_raw_jpeg: boolean;
  /** ファイルの内容のハッシュをバックグラウンドで計算する（ORG-03）。 */
  hash_in_background: boolean;
  /** バックグラウンドの計算で、ネットワークのボリュームのファイルも読む。 */
  hash_network_volumes: boolean;
}

export interface SettingsUpdate {
  preview_cache_dir?: string | null;
  preview_cache_bytes?: number | null;
  default_export_color_space?: OutputColorSpace | null;
  default_utc_offset_minutes?: number | null;
  import_skip_duplicates?: boolean | null;
  import_separate_raw_jpeg?: boolean | null;
  hash_in_background?: boolean | null;
  hash_network_volumes?: boolean | null;
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
  /** スタックの数（LIB-12）。古いコアでは無い。 */
  stacks?: number;
}

/** カタログの構造（ORG-07。genzo-api の CatalogSchema。文書は docs/catalog_schema.md）。 */
export interface CatalogSchema {
  schema_version: number;
  application_id: number;
  /** 公開ビュー（列の名前・意味を変えない約束のビュー）。 */
  public_views: string[];
  tables: SchemaTable[];
  objects: SchemaObject[];
}

export interface SchemaTable {
  name: string;
  /** table・virtual（FTS5）・view。 */
  kind: string;
  columns: SchemaColumn[];
}

export interface SchemaColumn {
  name: string;
  decl_type: string;
  not_null: boolean;
  default_value: string | null;
  primary_key: number;
}

export interface SchemaObject {
  /** table・view・index・trigger。 */
  kind: string;
  name: string;
  table: string;
  sql: string;
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
// キーワード・コレクション・スマートコレクション（LIB-09〜11。genzo-api の types.rs・genzo-model の smart.rs）
// ---------------------------------------------------------------------------

/** キーワードの一覧の 1 件（parent_id で木にする）。 */
export interface KeywordInfo {
  keyword_id: KeywordId;
  parent_id: KeywordId | null;
  name: string;
  /** 書き出すファイルのメタデータ（XMP の dc:subject）に書くか。 */
  include_on_export: boolean;
  /** 直接付いている写真の数（子の分は数えない）。 */
  photo_count: number;
}

/** 名前で探したキーワード（補完）。 */
export interface KeywordHit {
  keyword_id: KeywordId;
  name: string;
  /** 最上位から自分までの名前。 */
  path: string[];
}

/** 選択中の写真に付いているキーワード。 */
export interface VariantKeyword {
  keyword_id: KeywordId;
  name: string;
  path: string[];
  /** このキーワードが付いている写真の数（variants より少なければ一部の写真だけ）。 */
  count: number;
}

export interface VariantKeywords {
  variants: number;
  keywords: VariantKeyword[];
}

export interface CollectionRef {
  collection_id: CollectionId;
  name: string;
}

/** キーワードの使われ方（削除の確認）。 */
export interface KeywordUsage {
  photos: number;
  photos_with_descendants: number;
  descendants: number;
  smart_collections: CollectionRef[];
}

export type CollectionKind = "manual" | "smart" | "set";

/** コレクションの一覧の 1 件（parent_id で木にする）。 */
export interface CollectionInfo {
  collection_id: CollectionId;
  parent_id: CollectionId | null;
  name: string;
  kind: CollectionKind;
  rule: SmartRule | null;
  rule_error: string | null;
  /** 写真の数（手動は常に。スマート・セットは with_counts のときだけ。null は数えていない）。 */
  count: number | null;
}

export type MatchMode = "all" | "any";
export type NumberOp = "eq" | "ne" | "gte" | "lte";
export type EqualityOp = "is" | "is_not";
export type KeywordOp = "has" | "not_has" | "any" | "none";
export type DateOp = "range" | "in_last_days" | "unknown";
export type TextOp = "is" | "is_not" | "contains" | "unknown";
export type ContainsOp = "contains" | "not_contains";
export type MemberOp = "in" | "not_in";

/** スマートコレクションの条件の 1 つ（type で種類を表す。group で入れ子）。 */
export type SmartCondition =
  | { type: "group"; match: MatchMode; rules: SmartCondition[] }
  | { type: "rating"; op: NumberOp; value: Rating }
  | { type: "flag"; op: EqualityOp; value: Flag }
  | { type: "color_label"; op: EqualityOp; value: ColorLabel | null }
  | { type: "keyword"; op: KeywordOp; keyword_id?: KeywordId }
  | { type: "capture_time"; op: DateOp; from?: string; until?: string; days?: number }
  | { type: "camera"; op: TextOp; value: string }
  | { type: "lens"; op: TextOp; value: string }
  | { type: "kind"; value: AssetKind }
  | { type: "text"; op: ContainsOp; value: string }
  | { type: "folder"; op: MemberOp; folder_id: FolderId; include_subfolders: boolean }
  | { type: "collection"; op: MemberOp; collection_id: CollectionId };

/** スマートコレクションの条件（最上位。版付きの JSON）。 */
export interface SmartRule {
  version: number;
  match: MatchMode;
  rules: SmartCondition[];
}

// ---------------------------------------------------------------------------
// コマンド（UI → コア。genzo-api の Command）
// ---------------------------------------------------------------------------

export type Command =
  | { type: "import_folder"; path: string; recursive: boolean; options?: ImportOptions }
  | { type: "job"; job_id: number }
  | { type: "wait_job"; job_id: number; timeout_ms: number | null }
  | { type: "cancel_job"; job_id: number }
  | { type: "search"; filter: SearchFilter; sort: SearchSort }
  | { type: "range"; generation: number; start: number; len: number }
  | { type: "index_of"; variant_id: VariantId }
  | { type: "shown_variants"; variant_ids: VariantId[] }
  | { type: "set_rating"; variant_ids: VariantId[]; rating: Rating }
  | { type: "set_flag"; variant_ids: VariantId[]; flag: Flag }
  | { type: "set_color_label"; variant_ids: VariantId[]; label: ColorLabel | null }
  | { type: "set_caption"; variant_ids: VariantId[]; caption: string | null }
  | { type: "set_metadata"; variant_ids: VariantId[]; changes: IptcMetadata }
  | { type: "metadata_summary"; variant_ids: VariantId[] }
  | { type: "variant_details"; variant_id: VariantId }
  | { type: "folders" }
  | { type: "facets" }
  | { type: "check_files"; variant_ids: VariantId[] }
  | { type: "count"; filter: SearchFilter }
  | { type: "keyword_tree" }
  | { type: "find_keywords"; query: string; limit?: number | null }
  | { type: "create_keyword"; parent_id: KeywordId | null; path: string[] }
  | { type: "rename_keyword"; keyword_id: KeywordId; name: string }
  | { type: "move_keyword"; keyword_id: KeywordId; parent_id: KeywordId | null }
  | { type: "delete_keyword"; keyword_id: KeywordId }
  | { type: "set_keyword_export"; keyword_id: KeywordId; include: boolean }
  | { type: "keyword_usage"; keyword_id: KeywordId }
  | { type: "add_keywords"; variant_ids: VariantId[]; keyword_ids: KeywordId[]; paths: string[][] }
  | { type: "remove_keywords"; variant_ids: VariantId[]; keyword_ids: KeywordId[] }
  | { type: "variant_keywords"; variant_ids: VariantId[] }
  | { type: "collections"; with_counts: boolean }
  | {
      type: "create_collection";
      parent_id: CollectionId | null;
      name: string;
      kind: CollectionKind;
      rule: SmartRule | null;
      variant_ids: VariantId[];
    }
  | { type: "rename_collection"; collection_id: CollectionId; name: string }
  | { type: "move_collection"; collection_id: CollectionId; parent_id: CollectionId | null }
  | { type: "delete_collection"; collection_id: CollectionId }
  | { type: "set_smart_rule"; collection_id: CollectionId; rule: SmartRule }
  | { type: "add_to_collection"; collection_id: CollectionId; variant_ids: VariantId[] }
  | { type: "remove_from_collection"; collection_id: CollectionId; variant_ids: VariantId[] }
  | { type: "variant_collections"; variant_id: VariantId }
  | { type: "create_stack"; variant_ids: VariantId[]; top: VariantId | null }
  | { type: "add_to_stack"; stack_id: StackId; variant_ids: VariantId[] }
  | { type: "remove_from_stack"; variant_ids: VariantId[] }
  | { type: "unstack"; variant_ids: VariantId[] }
  | { type: "set_stack_top"; variant_id: VariantId }
  | { type: "set_stacks_collapsed"; stack_ids: StackId[] | null; collapsed: boolean }
  | { type: "variant_stacks"; variant_ids: VariantId[] }
  | { type: "auto_stack"; filter: SearchFilter; gap_seconds: number; dry_run: boolean }
  | { type: "rename_virtual_copy"; variant_id: VariantId; name: string }
  | { type: "set_as_master"; variant_id: VariantId }
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
  | { type: "set_before"; source: BeforeSource | null }
  | { type: "render_before_region"; request: RegionRequest }
  | { type: "wait_for_frame"; generation: number; timeout_ms: number }
  | { type: "flush_develop" }
  | { type: "close_develop" }
  | { type: "undo"; variant_id: VariantId }
  | { type: "redo"; variant_id: VariantId }
  | { type: "go_to_history"; variant_id: VariantId; entry_id: HistoryEntryId }
  | { type: "history"; variant_id: VariantId }
  | { type: "copy_settings"; variant_id: VariantId }
  | {
      type: "paste_settings";
      settings: DevelopSettings;
      targets: VariantId[];
      groups: SettingGroups;
    }
  | { type: "create_virtual_copy"; variant_id: VariantId; name: string | null }
  | { type: "white_balance_at"; variant_id: VariantId; x: number; y: number; uncropped?: boolean }
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
  | { type: "catalog_schema" }
  | { type: "capabilities" }
  | { type: "compute_content_hashes"; include_network?: boolean }
  | { type: "content_hash_status" }
  | { type: "duplicate_groups"; limit?: number }
  | { type: "identify_file"; path: string }
  | { type: "import_name_preview"; copy: CopySettings }
  | { type: "capture_devices" }
  | { type: "capture_device_of"; variant_id: VariantId }
  | { type: "plan_capture_time_edit"; target: CaptureTarget; edit: CaptureTimeEdit }
  | { type: "edit_capture_times"; target: CaptureTarget; edit: CaptureTimeEdit }
  | { type: "capture_time_edits" }
  | { type: "undo_capture_time_edit" }
  | { type: "timeline"; generation: number; gap_seconds: number }
  | { type: "volumes"; refresh?: boolean }
  | { type: "plan_move_files"; variant_ids: VariantId[]; dest_folder: FolderId }
  | { type: "move_files"; variant_ids: VariantId[]; dest_folder: FolderId }
  | { type: "plan_rename_files"; variant_ids: VariantId[]; spec: RenameSpec }
  | { type: "rename_files"; variant_ids: VariantId[]; spec: RenameSpec }
  | { type: "create_folder"; parent_id: FolderId; name: string }
  | { type: "rename_folder"; folder_id: FolderId; name: string }
  | { type: "move_folder"; folder_id: FolderId; parent_id: FolderId }
  | { type: "remove_folder"; folder_id: FolderId }
  | { type: "check_missing"; folder_id?: FolderId | null }
  | { type: "find_relink_candidates"; variant_ids?: VariantId[] | null; search_dir: string; recursive?: boolean }
  | { type: "relink_files"; links: RelinkRequest[] }
  | { type: "plan_relocate_folder"; folder_id: FolderId; path: string }
  | { type: "relocate_folder"; folder_id: FolderId; path: string };

/** 応答（genzo-api の Response）。 */
export type Response =
  | { type: "ok" }
  | { type: "job_started"; job_id: number }
  | ({ type: "job" } & JobInfo)
  | ({ type: "search" } & SearchResult)
  | ({ type: "range" } & RangeResult)
  | ({ type: "index_of" } & IndexOf)
  | ({ type: "shown_variants" } & ShownVariants)
  | { type: "updated"; count: number }
  | ({ type: "details" } & VariantDetails)
  | ({ type: "metadata" } & MetadataSummary)
  | { type: "folders"; folders: FolderInfo[] }
  | ({ type: "facets" } & Facets)
  | ({ type: "file_check" } & FileCheckReport)
  | { type: "count"; count: number }
  | { type: "keywords"; keywords: KeywordInfo[] }
  | { type: "keyword_hits"; hits: KeywordHit[] }
  | { type: "keyword_created"; keyword_id: KeywordId }
  | ({ type: "keyword_usage" } & KeywordUsage)
  | ({ type: "variant_keywords" } & VariantKeywords)
  | { type: "collections"; collections: CollectionInfo[] }
  | { type: "collection_created"; collection_id: CollectionId }
  | { type: "variant_collections"; collection_ids: CollectionId[] }
  | ({ type: "stack" } & StackInfo)
  | { type: "stacks"; stacks: StackInfo[] }
  | ({ type: "auto_stack" } & AutoStackReport)
  | { type: "master_changed"; previous_master: VariantId }
  | { type: "thumbnail"; thumbnail: { variant_id: VariantId; rev: string; jpeg: number[] } | null }
  | { type: "path"; path: string | null }
  | ({ type: "develop" } & DevelopState)
  | { type: "develop_or_none"; state: DevelopState | null }
  | { type: "generation"; generation: number }
  | ({ type: "frame" } & FrameInfo)
  | ({ type: "region" } & RegionInfo)
  | { type: "before"; before: BeforeInfo | null }
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
  | ({ type: "catalog_schema" } & CatalogSchema)
  | ({ type: "capabilities" } & Capabilities)
  | ({ type: "content_hash_status" } & ContentHashStatus)
  | ({ type: "duplicate_groups" } & DuplicateGroupList)
  | ({ type: "file_identity" } & FileIdentity)
  | ({ type: "name_preview" } & NamePreview)
  | { type: "capture_devices"; devices: CaptureDevice[] }
  | ({ type: "capture_device" } & CaptureDeviceKey)
  | ({ type: "capture_time_plan" } & CaptureTimePlan)
  | { type: "capture_time_edits"; edits: CaptureTimeEditInfo[] }
  | { type: "capture_time_undone"; undone: CaptureTimeUndo | null }
  | ({ type: "timeline" } & Timeline)
  | { type: "volumes"; volumes: VolumeInfo[] }
  | ({ type: "move_plan" } & FileMovePlan)
  | ({ type: "rename_plan" } & FileRenamePlan)
  | ({ type: "files_moved" } & FileMoveReport)
  | ({ type: "folder" } & FolderInfo)
  | ({ type: "relinked" } & RelinkReport)
  | ({ type: "relocate_plan" } & RelocatePlan);

// ---------------------------------------------------------------------------
// 撮影日時の一括修正（LIB-16）・タイムライン（ORG-01）
// ---------------------------------------------------------------------------

/** 機材（カメラの表示名・本体のシリアル番号・写真か動画か。genzo-api の CaptureDeviceKey）。 */
export interface CaptureDeviceKey {
  camera: string | null;
  serial: string | null;
  kind: AssetKind;
}

/** 機材と数（genzo-api の CaptureDevice）。 */
export interface CaptureDevice extends CaptureDeviceKey {
  /** 写真・動画の数（仮想コピーは数えない）。 */
  assets: number;
  /** 撮影日時の分かる数。 */
  with_time: number;
  /** 修正（時計のずれの補正・タイムゾーンの指定）のある数。 */
  adjusted: number;
  /** 最も早い・遅い撮影日時（既定のタイムゾーンの時刻。RFC 3339）。 */
  first: string | null;
  last: string | null;
}

/** 修正の対象。 */
export type CaptureTarget =
  | { kind: "variants"; variant_ids: VariantId[] }
  | { kind: "devices"; devices: CaptureDeviceKey[] };

/** タイムゾーンの指定の読み方（keep_local: 記録の時刻をその地域の時刻として読む、keep_instant: 撮影した瞬間はそのまま）。 */
export type ZoneMode = "keep_local" | "keep_instant";

/** 撮影日時の修正（genzo-api の CaptureTimeEdit）。 */
export type CaptureTimeEdit =
  | { kind: "shift"; seconds: number }
  | { kind: "sync"; reference: VariantId; correct: string; offset_minutes?: number | null }
  | { kind: "set_zone"; offset_minutes: number; mode: ZoneMode }
  | { kind: "clear_zone" }
  | { kind: "reset" };

export interface CaptureSourceCounts {
  exif: number;
  utc_only: number;
  user_default: number;
  user_set: number;
}

export interface CaptureTimeExample {
  asset_id: AssetId;
  file_name: string;
  before: CaptureTime;
  after: CaptureTime;
}

/** 修正の見込み・結果（genzo-api の CaptureTimePlan）。 */
export interface CaptureTimePlan {
  assets: number;
  changed: number;
  unchanged: number;
  without_time: number;
  sources: CaptureSourceCounts;
  /** ずらす秒数（ずらす・基準の写真に合わせるとき）。 */
  shift_seconds: number | null;
  examples: CaptureTimeExample[];
  /** 記録した修正（結果のとき。変わったものがなければ null）。 */
  edit_id: number | null;
}

export type CaptureEditKind = "shift" | "sync" | "set_zone" | "clear_zone" | "reset";

export interface CaptureTimeEditInfo {
  id: number;
  kind: CaptureEditKind;
  shift_seconds: number | null;
  zone: string | null;
  zone_mode: ZoneMode | null;
  asset_count: number;
  created_at: string;
}

export interface CaptureTimeUndo {
  edit: CaptureTimeEditInfo;
  restored: number;
}

/** タイムラインのまとまり（撮影日の中のイベント。genzo-api の TimelineGroup）。 */
export interface TimelineGroup {
  /** 検索結果の中の先頭の位置。 */
  start: number;
  count: number;
  /** 撮影日（"2024-05-01"。撮影日時の分からないまとまりは null）。 */
  day: string | null;
  /** その日の最初のまとまりか（日付の見出しを出す）。 */
  new_day: boolean;
  /** 最初・最後の写真の現地時刻（"2024-05-01T09:12:05"）。 */
  first: string | null;
  last: string | null;
  photos: number;
  videos: number;
}

/** タイムライン（genzo-api の Timeline）。 */
export interface Timeline {
  generation: number;
  gap_seconds: number;
  groups: TimelineGroup[];
  /** まとまりが多すぎるため、日付だけでまとめたか。 */
  events_merged: boolean;
  days: number;
  without_time: number;
}

// ---------------------------------------------------------------------------
// イベント（コア → UI。genzo-api の Event。Tauri のイベント名は "core-event"）
// ---------------------------------------------------------------------------

export type CatalogChange =
  | "imported"
  | "marks"
  | "caption"
  | "metadata"
  | "develop"
  | "variant_added"
  | "removed"
  | "file_status"
  | "capture_time"
  | "keywords"
  | "collections"
  | "stacks"
  | "variants"
  | "files"
  | "folders";

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
  | "render_deps_changed"
  | "import_copy_rolled_back"
  | "volumes_rebound"
  | "move_source_left";

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
  | { type: "before_updated"; variant_id: VariantId; generation: number; width: number; height: number }
  | { type: "thumbnail_updated"; variant_id: VariantId; rev: string }
  | { type: "search_updated"; generation: number; count: number; hidden?: number }
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
  /** 取り込みのダイアログの設定（古いアプリでは無い）。 */
  import?: ImportPreferences;
}

/** 取り込みのダイアログの設定（genzo-app の ImportPreferences。アプリに保存し、次に開いたときの初めの値）。 */
export interface ImportPreferences {
  copy: boolean;
  recursive: boolean;
  destination: string | null;
  folder_rule: FolderRule;
  rename: boolean;
  rename_rule: RenameRule;
  metadata: ImportMetadata;
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
  /** 既定のカタログの場所が OneDrive で同期されるフォルダの中か（Windows だけ。ほかの OS では常に false）。 */
  default_catalog_in_onedrive: boolean;
  catalog: CatalogStatus | null;
  tools: ExternalTools;
  /** ログ（SYS-07）。 */
  logs: LogStatus;
  /** 前回までのアプリのクラッシュレポートのうち、まだ知らせていないもの（新しい順。起動して最初の 1 回だけ）。 */
  crash_reports: CrashReportSummary[];
}

// ---------------------------------------------------------------------------
// ログ・クラッシュレポート（SYS-07。genzo-api の logging。外部へは送信しない）
// ---------------------------------------------------------------------------

export interface LogStatus {
  /** ログのフォルダ（分からなければ null）。 */
  dir: string | null;
  /** 今書いているログのファイル。 */
  file: string | null;
  /** ファイルに書く設定か。 */
  file_enabled: boolean;
  /** 初期化での問題。 */
  problems: string[];
}

export interface CrashReportSummary {
  path: string;
  file_name: string;
  /** 起きた日時（UTC。読めなければ null）。 */
  created_at: string | null;
  /** app・cli。 */
  program: string;
  /** パニックの内容（1 行目）。 */
  message: string | null;
}

export interface LogFileInfo {
  name: string;
  bytes: number;
  modified: string | null;
}

export interface LogFolderInfo {
  dir: string;
  exists: boolean;
  files: LogFileInfo[];
  crash_reports: CrashReportSummary[];
  total_bytes: number;
  current_file: string | null;
}

export interface ClearReport {
  removed: number;
  truncated: number;
  failed: string[];
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

// ---------------------------------------------------------------------------
// ボリューム（FILE-03）・アプリの中でのファイル操作（FILE-02）・見つからない写真と再リンク（FILE-04）
// ---------------------------------------------------------------------------

/** ボリューム（ドライブ）。genzo-api の `VolumeInfo`。 */
export interface VolumeInfo {
  volume_id: VolumeId;
  /** ボリュームの ID（`uuid:`・`winvol:`・`net:` は OS のボリューム ID、`path:` はパスの先頭から作ったもの）。 */
  uuid: string;
  label: string | null;
  /** マウント先（つながっていれば今のもの、つながっていなければ最後に確かめたもの）。 */
  mount_path: string | null;
  online: boolean;
  /** OS のボリューム ID で識別しているか。 */
  identified: boolean;
  network: boolean;
  fs_type: string | null;
  last_seen_at: string | null;
}

export type MoveMethod = "rename" | "copy";

export interface PlannedFileMove {
  file_id: FileId;
  role: FileRole;
  from: string | null;
  to: string | null;
  method: MoveMethod;
  size: number;
}

export interface PlannedPhotoMove {
  variant_id: VariantId;
  asset_id: AssetId;
  files: PlannedFileMove[];
  /** 移せない・名前を変えられない理由（あれば、この写真は飛ばす）。 */
  problem: string | null;
}

export interface FileMovePlan {
  dest_folder: FolderId;
  dest_path: string | null;
  photos: PlannedPhotoMove[];
  movable: number;
  copy_bytes: number;
}

export interface MovedFile {
  file_id: FileId;
  from: string;
  to: string;
}

export interface FileMoveReport {
  moved: MovedFile[];
  photos: number;
  skipped: FileIssue[];
  failed: FileIssue[];
  left_sources: FileIssue[];
  bytes_copied: number;
  cancelled: boolean;
}

/** 写真の名前の付け方（genzo-api の `RenameSpec`）。 */
export type RenameSpec = { kind: "template"; rule: RenameRule } | { kind: "name"; name: string };

export interface FileRenamePlan {
  photos: PlannedPhotoMove[];
  renamable: number;
}

export interface RelocatePlan {
  folder_id: FolderId;
  from: string | null;
  to: string;
  files: number;
  found: number;
  subfolders: number;
  problem: string | null;
}

export interface MissingCheckReport {
  checked: number;
  newly_missing: number;
  restored: number;
  missing: number;
  missing_folders: number;
  skipped_offline: number;
  skipped_network: number;
  cancelled: boolean;
}

/** 再リンクの候補の一致の仕方（確からしい順）。 */
export type RelinkMatch = "content" | "quick" | "name_and_time";

export interface RelinkCandidate {
  path: string;
  matched: RelinkMatch;
  size: number;
  same_name: boolean;
}

export interface RelinkTarget {
  file_id: FileId;
  variant_id: VariantId;
  role: FileRole;
  name: string;
  last_path: string | null;
  size: number;
  candidates: RelinkCandidate[];
}

export interface RelinkSearch {
  search_dir: string;
  targets: RelinkTarget[];
  scanned_files: number;
  unreadable: FileIssue[];
  cancelled: boolean;
}

export interface RelinkRequest {
  file_id: FileId;
  path: string;
  /** 内容が登録のものと違っても結び付けるか（name_and_time の候補を利用者が選んだとき）。 */
  accept_changed?: boolean;
}

export interface RelinkReport {
  linked: MovedFile[];
  reread: string[];
  failed: FileIssue[];
}
