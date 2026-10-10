//! lcms2 による ICC プロファイルの作成・読み込みと、浮動小数点の色変換。
//!
//! - 標準のプロファイル（[`StandardProfile`]）: (a) 画面用 3D LUT の変換元「BT.2020 の原色・D65・
//!   IEC 61966-2-1 の伝達関数」（04 の 2.6 節のステージ 17a）、(b) sRGB、(c) Display P3、
//!   (d) Adobe RGB (1998) 互換、(e) リニア BT.2020。書き出し時に埋め込む ICC のバイト列（IQ-06）を
//!   取り出せる。
//! - 外部の ICC のバイト列（モニターのプロファイル、画像に埋め込まれたプロファイル）を読み込む。
//! - 浮動小数点の RGB → RGB の変換（[`IccTransform`]）。RAW 以外の入力を作業色空間に変換する
//!   （2.6 節「RAW 以外の入力」）ときや、3D LUT の作成（[`crate::lut`]）に使う。
//!
//! ## 生成するプロファイルのバイト列を決定的にする
//!
//! lcms2 はヘッダーに作成日時と、ビルドした OS で決まるプラットフォーム（Windows なら 'MSFT'、
//! それ以外は 'APPL'）を書く。同じ設定で書き出したファイルが毎回・OS ごとに変わらないよう
//! （IQ-08、キャッシュのキー）、作成日時を [`PROFILE_HEADER_DATE`] に、プラットフォームを 0
//! （指定なし）に固定してから、v4 ではプロファイル ID（MD5）を計算し直す。
//!
//! ## 安全性
//!
//! 外部のプロファイルは信頼できない入力として扱う。大きさの上限（[`MAX_ICC_PROFILE_BYTES`]）を
//! 設け、ヘッダーの色空間・クラスは lcms2 の列挙型を経由せずバイト列から自前で読む
//! （未知の値を Rust の列挙型として受け取らないため）。

use std::fmt;
use std::sync::Arc;

use lcms2::{
    CIExyY, CIExyYTRIPLE, DisallowCache, Flags, GlobalContext, Intent, Locale, MLU, PixelFormat,
    Profile, Tag, TagSignature, ToneCurve, Transform,
};

use crate::error::{ColorError, Result};
use crate::matrix::Mat3;
use crate::space::{
    ADOBE_RGB_PRIMARIES, BT709_PRIMARIES, BT2020_PRIMARIES, BT2020_TO_XYZ, D65,
    DISPLAY_P3_PRIMARIES, RgbPrimaries, Xy, bradford_adaptation,
};
use crate::transfer::{ADOBE_RGB_GAMMA, TransferFunction};
use crate::transfer::{SRGB_DECODE_THRESHOLD, SRGB_GAMMA, SRGB_LINEAR_SLOPE, SRGB_OFFSET};

/// 読み込む ICC プロファイルの大きさの上限（バイト。仮置き）。
///
/// モニター・画像のプロファイルは通常数 KB〜数百 KB で、大きなプリンターのプロファイルでも
/// 数 MB 程度のため、余裕を見て 32 MiB とした。
pub const MAX_ICC_PROFILE_BYTES: usize = 32 * 1024 * 1024;

/// ICC のヘッダー（128 バイト）とタグの数（4 バイト）の大きさ。
const MIN_ICC_PROFILE_BYTES: usize = 132;

/// 生成するプロファイルのヘッダーに書く作成日時（年, 月, 日, 時, 分, 秒）。
///
/// 決定的なバイト列にするための固定値。生成するプロファイルの内容（原色・伝達関数・説明など）を
/// 変えたときは、この日付も更新する（同じ日付で中身が違うプロファイルを作らないため）。
pub const PROFILE_HEADER_DATE: [u16; 6] = [2026, 10, 9, 0, 0, 0];

/// 生成するプロファイルの著作権のタグ（'cprt'）の文字列（仮置き。**法的な表現のため要確認**）。
///
/// 書き出した画像に埋め込まれるため、lcms2 の既定の文字列に任せず、ここで明示する（lcms2 の
/// 版によって既定が変わると、生成するバイト列が黙って変わるため。IQ-08）。値は現時点では
/// lcms2 の既定と同じ文字列で、生成されるバイト列は変わらない。文言を変えるときは
/// [`PROFILE_HEADER_DATE`] も更新する。
pub const PROFILE_COPYRIGHT: &str = "No copyright, use freely";

/// genzo-color が作る標準のプロファイル。白色点はすべて D65。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StandardProfile {
    /// (a) BT.2020 の原色・D65・IEC 61966-2-1 の伝達関数。画面用 3D LUT の変換元（2.6 節 17a）。
    Bt2020SrgbTransfer,
    /// (b) sRGB（BT.709 の原色・IEC 61966-2-1 の伝達関数）。
    Srgb,
    /// (c) Display P3（IEC 61966-2-1 の伝達関数）。キャッシュ（B5）と書き出し。
    DisplayP3,
    /// (d) Adobe RGB (1998) 互換（ガンマ 563/256）。
    AdobeRgb1998,
    /// (e) リニア BT.2020（作業色空間そのもの）。
    LinearBt2020,
}

impl StandardProfile {
    /// すべての標準のプロファイル。
    pub const ALL: [StandardProfile; 5] = [
        StandardProfile::Bt2020SrgbTransfer,
        StandardProfile::Srgb,
        StandardProfile::DisplayP3,
        StandardProfile::AdobeRgb1998,
        StandardProfile::LinearBt2020,
    ];

