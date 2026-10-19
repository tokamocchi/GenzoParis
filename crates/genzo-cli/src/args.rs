//! コマンドラインの定義（clap の derive）と、値の解釈。
//!
//! 値の誤り（評価が 0〜5 の外、日付の形式、知らない色の名前など）は clap の段階で弾き、終了コード 2
//! （使い方の誤り）にする（[`crate::EXIT_USAGE`]）。

use std::ffi::OsString;
use std::path::PathBuf;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use clap::{Args, Parser, Subcommand, ValueEnum};
use genzo_api::{
    BeforeSource, CollectionId, ColorLabel, Flag, GpuMode, SettingGroups, SortDirection, SortKey,
    StackId, VariantId,
};
use genzo_model::{
    ConflictPolicy, ExportFormat, HistoryEntryId, MetadataField, OutputColorSpace, SnapshotId,
};

/// GenzoParis の検証用 CLI（ORG-05。04 の 1.4 節）。
///
/// 登録・検索・現像・書き出し・計測を UI なしで行う。結果は表（人が読む）か、--json で JSON（機械が
/// 読む）で標準出力に出す。進捗・警告は標準エラーに出す。
///
/// 終了コードは [`EXIT_CODES_HELP`]（`--help` の末尾に出す）。
#[derive(Debug, Parser)]
#[command(
    name = "genzo",
    version,
    about,
    long_about = None,
    max_term_width = 100,
    after_help = EXIT_CODES_HELP
)]
pub struct Cli {
    /// 全体のオプション。
    #[command(flatten)]
    pub global: GlobalArgs,
    /// サブコマンド。
    #[command(subcommand)]
    pub command: Command,
}

/// `--help` の末尾に出す終了コードの説明（`crate::EXIT_SUCCESS` 〜 `crate::EXIT_INTERRUPTED`。
/// implementation_status の No.30）。
pub const EXIT_CODES_HELP: &str = "終了コード:
  0    成功
  1    エラー（処理できなかった。trash を --yes なしで中止した場合を含む）
  2    使い方の誤り（引数の誤り・カタログの指定がない）
  3    結果は出したが、一部が失敗した・問題が見つかった（書き出し・削除・サムネイルの作り直しの一部の
       失敗、取り込みで登録できなかったファイル、写真の移動・名前の変更・再リンクで飛ばした・失敗した
       写真、catalog check で見つかった問題、missing check で見つかった見つからない写真、bench での悪化、
       profile import・preset import で取り込めなかったファイル）
  130  Ctrl+C で中断した";

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
    /// GPU の使用（auto: 使えれば使う / off: CPU 版だけ）。ソフトウェアの GPU（llvmpipe・WARP など）は、
    /// 環境変数 GENZO_GPU_ALLOW_SOFTWARE=1 のときだけ使う
    #[arg(long, global = true, value_enum, default_value_t = GpuArg::Auto)]
    pub gpu: GpuArg,
    /// ログの水準（error / warn / info / debug / trace、または tracing の指定）。省略時は環境変数
    /// GENZO_LOG、それもなければ warn
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log: Option<String>,
    /// ログのファイル（cli.log）とクラッシュレポートを置くフォルダ。省略時は環境変数 GENZO_LOG_DIR、
    /// それもなければアプリと同じログのフォルダ（genzo log info で表示）。外部へは送信しない
    #[arg(long, global = true, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,
    /// ログをファイルに残さない（標準エラーには出す。クラッシュレポートは残す）。環境変数
    /// GENZO_LOG_FILE=off と同じ
    #[arg(long, global = true)]
    pub no_log_file: bool,
    /// プリセット（現像プリセット・書き出しプリセット。DEV-29・EXP-02）のフォルダ。省略時は環境変数
    /// GENZO_PRESETS_DIR、それもなければアプリと同じ場所（genzo preset dir で表示）
    #[arg(long, global = true, value_name = "DIR")]
    pub presets_dir: Option<PathBuf>,
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
    /// 説明のメタデータ（タイトル・作成者・著作権・撮影場所など。LIB-15）を表示・変更する（カタログの中だけ。
    /// 元ファイルには書かない）。--set・--clear がなければ、項目ごとの値（写真によって異なれば「複数の値」）を
    /// 表示する
    #[command(after_help = METADATA_FIELDS_HELP)]
    Metadata {
        /// variant の ID（複数可。複数なら一括で変える。仮想コピーは元の写真と同じ値）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// 項目に値を入れる（項目=値。例: --set title=清水寺 --set creator="山田 太郎"。複数可）
        #[arg(long = "set", value_name = "FIELD=VALUE", value_parser = parse_metadata_assignment)]
        set: Vec<(MetadataField, String)>,
        /// 項目の値を消す（複数可）
        #[arg(long = "clear", value_name = "FIELD", value_parser = parse_metadata_field)]
        clear: Vec<MetadataField>,
    },
    /// 階層キーワード（LIB-09）の一覧・付ける・外す・作る・名前の変更・移動・削除・書き出しの印。
    /// キーワードは「場所>京都」のような階層のパス（「 > 」の前後の空白は無視）か、`id:12` の形の ID で指定する
    #[command(subcommand)]
    Keyword(KeywordCommand),
    /// コレクション（LIB-10）・スマートコレクション（LIB-11）・コレクションセットの一覧・作る・写真を入れる・
    /// 外す・名前の変更・移動・削除・条件。コレクションは ID で指定する（`collection list` で見られる）
    #[command(subcommand)]
    Collection(CollectionCommand),
    /// スタック（LIB-12）: まとめる・写真の追加と除外・解除・一番上の写真・折りたたみ・撮影時刻による自動
    /// スタック・表示。スタックは ID（`stack show` で見られる）で、写真は variant の ID で指定する
    #[command(subcommand)]
    Stack(StackCommand),
    /// 現像設定の表示・変更・履歴・コピー・仮想コピー
    #[command(subcommand)]
    Develop(DevelopCommand),
    /// カメラプロファイル（DCP）・ルック（3D LUT の .cube。DEV-17）: 取り込み・一覧・詳細・除く・ファイルの確認。
    /// 取り込んだものはカタログのデータのフォルダの profiles に写し（元のファイルは変えない）、`develop set
    /// --profile`・`--look` に ID で指定する。Adobe のプロファイルなどは同梱しない（利用者が用意したファイルを使う）
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// 現像プリセット（DEV-29）・書き出しプリセット（EXP-02）: 一覧・作成・更新・適用・名前・フォルダ・削除・
    /// ファイルの読み込みと書き出し。カタログをまたいで使うので、アプリと同じフォルダ（--presets-dir）に
    /// 1 件 1 ファイルで置く。プリセットは ID（16 桁の 16 進数）か名前で指定する
    #[command(subcommand)]
    Preset(PresetCommand),
    /// 書き出す（P3 のジョブ。進捗は標準エラー）
    Export(ExportArgs),
    /// 比較明・比較暗合成（DEV-34。P3 のジョブ）: 選んだ写真を撮影日時の順に現像して画素ごとに比べ、1 枚にして保存し、
    /// カタログに登録する（既定は 16bit の TIFF・Adobe RGB・1 枚目の写真と同じフォルダ・元の写真とスタックにまとめる）
    Composite(CompositeArgs),
    /// カタログなしで 1 つのファイルを現像して書き出す（ワーカーで展開する）
    Render(RenderArgs),
    /// 透かしの見本・フォントのフォルダ（EXP-03。カタログは使わない）
    #[command(subcommand)]
    Watermark(WatermarkCommand),
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
    /// ログ・クラッシュレポートの場所と中身の表示・消去（カタログは使わない。SYS-07）
    #[command(subcommand)]
    Log(LogCommand),
    /// ファイルの内容のハッシュ（全体の BLAKE3。ORG-03）: 計算・状況・内容が同じファイルの組・ファイルの
    /// 識別（場所・名前によらず、同じ内容の登録済みの写真を探す）
    #[command(subcommand)]
    Hash(HashCommand),
    /// 撮影日時の一括修正（LIB-16）: 機材の一覧・ずらす・基準の写真に合わせる・タイムゾーンの指定と取り消し・
    /// ファイルの値に戻す・履歴・元に戻す。カタログの中だけを変え、元のファイルには書かない。対象は variant の
    /// ID か、--device-of で指定した写真と同じ機材（カメラの機種・シリアル番号・写真か動画か）のすべて
    #[command(subcommand)]
    Time(TimeCommand),
    /// 写真と動画のタイムライン（ORG-01）: 撮影日時の順に並べ、撮影日と、撮影の間隔によるイベントで
    /// まとめて表示する（条件は search と同じ）
    Timeline(Box<TimelineArgs>),
    /// ボリューム（ドライブ。FILE-03）: 一覧（OS のボリューム ID・つながっているか・マウント先）
    #[command(subcommand)]
    Volume(VolumeCommand),
    /// フォルダ（FILE-02・FILE-04）: 一覧・作る・名前の変更・移動（ディスクのフォルダも変える。上書きしない）・
    /// カタログから除く・場所の指定し直し（カタログの中だけ）。フォルダは ID（`folder list`）か、登録した
    /// フォルダのパスで指定する
    #[command(subcommand)]
    Folder(FolderCommand),
    /// 写真のファイル（FILE-02）: 別のフォルダへ移す・名前を変える。RAW と同時記録の JPEG は一緒に動かし、
    /// 同じ名前のファイルがあればその写真は飛ばす（上書きしない）。別のドライブへはコピーして照合してから
    /// 登録し、元のファイルはゴミ箱へ移す
    #[command(subcommand)]
    File(FileCommand),
    /// 見つからない写真（FILE-04）: 存在の確認・候補を探して結び付け直す（再リンク）・1 つのファイルの
    /// 結び付け直し。見つからない写真の一覧は `search --missing`
    #[command(subcommand)]
    Missing(MissingCommand),
    /// 動画の再生（VID-04）: 再生の情報（アプリが配信するファイル・WebView で再生できない動画の再生用のファイル）・
    /// 再生用のファイルを作る（利用者の ffmpeg で H.264 ＋ AAC の MP4。長辺 1280）・キャッシュの状況と消去
    #[command(subcommand)]
    Video(VideoCommand),
}

/// カメラプロファイル・ルックの種類（genzo-api の `ProfileKind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProfileKindArg {
    /// カメラプロファイル（.dcp）。
    Camera,
    /// ルック（.cube）。
    Look,
}

impl From<ProfileKindArg> for genzo_api::ProfileKind {
    fn from(k: ProfileKindArg) -> Self {
        match k {
            ProfileKindArg::Camera => Self::Camera,
            ProfileKindArg::Look => Self::Look,
        }
    }
}

/// `profile` のサブコマンド（DEV-17）。
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// ファイルを取り込む（.dcp はカメラプロファイル、.cube はルック。ID は拡張子を除いたファイルの名前）。
    /// 取り込めなかったファイルがあれば終了コード 3
    Import {
        /// ファイル（複数可）
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// 同じ ID（大文字・小文字は区別しない）で内容の違うものを置き換える（使っている写真は、次に開いたときに
        /// 「内容が違う」と知らせる）
        #[arg(long)]
        replace: bool,
    },
    /// 取り込んだものの一覧（種類 → 表示の名前の順）
    List {
        /// 種類で絞る
        #[arg(long, value_enum)]
        kind: Option<ProfileKindArg>,
        /// 機種（例: "SONY ILCE-7M4"）。カメラプロファイルがその機種用かを表示する
        #[arg(long, conflicts_with = "for_variant")]
        camera: Option<String>,
        /// この写真の機種で、カメラプロファイルがその機種用かを表示する
        #[arg(long = "for", value_name = "VARIANT", value_parser = parse_variant_id)]
        for_variant: Option<VariantId>,
    },
    /// 詳細（説明・使わない情報・著作権の表示・埋め込みの方針・内容の SHA-256）
    Show {
        /// 種類
        #[arg(value_enum)]
        kind: ProfileKindArg,
        /// ID（`profile list` の ID）
        id: String,
    },
    /// 一覧から除く（使っている写真は、標準の色・ルックなしで描いて「見つからない」と知らせる。取り込んだ写しは、
    /// どの項目も指さなくなれば消す。元のファイルは変えない）
    Remove {
        /// 種類
        #[arg(value_enum)]
        kind: ProfileKindArg,
        /// ID
        id: String,
    },
    /// 取り込まずにファイルを読んで中身を表示する（カタログは使わない）
    Inspect {
        /// .dcp・.cube のファイル
        file: PathBuf,
        /// 機種（渡すと、その機種用かを表示する）
        #[arg(long)]
        camera: Option<String>,
    },
}

/// `video` のサブコマンド（VID-04）。
#[derive(Debug, Subcommand)]
pub enum VideoCommand {
    /// 再生の情報: 元のファイルを読めるか・MIME の型・コーデック・アプリが配信するファイル・再生用のファイルの
    /// 有無・作るときのエンコーダー（作れなければ理由）
    Info {
        /// 動画の variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// 再生用のファイルを作る（1 本ずつ。進捗は標準エラー。今の内容のものがあれば作らない）。作れなかった動画が
    /// あれば終了コード 3
    Proxy {
        /// 動画の variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// 再生用のファイルのキャッシュの状況（フォルダ・数・合計・上限）
    Cache,
    /// 再生用のファイルをすべて消す（作り直せる。元の動画は変わらない）
    Clear,
}

/// フォルダの指定（ID か、登録したフォルダのパス）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderArg {
    /// フォルダの ID（数字だけ）。
    Id(genzo_model::FolderId),
    /// 登録したフォルダのパス。
    Path(PathBuf),
}

/// フォルダの指定を解釈する（数字だけなら ID。数字だけの名前のフォルダは `./2024` のようにパスで書く）。
pub fn parse_folder_arg(s: &str) -> Result<FolderArg, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("フォルダを ID かパスで指定してください".to_owned());
    }
    if t.bytes().all(|b| b.is_ascii_digit()) {
        return match t.parse::<i64>() {
            Ok(n) if n >= 1 => Ok(FolderArg::Id(genzo_model::FolderId::new(n))),
            _ => Err(format!("フォルダの ID は 1 以上の整数です（{s}）")),
        };
    }
    Ok(FolderArg::Path(PathBuf::from(s)))
}

/// ファイルの ID（1 以上の整数。`missing relink --dry-run`・`show` で見られる）。
pub fn parse_file_id(s: &str) -> Result<genzo_model::FileId, String> {
    match s.trim().parse::<i64>() {
        Ok(n) if n >= 1 => Ok(genzo_model::FileId::new(n)),
        _ => Err(format!("ファイルの ID は 1 以上の整数です（{s}）")),
    }
}

/// `volume` のサブコマンド（FILE-03）。
#[derive(Debug, Subcommand)]
pub enum VolumeCommand {
    /// ボリュームの一覧（ID・識別の方法・ラベル・マウント先・つながっているか）
    List {
        /// つながりを確かめ直す（つないだドライブのフォルダのオフラインの印を外す。古い形式のボリュームを
        /// OS のボリューム ID に移す）
        #[arg(long)]
        refresh: bool,
    },
}

/// `folder` のサブコマンド（FILE-02・FILE-04）。
#[derive(Debug, Subcommand)]
pub enum FolderCommand {
    /// フォルダの一覧（ID・状態（オフライン・見つからない）・見つからない写真の数・パス）
    List,
    /// フォルダを作る（ディスクに作り、カタログに登録する）
    Create {
        /// 作る場所（フォルダの ID かパス）
        #[arg(value_parser = parse_folder_arg)]
        parent: FolderArg,
        /// 新しいフォルダの名前
        name: String,
    },
    /// フォルダの名前を変える（ディスクのフォルダも。中の写真の登録はそのまま）
    Rename {
        /// フォルダ（ID かパス）
        #[arg(value_parser = parse_folder_arg)]
        folder: FolderArg,
        /// 新しい名前
        name: String,
    },
    /// フォルダを別のフォルダの中へ移す（ディスクの上で。同じドライブの中だけ）
    Move {
        /// 移すフォルダ（ID かパス）
        #[arg(value_parser = parse_folder_arg)]
        folder: FolderArg,
        /// 移動先（この中へ。ID かパス）
        #[arg(value_parser = parse_folder_arg)]
        parent: FolderArg,
    },
    /// フォルダ（と配下）をカタログから除く（写真が登録されていないフォルダだけ。見つからないフォルダ・再リンクで
    /// 空になったフォルダの片付け。カタログの中だけを変え、ディスクのフォルダには触れない）
    Remove {
        /// フォルダ（ID かパス）
        #[arg(value_parser = parse_folder_arg)]
        folder: FolderArg,
    },
    /// フォルダの場所を指定し直す（OS の側で移した・名前を変えたフォルダを、新しい場所に結び付け直す。
    /// カタログの中だけを変え、ファイルは動かさない。配下のフォルダも）
    Relocate {
        /// フォルダ（ID か、カタログに登録した前のパス）
        #[arg(value_parser = parse_folder_arg)]
        folder: FolderArg,
        /// 新しい場所
        path: PathBuf,
        /// 変えずに、見つかるファイルの数だけを出す
        #[arg(long)]
        dry_run: bool,
    },
}

