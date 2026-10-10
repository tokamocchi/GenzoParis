//! コマンドラインの定義（clap の derive）と、値の解釈。
//!
//! 値の誤り（評価が 0〜5 の外、日付の形式、知らない色の名前など）は clap の段階で弾き、終了コード 2
//! （使い方の誤り）にする（[`crate::EXIT_USAGE`]）。

use std::ffi::OsString;
use std::path::PathBuf;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use clap::{Args, Parser, Subcommand, ValueEnum};
use genzo_api::{ColorLabel, Flag, GpuMode, SettingGroups, SortDirection, SortKey, VariantId};
use genzo_model::{ConflictPolicy, ExportFormat, OutputColorSpace};

/// GenzoParis の検証用 CLI（ORG-05。04 の 1.4 節）。
///
/// 登録・検索・現像・書き出し・計測を UI なしで行う。結果は表（人が読む）か、--json で JSON（機械が
/// 読む）で標準出力に出す。進捗・警告は標準エラーに出す。
///
/// 終了コード: 0 = 成功、1 = エラー（一部の失敗・確認の中止・チェックで問題が見つかった場合を含む）、
/// 2 = 使い方の誤り、130 = Ctrl+C で中断した。
#[derive(Debug, Parser)]
#[command(name = "genzo", version, about, long_about = None, max_term_width = 100)]
pub struct Cli {
    /// 全体のオプション。
    #[command(flatten)]
    pub global: GlobalArgs,
    /// サブコマンド。
    #[command(subcommand)]
    pub command: Command,
}

/// 全体のオプション（どのサブコマンドの後にも書ける）。
#[derive(Debug, Clone, Args)]
pub struct GlobalArgs {
    /// カタログ（catalog.db）のパス。省略時は環境変数 GENZO_CATALOG
    #[arg(long, global = true, value_name = "PATH")]
    pub catalog: Option<PathBuf>,
    /// データのフォルダ（thumbs.db・プレビュー・バックアップ）。省略時は環境変数 GENZO_DATA_DIR、
    /// それもなければカタログの隣の「<カタログ名>-data」
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// GPU の使用（auto: 使えれば使う / off: CPU 版だけ）
    #[arg(long, global = true, value_enum, default_value_t = GpuArg::Auto)]
    pub gpu: GpuArg,
    /// ログの水準（error / warn / info / debug / trace、または tracing の指定）。省略時は環境変数
    /// GENZO_LOG、それもなければ warn
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log: Option<String>,
    /// 結果を JSON で出す（機械が読む用）
    #[arg(long, global = true)]
    pub json: bool,
    /// 進捗を出さない（警告とエラーは出す）
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

/// GPU の使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GpuArg {
    /// 使えれば使う。
    Auto,
    /// 使わない。
    Off,
}

impl From<GpuArg> for GpuMode {
    fn from(g: GpuArg) -> Self {
        match g {
            GpuArg::Auto => GpuMode::Auto,
            GpuArg::Off => GpuMode::Off,
        }
    }
}

