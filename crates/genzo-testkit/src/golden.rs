//! 基準画像（golden）による回帰テスト（05 の M0 タスク 11、02 の MAINT-03・IQ-08）。
//!
//! # 仕組み
//!
//! 1 つの基準は、同じディレクトリ（例: `tests/golden/`）の 2 つのファイルからなる。
//!
//! | ファイル | 内容 |
//! |---|---|
//! | `<名前>.gzf` | 浮動小数点の画像（ロスレス。下の「画像の形式」） |
//! | `<名前>.json` | メタデータ（[`GoldenMeta`]）: 入力の ID とハッシュ、使った現像設定、process_version、許容差、画像のハッシュ |
//!
//! [`GoldenStore::check`] は、テストで作った画像を基準と比べる。
//!
//! - 通常（[`GoldenMode::Compare`]）: メタデータ（入力・設定・処理バージョン・許容差）が一致することを
//!   確かめてから、許容差（[`Tolerance`]）で比べる。許容差を超えたら、比べた画像・差分画像・
//!   報告を `target/golden-failures/<名前>/` に書き出して失敗する（[`GoldenError::Failed`]）。
//!   基準がなければ失敗する（CI で基準のないテストが黙って成功しないように）。
//! - 環境変数 `GENZO_UPDATE_GOLDEN=1`（[`GoldenMode::Update`]）: 基準を作り直す。前の基準との差を
//!   標準エラーに表示する。更新した基準は、git の差分を確認してからコミットする。
//!   ただし環境変数 `CI` が設定されている（CI で実行している）ときは作り直さずに比べる
//!   （CI で基準を作り直すとすべて合格になり、回帰を検出できないため。02 の MAINT-03）。
//!
//! 基準画像は公開リポジトリに入るため、**合成画像か、公開してよい被写体（人物・個人情報・GPS を
//! 含まないもの）だけ** を使う（05 の 3 章「注意：サンプルデータの扱い」）。
//!
//! # 画像の形式（`.gzf`）
//!
//! | 位置 | 大きさ | 内容 |
//! |---|---|---|
//! | 0 | 8 | 識別子 `GENZOF32`（ASCII） |
//! | 8 | 4 | 形式のバージョン（u32、リトルエンディアン。現在 1） |
//! | 12 | 4 | 幅（u32 LE） |
//! | 16 | 4 | 高さ（u32 LE） |
//! | 20 | 4 | チャンネル数（u32 LE。1・3・4） |
//! | 24 | 幅 × 高さ × チャンネル数 × 4 | 値（f32 LE。行優先、画素ごとにチャンネルを並べる） |
//!
//! f32 のビット列をそのまま書くので、読み戻した値は書いた値と完全に一致する。

use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diff::{DiffError, MetricReport, Tolerance, delta_e2000_map, diff_8bit_map};
use crate::image::{FloatImage, ImageError, NonFiniteSample};
use crate::preview::{encode_diff_png, encode_png16};
use crate::record::{
    InputRef, InvalidName, SettingsRecord, sha256_hex, validate_name, write_atomic,
};

/// 画像の形式の識別子。
pub const FLOAT_IMAGE_MAGIC: [u8; 8] = *b"GENZOF32";
/// 画像の形式のバージョン。
pub const FLOAT_IMAGE_VERSION: u32 = 1;
/// 画像の形式のヘッダの大きさ（バイト）。
pub const FLOAT_IMAGE_HEADER_LEN: usize = 24;
/// 基準画像のファイルの拡張子。
pub const GOLDEN_IMAGE_EXTENSION: &str = "gzf";
/// メタデータの形式のバージョン。
pub const GOLDEN_META_VERSION: u32 = 1;
/// 基準を作り直す環境変数（値が `1` のとき）。
pub const UPDATE_GOLDEN_ENV: &str = "GENZO_UPDATE_GOLDEN";
/// 失敗したときの出力先を指定する環境変数（既定は `target/golden-failures`）。
pub const GOLDEN_OUT_DIR_ENV: &str = "GENZO_GOLDEN_OUT_DIR";
/// CI で実行していることを示す環境変数（GitHub Actions などが `true` にする）。
pub const CI_ENV: &str = "CI";

/// 呼び出した crate の `tests/golden` ディレクトリ（`CARGO_MANIFEST_DIR` は呼び出し側で展開される）。
///
/// ```
/// let dir = genzo_testkit::golden_dir!();
/// assert!(dir.ends_with("tests/golden"));
/// ```
#[macro_export]
macro_rules! golden_dir {
    () => {
        ::std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("golden")
    };
}