    /// 原色。
    pub const fn primaries(self) -> RgbPrimaries {
        match self {
            Self::Bt2020SrgbTransfer | Self::LinearBt2020 => BT2020_PRIMARIES,
            Self::Srgb => BT709_PRIMARIES,
            Self::DisplayP3 => DISPLAY_P3_PRIMARIES,
            Self::AdobeRgb1998 => ADOBE_RGB_PRIMARIES,
        }
    }

    /// 白色点（D65）。
    pub const fn white_point(self) -> Xy {
        D65
    }

    /// 伝達関数。
    pub const fn transfer_function(self) -> TransferFunction {
        match self {
            Self::Bt2020SrgbTransfer | Self::Srgb | Self::DisplayP3 => TransferFunction::Srgb,
            Self::AdobeRgb1998 => TransferFunction::AdobeRgbGamma,
            Self::LinearBt2020 => TransferFunction::Linear,
        }
    }

    /// プロファイルの説明（ICC の 'desc' タグ。ASCII）。
    ///
    /// 「Adobe RGB (1998)」は Adobe の商標のため、互換であることを示す名前にしている。
    pub const fn description(self) -> &'static str {
        match self {
            Self::Bt2020SrgbTransfer => "GenzoParis BT.2020 D65 (IEC 61966-2-1 transfer)",
            Self::Srgb => "GenzoParis sRGB (IEC 61966-2-1)",
            Self::DisplayP3 => "GenzoParis Display P3",
            Self::AdobeRgb1998 => "GenzoParis Adobe RGB (1998) compatible",
            Self::LinearBt2020 => "GenzoParis Linear BT.2020 D65",
        }
    }
}

/// 生成するプロファイルの ICC のバージョン。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IccVersion {
    /// v2.4。古いビューアとの互換性が高い。IEC 61966-2-1 の伝達関数は 4096 点の表（16bit）になる
    /// （v2 には区分関数の形式がないため）。
    V2_4,
    /// v4.3（lcms2 の既定）。伝達関数は区分関数（'para'）で正確に表せる。
    #[default]
    V4_3,
}

impl IccVersion {
    fn as_f64(self) -> f64 {
        match self {
            Self::V2_4 => 2.4,
            Self::V4_3 => 4.3,
        }
    }
}

/// ICC のプロファイルのクラス（ヘッダーの 12〜15 バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IccDeviceClass {
    /// 'scnr'（入力機器）。
    Input,
    /// 'mntr'（ディスプレイ）。
    Display,
    /// 'prtr'（出力機器）。
    Output,
    /// 'link'（デバイスリンク）。
    Link,
    /// 'abst'（抽象）。
    Abstract,
    /// 'spac'（色空間の変換）。
    ColorSpace,
    /// 'nmcl'（名前付きの色）。
    NamedColor,
    /// その他（4 文字のシグネチャ）。
    Other([u8; 4]),
}

impl IccDeviceClass {
    fn from_signature(sig: [u8; 4]) -> Self {
        match &sig {
            b"scnr" => Self::Input,
            b"mntr" => Self::Display,
            b"prtr" => Self::Output,
            b"link" => Self::Link,
            b"abst" => Self::Abstract,
            b"spac" => Self::ColorSpace,
            b"nmcl" => Self::NamedColor,
            _ => Self::Other(sig),
        }
    }
}

/// ヘッダーから自前で読んだ情報。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeaderInfo {
    class: [u8; 4],
    color_space: [u8; 4],
    pcs: [u8; 4],
    version: (u8, u8),
}

fn sig_at(bytes: &[u8], offset: usize) -> [u8; 4] {
    [
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]
}

fn parse_header(bytes: &[u8]) -> Result<HeaderInfo> {
    if bytes.len() > MAX_ICC_PROFILE_BYTES {
        return Err(ColorError::IccInvalid {
            reason: format!(
                "大きすぎる（{} バイト、上限 {MAX_ICC_PROFILE_BYTES} バイト）",
                bytes.len()
            ),
        });
    }
    if bytes.len() < MIN_ICC_PROFILE_BYTES {
        return Err(ColorError::IccInvalid {
            reason: format!("短すぎる（{} バイト）", bytes.len()),
        });
    }
    if &bytes[36..40] != b"acsp" {
        return Err(ColorError::IccInvalid {
            reason: "ICC のシグネチャ（'acsp'）がない".to_owned(),
        });
    }
    Ok(HeaderInfo {
        class: sig_at(bytes, 12),
        color_space: sig_at(bytes, 16),
        pcs: sig_at(bytes, 20),
        version: (bytes[8], bytes[9] >> 4),
    })
}

/// ICC プロファイル（バイト列と、読み込み時に確認した情報）。
///
/// バイト列だけを持つので、スレッド間で共有・複製できる（lcms2 のハンドルは必要なときに作る）。
#[derive(Clone)]
pub struct IccProfile {
    bytes: Arc<[u8]>,
    description: Option<String>,
    header: HeaderInfo,
}

impl fmt::Debug for IccProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IccProfile")
            .field("description", &self.description)
            .field("len", &self.bytes.len())
            .field("class", &self.device_class())
            .field("version", &self.header.version)
            .finish()
    }
}