/// サブコマンド。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// カタログの作成・情報・詳細チェック・バックアップ・復元
    #[command(subcommand)]
    Catalog(CatalogCommand),
    /// フォルダを取り込む（進捗は標準エラー）
    Import(ImportArgs),
    // 条件が多く大きいので、ほかのサブコマンドと大きさをそろえるため箱に入れる。
    /// 検索する（条件はすべて AND）
    Search(Box<SearchArgs>),
    /// variant のメタデータ・評価・現像設定を表示する
    Show {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// 評価（0〜5）を変える
    Rate {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// 評価（0〜5）
        #[arg(value_parser = clap::value_parser!(u8).range(0..=5))]
        rating: u8,
    },
    /// フラグ（picked / rejected / none）を変える
    Flag {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// フラグ
        #[arg(value_enum)]
        flag: FlagArg,
    },
    /// カラーラベル（red / yellow / green / blue / purple / none）を変える
    Label {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// ラベル
        #[arg(value_enum)]
        label: LabelArg,
    },
    /// キャプションを変える（空の文字列で削除）
    Caption {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// キャプション
        text: String,
    },
    /// 現像設定の表示・変更・履歴・コピー・仮想コピー
    #[command(subcommand)]
    Develop(DevelopCommand),
    /// 書き出す（P3 のジョブ。進捗は標準エラー）
    Export(ExportArgs),
    /// カタログなしで 1 つのファイルを現像して書き出す（ワーカーで展開する）
    Render(RenderArgs),
    /// カタログなしでファイルのメタデータを表示する（ワーカーで読む）
    Info {
        /// 写真・動画のファイル
        file: PathBuf,
    },
    /// サムネイル（L0）・標準プレビュー（L1）
    #[command(subcommand)]
    Thumbs(ThumbsCommand),
    /// カタログから除く（元ファイルは変更しない）
    Remove {
        /// variant の ID（その asset のすべての variant を除く）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// 元ファイルを OS のゴミ箱へ移し、カタログから除く（影響の一覧を表示して確認する。端末でない
    /// 場合と --json では、--yes がなければ中止して終了コード 1）
    Trash {
        /// variant の ID（その asset のすべてのファイルが対象）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// 確認なしで実行する（影響の一覧は標準エラーに出す）
        #[arg(long)]
        yes: bool,
    },
    /// 計測（05 の 1.8 節のルール。結果は JSON に記録し、前回と比べる）
    Bench(BenchArgs),
}

/// `catalog` のサブコマンド。
#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// 新しいカタログを作る（既にあればエラー）
    Init,
    /// 件数・スキーマの版・前回の正常終了・バックアップの一覧
    Info,
    /// 詳細チェック（DATA-05: integrity_check・外部キー・テキスト検索の索引）。問題があれば終了コード 1
    Check {
        /// 元ファイルの有無も確かめる（見つからなければ missing にする）
        #[arg(long)]
        files: bool,
    },
    /// バックアップを作る（世代数を超えた古いものは削除する）
    Backup {
        /// 置き場（省略時はデータのフォルダの backups）
        #[arg(long, value_name = "DIR")]
        to: Option<PathBuf>,
    },
    /// バックアップを検証して、カタログとして復元する（--to に既にあるファイルは退避して残す）
    Restore {
        /// バックアップのファイル
        backup: PathBuf,
        /// 復元先のカタログのパス（このカタログを開いているアプリは先に終了する）
        #[arg(long, value_name = "PATH")]
        to: PathBuf,
    },
}

/// `import` の引数。
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// 取り込むフォルダ
    pub dir: PathBuf,
    /// 配下のフォルダを取り込まない
    #[arg(long)]
    pub no_recursive: bool,
    /// 取り込みの後に、現像結果からサムネイル・プレビューを作り直さない（埋め込みの JPEG のまま）
    #[arg(long)]
    pub no_previews: bool,
}

/// `search` の引数。
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// 評価の下限（0〜5）
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=5))]
    pub min_rating: Option<u8>,
    /// 評価の上限（0〜5）
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=5))]
    pub max_rating: Option<u8>,
    /// フラグ（複数指定でいずれか）
    #[arg(long, value_enum)]
    pub flag: Vec<FlagArg>,
    /// カラーラベル（複数指定でいずれか。none はラベルなし）
    #[arg(long, value_enum)]
    pub label: Vec<LabelArg>,
    /// 撮影日時の下限（YYYY-MM-DD はその日の 0 時から。時刻は YYYY-MM-DDTHH:MM（秒とオフセット ±HH:MM は省略できる）。
    /// オフセットがなければカタログの既定のタイムゾーン）
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub from: Option<DateArg>,
    /// 撮影日時の上限（YYYY-MM-DD はその日を含む。時刻を指定した場合はその時刻を含まない）
    #[arg(long, value_name = "DATE", value_parser = parse_date_arg)]
    pub to: Option<DateArg>,
    /// カメラ（完全一致。複数指定でいずれか）
    #[arg(long)]
    pub camera: Vec<String>,
    /// レンズ（完全一致。複数指定でいずれか）
    #[arg(long)]
    pub lens: Vec<String>,
    /// 種別
    #[arg(long, value_enum)]
    pub kind: Option<KindArg>,
    /// 長辺の画素数の下限（幅と高さの大きいほう。写真にも動画にも効く。例: 4K の動画は 3840）
    #[arg(long, value_name = "PX")]
    pub min_long_edge: Option<u32>,
    /// 長辺の画素数の上限
    #[arg(long, value_name = "PX")]
    pub max_long_edge: Option<u32>,
    /// 動画の長さ（秒）の下限（含む。動画の条件を指定すると写真は除く）
    #[arg(long, value_name = "SEC", value_parser = parse_non_negative)]
    pub min_duration: Option<f64>,
    /// 動画の長さ（秒）の上限（含む）
    #[arg(long, value_name = "SEC", value_parser = parse_non_negative)]
    pub max_duration: Option<f64>,
    /// 動画のフレームレートの下限（含む）
    #[arg(long, value_name = "FPS", value_parser = parse_non_negative)]
    pub min_fps: Option<f64>,
    /// 動画のフレームレートの上限（含む）
    #[arg(long, value_name = "FPS", value_parser = parse_non_negative)]
    pub max_fps: Option<f64>,
    /// 動画のコーデック（hevc・h264 など。大文字・小文字を区別しない。複数指定でいずれか）
    #[arg(long)]
    pub codec: Vec<String>,
    /// テキスト（ファイル名とキャプション。3.6 節）
    #[arg(long)]
    pub text: Option<String>,
    /// フォルダ（取り込んだフォルダのパス。配下のフォルダを含む）
    #[arg(long, value_name = "DIR")]
    pub folder: Option<PathBuf>,
    /// --folder の配下のフォルダを含めない
    #[arg(long, requires = "folder")]
    pub no_subfolders: bool,
    /// マスターだけ（仮想コピーを除く）
    #[arg(long)]
    pub masters_only: bool,
    /// 並べ替え
    #[arg(long, value_enum, default_value_t = SortArg::Capture)]
    pub sort: SortArg,
    /// 降順にする
    #[arg(long)]
    pub desc: bool,
    /// 表示する件数（0 なら全件）
    #[arg(long, default_value_t = DEFAULT_SEARCH_LIMIT)]
    pub limit: u64,
    /// 先頭から飛ばす件数
    #[arg(long, default_value_t = 0)]
    pub offset: u64,
    /// variant の ID だけを 1 行に 1 つずつ出す（--json と一緒なら --json を優先する）
    #[arg(long)]
    pub ids: bool,
}