/// 画像を `.gzf` の形式のバイト列にする。
pub fn encode_float_image(image: &FloatImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(FLOAT_IMAGE_HEADER_LEN + image.data().len() * 4);
    out.extend_from_slice(&FLOAT_IMAGE_MAGIC);
    out.extend_from_slice(&FLOAT_IMAGE_VERSION.to_le_bytes());
    out.extend_from_slice(&image.width().to_le_bytes());
    out.extend_from_slice(&image.height().to_le_bytes());
    out.extend_from_slice(&image.channels().to_le_bytes());
    for v in image.data() {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// 画像の形式の読み込みのエラー。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FormatError {
    /// ヘッダより短い。
    #[error("ヘッダより短いデータです（{0} バイト）")]
    TooShort(usize),
    /// 識別子が違う。
    #[error("識別子が GENZOF32 ではありません")]
    BadMagic,
    /// 対応していないバージョン。
    #[error("対応していない形式のバージョンです（{0}）")]
    UnsupportedVersion(u32),
    /// 寸法・チャンネル数が不正。
    #[error(transparent)]
    Image(#[from] ImageError),
    /// 長さが寸法と合わない。
    #[error("データの長さ {actual} バイトがヘッダから求めた {expected} バイトと一致しません")]
    LengthMismatch {
        /// ヘッダから求めた長さ。
        expected: u64,
        /// 実際の長さ。
        actual: usize,
    },
}

/// `.gzf` の形式のバイト列から画像を読む。
pub fn decode_float_image(bytes: &[u8]) -> Result<FloatImage, FormatError> {
    if bytes.len() < FLOAT_IMAGE_HEADER_LEN {
        return Err(FormatError::TooShort(bytes.len()));
    }
    if bytes[..8] != FLOAT_IMAGE_MAGIC {
        return Err(FormatError::BadMagic);
    }
    let u32_at =
        |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let version = u32_at(8);
    if version != FLOAT_IMAGE_VERSION {
        return Err(FormatError::UnsupportedVersion(version));
    }
    let (width, height, channels) = (u32_at(12), u32_at(16), u32_at(20));
    // 寸法とチャンネル数を先に検証する（巨大な寸法でも確保しない）。
    let count = FloatImage::value_count(width, height, channels)?;
    let expected = FLOAT_IMAGE_HEADER_LEN as u64 + count as u64 * 4;
    if bytes.len() as u64 != expected {
        return Err(FormatError::LengthMismatch {
            expected,
            actual: bytes.len(),
        });
    }
    let data = bytes[FLOAT_IMAGE_HEADER_LEN..]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Ok(FloatImage::new(width, height, channels, data)?)
}

/// 基準の内容（テストのコードで指定する部分）。
///
/// 比較のときは、保存されているものと **説明（description）以外がすべて一致** することを確かめる。
/// 入力・設定・処理バージョン・許容差のどれかを変えたら、基準を作り直す（`GENZO_UPDATE_GOLDEN=1`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoldenSpec {
    /// 名前（英数字・`.`・`_`・`-`。ファイル名になる）。
    pub name: String,
    /// 説明（比較しない）。
    #[serde(default)]
    pub description: String,
    /// 入力の ID とハッシュ（05 の 1.8 節）。
    #[serde(default)]
    pub inputs: Vec<InputRef>,
    /// 使った現像設定（ない場合はステージ単体のテストなど）。
    #[serde(default)]
    pub develop: Option<SettingsRecord>,
    /// 処理バージョン（04 の 2.5 節。IQ-08）。
    pub process_version: u32,
    /// 許容差。
    pub tolerance: Tolerance,
}

impl GoldenSpec {
    /// 名前と許容差から作る。処理バージョンは現在の値（[`genzo_model::CURRENT_PROCESS_VERSION`]）。
    pub fn new(name: impl Into<String>, tolerance: Tolerance) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            inputs: Vec::new(),
            develop: None,
            process_version: genzo_model::CURRENT_PROCESS_VERSION,
            tolerance,
        }
    }

    /// 説明を付ける。
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// 入力を追加する。
    pub fn with_input(mut self, input: InputRef) -> Self {
        self.inputs.push(input);
        self
    }

    /// 使った現像設定を記録する（処理バージョンも設定の値にする）。
    pub fn with_develop_settings(mut self, settings: &genzo_model::DevelopSettings) -> Self {
        let record = SettingsRecord::from_settings(settings);
        self.process_version = record.process_version;
        self.develop = Some(record);
        self
    }

    /// 処理バージョンを指定する。
    pub fn with_process_version(mut self, process_version: u32) -> Self {
        self.process_version = process_version;
        self
    }

    /// 保存されている内容との違い（最初に見つかった項目）。説明は比べない。
    fn first_difference(&self, stored: &GoldenSpec) -> Option<(&'static str, String, String)> {
        if self.inputs != stored.inputs {
            return Some(("inputs", json(&stored.inputs), json(&self.inputs)));
        }
        if self.develop != stored.develop {
            return Some(("develop", json(&stored.develop), json(&self.develop)));
        }
        if self.process_version != stored.process_version {
            return Some((
                "process_version",
                stored.process_version.to_string(),
                self.process_version.to_string(),
            ));
        }
        if self.tolerance != stored.tolerance {
            return Some(("tolerance", json(&stored.tolerance), json(&self.tolerance)));
        }
        None
    }
}

/// JSON の文字列にする（違いの表示用）。
fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| format!("<JSON にできません: {e}>"))
}

/// 保存した画像の情報。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldenImageInfo {
    /// ファイル名（`<名前>.gzf`）。
    pub file: String,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
    /// チャンネル数。
    pub channels: u32,
    /// ファイルの SHA-256（壊れていないかの確認用）。
    pub sha256: String,
}

/// 基準のメタデータ（`<名前>.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoldenMeta {
    /// メタデータの形式のバージョン（[`GOLDEN_META_VERSION`]）。
    pub format_version: u32,
    /// 基準の内容。
    pub spec: GoldenSpec,
    /// 画像の情報。
    pub image: GoldenImageInfo,
}

/// 比較か更新か。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoldenMode {
    /// 基準と比べる（既定）。
    Compare,
    /// 基準を作り直す。
    Update,
}

impl GoldenMode {
    /// 環境変数 `GENZO_UPDATE_GOLDEN` が `1` なら [`Update`](Self::Update)、それ以外は
    /// [`Compare`](Self::Compare)。
    ///
    /// ただし環境変数 `CI` が設定されている（空・`false`・`0` 以外）ときは、`GENZO_UPDATE_GOLDEN=1`
    /// でも [`Compare`](Self::Compare) にして、標準エラーに警告を出す（CI で基準を作り直すと、
    /// すべての比較が合格になって回帰を検出できないため。02 の MAINT-03）。CI の環境で基準を
    /// 作り直す必要があるときは、`CI` を外して実行する。
    pub fn from_env() -> Self {
        let update = env::var(UPDATE_GOLDEN_ENV).ok();
        let ci = env::var(CI_ENV).ok();
        let mode = Self::from_values(update.as_deref(), ci.as_deref());
        if mode == GoldenMode::Compare
            && Self::from_values(update.as_deref(), None) == GoldenMode::Update
        {
            eprintln!(
                "警告: CI（環境変数 {CI_ENV}）では {UPDATE_GOLDEN_ENV}=1 を無視し、基準画像と比べます"
            );
        }
        mode
    }