/// `file` のサブコマンド（FILE-02）。
#[derive(Debug, Subcommand)]
pub enum FileCommand {
    /// 写真を別のフォルダへ移す（進捗は標準エラー）
    Move {
        /// variant の ID（複数可。仮想コピーは元の写真と同じファイル）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// 移動先のフォルダ（ID か、登録したフォルダのパス。新しいフォルダは folder create で作る）
        #[arg(long, value_name = "FOLDER", value_parser = parse_folder_arg)]
        to: FolderArg,
        /// 移さずに、予定（移せない写真と理由・コピーの量）だけを出す
        #[arg(long)]
        dry_run: bool,
    },
    /// 写真のファイルの名前を変える（拡張子は変えない。--name か --template のどちらか）
    Rename {
        /// variant の ID（複数可。--name なら 1 つ）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// 新しい名前（拡張子を除く。1 枚だけ）
        #[arg(
            long,
            conflicts_with = "template",
            required_unless_present = "template"
        )]
        name: Option<String>,
        /// 名前のテンプレート（import --rename と同じ項目。例: "{date}-{seq}"。連番は指定した順）
        #[arg(long, value_name = "TEMPLATE")]
        template: Option<String>,
        /// テンプレートの {text} に入れる文字
        #[arg(
            long,
            value_name = "TEXT",
            requires = "template",
            conflicts_with = "name",
            default_value = ""
        )]
        text: String,
        /// 連番（{seq}）の開始番号
        #[arg(
            long,
            value_name = "N",
            requires = "template",
            conflicts_with = "name",
            default_value_t = 1
        )]
        seq_start: u32,
        /// 連番（{seq}）の桁数（1〜9）
        #[arg(long, value_name = "N", requires = "template", conflicts_with = "name", default_value_t = 4,
              value_parser = clap::value_parser!(u8).range(1..=9))]
        seq_digits: u8,
        /// 変えずに、新しい名前と変えられない理由だけを出す
        #[arg(long)]
        dry_run: bool,
    },
}

/// `missing` のサブコマンド（FILE-04）。
#[derive(Debug, Subcommand)]
pub enum MissingCommand {
    /// 登録したファイルがあるかを確かめ、見つからない印を付ける・外す（つながっていないドライブは飛ばす。
    /// 進捗は標準エラー）
    Check {
        /// このフォルダ（配下を含む。ID かパス）だけを確かめる
        #[arg(long, value_name = "FOLDER", value_parser = parse_folder_arg)]
        folder: Option<FolderArg>,
    },
    /// 見つからない写真の候補をフォルダから探し（名前・大きさ・撮影日時・内容のハッシュ）、確からしい候補が
    /// 1 つだけのものを結び付け直す
    Relink {
        /// 探すフォルダ
        #[arg(long = "in", value_name = "DIR")]
        dir: PathBuf,
        /// 対象の写真（variant の ID。省略するとすべての見つからない写真）
        #[arg(value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// サブフォルダを探さない
        #[arg(long)]
        no_recursive: bool,
        /// 名前と撮影日時だけが同じ（内容が変わっている）候補も、それしかなければ結び付ける（読み直す）
        #[arg(long)]
        accept_changed: bool,
        /// 結び付けずに、候補だけを出す
        #[arg(long)]
        dry_run: bool,
    },
    /// 見つからないファイルを、指定したファイルに結び付け直す（ファイルの ID は relink --dry-run で見られる）
    Link {
        /// ファイルの ID
        #[arg(value_parser = parse_file_id)]
        file: genzo_model::FileId,
        /// 結び付けるファイル
        path: PathBuf,
        /// 内容が登録のものと違っても結び付ける（読み直す）
        #[arg(long)]
        accept_changed: bool,
    },
}

/// `time` の対象（LIB-16）。
#[derive(Debug, Args)]
pub struct TimeTargetArgs {
    /// variant の ID（複数可。同じ写真の仮想コピーは撮影日時を共有する）
    #[arg(value_parser = parse_variant_id)]
    pub variants: Vec<VariantId>,
    /// この写真と同じ機材のすべての写真・動画を対象にする（複数可）
    #[arg(long, value_name = "ID", value_parser = parse_variant_id)]
    pub device_of: Vec<VariantId>,
    /// 変えずに、変わる数と例だけを出す
    #[arg(long)]
    pub dry_run: bool,
}

/// `time` のサブコマンド（LIB-16）。
#[derive(Debug, Subcommand)]
pub enum TimeCommand {
    /// 機材の一覧（カメラの機種・シリアル番号・写真か動画か・数・撮影日時の範囲（既定のタイムゾーン））
    Devices,
    /// 撮影日時をずらす（時計のずれの補正に足す。例: +1h、-90s、+1d2h3m4s、3600（秒））
    Shift {
        /// ずらす量（正で後の時刻にする。d・h・m・s の組み合わせか、秒の整数）
        #[arg(allow_hyphen_values = true, value_parser = parse_shift)]
        amount: i64,
        /// 対象。
        #[command(flatten)]
        target: TimeTargetArgs,
    },
    /// 基準の写真の正しい撮影日時から差を求めて、対象のすべてをその差だけずらす（対象を省略すると、基準の
    /// 写真と同じ機材のすべて）
    Sync {
        /// 基準の写真（variant の ID）
        #[arg(value_parser = parse_variant_id)]
        reference: VariantId,
        /// 基準の写真の正しい撮影日時（例: "2024-05-01 12:34:56"。オフセットを付けるとそれで読む）
        correct: String,
        /// 正しい撮影日時を読むタイムゾーン（例: +09:00。省略すると基準の写真の表示のタイムゾーン）
        #[arg(long, allow_hyphen_values = true, value_parser = parse_zone_minutes)]
        zone: Option<i32>,
        /// 対象。
        #[command(flatten)]
        target: TimeTargetArgs,
    },
    /// タイムゾーンを指定する（既定: 記録の時刻をそのタイムゾーンの時刻として読む。UTC の印だけの動画なども
    /// 現地時刻として読む。--keep-instant: 撮影した瞬間はそのままで、そのタイムゾーンの時刻で表示する）
    Zone {
        /// オフセット（例: +09:00、-05:30）
        #[arg(allow_hyphen_values = true, value_parser = parse_zone_minutes)]
        offset: i32,
        /// 撮影した瞬間を変えずに、表示だけを変える
        #[arg(long)]
        keep_instant: bool,
        /// 対象。
        #[command(flatten)]
        target: TimeTargetArgs,
    },
    /// タイムゾーンの指定を取り消す（ファイルの記録、なければ既定のタイムゾーンに戻す。ずれの補正は保つ）
    ClearZone {
        /// 対象。
        #[command(flatten)]
        target: TimeTargetArgs,
    },
    /// 修正をすべて取り消し、ファイルの値に戻す
    Reset {
        /// 対象。
        #[command(flatten)]
        target: TimeTargetArgs,
    },
    /// 修正の履歴（新しい順）
    History,
    /// 最後の修正を取り消す
    Undo,
}

/// `timeline` の引数（ORG-01）。
#[derive(Debug, Args)]
pub struct TimelineArgs {
    /// イベントの区切りの間隔（例: 2h、30m。0 ならイベントで分けない。既定 2h）
    #[arg(long, value_parser = parse_gap, default_value = "2h")]
    pub gap: u32,
    /// 新しい順にする
    #[arg(long)]
    pub desc: bool,
    /// スタックをすべて展開して並べる
    #[arg(long)]
    pub expand_stacks: bool,
    /// 対象の写真の条件（search と同じ）。
    #[command(flatten)]
    pub filter: FilterArgs,
}

/// 撮影日時をずらす量（`+1d2h3m4s`・`-90s`・`3600`）を秒にする。
pub fn parse_shift(s: &str) -> Result<i64, String> {
    let bad =
        || format!("ずらす量は +1h・-90s・+1d2h3m4s・3600（秒）の形で指定してください（{s}）");
    let t = s.trim();
    let (sign, body) = match t.as_bytes().first() {
        Some(b'+') => (1i64, &t[1..]),
        Some(b'-') => (-1i64, &t[1..]),
        _ => (1i64, t),
    };
    if body.is_empty() {
        return Err(bad());
    }
    if body.bytes().all(|b| b.is_ascii_digit()) {
        return body.parse::<i64>().map(|v| sign * v).map_err(|_| bad());
    }
    let mut total: i64 = 0;
    let mut number = String::new();
    let mut last_unit = 0usize;
    for c in body.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let (rank, unit) = match c.to_ascii_lowercase() {
            'd' => (1, 86_400),
            'h' => (2, 3_600),
            'm' => (3, 60),
            's' => (4, 1),
            _ => return Err(bad()),
        };
        // 単位は大きい順に 1 回ずつ。
        if number.is_empty() || rank <= last_unit {
            return Err(bad());
        }
        last_unit = rank;
        let v: i64 = number.parse().map_err(|_| bad())?;
        total = v
            .checked_mul(unit)
            .and_then(|x| total.checked_add(x))
            .ok_or_else(bad)?;
        number.clear();
    }
    if !number.is_empty() {
        return Err(bad());
    }
    Ok(sign * total)
}

/// イベントの区切りの間隔（`2h`・`30m`・`0`）を秒にする（0〜1 日）。
pub fn parse_gap(s: &str) -> Result<u32, String> {
    let v = parse_shift(s)?;
    if !(0..=i64::from(genzo_api::MAX_TIMELINE_GAP_SECONDS)).contains(&v) {
        return Err(format!("間隔は 0〜24h で指定してください（{s}）"));
    }
    Ok(v as u32)
}

/// タイムゾーンのオフセット（`+09:00`・`-0530`）を分にする。
pub fn parse_zone_minutes(s: &str) -> Result<i32, String> {
    let o = genzo_model::capture_time::parse_offset(s)
        .map_err(|_| format!("タイムゾーンは +09:00・-05:30 の形で指定してください（{s}）"))?;
    Ok(o.local_minus_utc() / 60)
}

/// `hash` のサブコマンド。
#[derive(Debug, Subcommand)]
pub enum HashCommand {
    /// 全体のハッシュのないファイルの内容のハッシュを計算して記録する（進捗は標準エラー。終わるまで待つ）。
    /// ネットワークのボリューム（SMB・NFS など）のファイルは、--include-network のときだけ読む。読めなかった
    /// ファイルがあれば終了コード 3
    Compute {
        /// ネットワークのボリュームのファイルも読む
        #[arg(long)]
        include_network: bool,
    },
    /// 計算済みのファイルの数・計算の対象の数・内容が同じファイルの組の数
    Status,
    /// 内容が同じファイルの組（全体のハッシュを記録したファイルの中で。最初に登録したファイルの順）
    Duplicates {
        /// 表示する組の上限（1〜1000）
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..=1000))]
        limit: u64,
    },
    /// ファイルの内容で、同じ内容の登録済みのファイル（写真）を探す（アプリの外で移した・名前を変えた
    /// ファイルにも使える。ファイルは読むだけ）
    Identify {
        /// 調べるファイル
        file: PathBuf,
    },
}