impl PartialEq for IccProfile {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for IccProfile {}

/// 説明の文字列の長さの上限（文字数）。外部のプロファイルの説明を UI に出すときのため。
const MAX_DESCRIPTION_CHARS: usize = 256;

fn lcms_err(what: &str, e: lcms2::Error) -> ColorError {
    ColorError::Lcms {
        reason: format!("{what}: {e}"),
    }
}

fn read_description(profile: &Profile) -> Option<String> {
    match profile.read_tag(TagSignature::ProfileDescriptionTag) {
        Tag::MLU(mlu) => mlu
            .text(Locale::none())
            .or_else(|_| mlu.text_ascii(Locale::none()))
            .ok()
            .map(|s| s.chars().take(MAX_DESCRIPTION_CHARS).collect()),
        _ => None,
    }
}

fn to_cie_xyy(xy: Xy) -> CIExyY {
    CIExyY {
        x: xy.x,
        y: xy.y,
        Y: 1.0,
    }
}

/// lcms2 のトーンカーブを作る。
fn tone_curve(tf: TransferFunction) -> Result<ToneCurve> {
    match tf {
        TransferFunction::Linear => Ok(ToneCurve::new(1.0)),
        TransferFunction::AdobeRgbGamma => Ok(ToneCurve::new(ADOBE_RGB_GAMMA)),
        // lcms2 の型 4（ICC の 'para' の関数型 3）: X ≥ d なら (aX + b)^g、X < d なら cX。
        TransferFunction::Srgb => ToneCurve::new_parametric(
            4,
            &[
                SRGB_GAMMA,
                1.0 / (1.0 + SRGB_OFFSET),
                SRGB_OFFSET / (1.0 + SRGB_OFFSET),
                1.0 / SRGB_LINEAR_SLOPE,
                SRGB_DECODE_THRESHOLD,
            ],
        )
        .map_err(|e| lcms_err("IEC 61966-2-1 のトーンカーブ", e)),
    }
}

/// ヘッダーの作成日時とプラットフォームを固定値にする。
fn normalize_header(bytes: &mut [u8]) {
    for (i, v) in PROFILE_HEADER_DATE.iter().enumerate() {
        let o = 24 + 2 * i;
        bytes[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
    bytes[40..44].fill(0);
}

fn build_standard_bytes(kind: StandardProfile, version: IccVersion) -> Result<Vec<u8>> {
    let p = kind.primaries();
    let white = to_cie_xyy(kind.white_point());
    let primaries = CIExyYTRIPLE {
        Red: to_cie_xyy(p.red),
        Green: to_cie_xyy(p.green),
        Blue: to_cie_xyy(p.blue),
    };
    let curve = tone_curve(kind.transfer_function())?;
    // 3 チャンネルに同じカーブを渡すと、lcms2 はタグを共有して小さく保存する。
    let mut profile = Profile::new_rgb(&white, &primaries, &[&curve, &curve, &curve])
        .map_err(|e| lcms_err("RGB プロファイルの作成", e))?;
    for (sig, text, what) in [
        (
            TagSignature::ProfileDescriptionTag,
            kind.description(),
            "説明",
        ),
        (TagSignature::CopyrightTag, PROFILE_COPYRIGHT, "著作権"),
    ] {
        let mut mlu = MLU::new(1);
        if !mlu.set_text_ascii(text, Locale::new("en_US")) {
            return Err(ColorError::Lcms {
                reason: format!("{what}の文字列を設定できない"),
            });
        }
        if !profile.write_tag(sig, Tag::MLU(&mlu)) {
            return Err(ColorError::Lcms {
                reason: format!("{what}のタグを書けない"),
            });
        }
    }
    profile.set_version(version.as_f64());
    let mut bytes = profile
        .icc()
        .map_err(|e| lcms_err("プロファイルの保存", e))?;
    if bytes.len() < MIN_ICC_PROFILE_BYTES {
        return Err(ColorError::Lcms {
            reason: "保存したプロファイルが短すぎる".to_owned(),
        });
    }
    normalize_header(&mut bytes);
    if version == IccVersion::V4_3 {
        // 固定したヘッダーで読み直し、プロファイル ID（MD5）を計算して保存し直す。
        let mut reloaded =
            Profile::new_icc(&bytes).map_err(|e| lcms_err("生成したプロファイルの読み直し", e))?;
        reloaded.set_default_profile_id();
        bytes = reloaded
            .icc()
            .map_err(|e| lcms_err("プロファイルの保存（ID の計算後）", e))?;
    }
    Ok(bytes)
}

impl IccProfile {
    /// 標準のプロファイル（ICC v4.3）を作る。
    pub fn standard(kind: StandardProfile) -> Result<Self> {
        Self::standard_with_version(kind, IccVersion::default())
    }

    /// 標準のプロファイルを、ICC のバージョンを指定して作る。
    pub fn standard_with_version(kind: StandardProfile, version: IccVersion) -> Result<Self> {
        let bytes = build_standard_bytes(kind, version)?;
        Self::from_bytes(&bytes)
    }

    /// 外部の ICC のバイト列から読み込む。
    ///
    /// lcms2 で解析できること、大きさが [`MAX_ICC_PROFILE_BYTES`] 以下であることを確認する。
    /// 色空間（RGB かどうか）はここでは問わない（用途ごとに確認する）。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let header = parse_header(bytes)?;
        let profile = Profile::new_icc(bytes).map_err(|e| ColorError::IccInvalid {
            reason: format!("lcms2 で解析できない: {e}"),
        })?;
        let description = read_description(&profile);
        Ok(Self {
            bytes: Arc::from(bytes),
            description,
            header,
        })
    }

    /// ICC のバイト列（書き出すファイルに埋め込む。IQ-06）。
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 説明（'desc' タグ。読めなければ `None`。長いものは 256 文字で切る）。
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// クラス。
    pub fn device_class(&self) -> IccDeviceClass {
        IccDeviceClass::from_signature(self.header.class)
    }

    /// データの色空間が RGB か。
    pub fn is_rgb(&self) -> bool {
        &self.header.color_space == b"RGB "
    }

    /// データの色空間のシグネチャ（4 文字。例: `b"RGB "`、`b"CMYK"`）。
    pub fn color_space_signature(&self) -> [u8; 4] {
        self.header.color_space
    }

    /// PCS のシグネチャ（`b"XYZ "` または `b"Lab "`。デバイスリンクでは別の色空間）。
    pub fn pcs_signature(&self) -> [u8; 4] {
        self.header.pcs
    }

    /// ICC のバージョン（主, 副）。例: v4.3 なら (4, 3)。
    pub fn version(&self) -> (u8, u8) {
        self.header.version
    }

    /// lcms2 のプロファイルを作る。
    pub(crate) fn to_lcms(&self) -> Result<Profile> {
        Profile::new_icc(&self.bytes).map_err(|e| ColorError::IccInvalid {
            reason: format!("lcms2 で解析できない: {e}"),
        })
    }

    /// 行列とトーンカーブで表された（matrix-shaper の）RGB プロファイルなら、リニア RGB →
    /// PCS の XYZ（D50 に順応した値）の行列を返す（rXYZ・gXYZ・bXYZ のタグを列に並べたもの）。
    ///
    /// LUT で表されたプロファイルや、RGB でないプロファイルでは `None`。
    pub fn rgb_to_pcs_xyz_matrix(&self) -> Option<Mat3> {
        if !self.is_rgb() || &self.header.pcs != b"XYZ " {
            return None;
        }
        let profile = self.to_lcms().ok()?;
        if !profile.is_matrix_shaper() {
            return None;
        }
        let col = |sig| match profile.read_tag(sig) {
            Tag::CIEXYZ(xyz) => Some([xyz.X, xyz.Y, xyz.Z]),
            _ => None,
        };
        let m = Mat3::from_cols([
            col(TagSignature::RedColorantTag)?,
            col(TagSignature::GreenColorantTag)?,
            col(TagSignature::BlueColorantTag)?,
        ]);
        m.is_finite().then_some(m)
    }

    /// matrix-shaper の RGB プロファイルなら、作業色空間（リニア BT.2020、D65）→ そのプロファイルの
    /// リニア RGB の行列を返す。色域の圧縮（[`crate::gamut`]）を、モニターのリニア RGB で行うために使う。
    ///
    /// 相対的な色域を維持するインテントの lcms2 の変換と同じく、D65 の白を Bradford 法で PCS の
    /// 白（D50 = XYZ (0.9642, 1, 0.8249)）に順応させてから、プロファイルの行列の逆行列を掛ける。
    /// LUT で表されたプロファイルなどでは `None`。
    pub fn working_to_linear_rgb_matrix(&self) -> Option<Mat3> {
        const ICC_D50: [f64; 3] = [0.9642, 1.0, 0.8249];
        let to_pcs = self.rgb_to_pcs_xyz_matrix()?;
        let from_pcs = to_pcs.inverse().ok()?;
        let adapt = bradford_adaptation(D65.to_xyz(1.0).ok()?, ICC_D50).ok()?;
        Some(from_pcs * adapt * BT2020_TO_XYZ)
    }
}

/// レンダリングインテント。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderingIntent {
    /// 知覚的。
    Perceptual,
    /// 相対的な色域を維持（画面用 3D LUT はこれを使う。2.6 節 17a）。
    RelativeColorimetric,
    /// 彩度。
    Saturation,
    /// 絶対的な色域を維持。
    AbsoluteColorimetric,
}

impl From<RenderingIntent> for Intent {
    fn from(i: RenderingIntent) -> Intent {
        match i {
            RenderingIntent::Perceptual => Intent::Perceptual,
            RenderingIntent::RelativeColorimetric => Intent::RelativeColorimetric,
            RenderingIntent::Saturation => Intent::Saturation,
            RenderingIntent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
        }
    }
}

/// 浮動小数点の RGB → RGB の色変換（lcms2）。
///
/// 値は 0〜1 を基準とする（lcms2 の浮動小数点の RGB の約束）。行列とトーンカーブのプロファイル
/// どうしでは、行列の部分は 0〜1 の外の値もそのまま計算する（lcms2 の unbounded な浮動小数点の
/// 変換）。ただし 0〜1 の外の値の扱いは **トーンカーブの種類で違う**（このコンテナの lcms2 で
/// 確認した挙動。テスト `out_of_range_values_depend_on_tone_curve_type` で固定している）:
///
/// - ガンマ 1（リニア）: 負の値も 1 超えもそのまま（[`StandardProfile::LinearBt2020`] への変換で
///   色域外の色が負の値として保たれる。IQ-02）。
/// - IEC 61966-2-1 の区分関数（v4 の 'para'）: 負の値は線形の区間を延長（符号を保って |x| に
///   適用する [`crate::transfer`] の関数とは値が違う）、1 超えは冪の区間を延長。
/// - 純粋なべき乗（Adobe RGB のガンマなど）: 負の値は 0、1 超えは延長。
/// - 表のカーブ（v2 の 2 点以上の 'curv'。IEC 61966-2-1 は v2 ではこの形になる）: 0〜1 に収める。
///   1 点の 'curv'（ガンマ値）は上のガンマ 1・純粋なべき乗と同じ扱い。
///
/// 黒点の補正は使わない。スレッド間で共有できる（lcms2 のキャッシュを無効にしている）。
pub struct IccTransform {
    inner: Transform<[f32; 3], [f32; 3], GlobalContext, DisallowCache>,
}

impl fmt::Debug for IccTransform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IccTransform").finish_non_exhaustive()
    }
}

