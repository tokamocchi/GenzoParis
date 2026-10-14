//! カメラプロファイル（DCP。DEV-17）の読み込みと、色の計算（docs/04_architecture.md の 2.1 節のステージ 8 と、
//! 段階 C のルックテーブル。2.6 節「カメラ RGB から作業色空間への変換」）。
//!
//! 利用者が用意した DCP のファイル（Adobe の DNG の仕様のカメラプロファイル）を読み、行列・色相彩度のマップ・
//! ルックテーブルで色を作る。**Adobe のプロファイルは同梱しない**（利用者が取り込んだものだけを使う）。
//!
//! # ファイルの形
//!
//! DCP は TIFF と同じ形（バイト順の印 `II` / `MM` の後の 16 bit の番号が 42 ではなく `0x4352`（"RC"））で、
//! 最初の IFD に DNG の仕様のタグを持つ。読むタグ（番号は libtiff の `tiff.h` と照合した）:
//!
//! | タグ | 番号 | 使い方 |
//! |---|---|---|
//! | UniqueCameraModel | 50708 | 機種の名前（一覧の絞り込み。[`DcpProfile::matches_camera`]） |
//! | ColorMatrix1 / 2 | 50721 / 50722 | XYZ → カメラ RGB（光源 1 / 2）。**1 は必須** |
//! | CalibrationIlluminant1 / 2 | 50778 / 50779 | 行列・マップの光源（EXIF の LightSource の番号。[`illuminant_temperature`]） |
//! | ProfileCalibrationSignature | 50932 | 表示だけ |
//! | ProfileName | 50936 | 表示の名前 |
//! | ProfileHueSatMapDims | 50937 | 色相・彩度・明度の分割の数 |
//! | ProfileHueSatMapData1 / 2 | 50938 / 50939 | 色相彩度のマップ（光源 1 / 2） |
//! | ProfileToneCurve | 50940 | 読んで表示するだけ（**使わない**。下の「トーンカーブ」） |
//! | ProfileEmbedPolicy | 50941 | 表示だけ（このアプリはプロファイルを出力に埋め込まない） |
//! | ProfileCopyright | 50942 | 表示だけ |
//! | ForwardMatrix1 / 2 | 50964 / 50965 | WB 済みのカメラ RGB → XYZ（D50） |
//! | ProfileLookTableDims / Data | 50981 / 50982 | ルックテーブル（段階 C） |
//! | ProfileHueSatMapEncoding / ProfileLookTableEncoding | 51107 / 51108 | 明度の軸の符号化（0: リニア、1: sRGB） |
//! | BaselineExposureOffset | 51109 | 読んで表示するだけ（**使わない**） |
//! | DefaultBlackRender | 51110 | 表示だけ |
//!
//! 3 つ目の光源（DNG 1.6 の CalibrationIlluminant3 など）は使わない（[`DcpProfile::ignored`] に理由を残す）。
//! 4 色以上のカメラ（行列の要素が 9 個でない）は読まない。
//!
//! # 計算（DNG の仕様の 6 章「カメラの色空間から CIE XYZ への対応」の考え方。式は自前の実装）
//!
//! 入力は、ステージ 3 の WB を掛けたカメラ RGB（B1。被写体の中立色が (1, 1, 1)）と、その WB の係数 `m`
//! （カメラの中立色 `n = 1 / m`）。[`DcpProfile::render`] が次を求める:
//!
//! 1. **白の色度と補間の重み**: D50 から始めて「今の白の相関色温度 T で補間した ColorMatrix の逆行列で `n` を
//!    XYZ にし、その色度を次の白にする」を、変化が `1e−7`（x と y の差の絶対値の和）より小さくなるまで（最大
//!    [`MAX_WHITE_PASSES`] 回。振動したら最後の 2 つの平均）繰り返す。重みは逆色温度で
//!    `g = (1/T − 1/T₂) / (1/T₁ − 1/T₂)`（T₁ < T₂ は 2 つの光源の色温度。T₁ 以下で 1、T₂ 以上で 0）。光源が
//!    1 つ・色温度がわからない・同じなら行列 1 だけを使う。T は [`crate::white_balance::correlated_color_temperature`]。
//! 2. **行列**: 各 ColorMatrix は `max(CM · D50) = 1` に、各 ForwardMatrix は `FM · (1, 1, 1) = D50` に正規化してから
//!    `g` で要素ごとに補間する（ForwardMatrix は補間の後にもう一度正規化する）。
//!    - ForwardMatrix があれば、WB 済みのカメラ RGB → XYZ（D50）は `FM` そのもの。
//!    - なければ `CA · CM⁻¹ · diag(n) / Y`（`CA` は白 → D50 の Bradford の色順応、`Y` は `CM⁻¹ · n` の Y）。
//! 3. **色空間**: XYZ（D50）→ リニア ProPhoto RGB（D50）。色相彩度のマップ（光源が 2 つなら同じ `g` で要素ごとに
//!    補間したもの）はこの空間で当てる（[`HsvTable::apply`]）。その後、ProPhoto → XYZ（D50）→ Bradford（D50 → D65）
//!    → リニア BT.2020（作業色空間、B2）。
//! 4. 中立色が中立色のまま（B1 の (1, 1, 1) が B2 の (1, 1, 1)）になるよう、カメラ → ProPhoto と ProPhoto → BT.2020
//!    の行列は行ごとに行の和で割る（丸めの誤差の分だけ。2.6 節の契約）。
//!
//! 光源が D65 の ColorMatrix だけのプロファイルで、白が D65 のときは、今のステージ 8（[`crate::camera`]）と同じ
//! 行列になる（テストで確かめる）。
//!
//! # 色相彩度のマップ・ルックテーブル（[`HsvTable`]）
//!
//! リニア ProPhoto の RGB を HSV（色相 0〜6、彩度、明度 = 最大のチャンネル）にし、表の色相のずれ（度）・彩度の
//! 倍率・明度の倍率を、色相（一周でつながる）・彩度（0〜1）・明度（分割が 2 以上のとき。符号化が sRGB なら
//! IEC 61966-2-1 で符号化した明度）の軸で線形に補間して当てる。このアプリの決めごと（DNG の仕様は 0〜1 の
//! 入力を想定する。シーンリニアで 1 を超える値・色域外の値を壊さないため）:
//!
//! - 明度は **1 を超えても切りそろえない**（表は端の値を使い、倍率はそのまま掛ける）。0 以下の画素は変えない。
//! - 彩度が 1 を超える（ProPhoto の色域の外。最小のチャンネルが負）画素は、表を彩度 1 の位置で引き、倍率を
//!   掛けた彩度を元の彩度より大きくしない。1 以下の画素は DNG の仕様と同じく 1 を上限にする。
//!
//! # トーンカーブ・露出のずれ（使わない）
//!
//! ProfileToneCurve と BaselineExposureOffset は、Adobe の描画（BaselineExposure を掛けた後の 0〜1 の値に
//! RGB のチャンネルごとに当てる）を前提にしたもの。このアプリのシーン → ディスプレイの変換（ステージ 15。
//! 輝度の基本カーブで、カメラの BaselineExposure は使わない。2.6 節）とは考え方が違うので、読んで表示するだけで
//! 使わない（決定事項ログの profile のパッケージの行）。

use std::fmt;

use crate::error::{ColorError, Result};
use crate::matrix::Mat3;
use crate::space::{
    BRADFORD_D50_TO_D65, BT2020_TO_XYZ, D50, PROPHOTO_TO_XYZ_D50, XYZ_D50_TO_PROPHOTO,
    XYZ_TO_BT2020, Xy, bradford_adaptation,
};
use crate::transfer::{srgb_decode_f32, srgb_encode_f32};
use crate::white_balance::correlated_color_temperature;

/// DCP の TIFF の番号（"RC"。TIFF の 42 の代わり）。
pub const DCP_MAGIC: u16 = 0x4352;

