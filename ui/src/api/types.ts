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
  | "network"
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
// 動画の再生（VID-04。genzo-api の playback）
// ---------------------------------------------------------------------------

/** 動画の再生の情報（genzo-api の VideoPlayback）。 */
export interface VideoPlayback {
  variant_id: VariantId;
  /** 元のファイルを読めるか（見つからない・ドライブが外れているなら false）。 */
  available: boolean;
  unavailable_reason: string | null;
  /** 元のファイルの MIME の型（video/mp4・video/quicktime）。 */
  content_type: string;
  size: number | null;
  video: VideoMetadata | null;
  /** 今の内容の再生用のファイル（H.264 ＋ AAC の MP4）。なければ null。 */
  proxy: VideoProxyInfo | null;
  /** 再生用のファイルを作っているジョブ。 */
  proxy_job: number | null;
  /** 再生用のファイルを作れない理由（ffmpeg がないなど。作れるなら null）。 */
  proxy_unavailable: string | null;
  /** 再生用のファイルを作るエンコーダー（ffmpeg の名前）。 */
  proxy_encoder: string | null;
}

export interface VideoProxyInfo {
  size: number;
  long_edge: number;
}

/** 再生用のファイルの作成の結果（ジョブの結果）。 */
export interface VideoProxyReport {
  variant_id: VariantId;
  size: number;
  reused: boolean;
  encoder: string | null;
}