impl IccTransform {
    /// 変換を作る。どちらかのプロファイルが RGB でない、または lcms2 が変換を作れないならエラー。
    pub fn new(src: &IccProfile, dst: &IccProfile, intent: RenderingIntent) -> Result<Self> {
        for (p, which) in [(src, "変換元"), (dst, "変換先")] {
            if !p.is_rgb() {
                return Err(ColorError::IccUnsupported {
                    reason: format!(
                        "{which}のプロファイルの色空間が RGB でない（{}）",
                        String::from_utf8_lossy(&p.header.color_space)
                    ),
                });
            }
            match p.device_class() {
                IccDeviceClass::Link | IccDeviceClass::Abstract | IccDeviceClass::NamedColor => {
                    return Err(ColorError::IccUnsupported {
                        reason: format!(
                            "{which}のプロファイルのクラスが対応外（{:?}）",
                            p.device_class()
                        ),
                    });
                }
                _ => {}
            }
        }
        let s = src.to_lcms()?;
        let d = dst.to_lcms()?;
        let inner = Transform::new_flags_context(
            GlobalContext::new(),
            &s,
            PixelFormat::RGB_FLT,
            &d,
            PixelFormat::RGB_FLT,
            intent.into(),
            Flags::NO_CACHE,
        )
        .map_err(|e| ColorError::IccUnsupported {
            reason: format!("lcms2 で変換を作れない: {e}"),
        })?;
        Ok(Self { inner })
    }

