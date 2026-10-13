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
       失敗、取り込みで登録できなかったファイル、catalog check で見つかった問題、bench での悪化）
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
    /// ログ・クラッシュレポートの場所と中身の表示・消去（カタログは使わない。SYS-07）
    #[command(subcommand)]
    Log(LogCommand),
    /// ファイルの内容のハッシュ（全体の BLAKE3。ORG-03）: 計算・状況・内容が同じファイルの組・ファイルの
    /// 識別（場所・名前によらず、同じ内容の登録済みの写真を探す）
    #[command(subcommand)]
    Hash(HashCommand),
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