/// 0 以上の有限の数（動画の長さ・fps の条件）。
fn parse_non_negative(s: &str) -> Result<f64, String> {
    match s.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(v),
        _ => Err(format!("0 以上の数を指定してください: {s}")),
    }
}

/// `search` で表示する件数の既定値（**仮置き**: 100。端末で読める量。`--limit 0` で全件）。
pub const DEFAULT_SEARCH_LIMIT: u64 = 100;

/// `develop` のサブコマンド。
#[derive(Debug, Subcommand)]
pub enum DevelopCommand {
    /// 現像設定を JSON で表示する
    Get {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// 現像設定を変える（JSON。部分的な指定は今の設定にマージする。履歴に 1 件記録する）
    Set {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// 設定の JSON のファイル（- で標準入力）。`--json <FILE>` とも書ける（必須。UTF-8、BOM 付きの
        /// UTF-8、UTF-16 を読める）
        #[arg(long = "settings", value_name = "FILE")]
        settings: Option<String>,
        /// マージせず、指定した JSON（欠けた項目は既定値）で置き換える
        #[arg(long)]
        replace: bool,
    },
    /// 現像設定を初期化する（何も調整していない設定。履歴に 1 件記録する）
    Reset {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// 1 つ前の履歴に戻す（DEV-27）
    Undo {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// 1 つ先の履歴に進める（DEV-27）
    Redo {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// 履歴（古い順）
    History {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// 設定を別の variant にコピーする（DEV-30）
    Copy {
        /// コピー元の variant の ID
        #[arg(value_parser = parse_variant_id)]
        from: VariantId,
        /// コピー先の variant の ID（複数可）
        #[arg(long, required = true, num_args = 1.., value_parser = parse_variant_id)]
        to: Vec<VariantId>,
        /// 写す項目（カンマ区切り。省略時はすべて）
        #[arg(long, value_enum, value_delimiter = ',')]
        groups: Vec<GroupArg>,
    },
    /// 仮想コピーを作る（LIB-13）
    VirtualCopy {
        /// 元の variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// 名前（省略時は「コピー n」）
        #[arg(long)]
        name: Option<String>,
    },
    /// 仮想コピーを削除する（マスターは削除できない）
    DeleteCopy {
        /// 仮想コピーの variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
}

/// `thumbs` のサブコマンド。
#[derive(Debug, Subcommand)]
pub enum ThumbsCommand {
    /// 現像結果からサムネイル（L0）と標準プレビュー（L1）を作り直す（省略時はすべての variant）
    Regenerate {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
}

/// 書き出しの設定のオプション（`export` と `render` で共通）。
#[derive(Debug, Clone, Args)]
pub struct ExportOpts {
    /// 形式（省略時は export では jpeg、render では --out の拡張子から）
    #[arg(long, value_enum)]
    pub format: Option<FormatArg>,
    /// JPEG の品質（1〜100。既定 90）
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub quality: Option<u8>,
    /// 色空間（省略時は export ではカタログの設定、render では srgb）
    #[arg(long, value_enum)]
    pub color_space: Option<ColorSpaceArg>,
    /// 長辺の画素数（拡大はしない。省略時は元の寸法）
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=65_535))]
    pub long_edge: Option<u32>,
    /// GPS の情報を書き出さない（EXP-04・SEC-03）
    #[arg(long)]
    pub remove_gps: bool,
    /// 同じ名前のファイルがあるとき（連番を付ける / 上書きする / 飛ばす）
    #[arg(long, value_enum, default_value_t = ConflictArg::Sequence)]
    pub on_conflict: ConflictArg,
}

/// `export` の引数。
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// variant の ID（複数可）
    #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
    pub variants: Vec<VariantId>,
    /// 書き出し先のフォルダ
    #[arg(long, value_name = "DIR")]
    pub out: PathBuf,
    /// 書き出しの設定。
    #[command(flatten)]
    pub opts: ExportOpts,
}

/// `render` の引数。
#[derive(Debug, Args)]
pub struct RenderArgs {
    /// 写真のファイル（RAW・JPEG・TIFF・PNG）
    pub file: PathBuf,
    /// 書き出すファイル
    #[arg(long, value_name = "FILE")]
    pub out: PathBuf,
    /// 現像設定の JSON のファイル（- で標準入力。既定の設定にマージする）
    #[arg(long, value_name = "FILE")]
    pub settings: Option<String>,
    /// 書き出しの設定。
    #[command(flatten)]
    pub opts: ExportOpts,
}

/// `bench` の引数。
#[derive(Debug, Args)]
pub struct BenchArgs {
    /// 計測する処理（preview: 段階 C・WB のドラッグ・離した後の最終品質 / export: 書き出しの時間の内訳）
    #[arg(value_enum)]
    pub target: BenchTarget,
    /// 入力のファイル（RAW・JPEG など。ワーカーで展開する）
    #[arg(long, value_name = "FILE", conflicts_with = "synthetic")]
    pub file: Option<PathBuf>,
    /// 合成の RAW の寸法（例: 7008x4672。--file も --synthetic もなければ 7008x4672）
    #[arg(long, value_name = "WxH", value_parser = parse_size)]
    pub synthetic: Option<(u32, u32)>,
    /// 計測の回数（省略時は 1.8 節の規則の最小値: 応答時間 30 回・一括処理 5 回）
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub runs: Option<u32>,
    /// 結果の記録先のフォルダ（省略時は環境変数 GENZO_BENCH_DIR、それもなければ bench-results）
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
}

/// 計測する処理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BenchTarget {
    /// プレビュー（PoC-3 の (1)(2)）。
    Preview,
    /// 書き出し（PoC-3 の (3)）。
    Export,
}

/// フラグ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FlagArg {
    /// 採用。
    Picked,
    /// 不採用。
    Rejected,
    /// フラグなし。
    None,
}

impl From<FlagArg> for Flag {
    fn from(f: FlagArg) -> Self {
        match f {
            FlagArg::Picked => Flag::Picked,
            FlagArg::Rejected => Flag::Rejected,
            FlagArg::None => Flag::None,
        }
    }
}

/// カラーラベル。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LabelArg {
    /// 赤。
    Red,
    /// 黄。
    Yellow,
    /// 緑。
    Green,
    /// 青。
    Blue,
    /// 紫。
    Purple,
    /// ラベルなし。
    None,
}