/// `keyword` のサブコマンド。
#[derive(Debug, Subcommand)]
pub enum KeywordCommand {
    /// すべてのキーワード（階層のパス・ID・直接付いている写真の数・書き出すファイルに含めるか）
    List,
    /// 写真にキーワードを付ける（なければ作る）
    Add {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// キーワードの階層のパス（例: --keyword "場所>京都" --keyword 家族。複数可）
        #[arg(long = "keyword", short = 'k', required = true, value_name = "PATH")]
        keywords: Vec<String>,
    },
    /// 写真からキーワードを外す（下の階層のキーワードは外さない）
    Remove {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// キーワード（パスか id:N。複数可）
        #[arg(long = "keyword", short = 'k', required = true, value_name = "KEYWORD")]
        keywords: Vec<String>,
    },
    /// 写真に付いているキーワード（複数なら、それぞれが付いている写真の数）
    Show {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// キーワードを作る（階層のパスごと。あれば何もしない）
    Create {
        /// 階層のパス（例: "場所>京都"）
        path: String,
    },
    /// キーワードの名前を変える（同じ階層に同じ名前があればエラー）
    Rename {
        /// キーワード（パスか id:N）
        keyword: String,
        /// 新しい名前
        name: String,
    },
    /// キーワードを別の親の下へ移す（自分・下の階層の下へは移せない）
    Move {
        /// キーワード（パスか id:N）
        keyword: String,
        /// 新しい親（パスか id:N。省略すると最上位）
        #[arg(long, value_name = "KEYWORD")]
        to: Option<String>,
    },
    /// キーワードを削除する（下の階層のキーワードと、写真への付与も。写真は削除しない）
    Delete {
        /// キーワード（パスか id:N）
        keyword: String,
    },
    /// 書き出すファイルのメタデータ（XMP の dc:subject）にキーワードを書くかを変える
    Export {
        /// キーワード（パスか id:N）
        keyword: String,
        /// 書く（--off と一緒には使えない）
        #[arg(long, conflicts_with = "off", required_unless_present = "off")]
        on: bool,
        /// 書かない
        #[arg(long)]
        off: bool,
    },
}

/// `collection` のサブコマンド。
#[derive(Debug, Subcommand)]
pub enum CollectionCommand {
    /// すべてのコレクション（ID・種類・階層のパス・写真の数。スマートコレクションとセットも数える）
    List,
    /// コレクションを作る（--rule でスマートコレクション、--kind set でコレクションセット）
    Create {
        /// 名前
        name: String,
        /// 入れる先のコレクションセットの ID（省略すると最上位）
        #[arg(long = "in", value_name = "SET_ID", value_parser = parse_collection_id)]
        parent: Option<CollectionId>,
        /// 種類（--rule を指定すると smart）
        #[arg(long, value_enum)]
        kind: Option<CollectionKindArg>,
        /// スマートコレクションの条件の JSON のファイル（- で標準入力。形は docs/04_architecture.md の
        /// 3.9 節・docs/catalog_schema.md の 2.5 節）
        #[arg(long, value_name = "FILE")]
        rule: Option<String>,
        /// 手動のコレクションに入れる variant の ID（複数可）
        #[arg(long = "add", value_name = "ID", num_args = 1.., value_parser = parse_variant_id)]
        add: Vec<VariantId>,
    },
    /// 手動のコレクションに写真を入れる
    Add {
        /// コレクションの ID
        #[arg(value_parser = parse_collection_id)]
        collection: CollectionId,
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// 手動のコレクションから写真を外す（写真は削除しない）
    Remove {
        /// コレクションの ID
        #[arg(value_parser = parse_collection_id)]
        collection: CollectionId,
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// コレクションの名前を変える
    Rename {
        /// コレクションの ID
        #[arg(value_parser = parse_collection_id)]
        collection: CollectionId,
        /// 新しい名前
        name: String,
    },
    /// コレクションを別のコレクションセットへ移す
    Move {
        /// コレクションの ID
        #[arg(value_parser = parse_collection_id)]
        collection: CollectionId,
        /// 移す先のコレクションセットの ID（省略すると最上位）
        #[arg(long, value_name = "SET_ID", value_parser = parse_collection_id)]
        to: Option<CollectionId>,
    },
    /// コレクションを削除する（セットなら中のコレクションも。写真は削除しない）
    Delete {
        /// コレクションの ID
        #[arg(value_parser = parse_collection_id)]
        collection: CollectionId,
    },
    /// スマートコレクションの条件を表示する（--set で変える）
    Rule {
        /// コレクションの ID
        #[arg(value_parser = parse_collection_id)]
        collection: CollectionId,
        /// 新しい条件の JSON のファイル（- で標準入力）
        #[arg(long = "set", value_name = "FILE")]
        set: Option<String>,
    },
}

/// コレクションの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CollectionKindArg {
    /// 手動のコレクション（写真を入れる）
    Manual,
    /// スマートコレクション（条件で決まる。--rule が要る）
    Smart,
    /// コレクションセット（コレクションを入れる）
    Set,
}

impl From<CollectionKindArg> for genzo_api::CollectionKind {
    fn from(k: CollectionKindArg) -> Self {
        match k {
            CollectionKindArg::Manual => Self::Manual,
            CollectionKindArg::Smart => Self::Smart,
            CollectionKindArg::Set => Self::Set,
        }
    }
}

/// コレクションの ID（1 以上の整数）。
pub fn parse_collection_id(s: &str) -> Result<CollectionId, String> {
    match s.trim().parse::<i64>() {
        Ok(n) if n > 0 => Ok(CollectionId::new(n)),
        _ => Err(format!("コレクションの ID は 1 以上の整数です: {s}")),
    }
}

/// スタックの ID（1 以上の整数）。
pub fn parse_stack_id(s: &str) -> Result<StackId, String> {
    match s.trim().parse::<i64>() {
        Ok(n) if n > 0 => Ok(StackId::new(n)),
        _ => Err(format!("スタックの ID は 1 以上の整数です: {s}")),
    }
}

/// `stack` のサブコマンド（LIB-12）。
#[derive(Debug, Subcommand)]
pub enum StackCommand {
    /// 写真をまとめて 1 つのスタックにする（2 枚以上。既にスタックに入っている写真があれば、そのスタックの写真も
    /// まとめる。順は撮影日時の順で、作ったスタックは折りたたむ）
    Create {
        /// variant の ID（2 枚以上）
        #[arg(required = true, num_args = 2.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
        /// 一番上にする写真（省略すると撮影日時の最も早いもの）
        #[arg(long, value_name = "ID", value_parser = parse_variant_id)]
        top: Option<VariantId>,
    },
    /// スタックに写真を足す（最後に、撮影日時の順で。ほかのスタックの写真は移す）
    Add {
        /// スタックの ID
        #[arg(value_parser = parse_stack_id)]
        stack: StackId,
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// 写真をスタックから外す（写真は消えない。1 枚になったスタックは消える）
    Remove {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// 写真が入っているスタックを解除する（写真は消えない）
    Unstack {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// 写真をそのスタックの一番上にする
    Top {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// スタックを折りたたむ（画面のグリッドと search で、一番上の写真（代表）だけを出す）
    Collapse {
        /// スタックの ID（複数可）
        #[arg(value_parser = parse_stack_id, required_unless_present = "all")]
        stacks: Vec<StackId>,
        /// すべてのスタック
        #[arg(long, conflicts_with = "stacks")]
        all: bool,
    },
    /// スタックを展開する（スタックの写真をすべて、スタックの中の順に並べる）
    Expand {
        /// スタックの ID（複数可）
        #[arg(value_parser = parse_stack_id, required_unless_present = "all")]
        stacks: Vec<StackId>,
        /// すべてのスタック
        #[arg(long, conflicts_with = "stacks")]
        all: bool,
    },
    /// 写真が入っているスタック（ID・写真の順（最初が一番上）・折りたたみ）
    Show {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// 撮影時刻の間隔で自動的にスタックを作る（条件は search と同じ。条件に合う写真のうち、スタックに入って
    /// いない、撮影日時の分かるものを撮影日時の順に並べ、前の写真との間隔が --gap 以下のものをまとめる）
    // 条件が多く大きいので、ほかのサブコマンドと大きさをそろえるため箱に入れる。
    Auto(Box<AutoStackArgs>),
}

/// `stack auto` の引数。
#[derive(Debug, Args)]
pub struct AutoStackArgs {
    /// 前の写真との間隔の上限（秒。0〜3600。小数も使える）
    #[arg(long, value_name = "SEC", value_parser = parse_non_negative)]
    pub gap: f64,
    /// 作らずに、作るスタックの数の見込みだけを出す
    #[arg(long)]
    pub dry_run: bool,
    /// 対象の写真の条件（search と同じ）。
    #[command(flatten)]
    pub filter: FilterArgs,
}

/// `log` のサブコマンド。
#[derive(Debug, Subcommand)]
pub enum LogCommand {
    /// ログのフォルダ・ファイル・クラッシュレポートを表示する
    Info,
    /// ログのフォルダのログとクラッシュレポートを消す（GenzoParis の名前の形のファイルだけ。一部を
    /// 消せなければ終了コード 3）
    Clear,
}

/// `catalog` のサブコマンド。
#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// 新しいカタログを作る（既にあればエラー）
    Init,
    /// 件数・スキーマの版・前回の正常終了・バックアップの一覧
    Info,
    /// 詳細チェック（DATA-05: integrity_check・外部キー・テキスト検索の索引）。問題があれば終了コード 3
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
    /// カタログの構造（表・列・索引・公開ビュー。ORG-07。文書は docs/catalog_schema.md）
    Schema {
        /// 表・索引・ビュー・トリガーを作る SQL をそのまま出す
        #[arg(long)]
        sql: bool,
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
    /// 取り込みの後に、現像結果からサムネイル・プレビューを作り直す（既定は作り直さず、埋め込みの
    /// JPEG のまま。現像設定を変えたときに作る）
    #[arg(long, conflicts_with = "no_previews")]
    pub previews: bool,
    /// 取り込みの後に、現像結果からサムネイル・プレビューを作り直さない（既定。以前の版との互換のため
    /// 残す）
    #[arg(long, hide = true)]
    pub no_previews: bool,
    /// 重複と思われるもの（内容が同じファイルが別の場所で登録済みのもの。全体のハッシュで確かめる）も、
    /// 別の写真として取り込む（省略時はカタログの設定。その既定は取り込まない）
    #[arg(long, conflicts_with = "skip_duplicates")]
    pub keep_duplicates: bool,
    /// 重複と思われるものを取り込まない（省略時はカタログの設定）
    #[arg(long)]
    pub skip_duplicates: bool,
    /// 同じフォルダで拡張子だけが違う RAW と JPEG を、別々の写真として取り込む（省略時はカタログの設定。
    /// その既定は 1 枚にまとめる）
    #[arg(long, conflicts_with = "pair_raw_jpeg")]
    pub separate_raw_jpeg: bool,
    /// RAW と JPEG を 1 枚の写真（RAW が主、JPEG は付属）にまとめる（省略時はカタログの設定）
    #[arg(long)]
    pub pair_raw_jpeg: bool,
    /// コピーして取り込む（IMP-02）: DIR の写真・動画をこのフォルダ（あるフォルダ）へコピーし、内容を元と照合して
    /// から登録する（元のファイルは変更・削除しない。同じ名前があれば上書きせず -1 などを付ける）
    #[arg(long, value_name = "DEST")]
    pub copy_to: Option<PathBuf>,
    /// コピー先の中のフォルダの規則（year-date: 2026/2026-10-13、year-month-day: 2026/10/13、year-month:
    /// 2026/2026-10、date: 2026-10-13、date-compact: 20261013、single: コピー先にそのまま、source: 元の
    /// フォルダの構成のまま。撮影日時がなければファイルの更新日時）
    #[arg(long, value_name = "RULE", default_value = "year-date", value_parser = parse_folder_rule, requires = "copy_to")]
    pub folders: genzo_api::FolderRule,
    /// コピー先のファイル名のテンプレート（IMP-03。{date} {time} {year} {month} {day} {hour} {minute}
    /// {second} {seq} {name} {camera} {text}。例: "{date}-{seq}"。省略時は元の名前のまま）
    #[arg(long, value_name = "TEMPLATE", requires = "copy_to")]
    pub rename: Option<String>,
    /// テンプレートの {text} に入れる文字
    #[arg(long, value_name = "TEXT", requires = "rename", default_value = "")]
    pub text: String,
    /// 連番（{seq}）の開始番号
    #[arg(long, value_name = "N", requires = "rename", default_value_t = 1)]
    pub seq_start: u32,
    /// 連番（{seq}）の桁数（1〜9）
    #[arg(long, value_name = "N", requires = "rename", default_value_t = 4,
          value_parser = clap::value_parser!(u8).range(1..=9))]
    pub seq_digits: u8,
    /// コピー先の名前を決めて表示するだけで、コピー・登録をしない
    #[arg(long, requires = "copy_to")]
    pub dry_run: bool,
    /// 新しく登録した写真に付けるキーワード（IMP-05。「場所>京都」のような階層のパス。複数可）
    #[arg(long = "keyword", value_name = "PATH")]
    pub keywords: Vec<String>,
    /// 新しく登録した写真に付ける説明のメタデータ（項目=値。例: --meta creator="山田 太郎"。複数可。項目の一覧は
    /// genzo metadata --help）
    #[arg(long = "meta", value_name = "FIELD=VALUE", value_parser = parse_metadata_assignment)]
    pub meta: Vec<(MetadataField, String)>,
}

impl ImportArgs {
    /// 取り込みの設定（指定しなかった項目はカタログの設定に従う）。
    pub fn options(&self) -> genzo_api::ImportOptions {
        let pick = |yes: bool, no: bool| match (yes, no) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        };
        genzo_api::ImportOptions {
            skip_duplicates: pick(self.skip_duplicates, self.keep_duplicates),
            separate_raw_jpeg: pick(self.separate_raw_jpeg, self.pair_raw_jpeg),
            copy: self.copy_to.as_ref().map(|dest| genzo_api::CopySettings {
                destination: dest.clone(),
                folder_rule: self.folders,
                rename: self.rename.as_ref().map(|t| genzo_api::RenameRule {
                    template: t.clone(),
                    custom_text: self.text.clone(),
                    sequence_start: self.seq_start,
                    sequence_digits: self.seq_digits,
                }),
            }),
            metadata: genzo_api::ImportMetadata {
                keywords: self
                    .keywords
                    .iter()
                    .map(|k| crate::organize::parse_keyword_path(k))
                    .collect(),
                fields: self.meta.iter().cloned().collect(),
            },
            dry_run: self.dry_run,
        }
    }
}

/// コピー先のフォルダの規則の名前を解釈する（`year-date`・`year_date`）。
pub fn parse_folder_rule(s: &str) -> Result<genzo_api::FolderRule, String> {
    genzo_api::FolderRule::from_name(s).ok_or_else(|| {
        format!(
            "フォルダの規則ではありません（{s}。year-date・year-month-day・year-month・date・date-compact・single・source）"
        )
    })
}

/// 写真の絞り込みの条件（`search`・`stack auto` で共通）。
#[derive(Debug, Args)]
pub struct FilterArgs {
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
    /// 元ファイルが見つからない写真だけ（FILE-04。missing check で確かめた結果）
    #[arg(long)]
    pub missing: bool,
    /// キーワード（パスか id:N。下の階層を含む。複数指定ですべてを持つ）
    #[arg(long, value_name = "KEYWORD")]
    pub keyword: Vec<String>,
    /// コレクションの ID（手動はメンバー、スマートは条件、セットは中のいずれか）
    #[arg(long, value_name = "ID", value_parser = parse_collection_id)]
    pub collection: Option<CollectionId>,
    /// スマートコレクションの条件の JSON のファイル（- で標準入力。保存していない条件で探す）
    #[arg(long, value_name = "FILE")]
    pub rule: Option<String>,
}

/// `search` の引数。
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// 絞り込みの条件。
    #[command(flatten)]
    pub filter: FilterArgs,
    /// スタック（LIB-12）を、保存した折りたたみの状態によらず、すべて展開して並べる（既定では画面と同じく、
    /// 折りたたんだスタックは一番上の写真（代表）だけを出す）
    #[arg(long)]
    pub expand_stacks: bool,
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
    /// 現像設定を変える（JSON。部分的な指定は今の設定にマージする。レンズ補正・ハイライト復元・ノイズ軽減・白黒・
    /// カラーミキサー・カラーグレーディング・テクスチャ・明瞭度・かすみの除去・シャープ・効果はオプションでも指定できる。
    /// 履歴に 1 件記録する）
    Set {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// 設定の JSON のファイル（- で標準入力）。`--json <FILE>` とも書ける（レンズ補正・ハイライト復元・ノイズ
        /// 軽減・色のオプションだけを指定するときは省ける。UTF-8、BOM 付きの UTF-8、UTF-16 を読める）
        #[arg(long = "settings", value_name = "FILE")]
        settings: Option<String>,
        /// マージせず、指定した JSON（欠けた項目は既定値）で置き換える
        #[arg(long)]
        replace: bool,
        /// レンズ補正（DEV-14。JSON の後に当てる）
        #[command(flatten)]
        lens: LensArgs,
        /// ハイライト復元・ノイズ軽減（DEV-06・DEV-13。JSON の後に当てる）
        #[command(flatten)]
        sensor: SensorArgs,
        /// 白黒・カラーミキサー・カラーグレーディング（DEV-25・DEV-09・DEV-10。JSON の後に当てる。項目が多いので
        /// 箱に入れる）
        #[command(flatten)]
        color: Box<ColorArgs>,
        /// テクスチャ・明瞭度・かすみの除去・シャープ・効果（DEV-11・DEV-12・DEV-24。JSON の後に当てる。箱に入れる）
        #[command(flatten)]
        detail: Box<DetailArgs>,
        /// カメラプロファイル・ルック（DEV-17。JSON の後に当てる）
        #[command(flatten)]
        profile: ProfileArgs,
    },
    /// 処理バージョンを最新に更新する（DATA-09。レンズ補正などは処理バージョン 2 から使われる。ほかの設定は
    /// 変えない。履歴に 1 件記録する。すでに最新の写真・動画は何もしない。複数の ID をまとめて更新できる）
    Upgrade {
        /// variant の ID（複数可）
        #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
        variants: Vec<VariantId>,
    },
    /// 元の画像の情報（RAW か・大きさ・デコーダ・撮影時の WB・埋め込みのレンズ補正の情報）と処理バージョンを
    /// 表示する（写真を開いて展開する）
    Source {
        /// variant の ID（写真。動画は開けない）
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
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
        /// コピー先の処理バージョンがコピー元より古ければ、コピー元の版に更新する（DATA-09。省略時はコピー先の
        /// 版のまま。古い版の写真では、レンズ補正・ハイライト復元・ノイズ軽減・カメラプロファイル・明瞭度・シャープ・効果・
        /// マスクなどは使われない）
        #[arg(long)]
        upgrade_process_version: bool,
    },
    /// 仮想コピーを作る（LIB-13。元の写真と同じスタックに入る（展開する））
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
    /// 仮想コピーの名前を変える（LIB-13。200 文字まで。書き出すファイルの名前は「元のファイル名-名前」）
    RenameCopy {
        /// 仮想コピーの variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// 新しい名前
        name: String,
    },
    /// 仮想コピーを元の写真（マスター）にする（LIB-13。前のマスターは仮想コピーになり、名前を受け継ぐ。現像設定・
    /// 評価などはそれぞれのまま）
    MakeMaster {
        /// 仮想コピーの variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// 表示範囲を描いて PNG（8bit、sRGB）に書く（ルーペの等倍表示・拡大とパンのコア API
    /// `render_region` を試す。範囲は出力の画像の内側に収める）
    Region(RegionArgs),
    /// ビフォー / アフターの表示（DEV-31）のビフォーのプレビューを PNG に書く（写真を開いて描く。切り抜きは
    /// 今の設定と同じ）
    Before(BeforeArgs),
    /// 比較表示（LIB-03）の同期ズームと同じ範囲を、2 枚までの写真について同時に描いて PNG に書く（コア API
    /// `render_view_region` を試す。範囲の中心は写真ごとの割合、大きさは出力の画素で、どの写真も同じ）
    Compare(CompareArgs),
    /// スナップショット（DEV-28）: 一覧・作成（今の状態・履歴の項目の状態）・名前の変更・削除・適用（履歴に
    /// 1 件記録する。処理バージョンも保存した版に戻す）。ビフォーに使うには develop before --source snapshot:<ID>
    #[command(subcommand)]
    Snapshot(SnapshotCommand),
    /// ローカル補正（マスク。DEV-18・DEV-19）: 一覧・追加・部品の追加と削除・名前と調整の変更・削除・重ねて
    /// 表示の PNG・色のスポイト。マスクと部品は 1 から数えた番号で指定する（`develop mask list` で見られる）。
    /// 位置・半径・ブラシの大きさは G 座標（ジオメトリ補正後・切り抜く前の全体の外接矩形の長辺を 1 とする。左上が
    /// 0,0）。処理バージョン 2 から使われる
    #[command(subcommand)]
    Mask(MaskCommand),
}

/// スナップショットの ID を解釈する（1 以上の整数）。
pub fn parse_snapshot_id(s: &str) -> Result<SnapshotId, String> {
    match s.trim().parse::<i64>() {
        Ok(n) if n >= 1 => Ok(SnapshotId::new(n)),
        _ => Err(format!("スナップショットの ID は 1 以上の整数です（{s}）")),
    }
}

/// 履歴の項目の ID を解釈する（1 以上の整数。`develop history` で見られる）。
pub fn parse_history_entry_id(s: &str) -> Result<HistoryEntryId, String> {
    match s.trim().parse::<i64>() {
        Ok(n) if n >= 1 => Ok(HistoryEntryId::new(n)),
        _ => Err(format!("履歴の項目の ID は 1 以上の整数です（{s}）")),
    }
}

/// `develop snapshot` のサブコマンド（DEV-28）。
#[derive(Debug, Subcommand)]
pub enum SnapshotCommand {
    /// 写真のスナップショットの一覧（作った順）
    List {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// スナップショットを作る（既定は今の状態。名前は 200 文字まで。同じ名前でもよい）
    Create {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// 名前
        name: String,
        /// 履歴の項目（develop history の ID）の状態から作る（今の設定・履歴の位置は変えない）
        #[arg(long, value_name = "ID", value_parser = parse_history_entry_id)]
        from_history: Option<HistoryEntryId>,
    },
    /// スナップショットの名前を変える
    Rename {
        /// スナップショットの ID
        #[arg(value_parser = parse_snapshot_id)]
        snapshot: SnapshotId,
        /// 新しい名前
        name: String,
    },
    /// スナップショットを削除する（写真の設定・履歴は変えない）
    Delete {
        /// スナップショットの ID
        #[arg(value_parser = parse_snapshot_id)]
        snapshot: SnapshotId,
    },
    /// スナップショットをその写真に適用する（履歴に「スナップショット「名前」を適用」を 1 件記録する。Undo で
    /// 戻せる）
    Apply {
        /// スナップショットの ID
        #[arg(value_parser = parse_snapshot_id)]
        snapshot: SnapshotId,
    },
}

/// プリセットの種類（DEV-29・EXP-02）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PresetKindArg {
    /// 現像プリセット。
    Develop,
    /// 書き出しプリセット。
    Export,
}

impl From<PresetKindArg> for genzo_api::PresetKind {
    fn from(k: PresetKindArg) -> Self {
        match k {
            PresetKindArg::Develop => genzo_api::PresetKind::Develop,
            PresetKindArg::Export => genzo_api::PresetKind::Export,
        }
    }
}

/// ファイル名の規則のオプション（`export`・`preset create-export`。EXP-02）。
#[derive(Debug, Clone, Default, Args)]
pub struct NamingArgs {
    /// 書き出すファイルの名前のテンプレート（import --rename と同じ項目: {date} {time} {year} {month} {day}
    /// {hour} {minute} {second} {seq} {name} {camera} {text}。例: "{date}-{seq}"。連番は指定した順。仮想コピーは
    /// 「-名前」を足す）
    #[arg(long, value_name = "TEMPLATE")]
    pub rename: Option<String>,
    /// テンプレートの {text} に入れる文字
    #[arg(long, value_name = "TEXT")]
    pub text: Option<String>,
    /// 連番（{seq}）の開始番号（既定 1）
    #[arg(long, value_name = "N")]
    pub seq_start: Option<u32>,
    /// 連番（{seq}）の桁数（1〜9。既定 4）
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=9))]
    pub seq_digits: Option<u8>,
}

/// `preset` のサブコマンド（DEV-29・EXP-02）。
#[derive(Debug, Subcommand)]
pub enum PresetCommand {
    /// プリセットのフォルダを表示する
    Dir,
    /// プリセットの一覧（フォルダ → 名前の順。使えないものは理由を付けて出す）
    List {
        /// 種類（省略時は両方）
        #[arg(long, value_enum)]
        kind: Option<PresetKindArg>,
    },
    /// プリセットの中身を表示する（現像プリセットは設定も。--json の settings はそのまま develop set に渡せる）
    Show {
        /// プリセット（ID か名前）
        preset: String,
        /// 種類（名前が両方の種類にあるときに指定する）
        #[arg(long, value_enum)]
        kind: Option<PresetKindArg>,
        /// フォルダ（名前で指定するときに、同じ名前が複数のフォルダにあれば指定する）
        #[arg(long, value_name = "FOLDER")]
        folder: Option<String>,
    },
    /// 写真の今の設定から現像プリセットを作る（--groups で含める項目。省略時は切り抜きと角度・マスクを除くすべて）
    CreateDevelop {
        /// 名前（100 文字まで）
        name: String,
        /// 元にする写真（variant の ID）
        #[arg(long, value_parser = parse_variant_id)]
        from: VariantId,
        /// フォルダ（省略時はフォルダなし）
        #[arg(long, value_name = "FOLDER", default_value = "")]
        folder: String,
        /// 含める項目（カンマ区切り）
        #[arg(long, value_enum, value_delimiter = ',')]
        groups: Vec<GroupArg>,
    },
    /// 現像プリセットを写真の今の設定で置き換える（--groups の省略時は今の含める項目のまま）
    UpdateDevelop {
        /// プリセット（ID か名前）
        preset: String,
        /// 元にする写真（variant の ID）
        #[arg(long, value_parser = parse_variant_id)]
        from: VariantId,
        /// 含める項目（カンマ区切り）
        #[arg(long, value_enum, value_delimiter = ',')]
        groups: Vec<GroupArg>,
        /// フォルダ（名前で指定するとき）
        #[arg(long, value_name = "FOLDER")]
        folder: Option<String>,
    },
    /// 現像プリセットを写真に適用する（含める項目だけを写す。写真の処理バージョンは変えない。履歴に
    /// 「プリセット「名前」を適用」を 1 件記録する。動画は除く）
    Apply {
        /// プリセット（ID か名前）
        preset: String,
        /// 適用先の variant の ID（複数可）
        #[arg(long, required = true, num_args = 1.., value_parser = parse_variant_id)]
        to: Vec<VariantId>,
        /// プリセットより古い処理バージョンの写真は、プリセットの版に更新する（DATA-09）
        #[arg(long)]
        upgrade_process_version: bool,
        /// フォルダ（名前で指定するとき）
        #[arg(long, value_name = "FOLDER")]
        folder: Option<String>,
    },
    /// 書き出しプリセットを作る（export と同じオプション。--dest で書き出し先を保存する（省略時は書き出すときに
    /// 指定する））
    CreateExport(Box<ExportPresetArgs>),
    /// 書き出しプリセットの中身を置き換える（指定しなかった項目は既定値。今の中身は preset show で見られる）
    UpdateExport(Box<ExportPresetArgs>),
    /// プリセットの名前を変える
    Rename {
        /// プリセット（ID か名前）
        preset: String,
        /// 新しい名前
        name: String,
        /// 種類
        #[arg(long, value_enum)]
        kind: Option<PresetKindArg>,
        /// フォルダ（名前で指定するとき）
        #[arg(long, value_name = "FOLDER")]
        folder: Option<String>,
    },
    /// プリセットを別のフォルダへ移す（空の名前 "" でフォルダなし）
    Move {
        /// プリセット（ID か名前）
        preset: String,
        /// 移す先のフォルダ
        #[arg(long, value_name = "FOLDER")]
        to: String,
        /// 種類
        #[arg(long, value_enum)]
        kind: Option<PresetKindArg>,
    },
    /// フォルダの名前を変える（そのフォルダのプリセットをすべて移す）
    RenameFolder {
        /// 種類
        #[arg(value_enum)]
        kind: PresetKindArg,
        /// 今のフォルダ
        folder: String,
        /// 新しい名前（"" でフォルダなし）
        name: String,
    },
    /// プリセットを削除する（ファイルを消す）
    Delete {
        /// プリセット（ID か名前）
        preset: String,
        /// 種類
        #[arg(long, value_enum)]
        kind: Option<PresetKindArg>,
        /// フォルダ（名前で指定するとき）
        #[arg(long, value_name = "FOLDER")]
        folder: Option<String>,
    },
    /// プリセットのファイル（.json）を読み込む（元のファイルは変えない。同じフォルダに同じ名前があれば
    /// 「 (2)」などを付ける）。読み込めなかったファイルがあれば終了コード 3
    Import {
        /// ファイル（複数可）
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// 種類（指定するとその種類のファイルだけ）
        #[arg(long, value_enum)]
        kind: Option<PresetKindArg>,
    },
    /// プリセットをファイル（<名前>.json）に書き出す（ほかの PC へ渡す。同じ名前のファイルがあれば -1 などを
    /// 付け、上書きしない）
    ExportFiles {
        /// プリセット（ID か名前。複数可）
        #[arg(required = true, num_args = 1..)]
        presets: Vec<String>,
        /// 書き出し先のフォルダ（あるフォルダ）
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// 種類
        #[arg(long, value_enum)]
        kind: Option<PresetKindArg>,
    },
}

/// `preset create-export`・`update-export` の引数（EXP-02）。
#[derive(Debug, Args)]
pub struct ExportPresetArgs {
    /// 名前（create-export）またはプリセット（update-export。ID か名前）
    pub name: String,
    /// フォルダ（create-export は保存するフォルダ。update-export は名前で指定するときのフォルダ）
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,
    /// 書き出し先のフォルダ（保存する。省略時は書き出すときに --out で指定する）
    #[arg(long, value_name = "DIR")]
    pub dest: Option<PathBuf>,
    /// 書き出しの設定。
    #[command(flatten)]
    pub opts: ExportOpts,
    /// ファイル名の規則。
    #[command(flatten)]
    pub naming: NamingArgs,
    /// 解像度・出力シャープ・動画（EXP-03・EXP-05）。
    #[command(flatten)]
    pub finish: FinishArgs,
    /// 透かし（EXP-03）。
    #[command(flatten)]
    pub watermark: WatermarkArgs,
}

/// `develop mask` のサブコマンド（DEV-18・DEV-19）。
#[derive(Debug, Subcommand)]
pub enum MaskCommand {
    /// マスクの一覧（番号・名前・有効か・部品・調整）
    List {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
    },
    /// マスクを足す（形を 1 つ指定する。調整もオプションで指定できる。履歴に 1 件記録する）
    Add {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// 最初の部品
        #[command(flatten)]
        part: MaskPartArgs,
        /// 名前（100 文字まで。省略時は「マスク 1」などの番号で表示する）
        #[arg(long)]
        name: Option<String>,
        /// 調整
        #[command(flatten)]
        delta: MaskDeltaArgs,
    },
    /// マスクに部品を足す（形を 1 つ指定する。色域・輝度の範囲の既定の合成は交差、ほかは加える）
    AddPart {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// マスクの番号（1 から）
        #[arg(value_parser = parse_mask_number)]
        mask: usize,
        /// 足す部品
        #[command(flatten)]
        part: MaskPartArgs,
    },
    /// マスクの部品を除く
    RemovePart {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// マスクの番号（1 から）
        #[arg(value_parser = parse_mask_number)]
        mask: usize,
        /// 部品の番号（1 から）
        #[arg(value_parser = parse_mask_number)]
        part: usize,
    },
    /// マスクの名前・有効か・調整を変える（指定しない項目は今の値のまま。履歴に 1 件記録する）
    Set {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// マスクの番号（1 から）
        #[arg(value_parser = parse_mask_number)]
        mask: usize,
        /// 名前（空にすると番号で表示する）
        #[arg(long)]
        name: Option<String>,
        /// 使うか
        #[arg(long, value_enum)]
        enabled: Option<SwitchArg>,
        /// 調整をすべて 0 に戻してから、指定した調整を当てる
        #[arg(long)]
        reset_adjustments: bool,
        /// 調整
        #[command(flatten)]
        delta: MaskDeltaArgs,
    },
    /// マスクを除く（履歴に 1 件記録する。Undo で戻せる）
    Remove {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// マスクの番号（1 から）
        #[arg(value_parser = parse_mask_number)]
        mask: usize,
    },
    /// マスクの重み（アプリの重ねて表示と同じ。プレビューの大きさ）を PNG（8bit のグレー。白が 100%）に書く
    /// （写真を開いて計算する）
    Overlay {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// マスクの番号（1 から）
        #[arg(value_parser = parse_mask_number)]
        mask: usize,
        /// 書き出す PNG のファイル（既にあれば使い方の誤り。上書きしない）
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// 切り抜く前の全体の大きさで書く（省略時は切り抜いた後のプレビュー）
        #[arg(long)]
        uncropped: bool,
    },
    /// 色域の範囲のスポイト: プレビューの点の G 座標と色（OKLab。`add --color-at` が見本に使う値）を表示する
    /// （写真を開いて計算する）
    PickColor {
        /// variant の ID
        #[arg(value_parser = parse_variant_id)]
        variant: VariantId,
        /// 点の横の位置（切り抜いた後のプレビューの幅に対する割合。0〜1）
        #[arg(long)]
        x: f64,
        /// 点の縦の位置（高さに対する割合。0〜1）
        #[arg(long)]
        y: f64,
    },
}

/// マスクの番号（1 から）を読む。
pub fn parse_mask_number(s: &str) -> Result<usize, String> {
    match s.trim().parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(format!("番号は 1 以上の整数で指定してください: {s}")),
    }
}