/// 読み込むファイルの大きさの上限（バイト。**仮置き**: たとえば 36 × 8 × 16 の 3D の表でも 4,608 点 × 12 バイト
/// = 55,296 バイトなので、十分に大きい値。壊れた・悪意のあるファイルで大量のメモリを使わないため。02 の SEC-05）。
pub const MAX_DCP_BYTES: usize = 32 << 20;

/// 色相彩度のマップ・ルックテーブルの表の点の数の上限（**仮置き**。1 点 3 個の f32 で 12 MiB）。
pub const MAX_TABLE_ENTRIES: usize = 1 << 20;

/// トーンカーブの点の数の上限（**仮置き**）。
pub const MAX_TONE_CURVE_POINTS: usize = 1 << 16;

/// 文字列のタグの文字数の上限（超えた分は切る）。
pub const MAX_TEXT_CHARS: usize = 1024;

/// IFD の項目の数の上限（**仮置き**）。
const MAX_IFD_ENTRIES: usize = 4096;

/// 白の色度を求める繰り返しの上限（DNG の仕様の手順。[`DcpProfile::render`]）。
pub const MAX_WHITE_PASSES: usize = 30;

/// 白の色度の繰り返しを止める変化の大きさ（x と y の差の絶対値の和）。
const WHITE_CONVERGENCE: f64 = 1e-7;

/// 色相のずれ（度）の絶対値の上限（表の値の検証）。
pub const MAX_HUE_SHIFT_DEG: f32 = 360.0;

/// 色相のずれ（度）を HSV の色相（0〜6）の単位にする倍率（`6 / 360`。f32。GPU 版にも同じ値を渡す）。
pub const HUE_SHIFT_TO_SECTORS: f32 = 6.0 / 360.0;

/// DNG のタグの番号（libtiff の `tiff.h` と照合した）。
pub mod tags {
    /// UniqueCameraModel。
    pub const UNIQUE_CAMERA_MODEL: u16 = 50708;
    /// ColorMatrix1。
    pub const COLOR_MATRIX_1: u16 = 50721;
    /// ColorMatrix2。
    pub const COLOR_MATRIX_2: u16 = 50722;
    /// CalibrationIlluminant1。
    pub const CALIBRATION_ILLUMINANT_1: u16 = 50778;
    /// CalibrationIlluminant2。
    pub const CALIBRATION_ILLUMINANT_2: u16 = 50779;
    /// ProfileCalibrationSignature。
    pub const PROFILE_CALIBRATION_SIGNATURE: u16 = 50932;
    /// ProfileName。
    pub const PROFILE_NAME: u16 = 50936;
    /// ProfileHueSatMapDims。
    pub const PROFILE_HUE_SAT_MAP_DIMS: u16 = 50937;
    /// ProfileHueSatMapData1。
    pub const PROFILE_HUE_SAT_MAP_DATA_1: u16 = 50938;
    /// ProfileHueSatMapData2。
    pub const PROFILE_HUE_SAT_MAP_DATA_2: u16 = 50939;
    /// ProfileToneCurve。
    pub const PROFILE_TONE_CURVE: u16 = 50940;
    /// ProfileEmbedPolicy。
    pub const PROFILE_EMBED_POLICY: u16 = 50941;
    /// ProfileCopyright。
    pub const PROFILE_COPYRIGHT: u16 = 50942;
    /// ForwardMatrix1。
    pub const FORWARD_MATRIX_1: u16 = 50964;
    /// ForwardMatrix2。
    pub const FORWARD_MATRIX_2: u16 = 50965;
    /// ProfileLookTableDims。
    pub const PROFILE_LOOK_TABLE_DIMS: u16 = 50981;
    /// ProfileLookTableData。
    pub const PROFILE_LOOK_TABLE_DATA: u16 = 50982;
    /// ProfileHueSatMapEncoding。
    pub const PROFILE_HUE_SAT_MAP_ENCODING: u16 = 51107;
    /// ProfileLookTableEncoding。
    pub const PROFILE_LOOK_TABLE_ENCODING: u16 = 51108;
    /// BaselineExposureOffset。
    pub const BASELINE_EXPOSURE_OFFSET: u16 = 51109;
    /// DefaultBlackRender。
    pub const DEFAULT_BLACK_RENDER: u16 = 51110;
    /// CalibrationIlluminant3（DNG 1.6。使わない）。
    pub const CALIBRATION_ILLUMINANT_3: u16 = 52529;
    /// ColorMatrix3（DNG 1.6。使わない）。
    pub const COLOR_MATRIX_3: u16 = 52531;
    /// ForwardMatrix3（DNG 1.6。使わない）。
    pub const FORWARD_MATRIX_3: u16 = 52532;
    /// ProfileHueSatMapData3（DNG 1.6。使わない）。
    pub const PROFILE_HUE_SAT_MAP_DATA_3: u16 = 52537;
}

/// CalibrationIlluminant（EXIF の LightSource の番号）の相関色温度（K）。わからない番号（0・255 など）は `None`
/// （そのときは行列 1 だけを使う）。
///
/// 値は、標準の光源はその色温度（A 2850 K、D50 5000 K、D55 5500 K、D65 6500 K、D75 7500 K など）、蛍光灯は
/// 区分の色温度の範囲の中点（昼光色 5700〜7100 K → 6400 K など）。Adobe の DNG SDK の扱いと同じ値と理解して
/// いる（**AI の理解。要確認**）。
pub fn illuminant_temperature(code: u16) -> Option<f64> {
    Some(match code {
        // 標準の光源 A、タングステン（白熱灯）。
        17 | 3 => 2850.0,
        // ISO スタジオ・タングステン。
        24 => 3200.0,
        // D50。
        23 => 5000.0,
        // D55、昼光、晴天、フラッシュ、標準の光源 B。
        20 | 1 | 9 | 4 | 18 => 5500.0,
        // D65、標準の光源 C、曇天。
        21 | 19 | 10 => 6500.0,
        // D75、日陰。
        22 | 11 => 7500.0,
        // 昼光色の蛍光灯（D 5700〜7100 K）。
        12 => (5700.0 + 7100.0) * 0.5,
        // 昼白色の蛍光灯（N 4600〜5500 K）。
        13 => (4600.0 + 5500.0) * 0.5,
        // 白色の蛍光灯（W 3800〜4500 K）と、区分のない蛍光灯。
        14 | 2 => (3800.0 + 4500.0) * 0.5,
        // 温白色の蛍光灯（WW 3250〜3800 K）。
        15 => (3250.0 + 3800.0) * 0.5,
        // 電球色の蛍光灯（L 2600〜3250 K）。
        16 => (2600.0 + 3250.0) * 0.5,
        _ => return None,
    })
}

/// CalibrationIlluminant の名前（表示用）。
pub fn illuminant_name(code: u16) -> String {
    let name = match code {
        1 => "昼光",
        2 => "蛍光灯",
        3 => "タングステン",
        4 => "フラッシュ",
        9 => "晴天",
        10 => "曇天",
        11 => "日陰",
        12 => "昼光色の蛍光灯",
        13 => "昼白色の蛍光灯",
        14 => "白色の蛍光灯",
        15 => "温白色の蛍光灯",
        16 => "電球色の蛍光灯",
        17 => "標準の光源 A",
        18 => "標準の光源 B",
        19 => "標準の光源 C",
        20 => "D55",
        21 => "D65",
        22 => "D75",
        23 => "D50",
        24 => "ISO スタジオ・タングステン",
        255 => "その他",
        _ => "不明",
    };
    format!("{name}（{code}）")
}

// ---------------------------------------------------------------------------
// 色相彩度のマップ・ルックテーブル
// ---------------------------------------------------------------------------

/// 色相彩度のマップ・ルックテーブル（HSV の 3 次元の表。モジュールの doc）。
///
/// 点の並びは DNG の仕様のとおり、明度が最も外側、次に色相、彩度が最も内側（`(v · 色相の数 + h) · 彩度の数 + s`）。
/// 1 点は (色相のずれ（度）, 彩度の倍率, 明度の倍率)。
#[derive(Clone, PartialEq)]
pub struct HsvTable {
    hue_divisions: u32,
    sat_divisions: u32,
    val_divisions: u32,
    srgb_value_encoding: bool,
    entries: Vec<[f32; 3]>,
}