impl LabelArg {
    /// カラーラベル（ラベルなしは `None`）。
    pub fn to_label(self) -> Option<ColorLabel> {
        match self {
            LabelArg::Red => Some(ColorLabel::Red),
            LabelArg::Yellow => Some(ColorLabel::Yellow),
            LabelArg::Green => Some(ColorLabel::Green),
            LabelArg::Blue => Some(ColorLabel::Blue),
            LabelArg::Purple => Some(ColorLabel::Purple),
            LabelArg::None => None,
        }
    }
}

/// 種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum KindArg {
    /// 写真。
    Photo,
    /// 動画。
    Video,
}

/// 並べ替えの項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortArg {
    /// 撮影日時（撮影日時のないものは最後）。
    Capture,
    /// ファイル名。
    Name,
    /// 評価。
    Rating,
    /// 登録順。
    Import,
}

impl SortArg {
    /// genzo-api の並べ替えの項目。
    pub fn key(self) -> SortKey {
        match self {
            SortArg::Capture => SortKey::CaptureTime,
            SortArg::Name => SortKey::FileName,
            SortArg::Rating => SortKey::Rating,
            SortArg::Import => SortKey::ImportOrder,
        }
    }
}

/// 並べ替えの向き。
pub fn direction(desc: bool) -> SortDirection {
    if desc {
        SortDirection::Descending
    } else {
        SortDirection::Ascending
    }
}