/// カンマ区切りの数を読む（`min`〜`max` 個）。
fn parse_numbers(s: &str, min: usize, max: usize, form: &str) -> Result<Vec<f32>, String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    if !(min..=max).contains(&parts.len()) {
        return Err(format!("「{form}」の形で指定してください: {s}"));
    }
    parts
        .iter()
        .map(|t| {
            t.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("値が数値ではありません: {t}"))
        })
        .collect()
}

/// `X0,Y0,X1,Y1` を読む。
fn parse_linear(s: &str) -> Result<[f32; 4], String> {
    let v = parse_numbers(s, 4, 4, "X0,Y0,X1,Y1")?;
    Ok([v[0], v[1], v[2], v[3]])
}

/// `CX,CY,RX,RY[,角度[,ぼかし]]` を読む（角度の既定 0、ぼかしの既定 50）。
fn parse_radial(s: &str) -> Result<[f32; 6], String> {
    let v = parse_numbers(s, 4, 6, "CX,CY,RX,RY[,角度[,ぼかし]]")?;
    Ok([
        v[0],
        v[1],
        v[2],
        v[3],
        v.get(4).copied().unwrap_or(0.0),
        v.get(5).copied().unwrap_or(50.0),
    ])
}

/// `MIN,MAX[,滑らかさ]` を読む（滑らかさの既定 50）。
fn parse_luminance(s: &str) -> Result<[f32; 3], String> {
    let v = parse_numbers(s, 2, 3, "MIN,MAX[,滑らかさ]")?;
    Ok([v[0], v[1], v.get(2).copied().unwrap_or(50.0)])
}

/// `X,Y`（プレビューの幅・高さに対する割合。0〜1）を読む。
fn parse_point(s: &str) -> Result<[f64; 2], String> {
    let v = parse_numbers(s, 2, 2, "X,Y")?;
    let (x, y) = (f64::from(v[0]), f64::from(v[1]));
    if !((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)) {
        return Err(format!("X・Y は 0〜1 の割合で指定してください: {s}"));
    }
    Ok([x, y])
}

/// マスクの部品の形（1 つだけ指定する）。
#[derive(Debug, Clone, Args)]
#[group(required = true, multiple = false)]
pub struct MaskShapeArgs {
    /// 線形グラデーション: 100% の点と 0% の点（X0,Y0,X1,Y1。G 座標）
    #[arg(long, value_name = "X0,Y0,X1,Y1", value_parser = parse_linear, allow_hyphen_values = true)]
    pub linear: Option<[f32; 4]>,
    /// 円形グラデーション: 中心と半径（CX,CY,RX,RY[,角度[,ぼかし]]。G 座標。角度は時計回りの度・既定 0、ぼかしは
    /// 0〜100・既定 50）
    #[arg(long, value_name = "CX,CY,RX,RY[,A[,F]]", value_parser = parse_radial, allow_hyphen_values = true)]
    pub radial: Option<[f32; 6]>,
    /// ブラシ: ストロークの JSON のファイル（- で標準入力。ストロークの配列か 1 つ。項目は erase・size（半径。
    /// G 座標）・feather・flow・density・points（G 座標の [x, y] の配列））
    #[arg(long, value_name = "FILE")]
    pub brush: Option<String>,
    /// 色域の範囲: プレビューの点（X,Y。切り抜いた後のプレビューの幅・高さに対する割合）の色を見本にする（繰り返すと
    /// 5 つまで。写真を開いて計算する）
    #[arg(long, value_name = "X,Y", value_parser = parse_point)]
    pub color_at: Vec<[f64; 2]>,
    /// 輝度の範囲（MIN,MAX[,滑らかさ]。0〜100。表示の明るさの %。滑らかさの既定 50）
    #[arg(long, value_name = "MIN,MAX[,S]", value_parser = parse_luminance)]
    pub luminance: Option<[f32; 3]>,
}

/// 部品の合成のしかた（genzo-model の MaskMode）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MaskModeArg {
    /// 加える（max）。
    Add,
    /// 除く（m · (1 − c)）。
    Subtract,
    /// 交差（m · c）。
    Intersect,
}

impl MaskModeArg {
    /// genzo-model の値。
    pub fn mode(self) -> genzo_model::MaskMode {
        match self {
            MaskModeArg::Add => genzo_model::MaskMode::Add,
            MaskModeArg::Subtract => genzo_model::MaskMode::Subtract,
            MaskModeArg::Intersect => genzo_model::MaskMode::Intersect,
        }
    }
}