impl fmt::Debug for HsvTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HsvTable")
            .field("hue_divisions", &self.hue_divisions)
            .field("sat_divisions", &self.sat_divisions)
            .field("val_divisions", &self.val_divisions)
            .field("srgb_value_encoding", &self.srgb_value_encoding)
            .finish_non_exhaustive()
    }
}

impl HsvTable {
    /// 分割の数（色相・彩度・明度）・明度の符号化・点から作る。
    ///
    /// エラー: 色相が 0、彩度が 2 未満、明度が 0、点の数が合わない・多すぎる、値が有限でない・色相のずれが
    /// ±[`MAX_HUE_SHIFT_DEG`] を超える・倍率が負。
    pub fn new(dims: [u32; 3], srgb_value_encoding: bool, entries: Vec<[f32; 3]>) -> Result<Self> {
        let [h, s, v] = dims;
        if h == 0 || s < 2 || v == 0 {
            return Err(dcp_error(format!(
                "表の分割の数が不正です（色相 {h}・彩度 {s}・明度 {v}。色相と明度は 1 以上、彩度は 2 以上）"
            )));
        }
        let n = (h as usize)
            .checked_mul(s as usize)
            .and_then(|x| x.checked_mul(v as usize))
            .filter(|&n| n <= MAX_TABLE_ENTRIES)
            .ok_or_else(|| {
                dcp_error(format!(
                    "表が大きすぎます（色相 {h}・彩度 {s}・明度 {v}。点は {MAX_TABLE_ENTRIES} 個まで）"
                ))
            })?;
        if entries.len() != n {
            return Err(dcp_error(format!(
                "表の点の数が分割の数と合いません（{} 個。{n} 個が必要）",
                entries.len()
            )));
        }
        for e in &entries {
            if !(e.iter().all(|x| x.is_finite())
                && e[0].abs() <= MAX_HUE_SHIFT_DEG
                && e[1] >= 0.0
                && e[2] >= 0.0)
            {
                return Err(dcp_error(format!(
                    "表の値が不正です（{e:?}。色相のずれは ±{MAX_HUE_SHIFT_DEG} 度、倍率は 0 以上の有限の値）"
                )));
            }
        }
        Ok(Self {
            hue_divisions: h,
            sat_divisions: s,
            val_divisions: v,
            srgb_value_encoding,
            entries,
        })
    }

    /// 分割の数（色相・彩度・明度）。
    pub fn dims(&self) -> [u32; 3] {
        [self.hue_divisions, self.sat_divisions, self.val_divisions]
    }

    /// 明度の軸を sRGB で符号化して引くか。
    pub fn srgb_value_encoding(&self) -> bool {
        self.srgb_value_encoding
    }

    /// 点（モジュールの doc の並び）。
    pub fn entries(&self) -> &[[f32; 3]] {
        &self.entries
    }

    /// 色相（0〜6）を表の色相の位置にする倍率（色相の分割の数 / 6。分割が 1 なら 0。f32。GPU 版にも同じ値を渡す）。
    pub fn hue_scale(&self) -> f32 {
        if self.hue_divisions < 2 {
            0.0
        } else {
            self.hue_divisions as f32 * (1.0 / 6.0)
        }
    }

    /// 点を f32 の平らな配列として（長さ 3 × 点の数。GPU へのアップロード用）。
    pub fn as_flat(&self) -> &[f32] {
        self.entries.as_flattened()
    }

    /// 何もしない表（すべての点が色相のずれ 0・倍率 1）か。
    pub fn is_identity(&self) -> bool {
        self.entries.iter().all(|e| *e == [0.0, 1.0, 1.0])
    }

    /// 同じ分割の数の 2 つの表を、点ごとに `weight_a`（a の重み。0〜1）で補間する（f64 で計算して f32 に丸める）。
    pub fn interpolate(a: &HsvTable, b: &HsvTable, weight_a: f64) -> Result<HsvTable> {
        if a.dims() != b.dims() || a.srgb_value_encoding != b.srgb_value_encoding {
            return Err(dcp_error("2 つの色相彩度のマップの分割の数が違います"));
        }
        let wa = weight_a.clamp(0.0, 1.0);
        let wb = 1.0 - wa;
        let entries = a
            .entries
            .iter()
            .zip(&b.entries)
            .map(|(x, y)| [0, 1, 2].map(|k| (wa * f64::from(x[k]) + wb * f64::from(y[k])) as f32))
            .collect();
        HsvTable::new(a.dims(), a.srgb_value_encoding, entries)
    }

    #[inline]
    fn at(&self, v: usize, h: usize, s: usize) -> [f32; 3] {
        self.entries[(v * self.hue_divisions as usize + h) * self.sat_divisions as usize + s]
    }

    /// 1 画素（リニア ProPhoto RGB）に当てる（モジュールの doc の決めごと。f32 で、GPU 版も同じ式・順序）。
    ///
    /// ```text
    /// v = max(r, g, b)。v ≤ 0（NaN を含む）なら変えない
    /// gap = v − min(r, g, b)。gap > 0 なら
    ///   h = (g − b) / gap（r が最大。負なら +6）、2 + (b − r) / gap（g が最大）、4 + (r − g) / gap（b が最大）
    ///   s = gap / v
    /// でなければ h = 0、s = 0
    /// 表を引く（色相: h · 色相の数 / 6（色相の数が 1 なら 0。[`hue_scale`](Self::hue_scale)）、彩度: min(s, 1) · (彩度の数 − 1)、
    ///   明度: 分割が 2 以上なら clamp(V, 0, 1) · (明度の数 − 1)（V は符号化が sRGB なら sRGB で符号化した v）。
    ///   色相は最後の分割の次を 0 にする（一周でつながる）。重みは 1 − 小数部・小数部で、明度 → 色相 → 彩度の
    ///   順の線形補間）→ (dh, ks, kv)
    /// h' = h + dh · 6 / 360（[`HUE_SHIFT_TO_SECTORS`]）
    /// s' = min(s · ks, 1)（s ≤ 1）、min(s · ks, s)（s > 1。色域の外の色の彩度を上げない）
    /// v' = srgb_decode(srgb_encode(v) · kv)（符号化が sRGB）、v · kv（リニア）
    /// RGB = HSV → RGB（h' を 0〜6 に折り返す。s' = 0 なら (v', v', v')）
    /// ```
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        // GPU 版は入力が有限であることを前提にする（パイプラインの経路では常に満たす）。
        if !rgb.iter().all(|x| x.is_finite()) {
            return rgb;
        }
        let [r, g, b] = rgb;
        let v = r.max(g).max(b);
        if v <= 0.0 {
            return rgb;
        }
        let gap = v - r.min(g).min(b);
        let (h, s) = if gap > 0.0 {
            let h = if r == v {
                let h = (g - b) / gap;
                if h < 0.0 { h + 6.0 } else { h }
            } else if g == v {
                2.0 + (b - r) / gap
            } else {
                4.0 + (r - g) / gap
            };
            (h, gap / v)
        } else {
            (0.0, 0.0)
        };
        let [dh, ks, kv] = self.lookup(h, s.min(1.0), v);
        let h2 = h + dh * HUE_SHIFT_TO_SECTORS;
        let s2 = if s <= 1.0 {
            (s * ks).min(1.0)
        } else {
            (s * ks).min(s)
        };
        let v2 = if self.srgb_value_encoding {
            srgb_decode_f32(srgb_encode_f32(v) * kv)
        } else {
            v * kv
        };
        hsv_to_rgb(h2, s2, v2)
    }

    /// 表を引く（[`apply`](Self::apply) の doc の式）。`s` は 0〜1。
    fn lookup(&self, h: f32, s: f32, v: f32) -> [f32; 3] {
        let hue_n = self.hue_divisions;
        let h_scaled = h * self.hue_scale();
        let s_scaled = s * (self.sat_divisions - 1) as f32;
        let mut h0 = (h_scaled.max(0.0) as u32).min(hue_n - 1) as usize;
        let mut h1 = h0 + 1;
        if h0 >= (hue_n - 1) as usize {
            h0 = (hue_n - 1) as usize;
            h1 = 0;
        }
        let s0 = (s_scaled.max(0.0) as u32).min(self.sat_divisions - 2) as usize;
        let hf1 = h_scaled - h0 as f32;
        let sf1 = s_scaled - s0 as f32;
        let hf0 = 1.0 - hf1;
        let sf0 = 1.0 - sf1;
        // 明度の 1 つの面の中の双線形（色相 → 彩度の順）。
        let plane = |vi: usize| -> [f32; 3] {
            let e00 = self.at(vi, h0, s0);
            let e01 = self.at(vi, h1, s0);
            let e10 = self.at(vi, h0, s0 + 1);
            let e11 = self.at(vi, h1, s0 + 1);
            [0, 1, 2].map(|k| {
                let a = hf0 * e00[k] + hf1 * e01[k];
                let c = hf0 * e10[k] + hf1 * e11[k];
                sf0 * a + sf1 * c
            })
        };
        if self.val_divisions < 2 {
            return plane(0);
        }
        let ve = if self.srgb_value_encoding {
            srgb_encode_f32(v)
        } else {
            v
        };
        let v_scaled = ve.clamp(0.0, 1.0) * (self.val_divisions - 1) as f32;
        let v0 = (v_scaled as u32).min(self.val_divisions - 2) as usize;
        let vf1 = v_scaled - v0 as f32;
        let vf0 = 1.0 - vf1;
        let p0 = plane(v0);
        let p1 = plane(v0 + 1);
        [0, 1, 2].map(|k| vf0 * p0[k] + vf1 * p1[k])
    }
}