/// 設定の貼り付けで写す項目（DEV-30）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GroupArg {
    /// ホワイトバランス。
    WhiteBalance,
    /// 露光量・コントラスト・ハイライト・シャドウ・白・黒。
    BasicTone,
    /// トーンカーブ。
    ToneCurve,
    /// 彩度・自然な彩度など。
    Color,
    /// 切り抜き・角度補正。
    Geometry,
    /// レンズ補正。
    Lens,
    /// シャープ・ノイズ軽減。
    Detail,
    /// ローカル補正。
    Masks,
}

/// 写す項目の指定から genzo-api の [`SettingGroups`] を作る（空ならすべて）。
pub fn setting_groups(groups: &[GroupArg]) -> SettingGroups {
    if groups.is_empty() {
        return SettingGroups::ALL;
    }
    let has = |g: GroupArg| groups.contains(&g);
    SettingGroups {
        white_balance: has(GroupArg::WhiteBalance),
        basic_tone: has(GroupArg::BasicTone),
        tone_curve: has(GroupArg::ToneCurve),
        color: has(GroupArg::Color),
        geometry: has(GroupArg::Geometry),
        lens: has(GroupArg::Lens),
        detail: has(GroupArg::Detail),
        masks: has(GroupArg::Masks),
    }
}

/// 書き出しの形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    /// JPEG（8bit）。
    #[value(alias = "jpg")]
    Jpeg,
    /// TIFF（16bit）。
    #[value(alias = "tif")]
    Tiff,
    /// PNG（8bit）。
    Png8,
    /// PNG（16bit）。
    Png16,
}

impl FormatArg {
    /// genzo-model の形式（JPEG の品質は `quality`、なければ既定の 90）。
    pub fn to_format(self, quality: Option<u8>) -> ExportFormat {
        match self {
            FormatArg::Jpeg => ExportFormat::Jpeg {
                quality: quality.unwrap_or(ExportFormat::DEFAULT_JPEG_QUALITY),
            },
            FormatArg::Tiff => ExportFormat::Tiff16,
            FormatArg::Png8 => ExportFormat::Png8,
            FormatArg::Png16 => ExportFormat::Png16,
        }
    }

    /// ファイルの拡張子から形式を推定する（PNG は 8bit）。
    pub fn from_extension(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" => Some(FormatArg::Jpeg),
            "tif" | "tiff" => Some(FormatArg::Tiff),
            "png" => Some(FormatArg::Png8),
            _ => None,
        }
    }
}

/// 書き出しの色空間。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorSpaceArg {
    /// sRGB。
    Srgb,
    /// Display P3。
    #[value(alias = "display-p3")]
    P3,
    /// Adobe RGB (1998)。
    #[value(alias = "adobe-rgb")]
    Adobergb,
}

impl From<ColorSpaceArg> for OutputColorSpace {
    fn from(c: ColorSpaceArg) -> Self {
        match c {
            ColorSpaceArg::Srgb => OutputColorSpace::Srgb,
            ColorSpaceArg::P3 => OutputColorSpace::DisplayP3,
            ColorSpaceArg::Adobergb => OutputColorSpace::AdobeRgb,
        }
    }
}

/// 名前の衝突の扱い。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConflictArg {
    /// 連番を付ける。
    Sequence,
    /// 上書きする（原本の照合は必ず行う）。
    Overwrite,
    /// 飛ばす。
    Skip,
}

impl From<ConflictArg> for ConflictPolicy {
    fn from(c: ConflictArg) -> Self {
        match c {
            ConflictArg::Sequence => ConflictPolicy::Sequence,
            ConflictArg::Overwrite => ConflictPolicy::Overwrite,
            ConflictArg::Skip => ConflictPolicy::Skip,
        }
    }
}

/// 日時の指定（オフセットのないものは、カタログの既定のタイムゾーンで解釈する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateArg {
    /// 日付だけ（下限はその日の 0 時、上限はその日を含む）。
    Date(NaiveDate),
    /// オフセットのない日時。
    Local(NaiveDateTime),
    /// オフセット付きの日時。
    Fixed(DateTime<FixedOffset>),
}

impl DateArg {
    /// UTC の境界にする。`upper` なら上限（含まない境界。日付だけなら翌日の 0 時）。
    pub fn to_utc(self, default_offset: FixedOffset, upper: bool) -> Option<DateTime<chrono::Utc>> {
        let local = match self {
            DateArg::Fixed(dt) => return Some(dt.to_utc()),
            DateArg::Local(dt) => dt,
            DateArg::Date(d) => {
                let d = if upper { d.succ_opt()? } else { d };
                d.and_hms_opt(0, 0, 0)?
            }
        };
        local
            .and_local_timezone(default_offset)
            .single()
            .map(|dt| dt.to_utc())
    }
}