/// マスクの部品（形・合成のしかた・反転）。
#[derive(Debug, Clone, Args)]
pub struct MaskPartArgs {
    /// 形
    #[command(flatten)]
    pub shape: MaskShapeArgs,
    /// 合成のしかた（省略時は、色域・輝度の範囲は intersect、ほかは add）
    #[arg(long, value_enum)]
    pub mode: Option<MaskModeArg>,
    /// 部品の範囲を反転する
    #[arg(long)]
    pub invert: bool,
    /// 色域の範囲の許容量（0〜100。既定 50。--color-at のとき）
    #[arg(long, value_name = "N")]
    pub color_amount: Option<f32>,
}

/// マスクの調整（genzo-model の LocalDelta。露光量は EV、ほかは -100〜100。指定しない項目は変えない）。
#[derive(Debug, Clone, Default, Args)]
pub struct MaskDeltaArgs {
    /// 色温度（-100〜100。正で暖かく、負で冷たく）
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub temperature: Option<f32>,
    /// 色かぶり補正（-100〜100。正でマゼンタ、負で緑）
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub tint: Option<f32>,
    /// 露光量（EV。-5〜5）
    #[arg(long, value_name = "EV", allow_hyphen_values = true)]
    pub exposure: Option<f32>,
    /// コントラスト
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub contrast: Option<f32>,
    /// ハイライト
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub highlights: Option<f32>,
    /// シャドウ
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub shadows: Option<f32>,
    /// 白レベル
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub whites: Option<f32>,
    /// 黒レベル
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub blacks: Option<f32>,
    /// テクスチャ
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub texture: Option<f32>,
    /// 明瞭度
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub clarity: Option<f32>,
    /// かすみの除去
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub dehaze: Option<f32>,
    /// 彩度
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub saturation: Option<f32>,
    /// シャープ（全体の適用量に足す。負の値はぼかす）
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub sharpness: Option<f32>,
}

impl MaskDeltaArgs {
    /// 指定した項目と値（genzo-model の LocalDelta::NAMES の名前）。
    fn items(&self) -> [(&'static str, Option<f32>); 13] {
        [
            ("temperature", self.temperature),
            ("tint", self.tint),
            ("exposure_ev", self.exposure),
            ("contrast", self.contrast),
            ("highlights", self.highlights),
            ("shadows", self.shadows),
            ("whites", self.whites),
            ("blacks", self.blacks),
            ("texture", self.texture),
            ("clarity", self.clarity),
            ("dehaze", self.dehaze),
            ("saturation", self.saturation),
            ("sharpness", self.sharpness),
        ]
    }

    /// どれかを指定したか。
    pub fn any(&self) -> bool {
        self.items().iter().any(|(_, v)| v.is_some())
    }

    /// 差分に当てる（値の範囲は genzo-model の検証で確かめる）。
    pub fn apply(&self, d: &mut genzo_model::LocalDelta) {
        let names = genzo_model::LocalDelta::NAMES;
        let items = self.items();
        for (slot, name) in d.values_mut().into_iter().zip(names) {
            if let Some((_, Some(v))) = items.iter().find(|(n, _)| *n == name) {
                *slot = *v;
            }
        }
    }
}

/// オン / オフの指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SwitchArg {
    /// 有効にする。
    On,
    /// 無効にする。
    Off,
}

impl SwitchArg {
    /// 有効か。
    pub fn is_on(self) -> bool {
        self == SwitchArg::On
    }
}

/// `develop set` のカメラプロファイル・ルックの指定（DEV-17。設定の JSON の `profile` と同じ項目。処理バージョン 2 から
/// 使われる）。値の範囲は genzo-model の検証で確かめる。
#[derive(Debug, Clone, Default, Args)]
pub struct ProfileArgs {
    /// カメラプロファイルの ID（`profile list` の ID。`standard` で標準の色。JSON の profile.camera）
    #[arg(long = "profile", value_name = "ID|standard")]
    pub camera: Option<String>,
    /// ルックの ID（`none` でルックなし。profile.look）
    #[arg(long, value_name = "ID|none")]
    pub look: Option<String>,
    /// ルックの適用量（0〜100。既定 100。profile.look_amount）
    #[arg(long, value_name = "N")]
    pub look_amount: Option<f32>,
}

impl ProfileArgs {
    /// どれかを指定したか。
    pub fn any(&self) -> bool {
        self.camera.is_some() || self.look.is_some() || self.look_amount.is_some()
    }

    /// 指定した項目を `profile` に当てる（指定しなかった項目は今の値のまま）。
    pub fn apply(&self, profile: &mut genzo_model::ProfileSettings) {
        if let Some(c) = &self.camera {
            profile.camera = (c != "standard").then(|| c.clone());
        }
        if let Some(l) = &self.look {
            profile.look = (l != "none").then(|| l.clone());
        }
        if let Some(a) = self.look_amount {
            profile.look_amount = a;
        }
    }
}

/// `develop set` のレンズ補正の指定（DEV-14。設定の JSON の `lens` と同じ項目。処理バージョン 2 から使われる）。
/// 値の範囲は genzo-model の検証で確かめる。
#[derive(Debug, Clone, Default, Args)]
pub struct LensArgs {
    /// プロファイル（RAW に埋め込まれた補正の情報）による補正（歪曲・周辺減光）を有効にする（JSON の
    /// lens.profile_enabled）
    #[arg(long, value_enum, value_name = "on|off")]
    pub lens_profile: Option<SwitchArg>,
    /// 倍率色収差を除去する（埋め込みの色収差の情報を使う。lens.remove_chromatic_aberration）
    #[arg(long, value_enum, value_name = "on|off")]
    pub remove_ca: Option<SwitchArg>,
    /// プロファイルの歪曲補正の適用量（%。0〜200、100 で情報どおり。lens.distortion_scale）
    #[arg(long, value_name = "PERCENT")]
    pub distortion_scale: Option<f32>,
    /// プロファイルの周辺減光補正の適用量（%。0〜200、100 で情報どおり。lens.vignetting_scale）
    #[arg(long, value_name = "PERCENT")]
    pub vignetting_scale: Option<f32>,
    /// 手動の歪曲補正（-100〜100。正で樽型、負で糸巻き型の歪曲を補正する。lens.distortion）
    #[arg(long, allow_hyphen_values = true, value_name = "N")]
    pub distortion: Option<f32>,
    /// 手動の周辺減光の補正量（-100〜100。正で四隅を明るくする。lens.vignetting_amount）
    #[arg(long, allow_hyphen_values = true, value_name = "N")]
    pub vignetting: Option<f32>,
    /// 手動の周辺減光の中点（0〜100。既定 50。lens.vignetting_midpoint）
    #[arg(long, value_name = "N")]
    pub vignetting_midpoint: Option<f32>,
    /// 手動の倍率色収差の補正の赤 / シアン（-100〜100。lens.ca_red_cyan）
    #[arg(long, allow_hyphen_values = true, value_name = "N")]
    pub ca_red_cyan: Option<f32>,
    /// 手動の倍率色収差の補正の青 / 黄（-100〜100。lens.ca_blue_yellow）
    #[arg(long, allow_hyphen_values = true, value_name = "N")]
    pub ca_blue_yellow: Option<f32>,
}

impl LensArgs {
    /// どれかを指定したか。
    pub fn any(&self) -> bool {
        self.lens_profile.is_some()
            || self.remove_ca.is_some()
            || self.distortion_scale.is_some()
            || self.vignetting_scale.is_some()
            || self.distortion.is_some()
            || self.vignetting.is_some()
            || self.vignetting_midpoint.is_some()
            || self.ca_red_cyan.is_some()
            || self.ca_blue_yellow.is_some()
    }

    /// 指定した項目を `lens` に当てる（指定しなかった項目は今の値のまま）。
    pub fn apply(&self, lens: &mut genzo_model::LensCorrection) {
        if let Some(v) = self.lens_profile {
            lens.profile_enabled = v.is_on();
        }
        if let Some(v) = self.remove_ca {
            lens.remove_chromatic_aberration = v.is_on();
        }
        let set = |dst: &mut f32, v: Option<f32>| {
            if let Some(v) = v {
                *dst = v;
            }
        };
        set(&mut lens.distortion_scale, self.distortion_scale);
        set(&mut lens.vignetting_scale, self.vignetting_scale);
        set(&mut lens.distortion, self.distortion);
        set(&mut lens.vignetting_amount, self.vignetting);
        set(&mut lens.vignetting_midpoint, self.vignetting_midpoint);
        set(&mut lens.ca_red_cyan, self.ca_red_cyan);
        set(&mut lens.ca_blue_yellow, self.ca_blue_yellow);
    }
}

/// ハイライト復元の方式（genzo-model の `HighlightMethod`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HighlightArg {
    /// 白で切りそろえる（復元しない。処理バージョン 1 と同じ）。
    Clip,
    /// 明るさだけを復元する（無彩色）。
    Neutral,
    /// 色を保って復元する（近傍の色の比で再構成する）。
    Reconstruct,
}

impl HighlightArg {
    /// genzo-model の値。
    pub fn method(self) -> genzo_model::HighlightMethod {
        match self {
            HighlightArg::Clip => genzo_model::HighlightMethod::Clip,
            HighlightArg::Neutral => genzo_model::HighlightMethod::Neutral,
            HighlightArg::Reconstruct => genzo_model::HighlightMethod::Reconstruct,
        }
    }
}

/// `develop set` のハイライト復元（DEV-06）・ノイズ軽減（DEV-13）の指定（設定の JSON の `highlight_recovery`・
/// `detail.noise_reduction` と同じ項目。処理バージョン 2 から使われる。RAW の写真だけ）。値の範囲は genzo-model の
/// 検証で確かめる。
#[derive(Debug, Clone, Default, Args)]
pub struct SensorArgs {
    /// ハイライト復元の方式（clip: 白で切りそろえる（既定）・neutral: 明るさだけ・reconstruct: 色を保って復元。
    /// highlight_recovery.method）
    #[arg(long, value_enum, value_name = "METHOD")]
    pub highlight_recovery: Option<HighlightArg>,
    /// ハイライト復元の量（0〜100。既定 100。highlight_recovery.amount）
    #[arg(long, value_name = "N")]
    pub highlight_amount: Option<f32>,
    /// 輝度ノイズの軽減（0〜100。detail.noise_reduction.luminance）
    #[arg(long, value_name = "N")]
    pub nr_luminance: Option<f32>,
    /// 輝度ノイズの軽減のディテール（0〜100。既定 50。detail.noise_reduction.luminance_detail）
    #[arg(long, value_name = "N")]
    pub nr_luminance_detail: Option<f32>,
    /// 輝度ノイズの軽減のコントラスト（0〜100。既定 0。detail.noise_reduction.luminance_contrast）
    #[arg(long, value_name = "N")]
    pub nr_luminance_contrast: Option<f32>,
    /// カラーノイズの軽減（0〜100。detail.noise_reduction.color）
    #[arg(long, value_name = "N")]
    pub nr_color: Option<f32>,
    /// カラーノイズの軽減のディテール（0〜100。既定 50。detail.noise_reduction.color_detail）
    #[arg(long, value_name = "N")]
    pub nr_color_detail: Option<f32>,
}

impl SensorArgs {
    /// どれかを指定したか。
    pub fn any(&self) -> bool {
        self.highlight_recovery.is_some()
            || self.highlight_amount.is_some()
            || self.nr_luminance.is_some()
            || self.nr_luminance_detail.is_some()
            || self.nr_luminance_contrast.is_some()
            || self.nr_color.is_some()
            || self.nr_color_detail.is_some()
    }

    /// 指定した項目を `settings` に当てる（指定しなかった項目は今の値のまま）。
    pub fn apply(&self, settings: &mut genzo_model::DevelopSettings) {
        if let Some(m) = self.highlight_recovery {
            settings.highlight_recovery.method = m.method();
        }
        let set = |dst: &mut f32, v: Option<f32>| {
            if let Some(v) = v {
                *dst = v;
            }
        };
        set(
            &mut settings.highlight_recovery.amount,
            self.highlight_amount,
        );
        let nr = &mut settings.detail.noise_reduction;
        set(&mut nr.luminance, self.nr_luminance);
        set(&mut nr.luminance_detail, self.nr_luminance_detail);
        set(&mut nr.luminance_contrast, self.nr_luminance_contrast);
        set(&mut nr.color, self.nr_color);
        set(&mut nr.color_detail, self.nr_color_detail);
    }
}

/// `develop set` のテクスチャ・明瞭度・かすみの除去（DEV-11）・シャープ（DEV-12）・効果（周辺光量・粒子。DEV-24）の
/// 指定（設定の JSON の `presence`・`detail.sharpening`・`effects` と同じ項目。処理バージョン 2 から使われる）。値の
/// 範囲は genzo-model の検証で確かめる。
#[derive(Debug, Clone, Default, Args)]
pub struct DetailArgs {
    /// テクスチャ（-100〜100。presence.texture）
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub texture: Option<f32>,
    /// 明瞭度（-100〜100。presence.clarity）
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub clarity: Option<f32>,
    /// かすみの除去（-100〜100。負でかすみを足す。presence.dehaze）
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub dehaze: Option<f32>,
    /// シャープの適用量（0〜150。detail.sharpening.amount）
    #[arg(long, value_name = "N")]
    pub sharpen_amount: Option<f32>,
    /// シャープの半径（0.5〜3.0。写真のフル解像度の画素。既定 1.0。detail.sharpening.radius）
    #[arg(long, value_name = "PX")]
    pub sharpen_radius: Option<f32>,
    /// シャープのディテール（0〜100。既定 25。detail.sharpening.detail）
    #[arg(long, value_name = "N")]
    pub sharpen_detail: Option<f32>,
    /// シャープのマスク（0〜100。既定 0。detail.sharpening.masking）
    #[arg(long, value_name = "N")]
    pub sharpen_masking: Option<f32>,
    /// 切り抜き後の周辺光量補正の量（-100〜100。負で暗く、正で明るく。effects.vignette.amount）
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub vignette_amount: Option<f32>,
    /// 切り抜き後の周辺光量補正の中点（0〜100。既定 50。effects.vignette.midpoint）
    #[arg(long, value_name = "N")]
    pub vignette_midpoint: Option<f32>,
    /// 切り抜き後の周辺光量補正の丸み（-100〜100。既定 0。effects.vignette.roundness）
    #[arg(long, value_name = "N", allow_hyphen_values = true)]
    pub vignette_roundness: Option<f32>,
    /// 切り抜き後の周辺光量補正のぼかし（0〜100。既定 50。effects.vignette.feather）
    #[arg(long, value_name = "N")]
    pub vignette_feather: Option<f32>,
    /// 切り抜き後の周辺光量補正のハイライトの保護（0〜100。既定 0。effects.vignette.highlights）
    #[arg(long, value_name = "N")]
    pub vignette_highlights: Option<f32>,
    /// 粒子の量（0〜100。effects.grain.amount）
    #[arg(long, value_name = "N")]
    pub grain_amount: Option<f32>,
    /// 粒子の大きさ（0〜100。既定 25。effects.grain.size）
    #[arg(long, value_name = "N")]
    pub grain_size: Option<f32>,
    /// 粒子の粗さ（0〜100。既定 50。effects.grain.roughness）
    #[arg(long, value_name = "N")]
    pub grain_roughness: Option<f32>,
}

impl DetailArgs {
    /// 値の指定と、当てる先（設定の項目）の組。
    fn fields(&self) -> [Option<f32>; 15] {
        [
            self.texture,
            self.clarity,
            self.dehaze,
            self.sharpen_amount,
            self.sharpen_radius,
            self.sharpen_detail,
            self.sharpen_masking,
            self.vignette_amount,
            self.vignette_midpoint,
            self.vignette_roundness,
            self.vignette_feather,
            self.vignette_highlights,
            self.grain_amount,
            self.grain_size,
            self.grain_roughness,
        ]
    }

    /// どれかを指定したか。
    pub fn any(&self) -> bool {
        self.fields().iter().any(Option::is_some)
    }

    /// 指定した項目を `settings` に当てる（指定しなかった項目は今の値のまま）。
    pub fn apply(&self, settings: &mut genzo_model::DevelopSettings) {
        let p = &mut settings.presence;
        let sh = &mut settings.detail.sharpening;
        let v = &mut settings.effects.vignette;
        let g = &mut settings.effects.grain;
        let targets: [&mut f32; 15] = [
            &mut p.texture,
            &mut p.clarity,
            &mut p.dehaze,
            &mut sh.amount,
            &mut sh.radius,
            &mut sh.detail,
            &mut sh.masking,
            &mut v.amount,
            &mut v.midpoint,
            &mut v.roundness,
            &mut v.feather,
            &mut v.highlights,
            &mut g.amount,
            &mut g.size,
            &mut g.roughness,
        ];
        for (dst, value) in targets.into_iter().zip(self.fields()) {
            if let Some(value) = value {
                *dst = value;
            }
        }
    }
}