/// HSV（色相 0〜6）→ RGB（[`HsvTable::apply`]。色相は 0〜6 に折り返す）。
#[inline]
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    if s.is_nan() || s <= 0.0 {
        return [v, v, v];
    }
    let mut h = h;
    if h < 0.0 {
        h += 6.0;
    }
    if h >= 6.0 {
        h -= 6.0;
    }
    // 大きなずれ（±360 度）でも 0〜6 に入るよう、もう一度折り返す。
    if h < 0.0 {
        h += 6.0;
    }
    if h >= 6.0 {
        h -= 6.0;
    }
    let i = (h.max(0.0) as u32).min(5);
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

// ---------------------------------------------------------------------------
// プロファイル
// ---------------------------------------------------------------------------

/// DCP のカメラプロファイル（読み込んだ値。モジュールの doc の表）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DcpProfile {
    /// ProfileName。
    pub name: Option<String>,
    /// UniqueCameraModel。
    pub unique_camera_model: Option<String>,
    /// ProfileCopyright。
    pub copyright: Option<String>,
    /// ProfileCalibrationSignature。
    pub calibration_signature: Option<String>,
    /// ProfileEmbedPolicy（0: コピーを許す、1: 使ったときだけ埋め込む、2: 埋め込まない、3: 制限なし）。
    pub embed_policy: Option<u32>,
    /// CalibrationIlluminant1 / 2。
    pub calibration_illuminants: [Option<u16>; 2],
    /// ColorMatrix1 / 2（XYZ → カメラ RGB。読んだ値のまま）。1 は必須。
    pub color_matrices: [Option<Mat3>; 2],
    /// ForwardMatrix1 / 2（WB 済みのカメラ RGB → XYZ（D50）。読んだ値のまま）。
    pub forward_matrices: [Option<Mat3>; 2],
    /// ProfileHueSatMapData1 / 2（同じ分割の数・符号化）。
    pub hue_sat_maps: [Option<HsvTable>; 2],
    /// ProfileLookTableData。
    pub look_table: Option<HsvTable>,
    /// ProfileToneCurve（(入力, 出力) の点。使わない）。
    pub tone_curve: Option<Vec<[f32; 2]>>,
    /// BaselineExposureOffset（EV。使わない）。
    pub baseline_exposure_offset: Option<f64>,
    /// DefaultBlackRender（0: 自動、1: なし。使わない）。
    pub default_black_render: Option<u32>,
    /// 読んだが使わない情報の説明（3 つ目の光源など）。
    pub ignored: Vec<String>,
}

/// [`DcpProfile::render`] の結果（ステージ 8 とルックテーブルに使う値）。
#[derive(Debug, Clone, PartialEq)]
pub struct DcpRendering {
    /// WB 済みのカメラ RGB（B1）→ リニア ProPhoto RGB（D50）。中立色が (1, 1, 1) になるよう行ごとに正規化したもの。
    pub camera_to_prophoto: Mat3,
    /// リニア ProPhoto RGB（D50）→ 作業色空間（リニア BT.2020・D65。B2）。中立色が (1, 1, 1) になるもの。
    pub prophoto_to_working: Mat3,
    /// 色相彩度のマップ（光源の重みで補間したもの）。ない・何もしない表なら `None`。
    pub hue_sat_map: Option<HsvTable>,
    /// 白の色度（カメラの中立色をプロファイルの行列で XYZ にしたもの）。
    pub white: Xy,
    /// 白の相関色温度（K）。
    pub temperature_k: f64,
    /// 光源 1 の行列・マップの重み（0〜1。光源が 1 つなら 1）。
    pub weight1: f64,
    /// ForwardMatrix を使ったか。
    pub uses_forward_matrix: bool,
}

impl DcpRendering {
    /// 色相彩度のマップがなければ、カメラ RGB → 作業色空間の 1 つの行列（今のステージ 8 と同じ形）。
    pub fn camera_to_working(&self) -> Mat3 {
        self.prophoto_to_working.mul_mat(&self.camera_to_prophoto)
    }
}