/// 日時の指定を解釈する（clap の value_parser）。
pub fn parse_date_arg(s: &str) -> Result<DateArg, String> {
    let s = s.trim();
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(DateArg::Date(d));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(DateArg::Fixed(dt));
    }
    for f in ["%Y-%m-%dT%H:%M:%S%:z", "%Y-%m-%dT%H:%M%:z"] {
        if let Ok(dt) = DateTime::parse_from_str(s, f) {
            return Ok(DateArg::Fixed(dt));
        }
    }
    for f in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, f) {
            return Ok(DateArg::Local(dt));
        }
    }
    Err(format!(
        "日時の形式が違います（{s}）。YYYY-MM-DD、YYYY-MM-DDTHH:MM[:SS]、またはオフセット付き（例: 2024-05-01T09:00:00+09:00）"
    ))
}

/// variant の ID（1 以上の整数）を解釈する。
pub fn parse_variant_id(s: &str) -> Result<VariantId, String> {
    match s.trim().parse::<i64>() {
        Ok(v) if v >= 1 => Ok(VariantId::new(v)),
        _ => Err(format!("variant の ID は 1 以上の整数です（{s}）")),
    }
}

/// 寸法（`WxH`）を解釈する（合成の RAW。LibRaw で読める 32 画素以上、genzo-raw の画素数の上限以下）。
pub fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let err = || format!("寸法は WxH の形で指定します（例: 7008x4672。{s}）");
    let (w, h) = s.trim().split_once(['x', 'X', '×']).ok_or_else(err)?;
    let w: u32 = w.trim().parse().map_err(|_| err())?;
    let h: u32 = h.trim().parse().map_err(|_| err())?;
    if w < MIN_SYNTHETIC_EDGE || h < MIN_SYNTHETIC_EDGE {
        return Err(format!(
            "合成の RAW の幅と高さは {MIN_SYNTHETIC_EDGE} 画素以上にしてください（{s}）"
        ));
    }
    if u64::from(w) * u64::from(h) > genzo_raw::MAX_PIXELS {
        return Err(format!(
            "合成の RAW の画素数が上限（{}）を超えます（{s}）",
            genzo_raw::MAX_PIXELS
        ));
    }
    Ok((w, h))
}

/// 合成の RAW の一辺の最小値（genzo-raw の dng モジュールの制限「LibRaw で読む場合は 22 画素以上」に、
/// CFA の 2 × 2 の余裕を足した値）。
pub const MIN_SYNTHETIC_EDGE: u32 = 32;

/// 引数を解釈する前の書き換え: `develop set <variant> --json <FILE>` の `--json <FILE>` を
/// `--settings <FILE>` にする。
///
/// 全体のオプション `--json`（出力を JSON にする。値を取らない）と名前が同じため、clap では両方を
/// 定義できない。`develop set` の後ろで、`--json` の次の引数が値（`-` で始まらないもの、または
/// 標準入力を表す `-`）なら設定の入力とみなす。`--json=FILE` も同じ。それ以外の `--json` は出力の指定の
/// まま残す。
pub fn rewrite_develop_set(args: Vec<OsString>) -> Vec<OsString> {
    let pos = args
        .windows(2)
        .position(|w| w[0] == "develop" && w[1] == "set");
    let Some(pos) = pos else {
        return args;
    };
    let mut out: Vec<OsString> = args[..pos + 2].to_vec();
    let mut it = args.into_iter().skip(pos + 2).peekable();
    while let Some(a) = it.next() {
        if a == "--" {
            out.push(a);
            out.extend(it);
            break;
        }
        if a == "--json" {
            let takes_value = it.peek().is_some_and(|next| {
                let s = next.to_string_lossy();
                s == "-" || !s.starts_with('-')
            });
            if takes_value {
                out.push("--settings".into());
                out.extend(it.next());
                continue;
            }
        } else if let Some(v) = a.to_str().and_then(|s| s.strip_prefix("--json=")) {
            out.push("--settings".into());
            out.push(v.into());
            continue;
        }
        out.push(a);
    }
    out
}

/// 引数を解釈する（[`rewrite_develop_set`] の後に clap で）。
pub fn parse_from(args: Vec<OsString>) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(rewrite_develop_set(args))
}

/// 出力の `--json` が指定されているか（[`rewrite_develop_set`] の後の引数で、`--` より前）。clap の
/// 解釈が失敗したときにも、エラーを JSON で出すかを決めるために使う。
pub fn json_requested(args: &[OsString]) -> bool {
    args.iter()
        .skip(1)
        .take_while(|a| *a != "--")
        .any(|a| a == "--json")
}