/// `develop set` の色（DEV-09・DEV-10・DEV-25）の指定（設定の JSON の `color.mixer`・`color.grading`・
/// `color.black_white` と同じ項目。処理バージョン 2 から使われる）。値の範囲は genzo-model の検証で確かめる。
#[derive(Debug, Clone, Default, Args)]
pub struct ColorArgs {
    /// 白黒にする（on）・カラーに戻す（off）（color.black_white.enabled。画面の V キー）
    #[arg(long, value_enum, value_name = "on|off")]
    pub black_white: Option<SwitchArg>,
    /// カラーミキサーの色相（帯=値。-100〜100。帯は red・orange・yellow・green・aqua・blue・purple・magenta。
    /// 繰り返して指定できる。color.mixer.<帯>.hue）
    #[arg(long, value_name = "BAND=N", value_parser = parse_band_value)]
    pub hsl_hue: Vec<(genzo_model::HueBand, f32)>,
    /// カラーミキサーの彩度（帯=値。-100〜100。color.mixer.<帯>.saturation）
    #[arg(long, value_name = "BAND=N", value_parser = parse_band_value)]
    pub hsl_saturation: Vec<(genzo_model::HueBand, f32)>,
    /// カラーミキサーの輝度（帯=値。-100〜100。color.mixer.<帯>.luminance）
    #[arg(long, value_name = "BAND=N", value_parser = parse_band_value)]
    pub hsl_luminance: Vec<(genzo_model::HueBand, f32)>,
    /// 白黒ミキサー（帯=値。-100〜100。白黒のときの色ごとの明るさ。color.black_white.mix.<帯>）
    #[arg(long, value_name = "BAND=N", value_parser = parse_band_value)]
    pub bw_mix: Vec<(genzo_model::HueBand, f32)>,
    /// カラーグレーディングのシャドウ（色相,強さ か 色相,強さ,輝度。色相 0〜360・強さ 0〜100・輝度 -100〜100
    /// （省くと 0）。color.grading.shadows）
    #[arg(long, value_name = "HUE,SAT[,LUM]", value_parser = parse_grading_wheel)]
    pub grading_shadows: Option<genzo_model::GradingWheel>,
    /// カラーグレーディングの中間調（色相,強さ か 色相,強さ,輝度。color.grading.midtones）
    #[arg(long, value_name = "HUE,SAT[,LUM]", value_parser = parse_grading_wheel)]
    pub grading_midtones: Option<genzo_model::GradingWheel>,
    /// カラーグレーディングのハイライト（色相,強さ か 色相,強さ,輝度。color.grading.highlights）
    #[arg(long, value_name = "HUE,SAT[,LUM]", value_parser = parse_grading_wheel)]
    pub grading_highlights: Option<genzo_model::GradingWheel>,
    /// カラーグレーディングの全体（色相,強さ か 色相,強さ,輝度。color.grading.global）
    #[arg(long, value_name = "HUE,SAT[,LUM]", value_parser = parse_grading_wheel)]
    pub grading_global: Option<genzo_model::GradingWheel>,
    /// カラーグレーディングのブレンド（0〜100。既定 50。color.grading.blending）
    #[arg(long, value_name = "N")]
    pub grading_blending: Option<f32>,
    /// カラーグレーディングのバランス（-100〜100。正でハイライトの範囲を広げる。color.grading.balance）
    #[arg(long, allow_hyphen_values = true, value_name = "N")]
    pub grading_balance: Option<f32>,
}

impl ColorArgs {
    /// どれかを指定したか。
    pub fn any(&self) -> bool {
        self.black_white.is_some()
            || !self.hsl_hue.is_empty()
            || !self.hsl_saturation.is_empty()
            || !self.hsl_luminance.is_empty()
            || !self.bw_mix.is_empty()
            || self.grading_shadows.is_some()
            || self.grading_midtones.is_some()
            || self.grading_highlights.is_some()
            || self.grading_global.is_some()
            || self.grading_blending.is_some()
            || self.grading_balance.is_some()
    }

    /// 指定した項目を `color` に当てる（指定しなかった項目は今の値のまま。同じ帯を繰り返したら後の値）。
    pub fn apply(&self, color: &mut genzo_model::ColorParams) {
        if let Some(v) = self.black_white {
            color.black_white.enabled = v.is_on();
        }
        for (band, v) in &self.hsl_hue {
            color.mixer.get_mut(*band).hue = *v;
        }
        for (band, v) in &self.hsl_saturation {
            color.mixer.get_mut(*band).saturation = *v;
        }
        for (band, v) in &self.hsl_luminance {
            color.mixer.get_mut(*band).luminance = *v;
        }
        for (band, v) in &self.bw_mix {
            *color.black_white.mix.get_mut(*band) = *v;
        }
        let g = &mut color.grading;
        for (dst, src) in [
            (&mut g.shadows, self.grading_shadows),
            (&mut g.midtones, self.grading_midtones),
            (&mut g.highlights, self.grading_highlights),
            (&mut g.global, self.grading_global),
        ] {
            if let Some(w) = src {
                *dst = w;
            }
        }
        if let Some(v) = self.grading_blending {
            g.blending = v;
        }
        if let Some(v) = self.grading_balance {
            g.balance = v;
        }
    }
}

/// `帯=値`（例: `blue=-30`）を読む。
fn parse_band_value(s: &str) -> Result<(genzo_model::HueBand, f32), String> {
    let (band, value) = s
        .split_once('=')
        .ok_or_else(|| format!("「帯=値」の形で指定してください（例: blue=-30）: {s}"))?;
    let band = genzo_model::HueBand::from_key(band.trim()).ok_or_else(|| {
        format!("帯は red・orange・yellow・green・aqua・blue・purple・magenta のどれかです: {band}")
    })?;
    let value: f32 = value
        .trim()
        .parse()
        .map_err(|_| format!("値が数値ではありません: {value}"))?;
    Ok((band, value))
}

/// `色相,強さ[,輝度]`（例: `220,30` や `40,25,-10`）を読む。
fn parse_grading_wheel(s: &str) -> Result<genzo_model::GradingWheel, String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(format!(
            "「色相,強さ」か「色相,強さ,輝度」の形で指定してください（例: 220,30,-10）: {s}"
        ));
    }
    let num = |t: &str| {
        t.parse::<f32>()
            .map_err(|_| format!("値が数値ではありません: {t}"))
    };
    Ok(genzo_model::GradingWheel {
        hue: num(parts[0])?,
        saturation: num(parts[1])?,
        luminance: match parts.get(2) {
            Some(t) => num(t)?,
            None => 0.0,
        },
    })
}

/// `develop compare` の引数（LIB-03）。
#[derive(Debug, Args)]
pub struct CompareArgs {
    /// variant の ID（1〜2 枚。写真。動画は描けない）
    #[arg(required = true, value_parser = parse_variant_id)]
    pub variants: Vec<VariantId>,
    /// 範囲の中心の横の位置（出力の画像の幅に対する割合。0〜1）
    #[arg(long, default_value_t = 0.5)]
    pub cx: f64,
    /// 範囲の中心の縦の位置（出力の画像の高さに対する割合。0〜1）
    #[arg(long, default_value_t = 0.5)]
    pub cy: f64,
    /// 範囲の幅（出力の画素。写真より大きければ写真の幅）
    #[arg(long)]
    pub width: f64,
    /// 範囲の高さ（出力の画素。写真より大きければ写真の高さ）
    #[arg(long)]
    pub height: f64,
    /// 表示の倍率（1 = 等倍、0.5 = 縮小、2 = 拡大）
    #[arg(long, default_value_t = 1.0)]
    pub scale: f64,
    /// PNG を書くフォルダ（`compare-<面>-<ID>.png`。なければ作る。既にあるファイルには書かない）
    #[arg(long, value_name = "DIR")]
    pub out_dir: PathBuf,
}

/// `develop region` の引数。
#[derive(Debug, Args)]
pub struct RegionArgs {
    /// variant の ID（写真。動画は描けない）
    #[arg(value_parser = parse_variant_id)]
    pub variant: VariantId,
    /// 範囲の左上の x（--space の座標）
    #[arg(long, allow_hyphen_values = true)]
    pub x: f64,
    /// 範囲の左上の y
    #[arg(long, allow_hyphen_values = true)]
    pub y: f64,
    /// 範囲の幅
    #[arg(long)]
    pub width: f64,
    /// 範囲の高さ
    #[arg(long)]
    pub height: f64,
    /// 座標系: output = 切り抜いた後の画像の画素（フル解像度）、geometry = 切り抜く前の全体で長辺を 1
    /// とした値
    #[arg(long, value_enum, default_value = "output")]
    pub space: RegionSpaceArg,
    /// 表示の倍率（1 = 等倍、0.5 = 縮小、2 = 拡大）
    #[arg(long, default_value_t = 1.0)]
    pub scale: f64,
    /// ビフォー / アフターの表示のビフォー（DEV-31）の設定で描く（initial = 取り込んだ直後の設定（既定）、
    /// current = 今の設定、history:<ID> = 履歴の項目、snapshot:<ID> = スナップショット。切り抜きは今の設定と同じ）
    #[arg(long, value_name = "SOURCE", value_parser = parse_before_source)]
    pub before: Option<BeforeSource>,
    /// 書き出す PNG のファイル（既にあれば使い方の誤り。上書きしない）
    #[arg(long, value_name = "FILE")]
    pub out: PathBuf,
}

/// ビフォーの出どころを解釈する（`initial`・`current`・`history:<ID>`・`snapshot:<ID>`）。
pub fn parse_before_source(s: &str) -> Result<BeforeSource, String> {
    let t = s.trim();
    let id = |v: &str| -> Result<i64, String> {
        match v.trim().parse::<i64>() {
            Ok(n) if n >= 1 => Ok(n),
            _ => Err(format!("ID は 1 以上の整数です（{s}）")),
        }
    };
    match t.split_once(':') {
        None if t == "initial" => Ok(BeforeSource::Initial),
        None if t == "current" => Ok(BeforeSource::Current),
        Some(("history", v)) => Ok(BeforeSource::History {
            entry_id: HistoryEntryId::new(id(v)?),
        }),
        Some(("snapshot", v)) => Ok(BeforeSource::Snapshot {
            snapshot_id: SnapshotId::new(id(v)?),
        }),
        _ => Err(format!(
            "ビフォーは initial・current・history:<ID>・snapshot:<ID> のどれかです（{s}）"
        )),
    }
}

/// `develop before` の引数（DEV-31）。
#[derive(Debug, Args)]
pub struct BeforeArgs {
    /// variant の ID（写真。動画は描けない）
    #[arg(value_parser = parse_variant_id)]
    pub variant: VariantId,
    /// ビフォーの出どころ（initial = 取り込んだ直後の設定、current = 今の設定、history:<ID>、snapshot:<ID>）
    #[arg(long, value_name = "SOURCE", default_value = "initial", value_parser = parse_before_source)]
    pub source: BeforeSource,
    /// ビフォーのプレビューを書き出す PNG のファイル（既にあれば使い方の誤り。上書きしない）
    #[arg(long, value_name = "FILE")]
    pub out: PathBuf,
    /// アフター（今の設定）のプレビューも書き出す PNG のファイル（同じ寸法。省略可）
    #[arg(long, value_name = "FILE")]
    pub after: Option<PathBuf>,
}

/// 表示範囲の座標系（genzo-api の `RegionSpace`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RegionSpaceArg {
    /// 出力座標（C）。
    Output,
    /// ジオメトリ補正後の全体（G）。
    Geometry,
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
    /// キーワード（XMP の dc:subject。LIB-09）を書き出さない（既定は、付けたキーワードと上の階層のうち
    /// 「書き出すファイルに含める」ものを書く）
    #[arg(long)]
    pub remove_keywords: bool,
    /// 同じ名前のファイルがあるとき（連番を付ける / 上書きする / 飛ばす。省略時は連番。export --preset では
    /// プリセットの値）
    #[arg(long, value_enum)]
    pub on_conflict: Option<ConflictArg>,
}

/// 合成の比べ方（DEV-34）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompositeModeArg {
    /// 比較明（チャンネルごとの最大。Photoshop の「比較（明）」）
    Lighten,
    /// 比較暗（チャンネルごとの最小）
    Darken,
    /// カラー比較明（輝度が最大の写真の画素をそのまま採る）
    LighterColor,
    /// カラー比較暗（輝度が最小の写真の画素をそのまま採る）
    DarkerColor,
}

impl From<CompositeModeArg> for genzo_model::CompositeMode {
    fn from(m: CompositeModeArg) -> Self {
        match m {
            CompositeModeArg::Lighten => Self::Lighten,
            CompositeModeArg::Darken => Self::Darken,
            CompositeModeArg::LighterColor => Self::LighterColor,
            CompositeModeArg::DarkerColor => Self::DarkerColor,
        }
    }
}

/// `composite` の引数（DEV-34）。
#[derive(Debug, Args)]
pub struct CompositeArgs {
    /// variant の ID（2 枚以上。動画は除く）
    #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
    pub variants: Vec<VariantId>,
    /// 比べ方（省略時は lighten）
    #[arg(long, value_enum, default_value_t = CompositeModeArg::Lighten)]
    pub mode: CompositeModeArg,
    /// 形式（省略時は tiff（16bit））
    #[arg(long, value_enum)]
    pub format: Option<FormatArg>,
    /// JPEG の品質（1〜100。--format jpeg のときだけ）
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub quality: Option<u8>,
    /// 色空間（比べるのもこの色空間の値。省略時は adobergb）
    #[arg(long, value_enum)]
    pub color_space: Option<ColorSpaceArg>,
    /// 保存先のフォルダ（省略時は 1 枚目（撮影日時が最も早い写真）と同じフォルダ）
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// 元の写真とスタックにまとめない
    #[arg(long)]
    pub no_stack: bool,
    /// GPS 情報を書かない
    #[arg(long)]
    pub remove_gps: bool,
    /// 合成せずに、確認の結果（写真の並び・設定の違い・保存先とファイル名）だけを表示する
    #[arg(long)]
    pub check: bool,
}

/// `export` の引数。
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// variant の ID（複数可）
    #[arg(required = true, num_args = 1.., value_parser = parse_variant_id)]
    pub variants: Vec<VariantId>,
    /// 書き出し先のフォルダ（--preset に書き出し先があれば省略できる）
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// 書き出しプリセット（EXP-02。ID か名前）。指定したオプションはプリセットの値より優先する（--remove-gps・
    /// --remove-keywords はオンにするだけ）
    #[arg(long, value_name = "PRESET")]
    pub preset: Option<String>,
    /// 書き出しの設定。
    #[command(flatten)]
    pub opts: ExportOpts,
    /// ファイル名の規則（EXP-02）。
    #[command(flatten)]
    pub naming: NamingArgs,
    /// プリセットのファイル名の規則を使わず、元のファイル名で書き出す
    #[arg(long, conflicts_with = "rename")]
    pub keep_names: bool,
    /// 解像度・出力シャープ・動画（EXP-03・EXP-05）。
    #[command(flatten)]
    pub finish: FinishArgs,
    /// 透かし（EXP-03）。
    #[command(flatten)]
    pub watermark: WatermarkArgs,
}

/// 解像度・出力シャープ（EXP-03）・動画（EXP-05）のオプション（`export` と `preset create-export`・`update-export`）。
#[derive(Debug, Clone, Default, Args)]
pub struct FinishArgs {
    /// 解像度（ppi。1〜1200。既定 72。Exif と PNG に書く（画素数は変えない）。印刷用の出力シャープの半径にも使う）
    #[arg(long, value_name = "PPI", value_parser = clap::value_parser!(u32).range(1..=1200))]
    pub ppi: Option<u32>,
    /// 出力シャープの対象（縮小の後に、出力の画素の大きさに合わせてかける。screen: 画面 / matte: マット紙 /
    /// glossy: 光沢紙。紙は --ppi で半径を決める）
    #[arg(
        long,
        value_enum,
        value_name = "MEDIA",
        conflicts_with = "no_output_sharpen"
    )]
    pub output_sharpen: Option<SharpenMediaArg>,
    /// 出力シャープの適用量（既定 standard）
    #[arg(long, value_enum, value_name = "AMOUNT")]
    pub sharpen_amount: Option<SharpenAmountArg>,
    /// 出力シャープをかけない（プリセットの出力シャープを使わない）
    #[arg(long)]
    pub no_output_sharpen: bool,
    /// 動画を書き出さない（既定は、元のファイルのまま再エンコードせずにコピーする。--remove-gps では書き出さない）
    #[arg(long)]
    pub skip_videos: bool,
}