    /// `src` を変換して `dst` に書く。長さが違えばエラー。
    pub fn transform(&self, src: &[[f32; 3]], dst: &mut [[f32; 3]]) -> Result<()> {
        if src.len() != dst.len() {
            return Err(ColorError::LengthMismatch {
                input: src.len(),
                output: dst.len(),
            });
        }
        // lcms2 は 1 回に u32::MAX 画素までしか処理しないため、分けて渡す。
        const CHUNK: usize = 1 << 20;
        for (s, d) in src.chunks(CHUNK).zip(dst.chunks_mut(CHUNK)) {
            self.inner.transform_pixels(s, d);
        }
        Ok(())
    }

    /// その場で変換する。
    pub fn transform_in_place(&self, pixels: &mut [[f32; 3]]) {
        const CHUNK: usize = 1 << 20;
        for c in pixels.chunks_mut(CHUNK) {
            self.inner.transform_in_place(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::delta_e2000;
    use crate::space::{RgbColorSpace, working_to};
    use crate::test_util::{lab_d50_profile, rgb_to_lab};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn types_are_send_sync() {
        assert_send_sync::<IccProfile>();
        assert_send_sync::<IccTransform>();
    }

    #[test]
    fn standard_profiles_are_valid_and_described() {
        for kind in StandardProfile::ALL {
            for version in [IccVersion::V2_4, IccVersion::V4_3] {
                let p = IccProfile::standard_with_version(kind, version).unwrap();
                assert!(p.is_rgb());
                assert_eq!(p.device_class(), IccDeviceClass::Display);
                assert_eq!(&p.pcs_signature(), b"XYZ ");
                assert_eq!(
                    p.description(),
                    Some(kind.description()),
                    "{kind:?} {version:?}"
                );
                let expected_version = match version {
                    IccVersion::V2_4 => (2, 4),
                    IccVersion::V4_3 => (4, 3),
                };
                assert_eq!(p.version(), expected_version);
                // 読み直せる。
                let again = IccProfile::from_bytes(p.as_bytes()).unwrap();
                assert_eq!(again, p);
            }
        }
    }

    #[test]
    fn standard_profiles_are_deterministic() {
        for kind in StandardProfile::ALL {
            let a = IccProfile::standard(kind).unwrap();
            let b = IccProfile::standard(kind).unwrap();
            assert_eq!(a.as_bytes(), b.as_bytes());
            let bytes = a.as_bytes();
            // 作成日時とプラットフォームが固定されている。
            assert_eq!(&bytes[24..26], &2026_u16.to_be_bytes());
            assert_eq!(&bytes[26..28], &10_u16.to_be_bytes());
            assert_eq!(&bytes[28..30], &9_u16.to_be_bytes());
            assert_eq!(&bytes[30..36], &[0; 6]);
            assert_eq!(&bytes[40..44], &[0; 4]);
            // v4 はプロファイル ID（MD5）が入っている。
            assert_ne!(&bytes[84..100], &[0; 16]);
        }
    }

    #[test]
    fn v2_header_is_fixed_and_v4_id_matches_final_bytes() {
        for kind in StandardProfile::ALL {
            // v2: 作成日時とプラットフォームが固定され、プロファイル ID は 0（v2 には ID がない）。
            let v2 = IccProfile::standard_with_version(kind, IccVersion::V2_4).unwrap();
            let again = IccProfile::standard_with_version(kind, IccVersion::V2_4).unwrap();
            assert_eq!(v2.as_bytes(), again.as_bytes(), "{kind:?}");
            let b = v2.as_bytes();
            assert_eq!(&b[24..26], &2026_u16.to_be_bytes());
            assert_eq!(&b[30..36], &[0; 6]);
            assert_eq!(&b[40..44], &[0; 4], "{kind:?}: プラットフォーム");
            assert_eq!(&b[84..100], &[0; 16], "{kind:?}: v2 の ID");
            // ヘッダーの大きさの欄が実際の長さと一致する。
            assert_eq!(
                u32::from_be_bytes(b[0..4].try_into().unwrap()) as usize,
                b.len()
            );

            // v4: ID（MD5）は最終のバイト列から計算されている。もう一度計算し直しても変わらない
            // （ID を計算した後にヘッダーを書き換えていない）。
            let v4 = IccProfile::standard(kind).unwrap();
            let mut p = Profile::new_icc(v4.as_bytes()).unwrap();
            p.set_default_profile_id();
            assert_eq!(p.icc().unwrap(), v4.as_bytes(), "{kind:?}");
            let b = v4.as_bytes();
            assert_eq!(
                u32::from_be_bytes(b[0..4].try_into().unwrap()) as usize,
                b.len()
            );
        }
    }

    #[test]
    fn copyright_tag_is_the_explicit_constant() {
        for kind in StandardProfile::ALL {
            for version in [IccVersion::V2_4, IccVersion::V4_3] {
                let p = IccProfile::standard_with_version(kind, version).unwrap();
                let lcms = p.to_lcms().unwrap();
                let text = match lcms.read_tag(TagSignature::CopyrightTag) {
                    Tag::MLU(mlu) => mlu.text(Locale::none()).unwrap(),
                    _ => panic!("{kind:?} {version:?}: 著作権のタグがない"),
                };
                assert_eq!(text, PROFILE_COPYRIGHT, "{kind:?} {version:?}");
            }
        }
    }

    #[test]
    fn out_of_range_values_depend_on_tone_curve_type() {
        // IccTransform の doc に書いた、0〜1 の外の値の扱い（lcms2 の挙動）を固定する。
        // 変換元はリニア BT.2020 で、無彩色を使う（どの変換先でも RGB の 3 成分が同じ値になる）。
        let lin = IccProfile::standard(StandardProfile::LinearBt2020).unwrap();
        let src = [[-0.2_f32; 3], [-0.001; 3], [2.0; 3], [0.5; 3]];
        let run = |kind: StandardProfile, version: IccVersion| -> [f32; 4] {
            let dst = IccProfile::standard_with_version(kind, version).unwrap();
            let t = IccTransform::new(&lin, &dst, RenderingIntent::RelativeColorimetric).unwrap();
            let mut out = [[0.0_f32; 3]; 4];
            t.transform(&src, &mut out).unwrap();
            // ICC の行列の量子化で 3 成分はわずかに違うので、平均を見る。
            out.map(|v| (v[0] + v[1] + v[2]) / 3.0)
        };
        let close = |a: f32, b: f64, tol: f64| (f64::from(a) - b).abs() < tol;

        // ガンマ 1: そのまま（v2 の 1 点の 'curv' もガンマ 1 として扱われる）。
        for version in [IccVersion::V2_4, IccVersion::V4_3] {
            let o = run(StandardProfile::LinearBt2020, version);
            for (v, e) in o.iter().zip([-0.2, -0.001, 2.0, 0.5]) {
                assert!(close(*v, e, 1e-4), "{version:?}: {o:?}");
            }
        }
        // IEC 61966-2-1（v4 の 'para'）: 負の値は線形の区間を延長、1 超えは冪の区間を延長。
        let o = run(StandardProfile::Srgb, IccVersion::V4_3);
        assert!(close(o[0], -0.2 * 12.92, 1e-3), "{o:?}");
        assert!(close(o[1], -0.001 * 12.92, 1e-4), "{o:?}");
        assert!(
            close(o[2], 1.055 * 2.0_f64.powf(1.0 / 2.4) - 0.055, 1e-3),
            "{o:?}"
        );
        assert!(
            close(o[3], crate::transfer::srgb_encode(0.5), 1e-4),
            "{o:?}"
        );
        // v2 の表のカーブ: 0〜1 に収める。
        let o = run(StandardProfile::Srgb, IccVersion::V2_4);
        assert!(close(o[0], 0.0, 1e-6) && close(o[1], 0.0, 1e-6), "{o:?}");
        assert!(close(o[2], 1.0, 1e-6), "{o:?}");
        // 純粋なべき乗: 負の値は 0、1 超えは延長。
        let o = run(StandardProfile::AdobeRgb1998, IccVersion::V4_3);
        assert!(close(o[0], 0.0, 1e-6) && close(o[1], 0.0, 1e-6), "{o:?}");
        assert!(close(o[2], 2.0_f64.powf(256.0 / 563.0), 1e-3), "{o:?}");
    }

    #[test]
    fn device_classes_for_transform() {
        // RGB でも、デバイスリンク・抽象・名前付きの色のクラスは変換に使わない。ディスプレイの
        // プロファイルとして渡されたら sRGB とみなす（IQ-05）。入力機器・出力機器・色空間の
        // クラスの RGB の matrix-shaper は変換を作れる。
        let srgb = IccProfile::standard(StandardProfile::Srgb).unwrap();
        for (sig, ok) in [
            (b"link", false),
            (b"abst", false),
            (b"nmcl", false),
            (b"spac", true),
            (b"scnr", true),
            (b"prtr", true),
        ] {
            let mut b = srgb.as_bytes().to_vec();
            b[12..16].copy_from_slice(sig);
            let p = IccProfile::from_bytes(&b).unwrap();
            assert_eq!(p.device_class(), IccDeviceClass::from_signature(*sig));
            let t = IccTransform::new(&srgb, &p, RenderingIntent::RelativeColorimetric);
            assert_eq!(t.is_ok(), ok, "{}: {t:?}", String::from_utf8_lossy(sig));
            if !ok {
                assert!(matches!(t, Err(ColorError::IccUnsupported { .. })));
                assert!(
                    IccTransform::new(&p, &srgb, RenderingIntent::RelativeColorimetric).is_err()
                );
            }
            let d = crate::display::DisplayProfile::resolve(Some(&b)).unwrap();
            assert_eq!(d.is_assumed_srgb(), !ok, "{}", String::from_utf8_lossy(sig));
        }
    }

    #[test]
    fn srgb_profile_matches_lcms_builtin_srgb() {
        // lcms2 内蔵の sRGB と、作った sRGB で同じ色を変換して比べる。
        let ours = IccProfile::standard(StandardProfile::Srgb).unwrap();
        let builtin = IccProfile::from_bytes(&Profile::new_srgb().icc().unwrap()).unwrap();
        let t = IccTransform::new(&ours, &builtin, RenderingIntent::RelativeColorimetric).unwrap();
        let src: Vec<[f32; 3]> = (0..=10)
            .flat_map(|i| (0..=10).map(move |j| [i as f32 / 10.0, j as f32 / 10.0, 0.37]))
            .collect();
        let mut dst = vec![[0.0; 3]; src.len()];
        t.transform(&src, &mut dst).unwrap();
        for (s, d) in src.iter().zip(&dst) {
            for k in 0..3 {
                assert!((s[k] - d[k]).abs() < 1e-4, "{s:?} → {d:?}");
            }
        }
    }

    #[test]
    fn transform_linear_bt2020_to_srgb_matches_matrix_and_transfer() {
        // リニア BT.2020 → sRGB の lcms2 の変換が、自前の行列と伝達関数と一致する
        // （lcms2 内部の s15Fixed16 の量子化と、D50 を経由する計算の誤差を見込む）。
        let lin = IccProfile::standard(StandardProfile::LinearBt2020).unwrap();
        let srgb = IccProfile::standard(StandardProfile::Srgb).unwrap();
        let t = IccTransform::new(&lin, &srgb, RenderingIntent::RelativeColorimetric).unwrap();
        let m = working_to(RgbColorSpace::Srgb).to_f32();
        // sRGB の色域内の色だけを使う（色域外の負の値の扱いは lcms2 と自前の伝達関数で違う:
        // lcms2 は線形の区間を延長し、自前の関数は符号を保って |x| に適用する）。
        let src = [
            [0.18_f32, 0.18, 0.18],
            [0.5, 0.3, 0.2],
            [0.3, 0.4, 0.35],
            [1.0, 1.0, 1.0],
        ];
        let mut dst = [[0.0; 3]; 4];
        t.transform(&src, &mut dst).unwrap();
        for (s, d) in src.iter().zip(&dst) {
            let lin_srgb = m.apply(*s);
            assert!(
                lin_srgb.iter().all(|v| (0.0..=1.0).contains(v)),
                "{s:?} は色域内のはず"
            );
            let expected = lin_srgb.map(crate::transfer::srgb_encode_f32);
            for k in 0..3 {
                assert!(
                    (d[k] - expected[k]).abs() < 5e-4,
                    "{s:?}: {d:?} vs {expected:?}"
                );
            }
        }
    }

    #[test]
    fn v2_and_v4_profiles_convert_the_same() {
        // v2 は伝達関数が 16bit の表になるため、差は ΔE2000 で小さいことを確認する。
        let src = IccProfile::standard(StandardProfile::LinearBt2020).unwrap();
        for kind in [
            StandardProfile::Srgb,
            StandardProfile::AdobeRgb1998,
            StandardProfile::DisplayP3,
        ] {
            let v2 = IccProfile::standard_with_version(kind, IccVersion::V2_4).unwrap();
            let v4 = IccProfile::standard_with_version(kind, IccVersion::V4_3).unwrap();
            let px: Vec<[f32; 3]> = (1..=8)
                .flat_map(|i| (1..=8).map(move |j| [i as f32 / 8.0, j as f32 / 8.0, 0.25]))
                .collect();
            let mut o2 = vec![[0.0; 3]; px.len()];
            let mut o4 = vec![[0.0; 3]; px.len()];
            IccTransform::new(&src, &v2, RenderingIntent::RelativeColorimetric)
                .unwrap()
                .transform(&px, &mut o2)
                .unwrap();
            IccTransform::new(&src, &v4, RenderingIntent::RelativeColorimetric)
                .unwrap()
                .transform(&px, &mut o4)
                .unwrap();
            // それぞれのプロファイルで Lab にして比べる。色域外の色は除く（v2 の表は 0〜1 の外を
            // 表せず、v4 の区分関数は外挿するため、値が違って当然）。
            let l2 = rgb_to_lab(&v2, &o2);
            let l4 = rgb_to_lab(&v4, &o4);
            let mut compared = 0;
            for ((a, b), o) in l2.iter().zip(&l4).zip(&o4) {
                if !o.iter().all(|v| (0.0..=1.0).contains(v)) {
                    continue;
                }
                compared += 1;
                assert!(delta_e2000(*a, *b) < 0.05, "{kind:?}: {a:?} vs {b:?}");
            }
            assert!(compared >= 10, "{kind:?}: 比べた色が少ない（{compared}）");
        }
    }

    #[test]
    fn matrix_extraction() {
        for (kind, space) in [
            (StandardProfile::Srgb, RgbColorSpace::Srgb),
            (StandardProfile::DisplayP3, RgbColorSpace::DisplayP3),
            (StandardProfile::AdobeRgb1998, RgbColorSpace::AdobeRgb1998),
            (StandardProfile::LinearBt2020, RgbColorSpace::Bt2020),
        ] {
            let p = IccProfile::standard(kind).unwrap();
            let m = p.working_to_linear_rgb_matrix().unwrap();
            // ICC の s15Fixed16 の量子化（約 1.5e-5）と、白の定義の違いの分を見込む。
            let d = m.max_abs_diff(&working_to(space));
            assert!(d < 3e-4, "{kind:?}: {d:e}");
            // 白は白。
            let w = m.apply([1.0, 1.0, 1.0]);
            for v in w {
                assert!((v - 1.0).abs() < 3e-4, "{kind:?}: {w:?}");
            }
        }
    }

    #[test]
    fn invalid_bytes_are_rejected() {
        assert!(matches!(
            IccProfile::from_bytes(&[]),
            Err(ColorError::IccInvalid { .. })
        ));
        assert!(matches!(
            IccProfile::from_bytes(&[0u8; 200]),
            Err(ColorError::IccInvalid { .. })
        ));
        // 正しいプロファイルを途中で切ったもの。
        let good = IccProfile::standard(StandardProfile::Srgb).unwrap();
        let truncated = &good.as_bytes()[..good.as_bytes().len() / 2];
        // ヘッダーは正しいので、lcms2 が解析できるかどうかで結果が決まる。パニックしないこと。
        let _ = IccProfile::from_bytes(truncated);
        // シグネチャを壊したもの。
        let mut broken = good.as_bytes().to_vec();
        broken[36] = b'x';
        assert!(matches!(
            IccProfile::from_bytes(&broken),
            Err(ColorError::IccInvalid { .. })
        ));
        // 大きすぎるもの。
        let huge = vec![0u8; MAX_ICC_PROFILE_BYTES + 1];
        assert!(matches!(
            IccProfile::from_bytes(&huge),
            Err(ColorError::IccInvalid { .. })
        ));
    }

    #[test]
    fn garbage_after_valid_header_does_not_panic() {
        // ヘッダーだけ正しく、残りがでたらめなバイト列。
        let good = IccProfile::standard(StandardProfile::Srgb).unwrap();
        let mut bytes = good.as_bytes()[..132].to_vec();
        bytes.extend((0..2000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8));
        if let Ok(p) = IccProfile::from_bytes(&bytes) {
            let _ = IccTransform::new(&p, &good, RenderingIntent::RelativeColorimetric);
            let _ = p.working_to_linear_rgb_matrix();
        }
    }

    #[test]
    fn non_rgb_profile_is_unsupported_for_transform() {
        let lab = IccProfile::from_bytes(&lab_d50_profile().icc().unwrap()).unwrap();
        assert!(!lab.is_rgb());
        assert_eq!(lab.rgb_to_pcs_xyz_matrix(), None);
        let srgb = IccProfile::standard(StandardProfile::Srgb).unwrap();
        assert!(matches!(
            IccTransform::new(&lab, &srgb, RenderingIntent::RelativeColorimetric),
            Err(ColorError::IccUnsupported { .. })
        ));
        assert!(matches!(
            IccTransform::new(&srgb, &lab, RenderingIntent::Perceptual),
            Err(ColorError::IccUnsupported { .. })
        ));
    }

    #[test]
    fn transform_length_mismatch_and_in_place() {
        let a = IccProfile::standard(StandardProfile::Srgb).unwrap();
        let b = IccProfile::standard(StandardProfile::DisplayP3).unwrap();
        let t = IccTransform::new(&a, &b, RenderingIntent::RelativeColorimetric).unwrap();
        let src = [[0.1_f32, 0.2, 0.3]; 3];
        let mut dst = [[0.0_f32; 3]; 2];
        assert!(matches!(
            t.transform(&src, &mut dst),
            Err(ColorError::LengthMismatch { .. })
        ));
        let mut out = [[0.0_f32; 3]; 3];
        t.transform(&src, &mut out).unwrap();
        let mut inplace = src;
        t.transform_in_place(&mut inplace);
        assert_eq!(inplace, out);
        // sRGB の色は P3 の色域内なので、値は 0〜1 に収まる。
        assert!(out.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
    }
}