impl DcpProfile {
    /// DCP のファイルの中身を読む（モジュールの doc の表。検証もする）。
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_DCP_BYTES {
            return Err(dcp_error(format!(
                "ファイルが大きすぎます（{} バイト。{MAX_DCP_BYTES} バイトまで）",
                bytes.len()
            )));
        }
        let ifd = read_ifd(bytes)?;
        let mut p = DcpProfile::default();
        let mut hsm_dims: Option<[u32; 3]> = None;
        let mut hsm_data: [Option<Vec<[f32; 3]>>; 2] = [None, None];
        let mut hsm_encoding = 0u32;
        let mut look_dims: Option<[u32; 3]> = None;
        let mut look_data: Option<Vec<[f32; 3]>> = None;
        let mut look_encoding = 0u32;
        for e in &ifd.entries {
            use tags::*;
            match e.tag {
                UNIQUE_CAMERA_MODEL => p.unique_camera_model = ifd.text(e)?,
                PROFILE_NAME => p.name = ifd.text(e)?,
                PROFILE_COPYRIGHT => p.copyright = ifd.text(e)?,
                PROFILE_CALIBRATION_SIGNATURE => p.calibration_signature = ifd.text(e)?,
                PROFILE_EMBED_POLICY => p.embed_policy = Some(ifd.single_uint(e)?),
                CALIBRATION_ILLUMINANT_1 => {
                    p.calibration_illuminants[0] = Some(illuminant_code(ifd.single_uint(e)?)?);
                }
                CALIBRATION_ILLUMINANT_2 => {
                    p.calibration_illuminants[1] = Some(illuminant_code(ifd.single_uint(e)?)?);
                }
                COLOR_MATRIX_1 => p.color_matrices[0] = Some(ifd.matrix(e, "ColorMatrix1")?),
                COLOR_MATRIX_2 => p.color_matrices[1] = Some(ifd.matrix(e, "ColorMatrix2")?),
                FORWARD_MATRIX_1 => p.forward_matrices[0] = Some(ifd.matrix(e, "ForwardMatrix1")?),
                FORWARD_MATRIX_2 => p.forward_matrices[1] = Some(ifd.matrix(e, "ForwardMatrix2")?),
                PROFILE_HUE_SAT_MAP_DIMS => hsm_dims = Some(ifd.dims(e, "ProfileHueSatMapDims")?),
                PROFILE_HUE_SAT_MAP_DATA_1 => hsm_data[0] = Some(ifd.triples(e)?),
                PROFILE_HUE_SAT_MAP_DATA_2 => hsm_data[1] = Some(ifd.triples(e)?),
                PROFILE_HUE_SAT_MAP_ENCODING => hsm_encoding = ifd.single_uint(e)?,
                PROFILE_LOOK_TABLE_DIMS => look_dims = Some(ifd.dims(e, "ProfileLookTableDims")?),
                PROFILE_LOOK_TABLE_DATA => look_data = Some(ifd.triples(e)?),
                PROFILE_LOOK_TABLE_ENCODING => look_encoding = ifd.single_uint(e)?,
                PROFILE_TONE_CURVE => p.tone_curve = Some(ifd.tone_curve(e)?),
                BASELINE_EXPOSURE_OFFSET => {
                    let v = ifd.rationals(e)?;
                    if v.len() != 1 || !v[0].is_finite() {
                        return Err(dcp_error("BaselineExposureOffset が不正です"));
                    }
                    p.baseline_exposure_offset = Some(v[0]);
                }
                DEFAULT_BLACK_RENDER => p.default_black_render = Some(ifd.single_uint(e)?),
                CALIBRATION_ILLUMINANT_3
                | COLOR_MATRIX_3
                | FORWARD_MATRIX_3
                | PROFILE_HUE_SAT_MAP_DATA_3 => {
                    let note =
                        "3 つ目の光源の情報（DNG 1.6）は使いません（光源 1・2 だけで補間します）";
                    if !p.ignored.iter().any(|n| n == note) {
                        p.ignored.push(note.to_owned());
                    }
                }
                _ => {}
            }
        }
        let encoding = |v: u32, what: &str| -> Result<bool> {
            match v {
                0 => Ok(false),
                1 => Ok(true),
                other => Err(dcp_error(format!(
                    "{what} の値 {other} に対応していません（0: リニア、1: sRGB）"
                ))),
            }
        };
        let srgb = encoding(hsm_encoding, "ProfileHueSatMapEncoding")?;
        for (k, data) in hsm_data.into_iter().enumerate() {
            if let Some(data) = data {
                let dims = hsm_dims.ok_or_else(|| {
                    dcp_error(
                        "ProfileHueSatMapData がありますが、ProfileHueSatMapDims がありません",
                    )
                })?;
                p.hue_sat_maps[k] = Some(HsvTable::new(dims, srgb, data)?);
            }
        }
        if let Some(data) = look_data {
            let dims = look_dims.ok_or_else(|| {
                dcp_error("ProfileLookTableData がありますが、ProfileLookTableDims がありません")
            })?;
            let srgb = encoding(look_encoding, "ProfileLookTableEncoding")?;
            p.look_table = Some(HsvTable::new(dims, srgb, data)?);
        }
        p.validate()?;
        Ok(p)
    }

    /// 使えるプロファイルか確かめる（ColorMatrix1 がある、光源 2 の行列には光源 2 の番号がある、ForwardMatrix は
    /// 対応する ColorMatrix がある、行列が有限で逆行列を持つ、2 つのマップの分割の数が同じ）。
    pub fn validate(&self) -> Result<()> {
        let Some(cm1) = &self.color_matrices[0] else {
            return Err(dcp_error("ColorMatrix1 がありません"));
        };
        check_matrix(cm1, "ColorMatrix1")?;
        if let Some(cm2) = &self.color_matrices[1] {
            check_matrix(cm2, "ColorMatrix2")?;
        }
        for (k, fm) in self.forward_matrices.iter().enumerate() {
            if let Some(fm) = fm {
                check_matrix(
                    fm,
                    if k == 0 {
                        "ForwardMatrix1"
                    } else {
                        "ForwardMatrix2"
                    },
                )?;
                if self.color_matrices[k].is_none() {
                    return Err(dcp_error(format!(
                        "ForwardMatrix{} に対応する ColorMatrix{} がありません",
                        k + 1,
                        k + 1
                    )));
                }
            }
        }
        if let [Some(a), Some(b)] = &self.hue_sat_maps
            && (a.dims() != b.dims() || a.srgb_value_encoding != b.srgb_value_encoding)
        {
            return Err(dcp_error("2 つの色相彩度のマップの分割の数が違います"));
        }
        if let Some(curve) = &self.tone_curve
            && (curve.len() < 2 || curve.len() > MAX_TONE_CURVE_POINTS)
        {
            return Err(dcp_error(format!(
                "ProfileToneCurve の点の数が不正です（{} 個。2〜{MAX_TONE_CURVE_POINTS} 個）",
                curve.len()
            )));
        }
        Ok(())
    }

    /// 表示の名前（ProfileName。なければ `None`）。
    pub fn display_name(&self) -> Option<&str> {
        self.name.as_deref().filter(|s| !s.trim().is_empty())
    }

    /// 2 つの光源で補間するか（ColorMatrix2 があり、2 つの光源の色温度がわかって違う）。
    pub fn is_dual_illuminant(&self) -> bool {
        self.illuminant_temperatures().is_some()
    }

    /// 2 つの光源の色温度（補間するときだけ）。
    fn illuminant_temperatures(&self) -> Option<(f64, f64)> {
        self.color_matrices[1]?;
        let t1 = illuminant_temperature(self.calibration_illuminants[0]?)?;
        let t2 = illuminant_temperature(self.calibration_illuminants[1]?)?;
        (t1 > 0.0 && t2 > 0.0 && t1 != t2).then_some((t1, t2))
    }

    /// 白の色度 `xy` での光源 1 の重みと、白の相関色温度。
    fn weight_at(&self, xy: Xy) -> Result<(f64, f64)> {
        let t = correlated_color_temperature(xy)?;
        let Some((t1, t2)) = self.illuminant_temperatures() else {
            return Ok((1.0, t));
        };
        // 光源 1 が低い色温度とは限らないので、低い方を基準に重みを求めてから光源 1 の重みに直す。
        let (lo, hi) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
        let g_lo = if t <= lo {
            1.0
        } else if t >= hi {
            0.0
        } else {
            (1.0 / t - 1.0 / hi) / (1.0 / lo - 1.0 / hi)
        };
        Ok((if t1 < t2 { g_lo } else { 1.0 - g_lo }, t))
    }

    /// 重み `w1` で補間した ColorMatrix（正規化したもの）。
    fn color_matrix_at(&self, w1: f64) -> Result<Mat3> {
        let cm1 = normalize_color_matrix(self.color_matrices[0].as_ref().expect("検証済み"))?;
        match (&self.color_matrices[1], w1 < 1.0) {
            (Some(cm2), true) => Ok(lerp_mat(&cm1, &normalize_color_matrix(cm2)?, w1)),
            _ => Ok(cm1),
        }
    }

    /// 重み `w1` で補間した ForwardMatrix（正規化したもの。使える ForwardMatrix がなければ `None`）。
    fn forward_matrix_at(&self, w1: f64) -> Result<Option<Mat3>> {
        let dual = self.illuminant_temperatures().is_some();
        let fm = match (&self.forward_matrices, dual) {
            ([Some(a), Some(b)], true) => Some(lerp_mat(
                &normalize_forward_matrix(a)?,
                &normalize_forward_matrix(b)?,
                w1,
            )),
            // 補間しないなら行列 1 だけを使う（ForwardMatrix2 だけのときは使わない）。
            ([Some(a), _], false) => Some(*a),
            // 光源が 2 つなのに片方しかない: 使わない（ColorMatrix で計算する）。
            _ => None,
        };
        fm.map(|m| normalize_forward_matrix(&m)).transpose()
    }

    /// カメラの中立色 `neutral`（WB の係数の逆数。尺度は任意）での、ステージ 8 の行列と色相彩度のマップ
    /// （モジュールの doc の「計算」）。
    pub fn render(&self, neutral: [f64; 3]) -> Result<DcpRendering> {
        self.validate()?;
        if !neutral.iter().all(|v| v.is_finite() && *v > 0.0) {
            return Err(dcp_error(
                "カメラの中立色が不正です（WB の係数が 0 以下か有限でない）",
            ));
        }
        let (white, w1, temperature_k) = self.find_white(neutral)?;
        let d50 = D50.to_xyz(1.0)?;
        let (camera_to_xyz_d50, uses_forward_matrix) = match self.forward_matrix_at(w1)? {
            Some(fm) => (fm, true),
            None => {
                let cm = self.color_matrix_at(w1)?;
                let cam_to_xyz = cm.inverse()?;
                let white_xyz = cam_to_xyz.apply(neutral);
                if !(white_xyz[1].is_finite() && white_xyz[1] > 0.0) {
                    return Err(dcp_error("カメラの中立色の明るさが 0 以下になります"));
                }
                let k = 1.0 / white_xyz[1];
                let white_xyz = white_xyz.map(|v| v * k);
                let ca = bradford_adaptation(white_xyz, d50)?;
                (
                    ca.mul_mat(&cam_to_xyz)
                        .mul_mat(&Mat3::diagonal(neutral))
                        .scale(k),
                    false,
                )
            }
        };
        let camera_to_prophoto = normalize_rows(&XYZ_D50_TO_PROPHOTO.mul_mat(&camera_to_xyz_d50))?;
        let hue_sat_map = match &self.hue_sat_maps {
            [Some(a), Some(b)] if self.illuminant_temperatures().is_some() => {
                Some(HsvTable::interpolate(a, b, w1)?)
            }
            [Some(a), _] => Some(a.clone()),
            [None, Some(b)] => Some(b.clone()),
            [None, None] => None,
        }
        .filter(|m| !m.is_identity());
        Ok(DcpRendering {
            camera_to_prophoto,
            prophoto_to_working: prophoto_to_working()?,
            hue_sat_map,
            white,
            temperature_k,
            weight1: w1,
            uses_forward_matrix,
        })
    }

    /// 白の色度・光源 1 の重み・相関色温度を求める（モジュールの doc の手順 1）。
    fn find_white(&self, neutral: [f64; 3]) -> Result<(Xy, f64, f64)> {
        let mut last = D50;
        for pass in 0..MAX_WHITE_PASSES {
            let (w1, _) = self.weight_at(last)?;
            let cm = self.color_matrix_at(w1)?;
            let xyz = cm.inverse()?.apply(neutral);
            let mut next = Xy::from_xyz(xyz).map_err(|_| {
                dcp_error("カメラの中立色をプロファイルの行列で XYZ にすると、色度を求められません")
            })?;
            if !(next.x.is_finite() && next.y.is_finite() && next.y > 0.0) {
                return Err(dcp_error("カメラの中立色の色度が不正になります"));
            }
            if (next.x - last.x).abs() + (next.y - last.y).abs() < WHITE_CONVERGENCE {
                last = next;
                break;
            }
            if pass == MAX_WHITE_PASSES - 1 {
                // 2 つの値を行き来している: 平均を使う。
                next = Xy::new(0.5 * (last.x + next.x), 0.5 * (last.y + next.y));
            }
            last = next;
        }
        let (w1, t) = self.weight_at(last)?;
        Ok((last, w1, t))
    }

    /// 写真の機種 `camera`（表示の名前。例: `SONY ILCE-7M4`）用のプロファイルか（UniqueCameraModel と比べる）。
    ///
    /// 大文字・小文字と空白の数を区別せずに比べ、同じか、長い方が「空白 + 短い方」で終わる（例: `ILCE-7M4` と
    /// `Sony ILCE-7M4`）なら同じ機種とみなす。UniqueCameraModel がなければ `false`。
    pub fn matches_camera(&self, camera: &str) -> bool {
        self.unique_camera_model
            .as_deref()
            .is_some_and(|m| camera_names_match(m, camera))
    }

    /// DCP のファイルの中身（リトルエンディアンの TIFF。**テスト・検証用**: このアプリは DCP を書き出す機能を
    /// 持たないが、テストの中で DCP を作るために使う）。値の範囲は検証しない（不正なファイルも作れる）。
    pub fn to_bytes(&self) -> Vec<u8> {
        self.write(false)
    }

    /// [`to_bytes`](Self::to_bytes) のビッグエンディアン版（テスト用）。
    pub fn to_bytes_big_endian(&self) -> Vec<u8> {
        self.write(true)
    }

    fn write(&self, big_endian: bool) -> Vec<u8> {
        use tags::*;
        let mut w = TiffWriter::new(big_endian);
        if let Some(s) = &self.unique_camera_model {
            w.ascii(UNIQUE_CAMERA_MODEL, s);
        }
        for (k, tag) in [(0, COLOR_MATRIX_1), (1, COLOR_MATRIX_2)] {
            if let Some(m) = &self.color_matrices[k] {
                w.srationals(tag, &m.rows().concat());
            }
        }
        for (k, tag) in [(0, CALIBRATION_ILLUMINANT_1), (1, CALIBRATION_ILLUMINANT_2)] {
            if let Some(c) = self.calibration_illuminants[k] {
                w.short(tag, c);
            }
        }
        if let Some(s) = &self.calibration_signature {
            w.ascii(PROFILE_CALIBRATION_SIGNATURE, s);
        }
        if let Some(s) = &self.name {
            w.ascii(PROFILE_NAME, s);
        }
        let hsm = self.hue_sat_maps.iter().flatten().next();
        if let Some(m) = hsm {
            w.longs(PROFILE_HUE_SAT_MAP_DIMS, &m.dims());
        }
        for (k, tag) in [
            (0, PROFILE_HUE_SAT_MAP_DATA_1),
            (1, PROFILE_HUE_SAT_MAP_DATA_2),
        ] {
            if let Some(m) = &self.hue_sat_maps[k] {
                w.floats(tag, m.as_flat());
            }
        }
        if let Some(c) = &self.tone_curve {
            w.floats(PROFILE_TONE_CURVE, c.as_flattened());
        }
        if let Some(v) = self.embed_policy {
            w.longs(PROFILE_EMBED_POLICY, &[v]);
        }
        if let Some(s) = &self.copyright {
            w.ascii(PROFILE_COPYRIGHT, s);
        }
        for (k, tag) in [(0, FORWARD_MATRIX_1), (1, FORWARD_MATRIX_2)] {
            if let Some(m) = &self.forward_matrices[k] {
                w.srationals(tag, &m.rows().concat());
            }
        }
        if let Some(t) = &self.look_table {
            w.longs(PROFILE_LOOK_TABLE_DIMS, &t.dims());
            w.floats(PROFILE_LOOK_TABLE_DATA, t.as_flat());
        }
        if let Some(m) = hsm {
            w.longs(
                PROFILE_HUE_SAT_MAP_ENCODING,
                &[u32::from(m.srgb_value_encoding)],
            );
        }
        if let Some(t) = &self.look_table {
            w.longs(
                PROFILE_LOOK_TABLE_ENCODING,
                &[u32::from(t.srgb_value_encoding)],
            );
        }
        if let Some(v) = self.baseline_exposure_offset {
            w.srationals(BASELINE_EXPOSURE_OFFSET, &[v]);
        }
        if let Some(v) = self.default_black_render {
            w.longs(DEFAULT_BLACK_RENDER, &[v]);
        }
        w.finish()
    }
}