/// 透かしのオプション（EXP-03。`export`・`preset create-export`・`update-export`・`watermark preview`）。
#[derive(Debug, Clone, Default, Args)]
pub struct WatermarkArgs {
    /// 透かしの文字（\n で改行。5 行・200 文字まで）。--watermark-font でフォントを指定する
    #[arg(long, value_name = "TEXT", conflicts_with = "watermark_image")]
    pub watermark_text: Option<String>,
    /// 透かしの文字のフォントのファイル（TrueType・OpenType・.ttc。アプリは同梱しない。OS のフォントのフォルダは
    /// genzo watermark font-dirs）
    #[arg(long, value_name = "FILE")]
    pub watermark_font: Option<PathBuf>,
    /// フォントのコレクション（.ttc）の中の番号（0 から。既定 0）
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(0..=255))]
    pub watermark_font_index: Option<u32>,
    /// 透かしの文字の色（#RRGGBB の sRGB。既定 #FFFFFF）
    #[arg(long, value_name = "#RRGGBB", value_parser = parse_rgb_color)]
    pub watermark_color: Option<genzo_model::RgbColor>,
    /// 透かしの画像（PNG・JPEG・TIFF。PNG の透明度を使う）
    #[arg(long, value_name = "FILE")]
    pub watermark_image: Option<PathBuf>,
    /// 透かしの位置（既定 bottom-right）
    #[arg(long, value_enum, value_name = "POSITION")]
    pub watermark_position: Option<WatermarkPositionArg>,
    /// 透かしの大きさ（%。文字は文字の大きさ、画像は幅を、写真の短辺に対する割合で。1〜100。既定は文字 4・画像 20）
    #[arg(long, value_name = "PERCENT", value_parser = clap::value_parser!(u8).range(1..=100))]
    pub watermark_size: Option<u8>,
    /// 透かしの不透明度（%。1〜100。既定 60）
    #[arg(long, value_name = "PERCENT", value_parser = clap::value_parser!(u8).range(1..=100))]
    pub watermark_opacity: Option<u8>,
    /// 透かしの余白（%。写真の端から、短辺に対する割合で。0〜25。既定 2）
    #[arg(long, value_name = "PERCENT", value_parser = clap::value_parser!(u8).range(0..=25))]
    pub watermark_margin: Option<u8>,
    /// 透かしを付けない（プリセットの透かしを使わない）
    #[arg(long, conflicts_with_all = ["watermark_text", "watermark_image"])]
    pub no_watermark: bool,
}

impl WatermarkArgs {
    /// 中身以外の透かしのオプション（フォント・色・位置・大きさ・不透明度・余白）を 1 つでも指定したか。
    pub fn has_adjustments(&self) -> bool {
        self.watermark_font.is_some()
            || self.watermark_font_index.is_some()
            || self.watermark_color.is_some()
            || self.watermark_position.is_some()
            || self.watermark_size.is_some()
            || self.watermark_opacity.is_some()
            || self.watermark_margin.is_some()
    }
}

/// 出力シャープの対象。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SharpenMediaArg {
    /// 画面。
    Screen,
    /// マット紙。
    Matte,
    /// 光沢紙。
    Glossy,
}

impl From<SharpenMediaArg> for genzo_model::SharpenMedia {
    fn from(m: SharpenMediaArg) -> Self {
        match m {
            SharpenMediaArg::Screen => Self::Screen,
            SharpenMediaArg::Matte => Self::MattePaper,
            SharpenMediaArg::Glossy => Self::GlossyPaper,
        }
    }
}

/// 出力シャープの適用量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SharpenAmountArg {
    /// 弱。
    Low,
    /// 標準。
    Standard,
    /// 強。
    High,
}

impl From<SharpenAmountArg> for genzo_model::SharpenAmount {
    fn from(a: SharpenAmountArg) -> Self {
        match a {
            SharpenAmountArg::Low => Self::Low,
            SharpenAmountArg::Standard => Self::Standard,
            SharpenAmountArg::High => Self::High,
        }
    }
}

/// 透かしの位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WatermarkPositionArg {
    /// 左上。
    TopLeft,
    /// 上。
    Top,
    /// 右上。
    TopRight,
    /// 左。
    Left,
    /// 中央。
    Center,
    /// 右。
    Right,
    /// 左下。
    BottomLeft,
    /// 下。
    Bottom,
    /// 右下。
    BottomRight,
}

impl From<WatermarkPositionArg> for genzo_model::WatermarkAnchor {
    fn from(p: WatermarkPositionArg) -> Self {
        match p {
            WatermarkPositionArg::TopLeft => Self::TopLeft,
            WatermarkPositionArg::Top => Self::Top,
            WatermarkPositionArg::TopRight => Self::TopRight,
            WatermarkPositionArg::Left => Self::Left,
            WatermarkPositionArg::Center => Self::Center,
            WatermarkPositionArg::Right => Self::Right,
            WatermarkPositionArg::BottomLeft => Self::BottomLeft,
            WatermarkPositionArg::Bottom => Self::Bottom,
            WatermarkPositionArg::BottomRight => Self::BottomRight,
        }
    }
}

/// `#RRGGBB` を読む。
fn parse_rgb_color(s: &str) -> Result<genzo_model::RgbColor, String> {
    genzo_model::RgbColor::parse(s)
        .ok_or_else(|| format!("色は #RRGGBB の形で指定してください（{s}）"))
}

/// `watermark` のサブコマンド（EXP-03。カタログは使わない）。
#[derive(Debug, Subcommand)]
pub enum WatermarkCommand {
    /// 透かしの見本を PNG に書く（灰色の濃淡の見本の画像に、書き出しと同じ計算で重ねる。透かしはワーカーで作る）
    Preview(WatermarkPreviewArgs),
    /// OS の既定のフォントのフォルダ（透かしの文字のフォントを探す場所。あるものだけ）
    FontDirs,
}