    /// 環境変数の値からモードを決める（[`from_env`](Self::from_env) の判定）。
    fn from_values(update: Option<&str>, ci: Option<&str>) -> Self {
        let in_ci = ci.is_some_and(|v| {
            let v = v.trim();
            !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
        });
        match update {
            Some(v) if v.trim() == "1" && !in_ci => GoldenMode::Update,
            _ => GoldenMode::Compare,
        }
    }
}

/// 失敗したときの出力先の既定値。
///
/// 1. 環境変数 `GENZO_GOLDEN_OUT_DIR`
/// 2. 実行中のテストのバイナリの上位にある Cargo の出力ディレクトリ（`CACHEDIR.TAG` があるもの。
///    通常は workspace の `target/`）の `golden-failures`
/// 3. 一時ディレクトリの `genzo-golden-failures`
pub fn default_failure_dir() -> PathBuf {
    if let Some(dir) = env::var_os(GOLDEN_OUT_DIR_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = env::current_exe() {
        for dir in exe.ancestors().skip(1) {
            if dir.join("CACHEDIR.TAG").is_file() {
                return dir.join("golden-failures");
            }
        }
    }
    env::temp_dir().join("genzo-golden-failures")
}

/// 失敗したときに書き出したファイル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureArtifacts {
    /// 書き出したディレクトリ。
    pub dir: PathBuf,
    /// 比べた画像（`.gzf`。問題がなければ基準として使える）。
    pub actual: PathBuf,
    /// 比べた画像の確認用の PNG（16bit。0〜1 に切り詰め）。
    pub actual_png: Option<PathBuf>,
    /// 基準の画像の確認用の PNG。
    pub expected_png: Option<PathBuf>,
    /// 差分画像（PNG）。許容差以内は灰色、超えた画素は赤。
    pub diff_png: Option<PathBuf>,
    /// 報告（テキスト）。
    pub report: PathBuf,
}

/// 比較に失敗した理由。
#[derive(Debug, Clone, PartialEq)]
pub enum FailureReason {
    /// 許容差を超えた。
    ToleranceExceeded(MetricReport),
    /// 比べられない（形が違う・NaN や無限大を含む）。
    Diff(DiffError),
}

/// 比較の失敗の詳細。
#[derive(Debug, Clone, PartialEq)]
pub struct GoldenFailure {
    /// 基準の名前。
    pub name: String,
    /// 理由。
    pub reason: FailureReason,
    /// 書き出したファイル（書き出しに失敗した場合は `None`）。
    pub artifacts: Option<FailureArtifacts>,
    /// 書き出しに失敗した場合のエラー。
    pub artifact_error: Option<String>,
}

impl fmt::Display for GoldenFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "基準画像 {} との比較に失敗しました: ", self.name)?;
        match &self.reason {
            FailureReason::ToleranceExceeded(r) => write!(f, "許容差を超えました。{r}")?,
            FailureReason::Diff(e) => write!(f, "{e}")?,
        }
        if let Some(a) = &self.artifacts {
            write!(f, "\n比べた画像・差分画像・報告: {}", a.dir.display())?;
        }
        if let Some(e) = &self.artifact_error {
            write!(f, "\n（差分画像などを書き出せませんでした: {e}）")?;
        }
        write!(
            f,
            "\n意図した変更なら {UPDATE_GOLDEN_ENV}=1 で基準を作り直し、差分を確認してからコミットしてください。"
        )
    }
}

/// 基準画像の扱いのエラー。
#[derive(Debug, thiserror::Error)]
pub enum GoldenError {
    /// 名前が不正。
    #[error(transparent)]
    InvalidName(#[from] InvalidName),
    /// 許容差が不正。
    #[error("基準の許容差が不正です: {0}")]
    InvalidTolerance(DiffError),
    /// 基準がない。
    #[error(
        "基準画像 {name} がありません（{}）。作成するには {UPDATE_GOLDEN_ENV}=1 を付けてテストを実行してください",
        meta_path.display()
    )]
    Missing {
        /// 名前。
        name: String,
        /// メタデータのパス。
        meta_path: PathBuf,
    },
    /// 保存されているメタデータと、テストの指定が違う。
    #[error(
        "基準画像 {name} の {field} がテストの指定と違います（保存: {stored}、指定: {current}）。意図した変更なら {UPDATE_GOLDEN_ENV}=1 で作り直してください"
    )]
    SpecMismatch {
        /// 名前。
        name: String,
        /// 項目。
        field: &'static str,
        /// 保存されている値。
        stored: String,
        /// テストで指定した値。
        current: String,
    },
    /// 基準のファイルが壊れている・形式が違う。
    #[error("基準画像のファイル {} が不正です: {reason}", path.display())]
    Corrupt {
        /// ファイルのパス。
        path: PathBuf,
        /// 理由。
        reason: String,
    },
    /// 基準として保存する画像に NaN・無限大がある。
    #[error("基準として保存する画像に有限でない値があります: {0}")]
    NonFiniteImage(NonFiniteSample),
    /// 比較に失敗した。
    #[error("{0}")]
    Failed(Box<GoldenFailure>),
    /// 入出力のエラー。
    #[error("{}: {source}", path.display())]
    Io {
        /// パス。
        path: PathBuf,
        /// 元のエラー。
        source: io::Error,
    },
}

impl GoldenError {
    fn io(path: &Path, source: io::Error) -> Self {
        GoldenError::Io {
            path: path.to_owned(),
            source,
        }
    }

    fn corrupt(path: &Path, reason: impl fmt::Display) -> Self {
        GoldenError::Corrupt {
            path: path.to_owned(),
            reason: reason.to_string(),
        }
    }
}