/// 2 つの機種の名前が同じ機種を指すか（[`DcpProfile::matches_camera`] の規則）。
pub fn camera_names_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    short.chars().count() >= 2 && long.ends_with(&format!(" {short}"))
}

/// リニア ProPhoto RGB（D50）→ 作業色空間（リニア BT.2020・D65）の行列（Bradford の色順応。中立色が (1, 1, 1)
/// になるよう行ごとに正規化したもの）。
pub fn prophoto_to_working() -> Result<Mat3> {
    normalize_rows(
        &XYZ_TO_BT2020
            .mul_mat(&BRADFORD_D50_TO_D65)
            .mul_mat(&PROPHOTO_TO_XYZ_D50),
    )
}

/// 作業色空間 → リニア ProPhoto RGB の行列（[`prophoto_to_working`] の逆。中立色が (1, 1, 1)）。
pub fn working_to_prophoto() -> Result<Mat3> {
    normalize_rows(
        &XYZ_D50_TO_PROPHOTO
            .mul_mat(&crate::space::BRADFORD_D65_TO_D50)
            .mul_mat(&BT2020_TO_XYZ),
    )
}

/// 行ごとに行の和で割る（中立色 (1, 1, 1) を (1, 1, 1) にする）。行の和が 0 以下ならエラー。
fn normalize_rows(m: &Mat3) -> Result<Mat3> {
    let mut rows = m.rows();
    for row in &mut rows {
        let sum = row[0] + row[1] + row[2];
        if !(sum.is_finite() && sum > 0.0) {
            return Err(dcp_error(
                "中立色の行き先が不正になります（行列の行の和が 0 以下）",
            ));
        }
        for v in row.iter_mut() {
            *v /= sum;
        }
    }
    let out = Mat3::from_rows(rows);
    out.inverse()?;
    Ok(out)
}