/// clap のエラーが `--help`・`--version` の表示か（終了コード 0。エラーではない）。
pub fn is_help_or_version(e: &clap::Error) -> bool {
    matches!(
        e.kind(),
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
    )
}

/// clap のエラーの説明（1 行目。先頭の `error: ` は除く。JSON の `message` に使う）。
pub fn clap_message(e: &clap::Error) -> String {
    let text = e.render().to_string();
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    first.trim().trim_start_matches("error:").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        parse_from(os(args))
    }

    #[test]
    fn definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn rating_takes_the_last_value_and_checks_the_range() {
        let cli = parse(&["genzo", "rate", "3", "4", "5", "2"]).unwrap();
        let Command::Rate { variants, rating } = cli.command else {
            panic!()
        };
        assert_eq!(
            variants,
            vec![VariantId::new(3), VariantId::new(4), VariantId::new(5)]
        );
        assert_eq!(rating, 2);
        let e = parse(&["genzo", "rate", "3", "6"]).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        let e = parse(&["genzo", "rate", "3"]).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        let e = parse(&["genzo", "rate", "0", "3"]).unwrap_err();
        assert_eq!(e.exit_code(), 2, "variant の ID は 1 以上");
    }

    #[test]
    fn global_options_can_follow_the_subcommand() {
        let cli = parse(&[
            "genzo",
            "search",
            "--min-rating",
            "3",
            "--json",
            "--catalog",
            "c.db",
            "--gpu",
            "off",
        ])
        .unwrap();
        assert!(cli.global.json);
        assert_eq!(cli.global.catalog, Some(PathBuf::from("c.db")));
        assert_eq!(cli.global.gpu, GpuArg::Off);
        let Command::Search(s) = cli.command else {
            panic!()
        };
        assert_eq!(s.min_rating, Some(3));
        assert_eq!(s.limit, DEFAULT_SEARCH_LIMIT);
        assert!(parse(&["genzo", "search", "--flag", "maybe"]).is_err());
        assert!(parse(&["genzo", "search", "--no-subfolders"]).is_err());
    }

    #[test]
    fn develop_set_accepts_json_for_the_input() {
        for args in [
            &["genzo", "develop", "set", "7", "--json", "s.json"][..],
            &["genzo", "develop", "set", "7", "--json=s.json"],
            &["genzo", "develop", "set", "7", "--settings", "s.json"],
        ] {
            let cli = parse(args).unwrap();
            assert!(!cli.global.json, "{args:?}");
            let Command::Develop(DevelopCommand::Set {
                variant, settings, ..
            }) = cli.command
            else {
                panic!()
            };
            assert_eq!(variant, VariantId::new(7));
            assert_eq!(settings.as_deref(), Some("s.json"));
        }
        // 標準入力と、出力の --json を両方使う。
        let cli = parse(&["genzo", "develop", "set", "7", "--json", "-", "--json"]).unwrap();
        assert!(cli.global.json);
        let Command::Develop(DevelopCommand::Set { settings, .. }) = cli.command else {
            panic!()
        };
        assert_eq!(settings.as_deref(), Some("-"));
        // 出力の --json が先でも、設定の入力は --json <FILE> で書ける。
        let cli = parse(&[
            "genzo",
            "--json",
            "develop",
            "set",
            "7",
            "--json",
            "a.json",
            "--replace",
        ])
        .unwrap();
        assert!(cli.global.json);
        let Command::Develop(DevelopCommand::Set {
            settings, replace, ..
        }) = cli.command
        else {
            panic!()
        };
        assert_eq!(settings.as_deref(), Some("a.json"));
        assert!(replace);
        // 他のサブコマンドの --json は書き換えない。
        assert_eq!(
            rewrite_develop_set(os(&["genzo", "develop", "get", "7", "--json"])),
            os(&["genzo", "develop", "get", "7", "--json"])
        );
        // 入力がない場合は、実行のときに `--json <FILE>` を案内する使い方の誤りにする（clap の
        // 「--settings が必要」では、--json で書いた利用者に分かりにくいため）。
        let cli = parse(&["genzo", "develop", "set", "7", "--json"]).unwrap();
        assert!(cli.global.json);
        assert!(matches!(
            cli.command,
            Command::Develop(DevelopCommand::Set { settings: None, .. })
        ));
    }

    #[test]
    fn json_is_detected_for_usage_errors() {
        let args = rewrite_develop_set(os(&["genzo", "rate", "1", "9", "--json"]));
        assert!(json_requested(&args));
        let e = Cli::try_parse_from(args).unwrap_err();
        assert!(!is_help_or_version(&e));
        let m = clap_message(&e);
        assert!(m.contains('9') && !m.starts_with("error"), "{m}");
        // develop set の入力の --json <FILE> は出力の指定ではない。
        let args = rewrite_develop_set(os(&["genzo", "develop", "set", "1", "--json", "a.json"]));
        assert!(!json_requested(&args));
        // `--` の後ろは値。
        assert!(!json_requested(&os(&[
            "genzo", "caption", "1", "--", "--json"
        ])));
        assert!(!json_requested(&os(&["genzo", "search"])));
        let help = Cli::try_parse_from(os(&["genzo", "--help"])).unwrap_err();
        assert!(is_help_or_version(&help));
    }

    #[test]
    fn export_options() {
        let cli = parse(&[
            "genzo",
            "export",
            "1",
            "2",
            "--out",
            "o",
            "--format",
            "tif",
            "--color-space",
            "display-p3",
            "--long-edge",
            "1024",
            "--remove-gps",
            "--on-conflict",
            "overwrite",
        ])
        .unwrap();
        let Command::Export(e) = cli.command else {
            panic!()
        };
        assert_eq!(e.variants.len(), 2);
        assert_eq!(e.opts.format, Some(FormatArg::Tiff));
        assert_eq!(e.opts.color_space, Some(ColorSpaceArg::P3));
        assert_eq!(e.opts.long_edge, Some(1024));
        assert!(e.opts.remove_gps);
        assert_eq!(e.opts.on_conflict, ConflictArg::Overwrite);
        assert!(parse(&["genzo", "export", "1", "--out", "o", "--quality", "0"]).is_err());
        assert!(parse(&["genzo", "export", "1"]).is_err(), "--out は必須");
        assert_eq!(
            FormatArg::from_extension(std::path::Path::new("a/B.JPEG")),
            Some(FormatArg::Jpeg)
        );
        assert_eq!(
            FormatArg::from_extension(std::path::Path::new("x.webp")),
            None
        );
        assert_eq!(
            FormatArg::Jpeg.to_format(Some(75)),
            ExportFormat::Jpeg { quality: 75 }
        );
    }

    #[test]
    fn dates() {
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();
        let d = parse_date_arg("2024-05-01").unwrap();
        assert_eq!(
            d.to_utc(jst, false).unwrap().to_rfc3339(),
            "2024-04-30T15:00:00+00:00"
        );
        assert_eq!(
            d.to_utc(jst, true).unwrap().to_rfc3339(),
            "2024-05-01T15:00:00+00:00",
            "上限の日付はその日を含む（翌日の 0 時を含まない境界）"
        );
        let t = parse_date_arg("2024-05-01T10:30").unwrap();
        assert_eq!(
            t.to_utc(jst, true).unwrap().to_rfc3339(),
            "2024-05-01T01:30:00+00:00"
        );
        let f = parse_date_arg("2024-05-01T10:30:00-05:00").unwrap();
        assert_eq!(
            f.to_utc(jst, false).unwrap().to_rfc3339(),
            "2024-05-01T15:30:00+00:00",
            "オフセット付きは既定のタイムゾーンを使わない"
        );
        assert!(parse_date_arg("2024/05/01").is_err());
        assert!(parse_date_arg("2024-13-01").is_err());
    }

    #[test]
    fn sizes_and_groups() {
        assert_eq!(parse_size("7008x4672").unwrap(), (7008, 4672));
        assert_eq!(parse_size(" 64 X 48 ").unwrap(), (64, 48));
        assert!(parse_size("16x16").is_err());
        assert!(parse_size("7008").is_err());
        assert!(parse_size("100000x100000").is_err());
        assert_eq!(setting_groups(&[]), SettingGroups::ALL);
        let g = setting_groups(&[GroupArg::BasicTone, GroupArg::Color]);
        assert!(g.basic_tone && g.color && !g.white_balance && !g.geometry);
        let cli = parse(&[
            "genzo",
            "develop",
            "copy",
            "1",
            "--to",
            "2",
            "3",
            "--groups",
            "basic-tone,white-balance",
        ])
        .unwrap();
        let Command::Develop(DevelopCommand::Copy { to, groups, .. }) = cli.command else {
            panic!()
        };
        assert_eq!(to.len(), 2);
        assert_eq!(groups, vec![GroupArg::BasicTone, GroupArg::WhiteBalance]);
    }

    #[test]
    fn labels_and_flags() {
        assert_eq!(LabelArg::None.to_label(), None);
        assert_eq!(LabelArg::Purple.to_label(), Some(ColorLabel::Purple));
        assert_eq!(Flag::from(FlagArg::Rejected), Flag::Rejected);
        let cli = parse(&["genzo", "label", "4", "none"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Label {
                label: LabelArg::None,
                ..
            }
        ));
    }
}