/// 比較・更新の結果。
#[derive(Debug, Clone, PartialEq)]
pub enum GoldenOutcome {
    /// 許容差の範囲内だった。
    Passed(MetricReport),
    /// 基準を作り直した。
    Updated {
        /// メタデータのパス。
        meta_path: PathBuf,
        /// 画像のパス。
        image_path: PathBuf,
        /// 前の基準との比較（前の基準がなかった・比べられなかった場合は `None`）。
        previous: Option<MetricReport>,
    },
}

/// 基準画像を置くディレクトリと、比較の方法。
#[derive(Debug, Clone)]
pub struct GoldenStore {
    dir: PathBuf,
    failure_dir: PathBuf,
    mode: GoldenMode,
}

impl GoldenStore {
    /// ディレクトリを指定して作る。モードは環境変数（[`GoldenMode::from_env`]）、失敗したときの
    /// 出力先は [`default_failure_dir`]。
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            failure_dir: default_failure_dir(),
            mode: GoldenMode::from_env(),
        }
    }

    /// モードを指定する（テスト用。通常は環境変数に従う）。
    pub fn with_mode(mut self, mode: GoldenMode) -> Self {
        self.mode = mode;
        self
    }

    /// 失敗したときの出力先を指定する。
    pub fn with_failure_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.failure_dir = dir.into();
        self
    }

    /// 基準のディレクトリ。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 失敗したときの出力先。
    pub fn failure_dir(&self) -> &Path {
        &self.failure_dir
    }

    /// モード。
    pub fn mode(&self) -> GoldenMode {
        self.mode
    }

    /// メタデータのパス（`<ディレクトリ>/<名前>.json`）。
    pub fn meta_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    /// 画像のパス（`<ディレクトリ>/<名前>.gzf`）。
    pub fn image_path(&self, name: &str) -> PathBuf {
        self.dir.join(image_file_name(name))
    }

    /// 基準を読む（画像のハッシュと寸法をメタデータと照合する）。
    pub fn load(&self, name: &str) -> Result<(GoldenMeta, FloatImage), GoldenError> {
        validate_name(name)?;
        let meta_path = self.meta_path(name);
        let meta_text = match fs::read_to_string(&meta_path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(GoldenError::Missing {
                    name: name.to_owned(),
                    meta_path,
                });
            }
            Err(e) => return Err(GoldenError::io(&meta_path, e)),
        };
        let meta: GoldenMeta = serde_json::from_str(&meta_text)
            .map_err(|e| GoldenError::corrupt(&meta_path, format!("JSON を解析できません: {e}")))?;
        if meta.format_version != GOLDEN_META_VERSION {
            return Err(GoldenError::corrupt(
                &meta_path,
                format!(
                    "対応していない形式のバージョンです（{}。対応しているのは {GOLDEN_META_VERSION}）",
                    meta.format_version
                ),
            ));
        }
        if meta.spec.name != name || meta.image.file != image_file_name(name) {
            return Err(GoldenError::corrupt(
                &meta_path,
                format!(
                    "名前（{:?}）か画像のファイル名（{:?}）がファイル名と一致しません",
                    meta.spec.name, meta.image.file
                ),
            ));
        }
        let image_path = self.image_path(name);
        let bytes = match fs::read(&image_path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(GoldenError::corrupt(
                    &image_path,
                    "メタデータはあるが画像のファイルがありません",
                ));
            }
            Err(e) => return Err(GoldenError::io(&image_path, e)),
        };
        let hash = sha256_hex(&bytes);
        if hash != meta.image.sha256 {
            return Err(GoldenError::corrupt(
                &image_path,
                format!(
                    "SHA-256 がメタデータと一致しません（メタデータ {}、ファイル {hash}）",
                    meta.image.sha256
                ),
            ));
        }
        let image = decode_float_image(&bytes).map_err(|e| GoldenError::corrupt(&image_path, e))?;
        if (image.width(), image.height(), image.channels())
            != (meta.image.width, meta.image.height, meta.image.channels)
        {
            return Err(GoldenError::corrupt(
                &image_path,
                "寸法かチャンネル数がメタデータと一致しません",
            ));
        }
        Ok((meta, image))
    }

    /// 基準を保存する（既存の基準は置き換える）。画像に NaN・無限大があれば保存しない。
    pub fn save(&self, spec: &GoldenSpec, image: &FloatImage) -> Result<GoldenMeta, GoldenError> {
        validate_name(&spec.name)?;
        spec.tolerance
            .validate()
            .map_err(GoldenError::InvalidTolerance)?;
        if let Some(s) = image.find_non_finite() {
            return Err(GoldenError::NonFiniteImage(s));
        }
        let bytes = encode_float_image(image);
        let meta = GoldenMeta {
            format_version: GOLDEN_META_VERSION,
            spec: spec.clone(),
            image: GoldenImageInfo {
                file: image_file_name(&spec.name),
                width: image.width(),
                height: image.height(),
                channels: image.channels(),
                sha256: sha256_hex(&bytes),
            },
        };
        let image_path = self.image_path(&spec.name);
        write_atomic(&image_path, &bytes).map_err(|e| GoldenError::io(&image_path, e))?;
        let meta_path = self.meta_path(&spec.name);
        let mut json = serde_json::to_string_pretty(&meta)
            .map_err(|e| GoldenError::corrupt(&meta_path, format!("JSON にできません: {e}")))?;
        json.push('\n');
        write_atomic(&meta_path, json.as_bytes()).map_err(|e| GoldenError::io(&meta_path, e))?;
        Ok(meta)
    }

    /// テストで作った画像を基準と比べる（モードが [`GoldenMode::Update`] なら基準を作り直す）。
    pub fn check(
        &self,
        spec: &GoldenSpec,
        actual: &FloatImage,
    ) -> Result<GoldenOutcome, GoldenError> {
        validate_name(&spec.name)?;
        spec.tolerance
            .validate()
            .map_err(GoldenError::InvalidTolerance)?;
        match self.mode {
            GoldenMode::Update => self.update(spec, actual),
            GoldenMode::Compare => self.compare(spec, actual),
        }
    }

    /// [`check`](Self::check) し、失敗したらパニックする（テストから呼ぶ）。
    #[track_caller]
    pub fn assert_matches(&self, spec: &GoldenSpec, actual: &FloatImage) -> GoldenOutcome {
        match self.check(spec, actual) {
            Ok(outcome) => outcome,
            Err(e) => panic!("{e}"),
        }
    }

    fn update(&self, spec: &GoldenSpec, actual: &FloatImage) -> Result<GoldenOutcome, GoldenError> {
        // 前の基準との差を表示する（読めない・比べられない場合は表示しない）。
        let previous = self
            .load(&spec.name)
            .ok()
            .and_then(|(_, expected)| spec.tolerance.evaluate(&expected, actual).ok());
        self.save(spec, actual)?;
        let meta_path = self.meta_path(&spec.name);
        let image_path = self.image_path(&spec.name);
        match &previous {
            Some(r) => eprintln!(
                "基準画像 {} を更新しました（{}）。前の基準との差: {r}",
                spec.name,
                meta_path.display()
            ),
            None => eprintln!(
                "基準画像 {} を作成しました（{}）",
                spec.name,
                meta_path.display()
            ),
        }
        Ok(GoldenOutcome::Updated {
            meta_path,
            image_path,
            previous,
        })
    }

    fn compare(
        &self,
        spec: &GoldenSpec,
        actual: &FloatImage,
    ) -> Result<GoldenOutcome, GoldenError> {
        let (meta, expected) = self.load(&spec.name)?;
        if let Some((field, stored, current)) = spec.first_difference(&meta.spec) {
            return Err(GoldenError::SpecMismatch {
                name: spec.name.clone(),
                field,
                stored,
                current,
            });
        }
        let reason = match spec.tolerance.evaluate(&expected, actual) {
            Ok(report) if report.passed() => return Ok(GoldenOutcome::Passed(report)),
            Ok(report) => FailureReason::ToleranceExceeded(report),
            Err(e @ DiffError::InvalidArgument(_)) => return Err(GoldenError::InvalidTolerance(e)),
            Err(e) => FailureReason::Diff(e),
        };
        let mut failure = GoldenFailure {
            name: spec.name.clone(),
            reason,
            artifacts: None,
            artifact_error: None,
        };
        match self.write_artifacts(spec, &expected, actual, &failure) {
            Ok(a) => failure.artifacts = Some(a),
            Err(e) => failure.artifact_error = Some(e.to_string()),
        }
        Err(GoldenError::Failed(Box::new(failure)))
    }

    /// 失敗したときのファイルを書き出す。
    fn write_artifacts(
        &self,
        spec: &GoldenSpec,
        expected: &FloatImage,
        actual: &FloatImage,
        failure: &GoldenFailure,
    ) -> io::Result<FailureArtifacts> {
        let dir = self.failure_dir.join(&spec.name);
        fs::create_dir_all(&dir)?;
        let to_io = |e: image::ImageError| io::Error::other(e.to_string());

        let actual_path = dir.join(image_file_name("actual"));
        write_atomic(&actual_path, &encode_float_image(actual))?;
        let actual_png = dir.join("actual.png");
        write_atomic(&actual_png, &encode_png16(actual).map_err(to_io)?)?;
        let expected_png = dir.join("expected.png");
        write_atomic(&expected_png, &encode_png16(expected).map_err(to_io)?)?;

        // 差分画像は、形が同じで値が有限のときだけ作れる。
        let map = match &spec.tolerance {
            Tolerance::Diff8Bit(_) => diff_8bit_map(expected, actual).ok(),
            Tolerance::DeltaE2000(t) => delta_e2000_map(expected, actual, t.conversion).ok(),
        };
        // 差分画像を作れない場合は、前回の失敗の diff.png を消す（今回の差分画像と取り違えない
        // ように）。消すのは失敗の出力先の中の、この関数が作るファイルだけ。
        let diff_path = dir.join("diff.png");
        if map.is_none() {
            match fs::remove_file(&diff_path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        let diff_png = match map {
            Some(map) => {
                let p = diff_path;
                let png = encode_diff_png(
                    &map,
                    expected.width(),
                    expected.height(),
                    spec.tolerance.per_pixel_limit(),
                )
                .map_err(to_io)?;
                write_atomic(&p, &png)?;
                Some(p)
            }
            None => None,
        };

        let report = dir.join("report.txt");
        let spec_json = serde_json::to_string_pretty(spec).map_err(io::Error::other)?;
        let text = format!("{failure}\n\n指定:\n{spec_json}\n");
        write_atomic(&report, text.as_bytes())?;
        Ok(FailureArtifacts {
            dir,
            actual: actual_path,
            actual_png: Some(actual_png),
            expected_png: Some(expected_png),
            diff_png,
            report,
        })
    }
}

fn image_file_name(name: &str) -> String {
    format!("{name}.{GOLDEN_IMAGE_EXTENSION}")
}

#[cfg(test)]
mod tests {
    use genzo_color::RgbColorSpace;

    use super::*;
    use crate::diff::LabConversion;

    fn sample() -> FloatImage {
        FloatImage::from_rgb_fn(8, 6, |x, y| {
            [x as f32 / 7.0, y as f32 / 5.0, ((x + y) % 3) as f32 * 0.25]
        })
        .unwrap()
    }

    struct Env {
        _tmp: tempfile::TempDir,
        golden: PathBuf,
        failures: PathBuf,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        Env {
            golden: tmp.path().join("golden"),
            failures: tmp.path().join("failures"),
            _tmp: tmp,
        }
    }

    fn store(e: &Env, mode: GoldenMode) -> GoldenStore {
        GoldenStore::new(&e.golden)
            .with_mode(mode)
            .with_failure_dir(&e.failures)
    }

    fn spec() -> GoldenSpec {
        GoldenSpec::new("sample", Tolerance::iq07a())
            .with_description("テスト用")
            .with_input(InputRef::from_bytes("synthetic:sample", b"input"))
    }

    #[test]
    fn float_image_format_roundtrip_is_bit_exact() {
        let mut img = sample();
        // 特殊な値（負のゼロ・非正規化数・NaN の特定のビット列）もそのまま残る。
        img.data_mut()[0] = -0.0;
        img.data_mut()[1] = f32::from_bits(1);
        img.data_mut()[2] = f32::from_bits(0x7fc0_0123);
        let bytes = encode_float_image(&img);
        assert_eq!(bytes.len(), FLOAT_IMAGE_HEADER_LEN + 8 * 6 * 3 * 4);
        assert_eq!(&bytes[..8], b"GENZOF32");
        assert_eq!(&bytes[8..12], &1u32.to_le_bytes());
        assert_eq!(&bytes[12..16], &8u32.to_le_bytes());
        // 値はリトルエンディアンの f32（画素 (1, 0) の R = 1/7）。
        assert_eq!(&bytes[36..40], &(1.0_f32 / 7.0).to_le_bytes());
        let back = decode_float_image(&bytes).unwrap();
        assert_eq!(back.width(), 8);
        let a: Vec<u32> = img.data().iter().map(|v| v.to_bits()).collect();
        let b: Vec<u32> = back.data().iter().map(|v| v.to_bits()).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn float_image_format_rejects_bad_input() {
        let bytes = encode_float_image(&sample());
        assert_eq!(
            decode_float_image(&bytes[..10]),
            Err(FormatError::TooShort(10))
        );
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(decode_float_image(&bad), Err(FormatError::BadMagic));
        let mut bad = bytes.clone();
        bad[8] = 2;
        assert_eq!(
            decode_float_image(&bad),
            Err(FormatError::UnsupportedVersion(2))
        );
        let mut bad = bytes.clone();
        bad[20] = 2; // チャンネル数 2
        assert!(matches!(
            decode_float_image(&bad),
            Err(FormatError::Image(ImageError::UnsupportedChannels(2)))
        ));
        // 巨大な寸法でも確保せずにエラーにする。
        let mut bad = bytes.clone();
        bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        bad[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            decode_float_image(&bad),
            Err(FormatError::Image(ImageError::TooManyPixels { .. }))
        ));
        let mut bad = bytes.clone();
        bad.pop();
        assert!(matches!(
            decode_float_image(&bad),
            Err(FormatError::LengthMismatch { .. })
        ));
        let mut bad = bytes;
        bad[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            decode_float_image(&bad),
            Err(FormatError::Image(ImageError::InvalidDimensions { .. }))
        ));
    }

    #[test]
    fn missing_golden_fails_in_compare_mode() {
        let e = env();
        let err = store(&e, GoldenMode::Compare)
            .check(&spec(), &sample())
            .unwrap_err();
        assert!(matches!(err, GoldenError::Missing { .. }), "{err}");
        assert!(err.to_string().contains("GENZO_UPDATE_GOLDEN=1"));
        // 何も書かない。
        assert!(!e.golden.exists());
    }

    #[test]
    fn update_then_compare_passes_and_meta_is_recorded() {
        let e = env();
        let img = sample();
        let out = store(&e, GoldenMode::Update).check(&spec(), &img).unwrap();
        assert!(matches!(out, GoldenOutcome::Updated { previous: None, .. }));
        let s = store(&e, GoldenMode::Compare);
        let (meta, loaded) = s.load("sample").unwrap();
        assert_eq!(loaded, img);
        assert_eq!(meta.spec, spec());
        assert_eq!(meta.image.file, "sample.gzf");
        assert_eq!(meta.image.sha256, sha256_hex(&encode_float_image(&img)));
        let json = fs::read_to_string(s.meta_path("sample")).unwrap();
        assert!(json.contains("\"metric\": \"diff_8bit\""), "{json}");
        assert!(json.contains("\"process_version\": 1"), "{json}");
        match s.check(&spec(), &img).unwrap() {
            GoldenOutcome::Passed(r) => assert!(r.passed()),
            other => panic!("{other:?}"),
        }
        // 説明だけ違うのは問題ない。
        let mut s2 = spec();
        s2.description = "別の説明".to_owned();
        assert!(matches!(
            s.check(&s2, &img).unwrap(),
            GoldenOutcome::Passed(_)
        ));
        // 許容差以内の変化（8bit 換算で 0.5）も合格。
        let near = img.map(|v| v + 0.5 / 255.0);
        assert!(matches!(
            s.check(&spec(), &near).unwrap(),
            GoldenOutcome::Passed(_)
        ));
        assert!(!e.failures.exists());
    }

    #[test]
    fn exceeding_tolerance_writes_artifacts_and_fails() {
        let e = env();
        let img = sample();
        store(&e, GoldenMode::Update).check(&spec(), &img).unwrap();
        let mut changed = img.clone();
        changed.pixel_mut(3, 2).unwrap()[0] += 2.0 / 255.0;
        let err = store(&e, GoldenMode::Compare)
            .check(&spec(), &changed)
            .unwrap_err();
        let GoldenError::Failed(f) = &err else {
            panic!("{err}");
        };
        let FailureReason::ToleranceExceeded(r) = &f.reason else {
            panic!("{:?}", f.reason);
        };
        assert_eq!(r.violations.len(), 1);
        let a = f.artifacts.as_ref().expect("書き出せる");
        assert_eq!(a.dir, e.failures.join("sample"));
        let written = decode_float_image(&fs::read(&a.actual).unwrap()).unwrap();
        assert_eq!(written, changed);
        // 差分画像: 変えた画素だけ赤、他は黒。
        let diff = image::open(a.diff_png.as_ref().unwrap())
            .unwrap()
            .into_rgb8();
        assert_eq!(diff.get_pixel(3, 2).0, [255, 0, 0]);
        assert_eq!(diff.get_pixel(0, 0).0, [0, 0, 0]);
        assert!(a.actual_png.as_ref().unwrap().is_file());
        assert!(a.expected_png.as_ref().unwrap().is_file());
        let report = fs::read_to_string(&a.report).unwrap();
        assert!(report.contains("許容差を超えました"), "{report}");
        let msg = err.to_string();
        assert!(msg.contains("GENZO_UPDATE_GOLDEN=1"), "{msg}");
        // 基準は変わっていない。
        let (_, kept) = store(&e, GoldenMode::Compare).load("sample").unwrap();
        assert_eq!(kept, img);
    }

    #[test]
    fn shape_mismatch_and_nan_are_failures_with_artifacts() {
        let e = env();
        store(&e, GoldenMode::Update)
            .check(&spec(), &sample())
            .unwrap();
        let s = store(&e, GoldenMode::Compare);
        let small = FloatImage::filled(4, 4, &[0.5, 0.5, 0.5]).unwrap();
        let err = s.check(&spec(), &small).unwrap_err();
        let GoldenError::Failed(f) = &err else {
            panic!("{err}");
        };
        assert!(matches!(
            f.reason,
            FailureReason::Diff(DiffError::ShapeMismatch { .. })
        ));
        let a = f.artifacts.as_ref().unwrap();
        assert!(a.diff_png.is_none());
        assert!(a.actual.is_file());

        let mut nan = sample();
        nan.data_mut()[5] = f32::NAN;
        let err = s.check(&spec(), &nan).unwrap_err();
        let GoldenError::Failed(f) = &err else {
            panic!("{err}");
        };
        assert!(matches!(
            f.reason,
            FailureReason::Diff(DiffError::NonFinite { .. })
        ));
        // NaN の画像は基準として保存しない。
        let err = store(&e, GoldenMode::Update)
            .check(&spec(), &nan)
            .unwrap_err();
        assert!(matches!(err, GoldenError::NonFiniteImage(_)), "{err}");
    }

    #[test]
    fn stale_diff_image_is_not_left_from_previous_failure() {
        // 1 回目は許容差を超えて差分画像を書く。2 回目は形が違って差分画像を作れない。
        // 前回の diff.png が残ると、今回の失敗の差分画像と取り違える。
        let e = env();
        let img = sample();
        store(&e, GoldenMode::Update).check(&spec(), &img).unwrap();
        let s = store(&e, GoldenMode::Compare);
        let changed = img.map(|v| v + 2.0 / 255.0);
        let GoldenError::Failed(f) = s.check(&spec(), &changed).unwrap_err() else {
            panic!()
        };
        let first = f.artifacts.unwrap();
        assert!(first.diff_png.as_ref().unwrap().is_file());
        let small = FloatImage::filled(4, 4, &[0.5, 0.5, 0.5]).unwrap();
        let GoldenError::Failed(f) = s.check(&spec(), &small).unwrap_err() else {
            panic!()
        };
        let second = f.artifacts.unwrap();
        assert!(second.diff_png.is_none());
        assert!(
            !second.dir.join("diff.png").exists(),
            "前回の差分画像が残っています"
        );
    }

    #[test]
    fn update_request_is_ignored_in_ci() {
        // CI で GENZO_UPDATE_GOLDEN=1 にすると、基準が作り直されてすべて合格になり、回帰を
        // 検出できない（MAINT-03）。CI では比較のままにする。
        use GoldenMode::{Compare, Update};
        assert_eq!(GoldenMode::from_values(None, None), Compare);
        assert_eq!(GoldenMode::from_values(Some("1"), None), Update);
        assert_eq!(GoldenMode::from_values(Some(" 1 "), None), Update);
        assert_eq!(GoldenMode::from_values(Some("0"), None), Compare);
        assert_eq!(GoldenMode::from_values(Some("true"), None), Compare);
        assert_eq!(GoldenMode::from_values(Some("1"), Some("true")), Compare);
        assert_eq!(GoldenMode::from_values(Some("1"), Some("1")), Compare);
        // CI の変数が空・false・0 なら CI ではない。
        for not_ci in ["", "false", "FALSE", "0"] {
            assert_eq!(
                GoldenMode::from_values(Some("1"), Some(not_ci)),
                Update,
                "{not_ci}"
            );
        }
    }

    #[test]
    fn spec_mismatch_requires_update() {
        let e = env();
        let img = sample();
        let s = store(&e, GoldenMode::Update);
        s.check(&spec(), &img).unwrap();
        let s = s.with_mode(GoldenMode::Compare);
        let cases: Vec<(GoldenSpec, &str)> = vec![
            (
                spec().with_input(InputRef::from_bytes("other", b"x")),
                "inputs",
            ),
            (
                spec().with_develop_settings(&genzo_model::DevelopSettings::default()),
                "develop",
            ),
            (spec().with_process_version(2), "process_version"),
            (
                {
                    let mut x = spec();
                    x.tolerance = Tolerance::diff_8bit(2.0);
                    x
                },
                "tolerance",
            ),
        ];
        for (sp, field) in cases {
            match s.check(&sp, &img) {
                Err(GoldenError::SpecMismatch { field: f, .. }) => assert_eq!(f, field),
                other => panic!("{field}: {other:?}"),
            }
        }
        // 作り直すと合格する。
        let changed = spec().with_process_version(2);
        s.clone()
            .with_mode(GoldenMode::Update)
            .check(&changed, &img)
            .unwrap();
        assert!(matches!(
            s.check(&changed, &img).unwrap(),
            GoldenOutcome::Passed(_)
        ));
    }

    #[test]
    fn update_reports_previous_difference_and_replaces() {
        let e = env();
        let s = store(&e, GoldenMode::Update);
        s.check(&spec(), &sample()).unwrap();
        let changed = sample().map(|v| v * 0.5);
        match s.check(&spec(), &changed).unwrap() {
            GoldenOutcome::Updated {
                previous: Some(r), ..
            } => assert!(!r.passed()),
            other => panic!("{other:?}"),
        }
        let (_, loaded) = s.load("sample").unwrap();
        assert_eq!(loaded, changed);
    }

    #[test]
    fn corrupted_files_are_detected() {
        let e = env();
        let s = store(&e, GoldenMode::Update);
        s.check(&spec(), &sample()).unwrap();
        // 画像を書き換える（長さは同じ）→ ハッシュが合わない。
        let p = s.image_path("sample");
        let mut bytes = fs::read(&p).unwrap();
        bytes[30] ^= 1;
        fs::write(&p, &bytes).unwrap();
        let err = s.load("sample").unwrap_err();
        assert!(matches!(err, GoldenError::Corrupt { .. }), "{err}");
        // 画像がない。
        fs::remove_file(&p).unwrap();
        assert!(matches!(
            s.load("sample").unwrap_err(),
            GoldenError::Corrupt { .. }
        ));
        // JSON が壊れている。
        fs::write(s.meta_path("sample"), "{").unwrap();
        assert!(matches!(
            s.load("sample").unwrap_err(),
            GoldenError::Corrupt { .. }
        ));
        // 名前の検査。
        assert!(matches!(
            s.load("../x").unwrap_err(),
            GoldenError::InvalidName(_)
        ));
    }

    #[test]
    fn meta_with_other_name_is_rejected() {
        let e = env();
        let s = store(&e, GoldenMode::Update);
        s.check(&spec(), &sample()).unwrap();
        // 別の名前のメタデータを置く（他のファイルを読ませない）。
        let json = fs::read_to_string(s.meta_path("sample")).unwrap();
        fs::write(s.meta_path("other"), json).unwrap();
        assert!(matches!(
            s.load("other").unwrap_err(),
            GoldenError::Corrupt { .. }
        ));
    }

    #[test]
    fn delta_e_golden() {
        let e = env();
        let conv = LabConversion::encoded(RgbColorSpace::Srgb);
        let sp = GoldenSpec::new("de", Tolerance::iq07b(conv));
        let img = sample();
        store(&e, GoldenMode::Update).check(&sp, &img).unwrap();
        let s = store(&e, GoldenMode::Compare);
        // 小さな変化は合格。
        let near = img.map(|v| v + 0.002);
        assert!(matches!(
            s.check(&sp, &near).unwrap(),
            GoldenOutcome::Passed(_)
        ));
        // 1 画素を大きく変えると、最大（5 以下）を超えて失敗し、差分画像はその画素が赤。
        let mut far = img.clone();
        far.pixel_mut(1, 1)
            .unwrap()
            .copy_from_slice(&[1.0, 0.0, 1.0]);
        let err = s.check(&sp, &far).unwrap_err();
        let GoldenError::Failed(f) = err else {
            panic!()
        };
        let diff = image::open(f.artifacts.unwrap().diff_png.unwrap())
            .unwrap()
            .into_rgb8();
        assert_eq!(diff.get_pixel(1, 1).0, [255, 0, 0]);
        // 1 チャンネルの画像は ΔE で比べられない（比較の失敗）。
        let gray = FloatImage::filled(8, 6, &[0.5]).unwrap();
        let sp_gray = GoldenSpec::new("de_gray", Tolerance::iq07b(conv));
        store(&e, GoldenMode::Update)
            .check(&sp_gray, &gray)
            .unwrap();
        let err = s.check(&sp_gray, &gray).unwrap_err();
        let GoldenError::Failed(f) = err else {
            panic!()
        };
        assert!(matches!(
            f.reason,
            FailureReason::Diff(DiffError::NotRgb(1))
        ));
    }

    #[test]
    fn invalid_spec_is_rejected_before_io() {
        let e = env();
        let s = store(&e, GoldenMode::Update);
        let bad = GoldenSpec::new("bad name", Tolerance::iq07a());
        assert!(matches!(
            s.check(&bad, &sample()),
            Err(GoldenError::InvalidName(_))
        ));
        let bad = GoldenSpec::new("x", Tolerance::diff_8bit(f64::NAN));
        assert!(matches!(
            s.check(&bad, &sample()),
            Err(GoldenError::InvalidTolerance(_))
        ));
        assert!(!e.golden.exists());
    }

    #[test]
    fn mode_from_env_default_is_compare() {
        // テストの並列実行で環境変数を書き換えないため、現在の値に応じた期待値だけを確かめる。
        let expected = GoldenMode::from_values(
            env::var(UPDATE_GOLDEN_ENV).ok().as_deref(),
            env::var(CI_ENV).ok().as_deref(),
        );
        assert_eq!(GoldenMode::from_env(), expected);
        if env::var_os(UPDATE_GOLDEN_ENV).is_none() {
            assert_eq!(GoldenMode::from_env(), GoldenMode::Compare);
        }
    }

    #[test]
    fn default_failure_dir_is_under_target() {
        if env::var_os(GOLDEN_OUT_DIR_ENV).is_none() {
            let d = default_failure_dir();
            // テストのバイナリは target/<プロファイル>/deps/ にあるので、target/golden-failures。
            assert!(d.ends_with("golden-failures"), "{}", d.display());
            assert!(d.parent().unwrap().join("CACHEDIR.TAG").is_file());
        }
    }

    #[test]
    fn golden_dir_macro_points_to_caller_manifest() {
        let d = crate::golden_dir!();
        assert_eq!(
            d,
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
        );
    }
}