/// ColorMatrix を `max(CM · D50) = 1` に正規化する。
fn normalize_color_matrix(m: &Mat3) -> Result<Mat3> {
    let d50 = D50.to_xyz(1.0)?;
    let c = m.apply(d50);
    let max = c[0].max(c[1]).max(c[2]);
    if !(max.is_finite() && max > 0.0) {
        return Err(dcp_error("ColorMatrix で D50 の白が正の値になりません"));
    }
    Ok(m.scale(1.0 / max))
}

/// ForwardMatrix を `FM · (1, 1, 1) = D50` に正規化する（行ごとに倍率を掛ける）。
fn normalize_forward_matrix(m: &Mat3) -> Result<Mat3> {
    let d50 = D50.to_xyz(1.0)?;
    let xyz = m.apply([1.0, 1.0, 1.0]);
    if !xyz.iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err(dcp_error("ForwardMatrix で中立色が正の XYZ になりません"));
    }
    Ok(Mat3::diagonal([d50[0] / xyz[0], d50[1] / xyz[1], d50[2] / xyz[2]]).mul_mat(m))
}

/// 要素ごとの線形補間（`a` の重み `wa`）。
fn lerp_mat(a: &Mat3, b: &Mat3, wa: f64) -> Mat3 {
    let (ra, rb) = (a.rows(), b.rows());
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = wa * ra[i][j] + (1.0 - wa) * rb[i][j];
        }
    }
    Mat3::from_rows(out)
}

fn check_matrix(m: &Mat3, what: &str) -> Result<()> {
    if !m.is_finite() {
        return Err(dcp_error(format!("{what} に有限でない値があります")));
    }
    if m.checked_inverse().is_none() {
        return Err(dcp_error(format!("{what} が特異です（逆行列がない）")));
    }
    Ok(())
}

fn illuminant_code(v: u32) -> Result<u16> {
    u16::try_from(v).map_err(|_| dcp_error(format!("CalibrationIlluminant の値 {v} が不正です")))
}

fn dcp_error(reason: impl Into<String>) -> ColorError {
    ColorError::DcpInvalid {
        reason: reason.into(),
    }
}

// ---------------------------------------------------------------------------
// TIFF の読み書き（DCP の範囲だけ）
// ---------------------------------------------------------------------------

/// TIFF の型の番号。
mod ty {
    pub const BYTE: u16 = 1;
    pub const ASCII: u16 = 2;
    pub const SHORT: u16 = 3;
    pub const LONG: u16 = 4;
    pub const RATIONAL: u16 = 5;
    pub const UNDEFINED: u16 = 7;
    pub const SRATIONAL: u16 = 10;
    pub const FLOAT: u16 = 11;
    pub const DOUBLE: u16 = 12;
}

fn type_size(t: u16) -> Option<usize> {
    Some(match t {
        ty::BYTE | ty::ASCII | ty::UNDEFINED => 1,
        ty::SHORT => 2,
        ty::LONG | ty::FLOAT => 4,
        ty::RATIONAL | ty::SRATIONAL | ty::DOUBLE => 8,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: u16,
    ty: u16,
    count: usize,
    /// 値の始まり（ファイルの中の位置）。
    offset: usize,
}

struct Ifd<'a> {
    bytes: &'a [u8],
    big_endian: bool,
    entries: Vec<Entry>,
}

fn read_ifd(bytes: &[u8]) -> Result<Ifd<'_>> {
    if bytes.len() < 8 {
        return Err(dcp_error("ファイルが短すぎます"));
    }
    let big_endian = match &bytes[..2] {
        b"II" => false,
        b"MM" => true,
        _ => return Err(dcp_error("TIFF のバイト順の印（II / MM）がありません")),
    };
    let rd = Reader { bytes, big_endian };
    let magic = rd.u16(2)?;
    if magic != DCP_MAGIC {
        return Err(dcp_error(if magic == 42 {
            "DCP ではなく TIFF / DNG のファイルです（DCP の印 0x4352 がない）".to_owned()
        } else {
            format!("DCP の印（0x4352）がありません（0x{magic:04x}）")
        }));
    }
    let ifd_off = rd.u32(4)? as usize;
    let count = rd.u16(ifd_off)? as usize;
    if count > MAX_IFD_ENTRIES {
        return Err(dcp_error(format!("IFD の項目が多すぎます（{count} 個）")));
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(count);
    for i in 0..count {
        let at = ifd_off
            .checked_add(2 + i * 12)
            .ok_or_else(|| dcp_error("IFD の位置が不正です"))?;
        let tag = rd.u16(at)?;
        let ty = rd.u16(at + 2)?;
        let count = rd.u32(at + 4)? as usize;
        let Some(size) = type_size(ty) else {
            // 知らない型のタグは読まない。
            continue;
        };
        let total = count
            .checked_mul(size)
            .filter(|&n| n <= MAX_DCP_BYTES)
            .ok_or_else(|| dcp_error(format!("タグ {tag} の値が大きすぎます")))?;
        let offset = if total <= 4 {
            at + 8
        } else {
            rd.u32(at + 8)? as usize
        };
        if offset
            .checked_add(total)
            .is_none_or(|end| end > bytes.len())
        {
            return Err(dcp_error(format!(
                "タグ {tag} の値がファイルの外を指しています"
            )));
        }
        // 同じタグが 2 つあれば最初のものを使う。
        if entries.iter().all(|e| e.tag != tag) {
            entries.push(Entry {
                tag,
                ty,
                count,
                offset,
            });
        }
    }
    Ok(Ifd {
        bytes,
        big_endian,
        entries,
    })
}

#[derive(Clone, Copy)]
struct Reader<'a> {
    bytes: &'a [u8],
    big_endian: bool,
}