/// `watermark preview` の引数。
#[derive(Debug, Args)]
pub struct WatermarkPreviewArgs {
    /// 書き出す PNG のファイル（同じ名前があれば連番を付ける）
    #[arg(long, value_name = "FILE")]
    pub out: PathBuf,
    /// 見本の画像の幅（16〜2000。既定 600）
    #[arg(long, value_parser = clap::value_parser!(u32).range(16..=2000))]
    pub width: Option<u32>,
    /// 見本の画像の高さ（16〜2000。既定 400）
    #[arg(long, value_parser = clap::value_parser!(u32).range(16..=2000))]
    pub height: Option<u32>,
    /// 透かし。
    #[command(flatten)]
    pub watermark: WatermarkArgs,
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
    /// カメラプロファイル（DCP）のファイル（取り込まずに使う。DEV-17。RAW だけ）
    #[arg(long, value_name = "FILE")]
    pub dcp: Option<PathBuf>,
    /// ルック（3D LUT の .cube）のファイル（取り込まずに使う。DEV-17）
    #[arg(long, value_name = "FILE")]
    pub cube: Option<PathBuf>,
    /// ルックの適用量（0〜100。既定 100）
    #[arg(long, value_name = "N")]
    pub look_amount: Option<f32>,
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
    /// 計測の設定に当てるハイライト復元・ノイズ軽減（DEV-06・DEV-13。`develop set` と同じオプション。処理バージョン 2）
    #[command(flatten)]
    pub sensor: SensorArgs,
    /// 計測の設定に当てる白黒・カラーミキサー・カラーグレーディング（DEV-25・DEV-09・DEV-10。`develop set` と同じ
    /// オプション。処理バージョン 2。項目が多いので箱に入れる）
    #[command(flatten)]
    pub color: Box<ColorArgs>,
    /// 計測の設定に当てるテクスチャ・明瞭度・かすみの除去・シャープ・効果（DEV-11・DEV-12・DEV-24。`develop set` と
    /// 同じオプション。処理バージョン 2。箱に入れる）
    #[command(flatten)]
    pub detail: Box<DetailArgs>,
    /// 計測の設定に当てるカメラプロファイル（DCP）のファイル（取り込まずに使う。DEV-17。`render --dcp` と同じ）
    #[arg(long, value_name = "FILE")]
    pub dcp: Option<PathBuf>,
    /// 計測の設定に当てるルック（3D LUT の .cube）のファイル（取り込まずに使う。`render --cube` と同じ）
    #[arg(long, value_name = "FILE")]
    pub cube: Option<PathBuf>,
    /// ルックの適用量（0〜100。既定 100）
    #[arg(long, value_name = "N")]
    pub look_amount: Option<f32>,
    /// 計測の設定に当てるマスク（DEV-18・DEV-19）: 現像設定の `masks` と同じ形の JSON（マスクの配列、または `masks` を
    /// 持つ現像設定。`develop get` の出力をそのまま使える）のファイル（- で標準入力）
    #[arg(long, value_name = "FILE", conflicts_with = "sample_masks")]
    pub masks: Option<String>,
    /// 計測の設定に、決まった見本のマスク（線形・円形・画像を塗りつぶすブラシ・輝度範囲。bench の doc）を当てる
    #[arg(long)]
    pub sample_masks: bool,
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
    /// 露光量・コントラスト・ハイライト・シャドウ・白・黒・ハイライト復元。
    BasicTone,
    /// テクスチャ・明瞭度・かすみの除去。
    Presence,
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
    /// 効果（切り抜き後の周辺光量補正・粒子）。
    Effects,
    /// カメラプロファイル・ルック・ルックの適用量（DEV-17）。
    Profile,
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
        presence: has(GroupArg::Presence),
        tone_curve: has(GroupArg::ToneCurve),
        color: has(GroupArg::Color),
        geometry: has(GroupArg::Geometry),
        lens: has(GroupArg::Lens),
        detail: has(GroupArg::Detail),
        effects: has(GroupArg::Effects),
        profile: has(GroupArg::Profile),
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

/// `metadata` の項目の一覧（`--help` の末尾）。
pub const METADATA_FIELDS_HELP: &str = "項目（FIELD）: title（タイトル）・headline（見出し）・caption（キャプション）・\
creator（作成者）・creator_job_title・creator_address・creator_city・creator_region・creator_postal_code・\
creator_country・creator_phone・creator_email・creator_url（作成者の連絡先）・copyright（著作権表示）・\
copyright_status（copyrighted / public_domain / unknown）・usage_terms（使用条件）・copyright_url・\
sublocation・city・province_state・country・country_code（撮影場所）・credit・source・instructions。\
- は _ と同じに扱います（creator-email など）。";

/// 説明のメタデータの項目の名前を解釈する（`creator_email`・`creator-email`）。
pub fn parse_metadata_field(s: &str) -> Result<MetadataField, String> {
    MetadataField::from_name(&s.trim().replace('-', "_")).ok_or_else(|| {
        format!("説明のメタデータの項目ではありません（{s}。項目の一覧は genzo metadata --help）")
    })
}

/// `項目=値` を解釈する（値は空でもよい。空なら削除）。
pub fn parse_metadata_assignment(s: &str) -> Result<(MetadataField, String), String> {
    let (field, value) = s
        .split_once('=')
        .ok_or_else(|| format!("項目=値 の形で指定します（例: title=清水寺。{s}）"))?;
    Ok((parse_metadata_field(field)?, value.to_owned()))
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

    /// フォルダの指定（FILE-02）: 数字だけなら ID、それ以外はパス。`file rename` は --name か --template。
    #[test]
    fn folder_arguments_and_file_rename_options() {
        assert_eq!(
            parse_folder_arg("12"),
            Ok(FolderArg::Id(genzo_model::FolderId::new(12)))
        );
        assert_eq!(
            parse_folder_arg("./2024"),
            Ok(FolderArg::Path(PathBuf::from("./2024")))
        );
        assert_eq!(
            parse_folder_arg("/写真/京都"),
            Ok(FolderArg::Path(PathBuf::from("/写真/京都")))
        );
        assert!(parse_folder_arg("0").is_err());
        assert!(parse_folder_arg(" ").is_err());
        assert!(parse(&["genzo", "folder", "remove", "3"]).is_ok());
        assert!(
            parse(&["genzo", "folder", "remove"]).is_err(),
            "フォルダは必須"
        );
        assert!(parse(&["genzo", "file", "move", "1", "--to", "3"]).is_ok());
        assert!(
            parse(&["genzo", "file", "move", "1"]).is_err(),
            "--to は必須"
        );
        assert!(parse(&["genzo", "file", "rename", "1"]).is_err());
        assert!(
            parse(&[
                "genzo",
                "file",
                "rename",
                "1",
                "--name",
                "a",
                "--template",
                "{seq}"
            ])
            .is_err()
        );
        assert!(
            parse(&["genzo", "file", "rename", "1", "--text", "x", "--name", "a"]).is_err(),
            "--text は --template と"
        );
        assert!(
            parse(&[
                "genzo",
                "file",
                "rename",
                "1",
                "2",
                "--template",
                "{seq}",
                "--seq-digits",
                "3"
            ])
            .is_ok()
        );
        assert!(
            parse(&[
                "genzo",
                "missing",
                "relink",
                "--in",
                "/x",
                "1",
                "2",
                "--dry-run"
            ])
            .is_ok()
        );
        assert!(
            parse(&["genzo", "missing", "relink", "1"]).is_err(),
            "--in は必須"
        );
        assert!(parse(&["genzo", "missing", "link", "0", "/x"]).is_err());
        let cli = parse(&["genzo", "search", "--missing"]).unwrap();
        let Command::Search(a) = cli.command else {
            panic!("search");
        };
        assert!(a.filter.missing);
    }

    /// コピー取り込み・リネーム・まとめて付けるものの引数（IMP-02・IMP-03・IMP-05）。
    #[test]
    fn copy_import_arguments_are_parsed() {
        let cli = parse(&[
            "genzo",
            "import",
            "/sd",
            "--copy-to",
            "/photos",
            "--folders",
            "date-compact",
            "--rename",
            "{date}-{seq}",
            "--seq-start",
            "3",
            "--keyword",
            "場所 > 京都",
            "--keyword",
            "家族",
            "--meta",
            "creator=岡本",
            "--dry-run",
        ])
        .unwrap();
        let Command::Import(a) = cli.command else {
            panic!("import ではない");
        };
        let o = a.options();
        let copy = o.copy.unwrap();
        assert_eq!(copy.destination, PathBuf::from("/photos"));
        assert_eq!(copy.folder_rule, genzo_api::FolderRule::DateCompact);
        let rename = copy.rename.unwrap();
        assert_eq!(
            (
                rename.template.as_str(),
                rename.sequence_start,
                rename.sequence_digits
            ),
            ("{date}-{seq}", 3, 4)
        );
        assert!(o.dry_run);
        assert_eq!(
            o.metadata.keywords,
            vec![
                vec!["場所".to_owned(), "京都".to_owned()],
                vec!["家族".to_owned()]
            ]
        );
        assert_eq!(
            o.metadata
                .fields
                .get(&MetadataField::Creator)
                .map(String::as_str),
            Some("岡本")
        );
        // コピーしない取り込みでは copy を送らない。--folders・--rename・--dry-run は --copy-to と一緒に。
        let Command::Import(plain) = parse(&["genzo", "import", "/sd"]).unwrap().command else {
            panic!();
        };
        assert_eq!(plain.options().copy, None);
        for bad in [
            vec!["genzo", "import", "/sd", "--rename", "{seq}"],
            vec!["genzo", "import", "/sd", "--dry-run"],
            vec![
                "genzo",
                "import",
                "/sd",
                "--copy-to",
                "/p",
                "--folders",
                "weekly",
            ],
            vec!["genzo", "import", "/sd", "--copy-to", "/p", "--text", "x"],
            vec![
                "genzo",
                "import",
                "/sd",
                "--copy-to",
                "/p",
                "--rename",
                "{seq}",
                "--seq-digits",
                "10",
            ],
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            parse_folder_rule("year_date"),
            Ok(genzo_api::FolderRule::YearDate)
        );
    }

    #[test]
    fn metadata_fields_and_assignments_are_parsed() {
        assert_eq!(parse_metadata_field("title"), Ok(MetadataField::Title));
        assert_eq!(
            parse_metadata_field("creator-email"),
            Ok(MetadataField::CreatorEmail)
        );
        assert!(parse_metadata_field("nope").is_err());
        assert_eq!(
            parse_metadata_assignment("caption=朝=夕"),
            Ok((MetadataField::Caption, "朝=夕".to_owned()))
        );
        assert_eq!(
            parse_metadata_assignment("city="),
            Ok((MetadataField::City, String::new()))
        );
        assert!(parse_metadata_assignment("title").is_err());
        // --help の項目の一覧に、すべての項目がある。
        for f in MetadataField::ALL {
            assert!(METADATA_FIELDS_HELP.contains(f.as_str()), "{f}");
        }
        let cli = parse(&[
            "genzo",
            "metadata",
            "1",
            "2",
            "--set",
            "title=清水寺",
            "--clear",
            "city",
        ])
        .unwrap();
        match cli.command {
            Command::Metadata {
                variants,
                set,
                clear,
            } => {
                assert_eq!(variants.len(), 2);
                assert_eq!(set, vec![(MetadataField::Title, "清水寺".to_owned())]);
                assert_eq!(clear, vec![MetadataField::City]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn before_sources_are_parsed() {
        assert_eq!(parse_before_source("initial"), Ok(BeforeSource::Initial));
        assert_eq!(parse_before_source(" current "), Ok(BeforeSource::Current));
        assert_eq!(
            parse_before_source("history:12"),
            Ok(BeforeSource::History {
                entry_id: HistoryEntryId::new(12)
            })
        );
        assert_eq!(
            parse_before_source("snapshot:3"),
            Ok(BeforeSource::Snapshot {
                snapshot_id: SnapshotId::new(3)
            })
        );
        for bad in ["history:0", "history:x", "snapshot", "yesterday", ""] {
            assert!(parse_before_source(bad).is_err(), "{bad}");
        }
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
        assert_eq!(s.filter.min_rating, Some(3));
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
    fn develop_set_accepts_lens_options() {
        let cli = parse(&[
            "genzo",
            "develop",
            "set",
            "7",
            "--lens-profile",
            "on",
            "--remove-ca",
            "off",
            "--distortion",
            "-20",
            "--vignetting",
            "-35.5",
            "--ca-blue-yellow",
            "-3",
            "--distortion-scale",
            "80",
            "--json",
        ])
        .unwrap();
        assert!(cli.global.json, "最後の --json は出力の指定");
        let Command::Develop(DevelopCommand::Set { settings, lens, .. }) = cli.command else {
            panic!()
        };
        assert_eq!(settings, None);
        assert!(lens.any());
        assert_eq!(lens.lens_profile, Some(SwitchArg::On));
        assert_eq!(lens.remove_ca, Some(SwitchArg::Off));
        let mut l = genzo_model::LensCorrection {
            remove_chromatic_aberration: true,
            ca_red_cyan: 9.0,
            ..Default::default()
        };
        lens.apply(&mut l);
        assert!(l.profile_enabled);
        assert!(!l.remove_chromatic_aberration);
        assert_eq!(l.distortion, -20.0);
        assert_eq!(l.vignetting_amount, -35.5);
        assert_eq!(l.ca_blue_yellow, -3.0);
        assert_eq!(l.distortion_scale, 80.0);
        assert_eq!(l.ca_red_cyan, 9.0, "指定しない項目は今の値のまま");
        assert_eq!(l.vignetting_midpoint, 50.0);
        // 何も指定しなければ any() は false（JSON も無ければ実行のときに使い方の誤り）。
        let cli = parse(&["genzo", "develop", "set", "7"]).unwrap();
        let Command::Develop(DevelopCommand::Set { lens, .. }) = cli.command else {
            panic!()
        };
        assert!(!lens.any());
        assert!(parse(&["genzo", "develop", "set", "7", "--lens-profile", "yes"]).is_err());
        assert!(parse(&["genzo", "develop", "set", "7", "--distortion", "abc"]).is_err());
        // 処理バージョンの更新・元の画像の情報。
        let cli = parse(&["genzo", "develop", "upgrade", "7"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Develop(DevelopCommand::Upgrade { variants }) if variants == [VariantId::new(7)]
        ));
        let cli = parse(&["genzo", "develop", "upgrade", "7", "8"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Develop(DevelopCommand::Upgrade { variants }) if variants.len() == 2
        ));
        assert!(parse(&["genzo", "develop", "upgrade"]).is_err());
        let cli = parse(&["genzo", "develop", "source", "7"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Develop(DevelopCommand::Source { variant }) if variant == VariantId::new(7)
        ));
    }

    /// カメラプロファイル・ルック（DEV-17）: `develop set --profile / --look / --look-amount` と `profile` の
    /// サブコマンド。`standard`・`none` は標準の色・ルックなし。
    #[test]
    fn develop_set_profile_options_and_profile_commands() {
        let cli = parse(&[
            "genzo",
            "develop",
            "set",
            "7",
            "--profile",
            "ILCE-7M4 標準",
            "--look",
            "film",
            "--look-amount",
            "40",
        ])
        .unwrap();
        let Command::Develop(DevelopCommand::Set { profile, .. }) = cli.command else {
            panic!()
        };
        assert!(profile.any());
        let mut p = genzo_model::ProfileSettings::default();
        profile.apply(&mut p);
        assert_eq!(p.camera.as_deref(), Some("ILCE-7M4 標準"));
        assert_eq!(p.look.as_deref(), Some("film"));
        assert_eq!(p.look_amount, 40.0);
        let cli = parse(&[
            "genzo",
            "develop",
            "set",
            "7",
            "--profile",
            "standard",
            "--look",
            "none",
        ])
        .unwrap();
        let Command::Develop(DevelopCommand::Set { profile, .. }) = cli.command else {
            panic!()
        };
        profile.apply(&mut p);
        assert_eq!((p.camera, p.look, p.look_amount), (None, None, 40.0));
        assert!(parse(&["genzo", "develop", "set", "7", "--look-amount", "x"]).is_err());
        let cli = parse(&["genzo", "profile", "import", "a.dcp", "b.cube", "--replace"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Profile(ProfileCommand::Import { files, replace: true }) if files.len() == 2
        ));
        assert!(parse(&["genzo", "profile", "import"]).is_err());
        let cli = parse(&["genzo", "profile", "remove", "look", "film"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Profile(ProfileCommand::Remove { kind: ProfileKindArg::Look, id }) if id == "film"
        ));
        assert!(parse(&["genzo", "profile", "list", "--camera", "x", "--for", "3"]).is_err());
        // `develop copy --groups profile` はプロファイル・ルックだけを写す。
        let g = setting_groups(&[GroupArg::Profile]);
        assert!(g.profile && !g.masks && !g.color && !g.basic_tone);
        assert!(!setting_groups(&[GroupArg::Masks]).profile);
    }

    #[test]
    fn develop_set_color_options() {
        use genzo_model::HueBand;
        let cli = parse(&[
            "genzo",
            "develop",
            "set",
            "7",
            "--black-white",
            "on",
            "--hsl-hue",
            "red=-15",
            "--hsl-luminance",
            "blue=-40",
            "--hsl-luminance",
            "aqua=10.5",
            "--bw-mix",
            "orange=30",
            "--grading-shadows",
            "220,30",
            "--grading-highlights",
            "40,25,-10",
            "--grading-balance",
            "-20",
            "--grading-blending",
            "70",
        ])
        .unwrap();
        let Command::Develop(DevelopCommand::Set { color, .. }) = cli.command else {
            panic!()
        };
        assert!(color.any());
        assert_eq!(color.black_white, Some(SwitchArg::On));
        assert_eq!(color.hsl_hue, [(HueBand::Red, -15.0)]);
        assert_eq!(
            color.hsl_luminance,
            [(HueBand::Blue, -40.0), (HueBand::Aqua, 10.5)]
        );
        assert_eq!(color.bw_mix, [(HueBand::Orange, 30.0)]);
        let sh = color.grading_shadows.unwrap();
        assert_eq!((sh.hue, sh.saturation, sh.luminance), (220.0, 30.0, 0.0));
        let hi = color.grading_highlights.unwrap();
        assert_eq!((hi.hue, hi.saturation, hi.luminance), (40.0, 25.0, -10.0));
        assert_eq!(color.grading_balance, Some(-20.0));
        assert_eq!(color.grading_blending, Some(70.0));
        let mut c = genzo_model::ColorParams::default();
        c.mixer.green.saturation = 5.0;
        color.apply(&mut c);
        assert!(c.black_white.enabled);
        assert_eq!(c.mixer.red.hue, -15.0);
        assert_eq!(c.mixer.aqua.luminance, 10.5);
        assert_eq!(
            c.mixer.green.saturation, 5.0,
            "指定しない項目は今の値のまま"
        );
        assert_eq!(c.black_white.mix.orange, 30.0);
        assert_eq!(c.grading.midtones, genzo_model::GradingWheel::default());
        // 形の誤り。
        for bad in [
            ["--hsl-hue", "cyan=10"],
            ["--hsl-hue", "red"],
            ["--hsl-hue", "red=abc"],
            ["--grading-shadows", "220"],
            ["--grading-shadows", "1,2,3,4"],
            ["--black-white", "yes"],
        ] {
            let mut args = vec!["genzo", "develop", "set", "7"];
            args.extend(bad);
            assert!(parse(&args).is_err(), "{bad:?}");
        }
        let cli = parse(&["genzo", "develop", "set", "7"]).unwrap();
        let Command::Develop(DevelopCommand::Set { color, .. }) = cli.command else {
            panic!()
        };
        assert!(!color.any());
    }

    #[test]
    fn develop_set_detail_options() {
        let cli = parse(&[
            "genzo",
            "develop",
            "set",
            "7",
            "--texture",
            "-30",
            "--clarity",
            "40",
            "--dehaze",
            "-15.5",
            "--sharpen-amount",
            "80",
            "--sharpen-radius",
            "1.5",
            "--vignette-amount",
            "-25",
            "--vignette-roundness",
            "-60",
            "--grain-amount",
            "20",
            "--grain-roughness",
            "0",
        ])
        .unwrap();
        let Command::Develop(DevelopCommand::Set { detail, .. }) = cli.command else {
            panic!()
        };
        assert!(detail.any());
        let mut s = genzo_model::DevelopSettings::default();
        s.effects.vignette.feather = 70.0;
        detail.apply(&mut s);
        assert_eq!(s.presence.texture, -30.0);
        assert_eq!(s.presence.clarity, 40.0);
        assert_eq!(s.presence.dehaze, -15.5);
        assert_eq!(s.detail.sharpening.amount, 80.0);
        assert_eq!(s.detail.sharpening.radius, 1.5);
        assert_eq!(
            s.detail.sharpening.detail, 25.0,
            "指定しない項目は今の値のまま"
        );
        assert_eq!(s.effects.vignette.amount, -25.0);
        assert_eq!(s.effects.vignette.roundness, -60.0);
        assert_eq!(s.effects.vignette.feather, 70.0);
        assert_eq!(s.effects.grain.amount, 20.0);
        assert_eq!(s.effects.grain.roughness, 0.0);
        assert!(parse(&["genzo", "develop", "set", "7", "--clarity", "abc"]).is_err());
    }

    /// `bench` にも `develop set` と同じ色・外観・シャープ・効果のオプションがある（計測の設定に当てる）。
    #[test]
    fn bench_accepts_color_and_detail_options() {
        let cli = parse(&[
            "genzo",
            "bench",
            "preview",
            "--synthetic",
            "640x426",
            "--nr-luminance",
            "20",
            "--black-white",
            "on",
            "--bw-mix",
            "blue=-30",
            "--grading-shadows",
            "220,30",
            "--clarity",
            "-30",
            "--dehaze",
            "25",
            "--sharpen-amount",
            "60",
            "--grain-amount",
            "25",
        ])
        .unwrap();
        let Command::Bench(a) = cli.command else {
            panic!()
        };
        assert_eq!(a.target, BenchTarget::Preview);
        assert!(a.sensor.any() && a.color.any() && a.detail.any());
        assert_eq!(a.detail.clarity, Some(-30.0));
        assert_eq!(a.detail.sharpen_amount, Some(60.0));
        assert_eq!(a.color.bw_mix, vec![(genzo_model::HueBand::Blue, -30.0)]);
        // 指定しなければ何も当てない。
        let cli = parse(&["genzo", "bench", "export"]).unwrap();
        let Command::Bench(a) = cli.command else {
            panic!()
        };
        assert!(!a.sensor.any() && !a.color.any() && !a.detail.any());
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
        assert_eq!(e.opts.on_conflict, Some(ConflictArg::Overwrite));
        assert!(parse(&["genzo", "export", "1", "--out", "o", "--quality", "0"]).is_err());
        // --out は --preset に書き出し先があれば省略できる（なければ実行するときに使い方の誤り。export.rs）。
        assert!(parse(&["genzo", "export", "1"]).is_ok());
        let cli = parse(&[
            "genzo",
            "export",
            "1",
            "--preset",
            "Web",
            "--rename",
            "{date}-{seq}",
            "--text",
            "旅",
            "--seq-digits",
            "3",
        ])
        .unwrap();
        let Command::Export(e) = cli.command else {
            panic!()
        };
        assert_eq!(e.preset.as_deref(), Some("Web"));
        assert_eq!(e.out, None);
        assert_eq!(e.opts.on_conflict, None);
        assert_eq!(e.naming.rename.as_deref(), Some("{date}-{seq}"));
        assert_eq!(e.naming.text.as_deref(), Some("旅"));
        assert_eq!(e.naming.seq_digits, Some(3));
        assert!(
            parse(&["genzo", "export", "1", "--keep-names", "--rename", "{seq}"]).is_err(),
            "--keep-names と --rename は一緒に使えない"
        );
        assert!(parse(&["genzo", "export", "1", "--seq-digits", "10"]).is_err());
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

    /// スナップショット（DEV-28）とプリセット（DEV-29・EXP-02）のサブコマンド。
    #[test]
    fn snapshot_and_preset_commands() {
        let cli = parse(&[
            "genzo",
            "develop",
            "snapshot",
            "create",
            "3",
            "途中",
            "--from-history",
            "12",
        ])
        .unwrap();
        let Command::Develop(DevelopCommand::Snapshot(SnapshotCommand::Create {
            variant,
            name,
            from_history,
        })) = cli.command
        else {
            panic!()
        };
        assert_eq!(variant, VariantId::new(3));
        assert_eq!(name, "途中");
        assert_eq!(from_history, Some(HistoryEntryId::new(12)));
        assert!(parse(&["genzo", "develop", "snapshot", "apply", "0"]).is_err());
        assert!(parse(&["genzo", "develop", "snapshot", "rename", "2", "新しい名前"]).is_ok());

        let cli = parse(&[
            "genzo",
            "--presets-dir",
            "p",
            "preset",
            "create-develop",
            "明るく",
            "--from",
            "1",
            "--groups",
            "basic-tone,presence",
            "--folder",
            "基本",
        ])
        .unwrap();
        assert_eq!(cli.global.presets_dir, Some(PathBuf::from("p")));
        let Command::Preset(PresetCommand::CreateDevelop {
            name,
            from,
            folder,
            groups,
        }) = cli.command
        else {
            panic!()
        };
        assert_eq!(
            (name.as_str(), from, folder.as_str()),
            ("明るく", VariantId::new(1), "基本")
        );
        assert_eq!(groups, vec![GroupArg::BasicTone, GroupArg::Presence]);
        let cli = parse(&[
            "genzo",
            "preset",
            "apply",
            "明るく",
            "--to",
            "1",
            "2",
            "--upgrade-process-version",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Preset(PresetCommand::Apply { ref to, upgrade_process_version: true, .. }) if to.len() == 2
        ));
        assert!(
            parse(&["genzo", "preset", "apply", "明るく"]).is_err(),
            "--to は必須"
        );
        let cli = parse(&[
            "genzo",
            "preset",
            "create-export",
            "Web",
            "--format",
            "png8",
            "--long-edge",
            "2048",
            "--dest",
            "/out",
            "--rename",
            "{date}-{seq}",
        ])
        .unwrap();
        let Command::Preset(PresetCommand::CreateExport(a)) = cli.command else {
            panic!()
        };
        assert_eq!(a.name, "Web");
        assert_eq!(a.dest, Some(PathBuf::from("/out")));
        assert_eq!(a.opts.format, Some(FormatArg::Png8));
        assert_eq!(a.naming.rename.as_deref(), Some("{date}-{seq}"));
        assert!(parse(&["genzo", "preset", "list", "--kind", "export"]).is_ok());
        assert!(parse(&["genzo", "preset", "list", "--kind", "other"]).is_err());
        assert!(parse(&["genzo", "preset", "export-files", "a", "b", "--out", "d"]).is_ok());
        assert!(parse(&["genzo", "preset", "rename-folder", "develop", "旧", ""]).is_ok());
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
        assert!(!g.presence && !g.effects);
        let g = setting_groups(&[GroupArg::Presence, GroupArg::Effects]);
        assert!(g.presence && g.effects && !g.basic_tone && !g.detail);
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
        let Command::Develop(DevelopCommand::Copy {
            to,
            groups,
            upgrade_process_version,
            ..
        }) = cli.command
        else {
            panic!()
        };
        assert_eq!(to.len(), 2);
        assert_eq!(groups, vec![GroupArg::BasicTone, GroupArg::WhiteBalance]);
        assert!(!upgrade_process_version, "既定はコピー先の版のまま");
        let cli = parse(&[
            "genzo",
            "develop",
            "copy",
            "1",
            "--to",
            "2",
            "--upgrade-process-version",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Develop(DevelopCommand::Copy {
                upgrade_process_version: true,
                ..
            })
        ));
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