/** 再生用のファイルのキャッシュの状況。 */
export interface VideoProxyCacheStatus {
  dir: string;
  count: number;
  total_bytes: number;
  capacity_bytes: number;
  removed: number;
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

/** カラーミキサー・白黒ミキサーの色相の帯（genzo-model の HueBand。色相の順）。 */
export type HueBand = "red" | "orange" | "yellow" | "green" | "aqua" | "blue" | "purple" | "magenta";

/** 帯の順（genzo-model の HueBand::ALL）。 */
export const HUE_BANDS: readonly HueBand[] = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"];

/** 色相の帯ごとの値（JSON は帯の名前のオブジェクト）。 */
export type HueBands<T> = Record<HueBand, T>;

/** カラーミキサーの 1 つの帯の調整（各 -100〜100）。 */
export interface HslAdjustment {
  hue: number;
  saturation: number;
  luminance: number;
}

/** カラーグレーディングの 1 つの範囲（色相環）。 */
export interface GradingWheel {
  /** 色相（度。0〜360。sRGB の色相環で 0 がレッド）。 */
  hue: number;
  /** 強さ（0〜100）。 */
  saturation: number;
  /** 輝度（-100〜100）。 */
  luminance: number;
}

/** カラーグレーディング（DEV-10。genzo-model の ColorGrading。処理バージョン 2 から使える）。 */
export interface ColorGrading {
  shadows: GradingWheel;
  midtones: GradingWheel;
  highlights: GradingWheel;
  global: GradingWheel;
  /** ブレンド（0〜100。既定 50）。 */
  blending: number;
  /** バランス（-100〜100。正でハイライトの範囲を広げる）。 */
  balance: number;
}

/** 白黒（DEV-25。genzo-model の BlackWhite。処理バージョン 2 から使える）。 */
export interface BlackWhite {
  enabled: boolean;
  /** 白黒ミキサー（帯ごとの明るさ。-100〜100）。 */
  mix: HueBands<number>;
}

export interface ColorParams {
  saturation: number;
  vibrance: number;
  /** カラーミキサー（DEV-09。古いコアの応答では欠けうる）。 */
  mixer: HueBands<HslAdjustment>;
  /** カラーグレーディング（DEV-10。古いコアの応答では欠けうる）。 */
  grading: ColorGrading;
  /** 白黒（DEV-25。古いコアの応答では欠けうる）。 */
  black_white: BlackWhite;
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

/** レンズ補正（DEV-14。genzo-model の LensCorrection。処理バージョン 2 から使える）。 */
export interface LensCorrection {
  /** プロファイル（RAW に埋め込まれた補正の情報）による補正（歪曲・周辺減光）を有効にする。 */
  profile_enabled: boolean;
  /** 歪曲補正の適用量（%。0〜200、100 でプロファイルどおり）。 */
  distortion_scale: number;
  /** 周辺減光補正の適用量（%。0〜200、100 でプロファイルどおり）。 */
  vignetting_scale: number;
  /** 倍率色収差を除去する（プロファイルの色収差の情報を使う）。 */
  remove_chromatic_aberration: boolean;
  /** 手動の歪曲補正（-100〜100。正で樽型、負で糸巻き型の歪曲を補正する）。 */
  distortion: number;
  /** 手動の周辺減光の補正量（-100〜100。正で四隅を明るくする）。 */
  vignetting_amount: number;
  /** 手動の周辺減光の中点（0〜100。既定 50。小さいほど中心に近い位置から補正する）。 */
  vignetting_midpoint: number;
  /** 手動の倍率色収差の補正の赤 / シアン（-100〜100）。 */
  ca_red_cyan: number;
  /** 手動の倍率色収差の補正の青 / 黄（-100〜100）。 */
  ca_blue_yellow: number;
}

/** ノイズ軽減（DEV-13。genzo-model の NoiseReduction。処理バージョン 2 から使える。RAW だけ）。 */
export interface NoiseReduction {
  /** 輝度ノイズの軽減（0〜100）。 */
  luminance: number;
  /** 輝度ノイズの軽減のディテール（0〜100。既定 50）。 */
  luminance_detail: number;
  /** 輝度ノイズの軽減のコントラスト（0〜100。既定 0）。 */
  luminance_contrast: number;
  /** カラーノイズの軽減（0〜100）。 */
  color: number;
  /** カラーノイズの軽減のディテール（0〜100。既定 50）。 */
  color_detail: number;
}

/** シャープ（DEV-12。genzo-model の Sharpening。処理バージョン 2 から使える）。 */
export interface Sharpening {
  /** 適用量（0〜150）。 */
  amount: number;
  /** 半径（0.5〜3.0。写真のフル解像度の画素。既定 1.0）。 */
  radius: number;
  /** ディテール（0〜100。既定 25。小さいほど細かい模様を強めず、輪郭のにじみを抑える）。 */
  detail: number;
  /** マスク（0〜100。既定 0。大きいほど平らな部分にかけない）。 */
  masking: number;
}

export interface DetailParams {
  sharpening: Sharpening;
  noise_reduction: NoiseReduction;
}

/** テクスチャ・明瞭度・かすみの除去（DEV-11。genzo-model の PresenceParams。各 -100〜100。処理バージョン 2 から）。 */
export interface PresenceParams {
  texture: number;
  clarity: number;
  dehaze: number;
}

/** 切り抜き後の周辺光量補正（DEV-24。genzo-model の PostCropVignette。処理バージョン 2 から）。 */
export interface PostCropVignette {
  /** 量（-100〜100。負で暗く、正で明るく）。 */
  amount: number;
  /** 中点（0〜100。既定 50）。 */
  midpoint: number;
  /** 丸み（-100〜100。既定 0）。 */
  roundness: number;
  /** ぼかし（0〜100。既定 50）。 */
  feather: number;
  /** ハイライトの保護（0〜100。既定 0）。 */
  highlights: number;
  /**
   * 手動の中心（切り抜く前の画像の幅・高さに対する割合。各軸 0〜1）。ない（null）なら切り抜きの中心
   * （切り抜きを動かすと一緒に動く）。コアは、ないときはこの項目を返さない。
   */
  center?: VignetteCenter | null;
}

/** 切り抜き後の周辺光量補正の手動の中心（genzo-model の VignetteCenter。座標は切り抜きと同じ）。 */
export interface VignetteCenter {
  x: number;
  y: number;
}

/** 粒子（DEV-24。genzo-model の Grain。処理バージョン 2 から）。 */
export interface Grain {
  /** 量（0〜100）。 */
  amount: number;
  /** 大きさ（0〜100。既定 25）。 */
  size: number;
  /** 粗さ（0〜100。既定 50）。 */
  roughness: number;
}

/** 効果（DEV-24。genzo-model の EffectsParams）。 */
export interface EffectsParams {
  vignette: PostCropVignette;
  grain: Grain;
}

/** ハイライト復元の方式（genzo-model の HighlightMethod）。 */
export type HighlightMethod = "clip" | "neutral" | "reconstruct";

/** ハイライト復元（DEV-06。genzo-model の HighlightRecovery。処理バージョン 2 から使える。RAW だけ）。 */
export interface HighlightRecovery {
  /** 方式（clip: 白で切りそろえる（既定）・neutral: 明るさだけ・reconstruct: 色を保って復元）。 */
  method: HighlightMethod;
  /** 量（0〜100。既定 100。clip では使わない）。 */
  amount: number;
}

export interface RenderDeps {
  camera_profile: { id: string; sha256: string };
  lens_profile: { id: string; sha256: string } | null;
  raw_decoder: string;
  /** 使ったカメラプロファイルのファイル（DEV-17。ID は `dcp:<ID>`。使っていなければ無い）。 */
  profile?: { id: string; sha256: string } | null;
  /** 使ったルックのファイル（ID は `cube:<ID>`）。 */
  look?: { id: string; sha256: string } | null;
}

/**
 * カメラプロファイル・ルック（DEV-17。genzo-model の ProfileSettings。処理バージョン 2 から使える）。ID は取り込んだ
 * ライブラリの ID（genzo-api の ProfileEntry の id）。
 */
export interface ProfileSettings {
  /** カメラプロファイル（DCP）の ID（null で標準の色。RAW だけ）。 */
  camera: string | null;
  /** ルック（3D LUT の .cube）の ID（null でルックなし）。 */
  look: string | null;
  /** ルックの適用量（0〜100。既定 100）。 */
  look_amount: number;
}

/** 取り込んだものの種類（genzo-api の ProfileKind）。 */
export type ProfileKind = "camera" | "look";

/** 取り込んだカメラプロファイル・ルック（genzo-api の ProfileEntry）。 */
export interface ProfileEntry {
  kind: ProfileKind;
  /** ID（現像設定に保存する値。元のファイルの名前から拡張子を除いたもの）。 */
  id: string;
  /** 表示の名前（DCP の ProfileName・.cube の TITLE。なければ ID）。 */
  name: string;
  /** DCP の機種（UniqueCameraModel）。 */
  camera_model: string | null;
  /** 一覧を求めたときに渡した機種用か（カメラプロファイルだけ。機種を渡さなかった・ルックは null）。 */
  matches_camera: boolean | null;
  /** 使えるか（取り込んだ写しがない・内容が違う・解析できないなら false）。 */
  usable: boolean;
  /** 中身の説明。 */
  description: string;
  /** 使わない情報（DCP のトーンカーブなど）の説明。 */
  notes: string[];
  /** 著作権の表示（DCP の ProfileCopyright。そのまま表示する）。 */
  copyright: string | null;
  /** 埋め込みの方針（DCP の ProfileEmbedPolicy の説明）。 */
  embed_policy: string | null;
  original_name: string;
  sha256: string;
  size: number;
  /** 取り込んだ日時（UTC。RFC 3339）。 */
  imported_at: string;
}

/** 取り込めなかったファイル。 */
export interface ProfileImportFailure {
  path: string;
  message: string;
}

/** 取り込みの結果（genzo-api の ProfileImportReport）。 */
export interface ProfileImportReport {
  imported: ProfileEntry[];
  /** 同じ ID のものを置き換えた（使っている写真は、次に開いたときに「内容が違う」と知らせる）。 */
  replaced: ProfileEntry[];
  /** 同じ ID・同じ内容のものがあり、消えた・壊れた写しを書き直した（使えるようになった）。 */
  repaired: ProfileEntry[];
  /** 同じ ID・同じ内容のものがあった。 */
  unchanged: ProfileEntry[];
  failed: ProfileImportFailure[];
}

export interface DevelopSettings {
  schema_version: number;
  process_version: number;
  white_balance: WhiteBalance;
  /** ハイライト復元（DEV-06。古いコアの応答では欠けうる）。 */
  highlight_recovery: HighlightRecovery;
  /** 露光量（EV。-5〜5）。 */
  exposure_ev: number;
  /** コントラスト（-100〜100）。 */
  contrast: number;
  tone: ToneParams;
  /** テクスチャ・明瞭度・かすみの除去（DEV-11。古いコアの応答では欠けうる）。 */
  presence: PresenceParams;
  tone_curve: ToneCurve;
  color: ColorParams;
  geometry: Geometry;
  lens: LensCorrection;
  detail: DetailParams;
  /** 効果（切り抜き後の周辺光量補正・粒子。DEV-24。古いコアの応答では欠けうる）。 */
  effects: EffectsParams;
  /** カメラプロファイル・ルック（DEV-17。古いコアの応答では欠けうる）。 */
  profile?: ProfileSettings;
  /** ローカル補正（マスク。DEV-18・DEV-19。処理バージョン 2 から使える）。 */
  masks: LocalAdjustment[];
  /** スポット修正（DEV-21。処理バージョン 2 から使える。空ならコアは送らない）。 */
  retouch?: RetouchSpot[];
  render_deps: RenderDeps;
}

// ---------------------------------------------------------------------------
// スポット修正（genzo-model の develop::retouch。DEV-21）
//
// 位置・大きさ・コピー元のずれは O 座標（RAW の向きを反映した画像の長辺を 1 とする座標。左上が (0, 0)。角度補正・
// 歪曲補正・切り抜きの前）。画面の画像の上の位置との対応は FrameInfo.o_grid（この画像の上の点の O 座標の表）で求める。
// ---------------------------------------------------------------------------

/** スポットの直し方（heal: 修復、clone: コピースタンプ）。 */
export type RetouchMode = "heal" | "clone";

/** スポット修正 1 つ分（genzo-model の RetouchSpot）。 */
export interface RetouchSpot {
  enabled: boolean;
  mode: RetouchMode;
  /** 消したい場所の点の列（O 座標の [x, y]）。1 点なら円、2 点以上なら線。 */
  points: [number, number][];
  /** 大きさ（半径。O 座標。RANGES.retouchSize）。 */
  size: number;
  /** ぼかし（0〜100）。 */
  feather: number;
  /** 不透明度（0〜100）。 */
  opacity: number;
  /** コピー元のずれ（O 座標。コピー元 = 消したい場所 + ずれ）。 */
  offset: [number, number];
}

/** スポット修正の数などの上限（genzo-model の develop::retouch の定数）。 */
export const RETOUCH_LIMITS = {
  spots: 1000,
  points: 5000,
  totalPoints: 100_000,
} as const;

/** FrameInfo.o_grid の一辺の点の数（genzo-pipeline の retouch::ORIENTED_GRID_SIZE）。 */
export const ORIENTED_GRID_SIZE = 33;

// ---------------------------------------------------------------------------
// ローカル補正（マスク。genzo-model の develop::masks。DEV-18・DEV-19）
//
// 位置・半径・ブラシの大きさは G 座標（ジオメトリ補正後・切り抜く前の全体の外接矩形の長辺を 1 とする座標。左上が
// (0, 0)）。画面の画像の上の位置との対応は FrameInfo.g_bounds（この画像の縁の G 座標）で求める。
// ---------------------------------------------------------------------------

/** 部品の合成のしかた（加える: max、除く: m · (1 − c)、交差: m · c）。 */
export type MaskMode = "add" | "subtract" | "intersect";

/** ブラシのストローク（genzo-model の BrushStroke）。 */
export interface BrushStroke {
  /** 消しゴム（重みを減らす）か。 */
  erase: boolean;
  /** 大きさ（半径。G 座標。RANGES.brushSize）。 */
  size: number;
  /** ぼかし（0〜100）。 */
  feather: number;
  /** 流量（0〜100）。 */
  flow: number;
  /** 密度（0〜100）。 */
  density: number;
  /** 点の列（G 座標の [x, y]）。1 点なら円。 */
  points: [number, number][];
}

/** 色域の範囲の見本の色（genzo-model の ColorSample。スポイトの結果 PickedMaskColor をそのまま使う）。 */
export interface ColorSample {
  x: number;
  y: number;
  /** OKLab（露光量の前のシーンリニアの値から求めたもの）。 */
  lab: [number, number, number];
}

/** 線形グラデーション: (x0, y0) を通る線で 100%、(x1, y1) を通る線で 0%。 */
export interface LinearShape {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** 円形グラデーション（楕円の内側で 100%。angle_deg は時計回り。feather は縁から内側の割合）。 */
export interface RadialShape {
  cx: number;
  cy: number;
  rx: number;
  ry: number;
  angle_deg: number;
  feather: number;
}

/** 色域の範囲（DEV-19）。 */
export interface ColorRangeShape {
  samples: ColorSample[];
  /** 許容量（0〜100）。 */
  amount: number;
}

/** 輝度の範囲（DEV-19。表示の明るさの百分率）。 */
export interface LuminanceRangeShape {
  min: number;
  max: number;
  smoothness: number;
}

/** 部品の形（genzo-model の MaskShape。外部タグの JSON）。 */
export type MaskShape =
  | { linear: LinearShape }
  | { radial: RadialShape }
  | { brush: { strokes: BrushStroke[] } }
  | { color_range: ColorRangeShape }
  | { luminance_range: LuminanceRangeShape };

/** 部品（genzo-model の MaskComponent）。 */
export interface MaskComponent {
  mode: MaskMode;
  invert: boolean;
  shape: MaskShape;
}

/**
 * ローカル補正の調整値の差分（genzo-model の LocalDelta）。露光量は EV（−5〜5）、ほかは −100〜+100。マスクの重み
 * を掛けて全体の値に足す。
 */
export interface LocalDelta {
  temperature: number;
  tint: number;
  exposure_ev: number;
  contrast: number;
  highlights: number;
  shadows: number;
  whites: number;
  blacks: number;
  texture: number;
  clarity: number;
  dehaze: number;
  saturation: number;
  sharpness: number;
}

/** マスク 1 つ分（genzo-model の LocalAdjustment）。 */
export interface LocalAdjustment {
  enabled: boolean;
  /** 名前（空なら「マスク 1」などの番号で表示する。最大 MAX_MASK_NAME_CHARS 文字）。 */
  name: string;
  components: MaskComponent[];
  delta: LocalDelta;
}

/** マスクの数などの上限（genzo-model の develop::masks の定数）。 */
export const MASK_LIMITS = {
  masks: 100,
  components: 32,
  strokes: 1000,
  strokePoints: 5000,
  totalBrushPoints: 100_000,
  colorSamples: 5,
  nameChars: 100,
} as const;

/** 色域のスポイトの結果（genzo-api の PickedMaskColor）。 */
export interface PickedMaskColor {
  x: number;
  y: number;
  lab: [number, number, number];
}

/** マスクの重ねて表示の画像の情報（genzo-api の MaskOverlayInfo。画素は mask_overlay_frame で取る）。 */
export interface MaskOverlayInfo {
  variant_id: VariantId;
  generation: number;
  index: number;
  width: number;
  height: number;
  uncropped: boolean;
}

/** 値の範囲（genzo-model の develop::ranges）。 */
export const RANGES = {
  exposureEv: { min: -5, max: 5 },
  slider: { min: -100, max: 100 },
  amount: { min: 0, max: 100 },
  temperatureK: { min: 2000, max: 50000 },
  tint: { min: -150, max: 150 },
  angleDeg: { min: -45, max: 45 },
  /** レンズ補正のプロファイルの適用量（%）。 */
  lensScale: { min: 0, max: 200 },
  /** レンズ補正の手動の補正（歪曲・周辺減光の量・色収差）。 */
  lensManual: { min: -100, max: 100 },
  /** 手動の周辺減光の中点。 */
  lensMidpoint: { min: 0, max: 100 },
  /** カラーグレーディングの色相（度）。 */
  hueDeg: { min: 0, max: 360 },
  /** シャープの適用量。 */
  sharpenAmount: { min: 0, max: 150 },
  /** シャープの半径（写真のフル解像度の画素）。 */
  sharpenRadius: { min: 0.5, max: 3 },
  /** マスクの位置（G 座標。画像の外側も少し許す）。 */
  maskCoord: { min: -1, max: 2 },
  /** 円形マスクの半径（G 座標）。 */
  maskRadius: { min: 0, max: 2 },
  /** 円形マスクの回転（度）。 */
  maskAngleDeg: { min: -180, max: 180 },
  /** ブラシの大きさ（半径。G 座標）。 */
  brushSize: { min: 0.0005, max: 0.5 },
  /** スポット修正の位置（O 座標）。 */
  retouchCoord: { min: -1, max: 2 },
  /** スポット修正の大きさ（半径。O 座標）。 */
  retouchSize: { min: 0.0005, max: 0.25 },
  /** スポット修正のコピー元のずれ（O 座標）。 */
  retouchOffset: { min: -2, max: 2 },
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
  /** テクスチャ・明瞭度・かすみの除去（DEV-11）。 */
  presence: boolean;
  tone_curve: boolean;
  color: boolean;
  geometry: boolean;
  lens: boolean;
  detail: boolean;
  /** 効果（切り抜き後の周辺光量補正・粒子。DEV-24）。 */
  effects: boolean;
  /** カメラプロファイル・ルック・ルックの適用量（DEV-17。古い形では欠け、コアは写す）。 */
  profile?: boolean;
  masks: boolean;
  /** スポット修正（DEV-21。欠けていればコアは写さない）。 */
  retouch?: boolean;
}

/**
 * 貼り付け先の処理バージョンの扱い（genzo-api の PasteVersion。DATA-09）。keep: 貼り付け先の版のまま（既定）、
 * upgrade: 貼り付け元より古ければ貼り付け元の版に上げる（利用者が選んだとき）、source: 貼り付け元の版にする。
 */
export type PasteVersion = "keep" | "upgrade" | "source";

/** 設定の貼り付けの結果（genzo-api の PasteReport）。 */
export interface PasteReport {
  /** 適用した写真の数。 */
  count: number;
  /** 処理バージョンを上げた写真の数。 */
  upgraded: number;
  /** 処理バージョンが古いため、写した項目の一部（レンズ補正など）が使われない写真の数。 */
  needs_upgrade: number;
  /** 除いた動画の数（現像プリセットの適用。設定の貼り付けでは 0。古いコアは送らない）。 */
  skipped_videos?: number;
}

// ---------------------------------------------------------------------------
// 書き出し（genzo-model の ExportSettings）
// ---------------------------------------------------------------------------

export type ExportFormat = { jpeg: { quality: number } } | "tiff16" | "png8" | "png16";
export type ExportSize = "original" | { long_edge: number };
export type ConflictPolicy = "sequence" | "overwrite" | "skip";

/** 出力シャープの対象（EXP-03。genzo-model の SharpenMedia）。 */
export type SharpenMedia = "screen" | "matte_paper" | "glossy_paper";
/** 出力シャープの適用量（genzo-model の SharpenAmount）。 */
export type SharpenAmount = "low" | "standard" | "high";

/** 出力シャープ（EXP-03。縮小の後に、出力の画素の大きさに合わせてかける）。 */
export interface OutputSharpening {
  media: SharpenMedia;
  amount: SharpenAmount;
}

/** 透かしの位置（写真の 9 か所。genzo-model の WatermarkAnchor）。 */
export type WatermarkAnchor =
  | "top_left"
  | "top"
  | "top_right"
  | "left"
  | "center"
  | "right"
  | "bottom_left"
  | "bottom"
  | "bottom_right";

/**
 * 透かしの中身（genzo-model の WatermarkContent）。文字のフォントは利用者の OS のフォントのファイル（アプリは同梱
 * しない）。色は sRGB の "#RRGGBB"。
 */
export type WatermarkContent =
  | { text: { text: string; font: string; font_index?: number; color?: string } }
  | { image: { path: string } };

/**
 * 透かし（EXP-03。genzo-model の Watermark）。大きさ（文字は文字の大きさ、画像は幅）・余白は写真の短辺に対する %。
 */
export interface Watermark {
  content: WatermarkContent;
  anchor?: WatermarkAnchor;
  /** 1〜100 %。 */
  size: number;
  /** 1〜100 %（省略時 60）。 */
  opacity?: number;
  /** 0〜25 %（省略時 2）。 */
  margin?: number;
}

/** 動画の書き出し（EXP-05。copy: 元のファイルのまま再エンコードせずにコピーする）。 */
export type VideoExport = "copy" | "skip";

export interface ExportSettings {
  format: ExportFormat;
  color_space: OutputColorSpace;
  size: ExportSize;
  remove_gps: boolean;
  on_conflict: ConflictPolicy;
  /** キーワード（XMP の dc:subject）を書かない（LIB-09。省略時は書く）。 */
  remove_keywords?: boolean;
  /** 解像度（ppi。1〜1200。省略時 72。EXP-03）。 */
  resolution_ppi?: number;
  /** 出力シャープ（EXP-03。null・省略ならかけない）。 */
  output_sharpening?: OutputSharpening | null;
  /** 透かし（EXP-03。null・省略なら付けない）。 */
  watermark?: Watermark | null;
  /** 動画の扱い（EXP-05。省略時 copy。GPS を削除する設定では書き出さない）。 */
  videos?: VideoExport;
}

/** 透かしの見本（genzo-api の WatermarkPreview）。 */
export interface WatermarkPreview {
  width: number;
  height: number;
  /** PNG（sRGB）の base64。 */
  png_base64: string;
  mark_width: number;
  mark_height: number;
  /** フォントにない文字・縮めたなど。 */
  warnings: string[];
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
  | "find_relink_candidates"
  | "video_proxy"
  | "composite";
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
  | { status: "copied"; path: string; bytes: number; full_hash: string; warnings: string[] }
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
  /** コピーした動画の数（EXP-05。古いコアの応答にはない）。 */
  copied?: number;
  skipped: number;
  failed: number;
  cancelled: boolean;
}

// ---------------------------------------------------------------------------
// 比較明・比較暗合成（DEV-34。docs/composite_design.md）
// ---------------------------------------------------------------------------

/** 合成の比べ方（genzo-model の CompositeMode）。 */
export type CompositeMode = "lighten" | "darken" | "lighter_color" | "darker_color";

/** 合成の設定（genzo-model の CompositeSettings。省いた項目はコアの既定値）。 */
export interface CompositeSettings {
  mode: CompositeMode;
  format: ExportFormat;
  color_space: OutputColorSpace;
  remove_gps: boolean;
  stack_with_sources: boolean;
}

/** 合成する前の確認（genzo-api の CompositeCheck）。 */
export interface CompositeCheck {
  /** 合成する写真（撮影日時の順）。 */
  sources: VariantId[];
  skipped_videos: number;
  /** 合成できない理由（空なら合成できる）。 */
  problems: string[];
  missing: VariantId[];
  size_differs: VariantId[];
  geometry_differs: VariantId[];
  settings_differ: VariantId[];
  /** 合成の元の写真が入っているスタックの数。 */
  stacks: number;
  estimated_buffer_bytes: number;
  dest_dir: string | null;
  file_name: string | null;
}

export interface CompositeWarning {
  variant_id: VariantId;
  message: string;
}

export interface CompositeFailure {
  variant_id: VariantId | null;
  error: ErrorInfo;
}

/** 合成の結果（genzo-api の CompositeReport）。 */
export interface CompositeReport {
  mode: CompositeMode;
  sources: VariantId[];
  skipped_videos: number;
  composited: number;
  /** 幅・高さ。 */
  size: [number, number] | null;
  path: string | null;
  registered: VariantId | null;
  registration_error: string | null;
  stacked: boolean;
  stack_error: string | null;
  cpu_renders: number;
  warnings: CompositeWarning[];
  failure: CompositeFailure | null;
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
      /** 削除した動画の再生用のファイル（VID-04）の数（古いコアでは省かれる）。 */
      video_proxies_removed?: number;
    }
  | ({ type: "hash_files" } & HashReport)
  | ({ type: "move_files" } & FileMoveReport)
  | ({ type: "check_missing" } & MissingCheckReport)
  | ({ type: "relink_candidates" } & RelinkSearch)
  | ({ type: "video_proxy" } & VideoProxyReport)
  | ({ type: "composite" } & CompositeReport);

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
  /**
   * 使う位置（LIB-21。後から付けた位置があればそれ、なければカメラ（ファイル）の位置。書き出しの Exif に入る位置と
   * 同じ）。
   */
  gps: GpsCoord | null;
  /** gps の出どころ（LIB-21。位置がなければ null）。古いコアでは無い。 */
  gps_source?: GpsSource | null;
  /** カメラ（ファイル）が記録した位置（LIB-21。後から付けた位置を使っていても、ファイルの値）。古いコアでは無い。 */
  camera_gps?: GpsCoord | null;
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
  /**
   * RAW に埋め込まれたレンズ補正の情報（DEV-14。Sony の ARW のタグ・DNG の opcode）の要約。情報がない・
   * RAW 以外は null（手動の補正だけを使える。古いコアでは無い）。
   */
  lens_correction?: LensCorrectionInfo | null;
  /**
   * 埋め込みのレンズ補正の情報が読めない・不正だったため使わなかった理由（genzo-api の SourceInfo の
   * lens_correction_note）。もともと情報がない場合と区別して出す。読めた・なかった・RAW 以外・古いコアでは無い。
   */
  lens_correction_note?: string | null;
  /** 機種（メーカーと機種の名前。RAW だけ。カメラプロファイルの UniqueCameraModel と比べる。DEV-17）。 */
  camera?: string | null;
}

/** 埋め込みのレンズ補正の情報の要約（genzo-api の LensCorrectionInfo）。 */
export interface LensCorrectionInfo {
  /** 出どころ（"sony_arw"・"dng_opcode"）。 */
  source: string;
  /** 説明（例: "Sony の ARW（歪曲・周辺減光・色収差）"）。 */
  description: string;
  /** 歪曲の情報があるか。 */
  distortion: boolean;
  /** 周辺減光の情報があるか。 */
  vignetting: boolean;
  /** 倍率色収差の情報があるか。 */
  chromatic_aberration: boolean;
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
  /**
   * このアプリの最新の処理バージョン（DATA-09）。設定の process_version がこれより古ければ「処理バージョンを
   * 更新」を出す（upgrade_process_version）。古いコアでは無い。
   */
  latest_process_version?: number;
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

// ---------------------------------------------------------------------------
// スナップショット（DEV-28）・プリセット（DEV-29・EXP-02）
// ---------------------------------------------------------------------------

/** スナップショットの 1 件（genzo-api の SnapshotInfo）。 */
export interface SnapshotInfo {
  id: SnapshotId;
  variant_id: VariantId;
  name: string;
  created_at: string;
  /** 保存した設定の処理バージョン（適用すると写真もこの版になる）。 */
  process_version: number;
}

/** スナップショットの出どころ（genzo-api の SnapshotSource。省略時は今の状態）。 */
export type SnapshotSource = { kind: "current" } | { kind: "history"; entry_id: HistoryEntryId };

/** プリセットの種類（genzo-api の PresetKind）。 */
export type PresetKind = "develop" | "export";

/**
 * 書き出しプリセットの中身（EXP-02。genzo-api の ExportPresetSettings）。出力シャープ・透かし・解像度・動画（EXP-03・
 * EXP-05）は export の中の項目（無い項目は既定値として読む）。
 */
export interface ExportPresetSettings {
  export: ExportSettings;
  /** 書き出し先のフォルダ（null なら書き出すときに選ぶ）。 */
  destination: string | null;
  /** ファイル名の規則（null なら元のファイル名。仮想コピーは名前を足す）。 */
  file_naming: RenameRule | null;
}

/** プリセットの一覧の 1 件（genzo-api の PresetInfo）。 */
export interface PresetInfo {
  kind: PresetKind;
  /** ID（16 桁の 16 進数）。 */
  id: string;
  name: string;
  /** フォルダ（空ならフォルダなし）。 */
  folder: string;
  created_at: string | null;
  updated_at: string | null;
  format_version: number;
  /** 使えない理由（新しい版のアプリで作った・壊れているなど）。使えるなら null。 */
  problem: string | null;
  /** 現像プリセット: 含める項目。 */
  groups?: SettingGroups;
  /** 現像プリセット: 作ったときの写真の処理バージョン。 */
  process_version?: number;
  /** 書き出しプリセット: 中身（この PC で使えないパス（別の OS のパス）は外したもの）。 */
  export?: ExportPresetSettings;
  /** 使えるが、一部の値を使わないことの知らせ（別の OS で作ったプリセットの書き出し先・透かしのファイルなど）。 */
  warnings?: string[];
}

/** プリセットのファイルの読み込みの結果（genzo-api の PresetImportReport）。 */
export interface PresetImportReport {
  imported: PresetInfo[];
  failed: { path: string; message: string }[];
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
  /**
   * この画像の縁の G 座標（左・上・右・下。ローカル補正の位置を画像の上に描くのに使う。画像の幅に対する割合 fx の
   * G 座標の x は 左 + fx · (右 − 左)）。古いコアでは無い。
   */
  g_bounds?: [number, number, number, number];
  /**
   * この画像の上の点の O 座標（スポット修正の位置）の表。一辺 ORIENTED_GRID_SIZE 点の格子で、点 (i, j) は画像の幅・
   * 高さに対する割合 (i / 8, j / 8) の位置、j · 9 + i 番目（回転・歪曲補正があると線形でないので双線形補間する）。
   * 古いコアでは無い。
   */
  o_grid?: [number, number][];
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
  /** 比較表示の面（LIB-03。render_view_region で描いたものだけ。0 から）。 */
  pane?: number | null;
}

/** 比較表示（LIB-03）の面で使う写真の、出力の画像（C）のフル解像度の大きさ（genzo-api の ViewOutputSize）。 */
export interface ViewOutputSize {
  variant_id: VariantId;
  width: number;
  height: number;
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
  /** 動画の再生用のファイル（VID-04）のキャッシュの上限（バイト。古いコアでは省かれる）。 */
  video_proxy_cache_bytes?: number;
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
  video_proxy_cache_bytes?: number | null;
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
  | { type: "upgrade_process_version" }
  | { type: "upgrade_process_versions"; variant_ids: VariantId[] }
  // カメラプロファイル・ルック（DEV-17。genzo-api の profiles.rs）。
  | { type: "list_profiles"; camera?: string | null }
  | { type: "import_profiles"; paths: string[]; replace: boolean }
  | { type: "remove_profile"; kind: ProfileKind; id: string }
  | { type: "render_develop" }
  | { type: "render_region"; request: RegionRequest }
  | { type: "cancel_region" }
  | { type: "set_before"; source: BeforeSource | null }
  | { type: "render_before_region"; request: RegionRequest }
  // 比較表示（LIB-03）の面ごとの表示範囲の描画（genzo-api の view.rs）。
  | { type: "view_output_size"; pane: number; variant_id: VariantId }
  | { type: "render_view_region"; pane: number; request: RegionRequest }
  | { type: "cancel_view_regions" }
  | { type: "release_view" }
  | { type: "wait_for_frame"; generation: number; timeout_ms: number }
  | { type: "flush_develop" }
  | { type: "close_develop" }
  | { type: "undo"; variant_id: VariantId }
  | { type: "redo"; variant_id: VariantId }
  | { type: "go_to_history"; variant_id: VariantId; entry_id: HistoryEntryId }
  | { type: "history"; variant_id: VariantId }
  // スナップショット（DEV-28。genzo-api の snapshots.rs）。
  | { type: "snapshots"; variant_id: VariantId }
  | { type: "create_snapshot"; variant_id: VariantId; name: string; source?: SnapshotSource }
  | { type: "rename_snapshot"; snapshot_id: SnapshotId; name: string }
  | { type: "delete_snapshot"; snapshot_id: SnapshotId }
  | { type: "apply_snapshot"; snapshot_id: SnapshotId }
  // プリセット（DEV-29・EXP-02。genzo-api の presets.rs）。
  | { type: "presets"; kind: PresetKind }
  | { type: "develop_preset_settings"; preset_id: string }
  | { type: "create_develop_preset"; name: string; folder: string; groups: SettingGroups; variant_id: VariantId }
  | { type: "update_develop_preset"; preset_id: string; groups: SettingGroups; variant_id: VariantId }
  | { type: "apply_develop_preset"; preset_id: string; targets: VariantId[]; process_version?: PasteVersion }
  | { type: "create_export_preset"; name: string; folder: string; settings: ExportPresetSettings }
  | { type: "update_export_preset"; preset_id: string; settings: ExportPresetSettings }
  | { type: "rename_preset"; kind: PresetKind; preset_id: string; name: string }
  | { type: "move_preset"; kind: PresetKind; preset_id: string; folder: string }
  | { type: "rename_preset_folder"; kind: PresetKind; folder: string; name: string }
  | { type: "delete_preset"; kind: PresetKind; preset_id: string }
  | { type: "import_presets"; paths: string[]; kind?: PresetKind | null }
  | { type: "export_preset_files"; kind: PresetKind; preset_ids: string[]; dest_dir: string }
  | { type: "export_name_preview"; file_naming: RenameRule | null; format: ExportFormat }
  | { type: "watermark_preview"; watermark: Watermark; width?: number; height?: number }
  | { type: "system_font_dirs" }
  | { type: "copy_settings"; variant_id: VariantId }
  | {
      type: "paste_settings";
      settings: DevelopSettings;
      targets: VariantId[];
      groups: SettingGroups;
      process_version?: PasteVersion;
    }
  | { type: "create_virtual_copy"; variant_id: VariantId; name: string | null }
  | { type: "white_balance_at"; variant_id: VariantId; x: number; y: number; uncropped?: boolean }
  | { type: "mask_color_at"; variant_id: VariantId; x: number; y: number; uncropped?: boolean }
  | { type: "retouch_auto_source"; variant_id: VariantId; spot: RetouchSpot }
  | { type: "mask_overlay"; variant_id: VariantId; index: number; uncropped?: boolean }
  | { type: "set_develop_view"; uncropped: boolean }
  | {
      type: "constrain_crop";
      variant_id: VariantId;
      crop: CropRect;
      angle_deg: number;
    }
  | {
      type: "export";
      variant_ids: VariantId[];
      settings: ExportSettings;
      dest_dir: string;
      /** ファイル名の規則（EXP-02。省略・null なら元のファイル名）。 */
      file_naming?: RenameRule | null;
    }
  | {
      type: "composite_check";
      variant_ids: VariantId[];
      settings: CompositeSettings;
      /** 保存先のフォルダ（省略・null なら 1 枚目の写真と同じフォルダ）。 */
      dest_dir?: string | null;
    }
  | { type: "composite"; variant_ids: VariantId[]; settings: CompositeSettings; dest_dir?: string | null }
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
  | { type: "video_playback"; variant_id: VariantId }
  | { type: "create_video_proxy"; variant_id: VariantId }
  | { type: "video_proxy_cache" }
  | { type: "clear_video_proxies" }
  | { type: "capture_devices" }
  | { type: "capture_device_of"; variant_id: VariantId }
  | { type: "plan_capture_time_edit"; target: CaptureTarget; edit: CaptureTimeEdit }
  | { type: "edit_capture_times"; target: CaptureTarget; edit: CaptureTimeEdit }
  | { type: "capture_time_edits" }
  | { type: "undo_capture_time_edit" }
  // 位置情報の後付け（LIB-21）。location は省略できない（null は付けた位置を消して、カメラの位置に戻す）。
  | { type: "plan_location_edit"; variant_ids: VariantId[]; location: GpsCoord | null }
  | { type: "edit_locations"; variant_ids: VariantId[]; location: GpsCoord | null }
  | { type: "location_edits" }
  | { type: "undo_location_edit" }
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
  | ({ type: "pasted" } & PasteReport)
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
  | ({ type: "view_size" } & ViewOutputSize)
  | { type: "before"; before: BeforeInfo | null }
  | { type: "frame_or_none"; frame: FrameInfo | null }
  | { type: "saved"; saved: boolean }
  | { type: "settings"; settings: DevelopSettings }
  | { type: "settings_or_none"; settings: DevelopSettings | null }
  | { type: "history"; entries: HistoryItem[] }
  | { type: "snapshots"; snapshots: SnapshotInfo[] }
  | { type: "snapshot"; snapshot: SnapshotInfo }
  | { type: "presets"; presets: PresetInfo[]; dir: string | null }
  | { type: "preset"; preset: PresetInfo }
  | ({ type: "preset_import" } & PresetImportReport)
  | { type: "paths"; paths: string[] }
  | { type: "variant_created"; variant_id: VariantId }
  | { type: "white_balance"; temperature_k: number; tint: number }
  | ({ type: "mask_color" } & PickedMaskColor)
  | { type: "retouch_source"; offset: [number, number] | null }
  | ({ type: "mask_overlay" } & MaskOverlayInfo)
  | { type: "crop"; crop: CropRect }
  | ({ type: "display" } & DisplayInfo)
  | { type: "profiles"; profiles: ProfileEntry[] }
  | ({ type: "profile_import" } & ProfileImportReport)
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
  | ({ type: "watermark_preview" } & WatermarkPreview)
  | ({ type: "composite_check" } & CompositeCheck)
  | ({ type: "video_playback" } & VideoPlayback)
  | ({ type: "video_proxy_cache" } & VideoProxyCacheStatus)
  | { type: "capture_devices"; devices: CaptureDevice[] }
  | ({ type: "capture_device" } & CaptureDeviceKey)
  | ({ type: "capture_time_plan" } & CaptureTimePlan)
  | { type: "capture_time_edits"; edits: CaptureTimeEditInfo[] }
  | { type: "capture_time_undone"; undone: CaptureTimeUndo | null }
  | ({ type: "location_edit_plan" } & LocationEditPlan)
  | { type: "location_edits"; edits: LocationEditInfo[] }
  | { type: "location_undone"; undone: LocationUndo | null }
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
// 位置情報の後付け（LIB-21。ジオタグ付け）
// ---------------------------------------------------------------------------

/** 使う位置の出どころ（genzo-api の GpsSource。camera: カメラ（ファイル）が記録した位置、assigned: 後から付けた位置）。 */
export type GpsSource = "camera" | "assigned";

/**
 * 位置の変更の見込み・結果（genzo-api の LocationEditPlan）。数はどれも写真の数（asset の数。同じ写真の仮想コピーは
 * 位置を共有するので 1 枚）。
 */
export interface LocationEditPlan {
  /** 対象の写真の数（動画・見つからないものを除く）。 */
  photos: number;
  /** 変わる（変えた）数。 */
  changed: number;
  /** 変わらない数（同じ位置がすでに付いている。消すときは付けた位置がない）。 */
  unchanged: number;
  /** 対象のうち、カメラの位置がある数。 */
  with_camera_gps: number;
  /** 対象のうち、変える前に付けた位置がある数。 */
  with_assigned: number;
  /** 除いた動画の数（動画には位置を付けない）。 */
  skipped_videos: number;
  /** 見つからなかった写真の数（削除されたものなど。variant の数）。 */
  missing: number;
  /** 記録した変更（結果のとき。変わったものがなければ null。見込みでは常に null）。 */
  edit_id: number | null;
}

/** 位置の変更の種類（set: 位置を付けた、clear: 付けた位置を消した）。 */
export type LocationEditKind = "set" | "clear";

/** 位置の変更の履歴の 1 件（genzo-api の LocationEditInfo）。 */
export interface LocationEditInfo {
  id: number;
  kind: LocationEditKind;
  /** 付けた位置（set。clear は null）。 */
  location: GpsCoord | null;
  /** 変えた写真の数。 */
  asset_count: number;
  /** 変更した日時（UTC。RFC 3339）。 */
  created_at: string;
}

/** 位置の変更の取り消しの結果（genzo-api の LocationUndo）。 */
export interface LocationUndo {
  edit: LocationEditInfo;
  /** 戻した写真の数。 */
  restored: number;
  /** 変更の後に削除された写真の数（戻すものがない）。 */
  removed: number;
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
  | "folders"
  // 後から付けた位置（LIB-21）。取り消しでは variant_ids は空で、all が真。
  | "location";

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
  | "move_source_left"
  | "video_copy";

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
  /** オンラインの地図の設定（LIB-21。古いアプリでは無い。無いときは無効とみなす）。 */
  map?: MapPreferences;
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

/**
 * オンラインの地図の設定（位置情報の後付け LIB-21 の第 2 段階。genzo-app の MapPreferences。docs/04 の 3.16 節）。
 * 保存は専用のコマンド save_map_preferences（すぐ効く）。
 */
export interface MapPreferences {
  /** オンラインの地図（地図のタイルと地名の検索）を使う（既定は false。利用者が明示的に有効にしたときだけ通信する）。 */
  online: boolean;
  /** 初めて有効にした日時（UTC。RFC 3339。アプリが入れ、画面が送った値は使わない）。 */
  consented_at: string | null;
  /** 地図のタイルの URL のひな形（https。{z}・{x}・{y} を含む。null なら OpenStreetMap の標準）。 */
  tile_url: string | null;
  /**
   * 地名の検索のサービス（Nominatim と同じ API の /search）の URL（https）。あればこの URL に問い合わせる。null なら、
   * public_nominatim が true のときだけ OSM Foundation の公開の Nominatim を使い、false なら地名の検索は使えない
   * （geoMapLogic.ts の searchService）。
   */
  search_url: string | null;
  /**
   * search_url がないときに OSM Foundation の公開の Nominatim（nominatim.openstreetmap.org）を使う（設定の「OSM の
   * 公開の地名の検索（Nominatim）を使う」。既定は false。利用者が ON にしたときだけ）。前のアプリには無い
   * （mapPreferencesOf が false で埋める）。
   */
  public_nominatim: boolean;
  /**
   * 設定で変えたタイルの提供元の帰属の表示（例「Maps © 提供元の名前」。地図の右下に「© OpenStreetMap contributors」と
   * 並べて文字だけで出す。geoMapLogic.ts の providerAttribution）。tile_url が null（OSM の標準）なら使わない（アプリが
   * 保存のときに null にする）。前後の空白はアプリが除き、改行などの制御文字と 200 文字を超えるものはアプリが断る。
   * 前のアプリには無い（mapPreferencesOf が null で埋める）。文言は提供元の利用条件で決まる（利用者が入れる）。
   */
  tile_attribution: string | null;
}

/** 地名の検索の結果（genzo-geo の Place）。 */
export interface Place {
  /** 名前（ないときは空）。 */
  name: string;
  /** 住所などを並べた表示用の名前。 */
  display_name: string;
  lat: number;
  lon: number;
  /** 範囲（[最小緯度, 最大緯度, 最小経度, 最大経度]。Nominatim の boundingbox と同じ並び）。 */
  bbox: [number, number, number, number] | null;
  /** 分類（例 "railway"）。 */
  category: string;
  /** 種類（例 "station"）。 */
  kind: string;
}

/** 地図のタイルのキャッシュの大きさ（genzo-geo の CacheInfo。files はタイルと付随の .json の両方を数える）。 */
export interface MapCacheInfo {
  files: number;
  bytes: number;
}

/** 既定のブラウザで開く OSM のページ（genzo-app の MapLink。任意の URL は開けない）。 */
export type MapLink = "copyright" | "fix_the_map" | "tile_policy" | "nominatim_policy";

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

/** 同梱したライセンスの文書（genzo-app の licenses.rs の `LicenseDocument`。DIST-05）。 */
export interface LicenseDocument {
  /** ライセンスの文書のフォルダからの相対のパス（区切りは "/"。例: "third-party/rust.txt"）。 */
  path: string;
  size: number;
}

/** 同梱したライセンスの文書の一覧（`LicenseDocuments`）。`dir` がなければ文書はない（開発用のビルドなど）。 */
export interface LicenseDocuments {
  dir: string | null;
  documents: LicenseDocument[];
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