impl Reader<'_> {
    fn slice<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        at.checked_add(N)
            .and_then(|end| self.bytes.get(at..end))
            .map(|s| s.try_into().expect("長さ N"))
            .ok_or_else(|| dcp_error("ファイルが途中で終わっています"))
    }
    fn u16(&self, at: usize) -> Result<u16> {
        let b = self.slice::<2>(at)?;
        Ok(if self.big_endian {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }
    fn u32(&self, at: usize) -> Result<u32> {
        let b = self.slice::<4>(at)?;
        Ok(if self.big_endian {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }
    fn u64(&self, at: usize) -> Result<u64> {
        let b = self.slice::<8>(at)?;
        Ok(if self.big_endian {
            u64::from_be_bytes(b)
        } else {
            u64::from_le_bytes(b)
        })
    }
}

impl Ifd<'_> {
    fn reader(&self) -> Reader<'_> {
        Reader {
            bytes: self.bytes,
            big_endian: self.big_endian,
        }
    }

    /// 整数の値（BYTE・SHORT・LONG）。
    fn uints(&self, e: &Entry) -> Result<Vec<u32>> {
        let rd = self.reader();
        let size = type_size(e.ty).unwrap_or(1);
        (0..e.count)
            .map(|i| {
                let at = e.offset + i * size;
                match e.ty {
                    ty::BYTE => Ok(u32::from(self.bytes[at])),
                    ty::SHORT => rd.u16(at).map(u32::from),
                    ty::LONG => rd.u32(at),
                    _ => Err(dcp_error(format!(
                        "タグ {} の型が整数ではありません",
                        e.tag
                    ))),
                }
            })
            .collect()
    }

    fn single_uint(&self, e: &Entry) -> Result<u32> {
        let v = self.uints(e)?;
        if v.len() != 1 {
            return Err(dcp_error(format!(
                "タグ {} の値の数が 1 ではありません",
                e.tag
            )));
        }
        Ok(v[0])
    }

    /// 有理数（RATIONAL・SRATIONAL）。分母が 0 ならエラー。
    fn rationals(&self, e: &Entry) -> Result<Vec<f64>> {
        let rd = self.reader();
        (0..e.count)
            .map(|i| {
                let at = e.offset + i * 8;
                let (n, d) = match e.ty {
                    ty::SRATIONAL => (
                        f64::from(rd.u32(at)? as i32),
                        f64::from(rd.u32(at + 4)? as i32),
                    ),
                    ty::RATIONAL => (f64::from(rd.u32(at)?), f64::from(rd.u32(at + 4)?)),
                    _ => {
                        return Err(dcp_error(format!(
                            "タグ {} の型が有理数ではありません",
                            e.tag
                        )));
                    }
                };
                if d == 0.0 {
                    return Err(dcp_error(format!("タグ {} の分母が 0 です", e.tag)));
                }
                Ok(n / d)
            })
            .collect()
    }

    /// 浮動小数点（FLOAT・DOUBLE）。
    fn floats(&self, e: &Entry) -> Result<Vec<f32>> {
        let rd = self.reader();
        (0..e.count)
            .map(|i| {
                let v = match e.ty {
                    ty::FLOAT => f32::from_bits(rd.u32(e.offset + i * 4)?),
                    ty::DOUBLE => f64::from_bits(rd.u64(e.offset + i * 8)?) as f32,
                    _ => {
                        return Err(dcp_error(format!(
                            "タグ {} の型が浮動小数点ではありません",
                            e.tag
                        )));
                    }
                };
                if !v.is_finite() {
                    return Err(dcp_error(format!(
                        "タグ {} に有限でない値があります",
                        e.tag
                    )));
                }
                Ok(v)
            })
            .collect()
    }

    fn text(&self, e: &Entry) -> Result<Option<String>> {
        if !matches!(e.ty, ty::ASCII | ty::BYTE | ty::UNDEFINED) {
            return Err(dcp_error(format!(
                "タグ {} の型が文字列ではありません",
                e.tag
            )));
        }
        let raw = &self.bytes[e.offset..e.offset + e.count];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let s = String::from_utf8_lossy(&raw[..end]);
        let s: String = s
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_TEXT_CHARS)
            .collect();
        let s = s.trim().to_owned();
        Ok((!s.is_empty()).then_some(s))
    }

    fn matrix(&self, e: &Entry, what: &str) -> Result<Mat3> {
        let v = self.rationals(e)?;
        if v.len() != 9 {
            return Err(dcp_error(format!(
                "{what} の要素の数が 9 ではありません（{} 個。3 色のカメラだけに対応）",
                v.len()
            )));
        }
        let m = Mat3::from_rows([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]]);
        check_matrix(&m, what)?;
        Ok(m)
    }

    fn dims(&self, e: &Entry, what: &str) -> Result<[u32; 3]> {
        let v = self.uints(e)?;
        if v.len() != 3 {
            return Err(dcp_error(format!("{what} の値の数が 3 ではありません")));
        }
        Ok([v[0], v[1], v[2]])
    }

    fn triples(&self, e: &Entry) -> Result<Vec<[f32; 3]>> {
        if !e.count.is_multiple_of(3) || e.count / 3 > MAX_TABLE_ENTRIES {
            return Err(dcp_error(format!(
                "タグ {} の値の数が不正です（{} 個）",
                e.tag, e.count
            )));
        }
        let v = self.floats(e)?;
        Ok(v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
    }

    fn tone_curve(&self, e: &Entry) -> Result<Vec<[f32; 2]>> {
        if !e.count.is_multiple_of(2) || e.count / 2 > MAX_TONE_CURVE_POINTS {
            return Err(dcp_error("ProfileToneCurve の値の数が不正です"));
        }
        let v = self.floats(e)?;
        Ok(v.chunks_exact(2).map(|c| [c[0], c[1]]).collect())
    }
}

/// TIFF（DCP）を書く（テスト・検証用。[`DcpProfile::to_bytes`]）。
struct TiffWriter {
    big_endian: bool,
    /// (タグ, 型, 数, 値のバイト列)。
    entries: Vec<(u16, u16, u32, Vec<u8>)>,
}

impl TiffWriter {
    fn new(big_endian: bool) -> Self {
        Self {
            big_endian,
            entries: Vec::new(),
        }
    }
    fn u16b(&self, v: u16) -> [u8; 2] {
        if self.big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    }
    fn u32b(&self, v: u32) -> [u8; 4] {
        if self.big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    }
    fn ascii(&mut self, tag: u16, s: &str) {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        self.entries.push((tag, ty::ASCII, b.len() as u32, b));
    }
    fn short(&mut self, tag: u16, v: u16) {
        let b = self.u16b(v).to_vec();
        self.entries.push((tag, ty::SHORT, 1, b));
    }
    fn longs(&mut self, tag: u16, v: &[u32]) {
        let b = v.iter().flat_map(|&x| self.u32b(x)).collect();
        self.entries.push((tag, ty::LONG, v.len() as u32, b));
    }
    fn floats(&mut self, tag: u16, v: &[f32]) {
        let b = v.iter().flat_map(|&x| self.u32b(x.to_bits())).collect();
        self.entries.push((tag, ty::FLOAT, v.len() as u32, b));
    }
    fn srationals(&mut self, tag: u16, v: &[f64]) {
        const DEN: i32 = 1_000_000;
        let b = v
            .iter()
            .flat_map(|&x| {
                let n = (x * f64::from(DEN)).round() as i32;
                let mut out = self.u32b(n as u32).to_vec();
                out.extend(self.u32b(DEN as u32));
                out
            })
            .collect();
        self.entries.push((tag, ty::SRATIONAL, v.len() as u32, b));
    }
    fn finish(mut self) -> Vec<u8> {
        self.entries.sort_by_key(|e| e.0);
        let n = self.entries.len();
        let ifd_off = 8u32;
        let mut data_off = ifd_off as usize + 2 + n * 12 + 4;
        let mut out = Vec::new();
        out.extend_from_slice(if self.big_endian { b"MM" } else { b"II" });
        out.extend(self.u16b(DCP_MAGIC));
        out.extend(self.u32b(ifd_off));
        out.extend(self.u16b(n as u16));
        let mut data = Vec::new();
        for (tag, t, count, bytes) in &self.entries {
            out.extend(self.u16b(*tag));
            out.extend(self.u16b(*t));
            out.extend(self.u32b(*count));
            if bytes.len() <= 4 {
                let mut inline = bytes.clone();
                inline.resize(4, 0);
                out.extend(inline);
            } else {
                out.extend(self.u32b(data_off as u32));
                data.extend_from_slice(bytes);
                if bytes.len() % 2 == 1 {
                    data.push(0);
                }
                data_off += bytes.len().div_ceil(2) * 2;
            }
        }
        out.extend(self.u32b(0));
        out.extend(data);
        out
    }
}

#[cfg(test)]
mod tests;
