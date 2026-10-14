//! RAW に埋め込まれたレンズ補正の情報（DEV-14。docs/04_architecture.md の 2.1 節のステージ 7・9）。
//!
//! 利用者の決定（DEV-14）: **ARW / DNG に埋め込まれた補正情報を使い、ない場合は手動のスライダーで
//! 補正する**。lensfun などの外部のデータは使わない・同梱しない。この モジュールは、RAW のファイルの
//! TIFF の構造を自前で読み、補正の情報（[`LensCorrectionData`]）を取り出すだけで、補正の計算
//! （歪曲・周辺減光・色収差の式）は genzo-pipeline の `lens` が行う。
//!
//! # 読む情報
//!
//! | 形式 | 場所 | タグ・opcode | 値 |
//! |---|---|---|---|
//! | Sony の ARW | RAW のデータの IFD（IFD0 の SubIFDs）。なければ IFD0（他のソフトが DNG に写した場合） | 0x7032 VignettingCorrParams・0x7035 ChromaticAberrationCorrParams・0x7037 DistortionCorrParams（SSHORT の配列） | 先頭が節点の数 `n`（色収差は `2n`）、続いて節点ごとの値（色収差は赤の `n` 個の後に青の `n` 個） |
//! | DNG | RAW の IFD（NewSubFileType = 0）。なければ他の IFD | OpcodeList3（0xC74E）の WarpRectilinear（ID 1）・FixVignetteRadial（ID 3）。FixVignetteRadial は OpcodeList2（0xC741）にあれば読む | Adobe の DNG Specification 1.4.0.0 の opcode の形（常にビッグエンディアン） |
//!
//! - Sony のタグの場所（RAW の SubIFD）と、値の並び（先頭が個数）は、darktable の `src/common/exif.cc`
//!   （Exiv2 の `Exif.SubImage1.DistortionCorrParams` など）で確かめた（2026-10-13。AI の調査。ExifTool の
//!   Sony のタグの表の型（int16s[17]・int16s[33]）は AI の知識によるもので、この調査では取得していない。
//!   **実機の ARW では未確認**）。値の意味（節点の位置・尺度）は genzo-pipeline の `lens` の doc に書く。
//! - DNG の opcode の形（件数、ID・版・フラグ・バイト数の見出し、WarpRectilinear の面の数・係数・中心、
//!   FixVignetteRadial の係数・中心）は darktable の `src/common/dng_opcode.c` で確かめた（DNG の仕様
//!   （1.4.0.0）の記述は AI の知識によるもので、この調査では仕様書を取得していない）。
//! - 埋め込みの情報は **カメラ・変換ソフトが書いた値をそのまま** 持つ（換算は genzo-pipeline）。値の検証
//!   （[`LensCorrectionData::validate`]）は、本体がワーカーから受け取ったときにも行う（04 の 1.2 節）。
//!
//! # 安全のための上限
//!
//! 信頼できない入力なので、読む量と回数に上限を設ける: IFD は合わせて [`MAX_IFDS`] 個・深さ 3 まで、
//! 1 つの IFD の項目は [`MAX_IFD_ENTRIES`] 個まで、opcode の列は [`MAX_OPCODE_LIST_BYTES`] バイトまで、
//! 同じ位置の IFD は 1 回だけ読む（循環の対策）。読み取りはファイルの必要な位置だけを読む（ファイル
//! 全体をメモリに読み込まない）。形が壊れていれば [`RawError::Decode`]。TIFF でないファイル（ヘッダが
//! 違う）は `Ok(None)`。
//!
//! 値も確かめる（[`LensCorrectionData::validate`]）: DNG の係数は有限で絶対値 100 まで、WarpRectilinear の
//! 半径方向の関数（取る位置の倍率）は `ρ` = 0〜1 で [`DNG_WARP_RADIAL_RANGE`] の中、3 つの面なら R・B の面と
//! G の面の比（倍率色収差）の 1 からの差は [`MAX_CA_RATIO_DEVIATION`] まで（どれも **仮置き**。実際のレンズより
//! 十分に広い。細工した値でステージ 7 の入力の範囲が巨大にならないため。genzo-pipeline も表の値を収める）。
//!
//! # 使う場所
//!
//! ワーカーの中で LibRaw の展開の後に読む（`crate::decode`。信頼できない入力の解析を本体で行わない。
//! SEC-05）。結果は [`crate::RawImage::lens_correction`] に入れて本体へ渡す。

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::RawError;

/// Sony の補正の節点の数の上限（darktable の `MAXKNOTS` と同じ 16）。
pub const SONY_MAX_KNOTS: usize = 16;
/// Sony の補正の節点の数の下限（線形補間に 2 点が要る）。
pub const SONY_MIN_KNOTS: usize = 2;

/// Sony の VignettingCorrParams。
pub const TAG_SONY_VIGNETTING: u16 = 0x7032;
/// Sony の ChromaticAberrationCorrParams。
pub const TAG_SONY_CHROMATIC_ABERRATION: u16 = 0x7035;
/// Sony の DistortionCorrParams。
pub const TAG_SONY_DISTORTION: u16 = 0x7037;
/// DNG の OpcodeList2（CFA の段階で適用する opcode）。
pub const TAG_OPCODE_LIST2: u16 = 0xC741;
/// DNG の OpcodeList3（デモザイクの後に適用する opcode）。
pub const TAG_OPCODE_LIST3: u16 = 0xC74E;

/// DNG の opcode の ID: WarpRectilinear。
pub const OPCODE_WARP_RECTILINEAR: u32 = 1;
/// DNG の opcode の ID: FixVignetteRadial。
pub const OPCODE_FIX_VIGNETTE_RADIAL: u32 = 3;

/// 読む IFD の数の上限（合計）。
pub const MAX_IFDS: usize = 64;
/// 1 つの IFD の項目の数の上限。
pub const MAX_IFD_ENTRIES: usize = 4096;
/// opcode の列のバイト数の上限（OpcodeList2 の GainMap は数百 KB になりうるので、それより大きく取る）。
pub const MAX_OPCODE_LIST_BYTES: u64 = 16 * 1024 * 1024;
/// opcode の列の件数の上限。
pub const MAX_OPCODES: u32 = 4096;
/// SubIFD をたどる深さの上限。
const MAX_DEPTH: u32 = 3;
/// SubIFDs の 1 つのタグに並ぶ位置の数の上限。
const MAX_SUB_IFDS: u32 = 16;

/// DNG の係数の絶対値の上限（壊れた値を受け付けないため。実際の値は 1 前後〜0.1 以下）。
const MAX_DNG_COEFFICIENT: f64 = 100.0;
/// DNG の光学中心（正規化した座標）として受け付ける範囲。
const DNG_CENTER_RANGE: (f64, f64) = (-1.0, 2.0);
/// DNG の WarpRectilinear の半径方向の関数 `kr0 + kr1·ρ² + kr2·ρ⁴ + kr3·ρ⁶`（取る位置の倍率）として受け付ける
/// 範囲（`ρ` = 0〜1。実際のレンズは 1 前後。**仮置き**: 0.5〜2）。
pub const DNG_WARP_RADIAL_RANGE: (f64, f64) = (0.5, 2.0);
/// 倍率色収差の取る位置の倍率（3 つの面の WarpRectilinear の R・B の面と G の面の比。Sony の色収差の値も含む）の
/// 1 からの差の上限（実際のレンズは 0.1 % 前後。**仮置き**: 5 %）。genzo-pipeline のステージ 7 も、表の値を
/// この範囲に収める（壊れた値でも、入力の範囲の余白を有限に保つ）。
pub const MAX_CA_RATIO_DEVIATION: f64 = 0.05;
/// 上の範囲を確かめる `ρ` の点の数（0〜1 を等分した両端を含む点）。
const DNG_RANGE_SAMPLES: usize = 65;

/// DNG の WarpRectilinear の半径方向の関数（genzo-pipeline の `dng_radial` と同じ式）。
fn dng_warp_radial(kr: &[f64; 4], rho: f64) -> f64 {
    let r2 = rho * rho;
    kr[0] + r2 * (kr[1] + r2 * (kr[2] + r2 * kr[3]))
}

/// `ρ` = 0〜1 の点（[`DNG_RANGE_SAMPLES`] 個）。
fn dng_range_samples() -> impl Iterator<Item = f64> {
    (0..DNG_RANGE_SAMPLES).map(|i| i as f64 / (DNG_RANGE_SAMPLES - 1) as f64)
}

/// 埋め込みのレンズ補正の情報（[`crate::RawImage::lens_correction`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LensCorrectionData {
    /// Sony の ARW のタグ。
    SonyArw(SonyLensCorrection),
    /// DNG の opcode。
    DngOpcode(DngLensCorrection),
}

/// Sony の ARW の補正のパラメータ（タグの値の、先頭の個数を除いた節点ごとの値。ないものは空）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SonyLensCorrection {
    /// DistortionCorrParams（0x7037）の節点ごとの値。
    pub distortion: Vec<i16>,
    /// VignettingCorrParams（0x7032）の節点ごとの値。
    pub vignetting: Vec<i16>,
    /// ChromaticAberrationCorrParams（0x7035）の赤の節点ごとの値。
    pub ca_red: Vec<i16>,
    /// ChromaticAberrationCorrParams（0x7035）の青の節点ごとの値（赤と同じ数）。
    pub ca_blue: Vec<i16>,
}

/// DNG の WarpRectilinear の 1 つの面（Adobe の DNG Specification の係数）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DngWarpPlane {
    /// 半径方向の係数 kr0〜kr3（`kr0 + kr1·r² + kr2·r⁴ + kr3·r⁶`）。
    pub kr: [f64; 4],
    /// 接線方向の係数 kt0・kt1。
    pub kt: [f64; 2],
}

/// DNG の WarpRectilinear（歪曲と倍率色収差）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DngWarpRectilinear {
    /// 面（1 つならすべての色に共通、3 つなら R・G・B の順）。
    pub planes: Vec<DngWarpPlane>,
    /// 光学中心（画像の左上を (0, 0)、右下を (1, 1) とした座標）。
    pub center: [f64; 2],
}

/// DNG の FixVignetteRadial（周辺減光）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DngVignetteRadial {
    /// 係数 k0〜k4（利得 `1 + k0·r² + k1·r⁴ + k2·r⁶ + k3·r⁸ + k4·r¹⁰`）。
    pub k: [f64; 5],
    /// 光学中心（画像の左上を (0, 0)、右下を (1, 1) とした座標）。
    pub center: [f64; 2],
}

/// DNG の opcode の補正。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DngLensCorrection {
    /// WarpRectilinear（OpcodeList3）。
    pub warp: Option<DngWarpRectilinear>,
    /// FixVignetteRadial（OpcodeList3、なければ OpcodeList2）。
    pub vignette: Option<DngVignetteRadial>,
}

impl LensCorrectionData {
    /// 出どころの識別子（`render_deps.lens_profile` の ID の前半。`"sony_arw"`・`"dng_opcode"`）。
    pub fn source_id(&self) -> &'static str {
        match self {
            Self::SonyArw(_) => "sony_arw",
            Self::DngOpcode(_) => "dng_opcode",
        }
    }

    /// 歪曲の情報があるか。
    pub fn has_distortion(&self) -> bool {
        match self {
            Self::SonyArw(s) => !s.distortion.is_empty(),
            Self::DngOpcode(d) => d.warp.is_some(),
        }
    }

    /// 周辺減光の情報があるか。
    pub fn has_vignetting(&self) -> bool {
        match self {
            Self::SonyArw(s) => !s.vignetting.is_empty(),
            Self::DngOpcode(d) => d.vignette.is_some(),
        }
    }

    /// 倍率色収差の情報があるか（DNG は WarpRectilinear が 3 つの面のとき）。
    pub fn has_chromatic_aberration(&self) -> bool {
        match self {
            Self::SonyArw(s) => !s.ca_red.is_empty(),
            Self::DngOpcode(d) => d.warp.as_ref().is_some_and(|w| w.planes.len() == 3),
        }
    }

    /// 補正の情報が何もないか（タグはあったが値がない場合など）。
    pub fn is_empty(&self) -> bool {
        !(self.has_distortion() || self.has_vignetting() || self.has_chromatic_aberration())
    }

    /// 値を検証する（本体がワーカーから受け取ったときにも使う。04 の 1.2 節）。
    pub fn validate(&self) -> Result<(), RawError> {
        let bad = |m: String| Err(RawError::InvalidLensCorrection(m));
        match self {
            Self::SonyArw(s) => {
                for (name, v) in [
                    ("DistortionCorrParams", &s.distortion),
                    ("VignettingCorrParams", &s.vignetting),
                    ("ChromaticAberrationCorrParams（赤）", &s.ca_red),
                    ("ChromaticAberrationCorrParams（青）", &s.ca_blue),
                ] {
                    if !v.is_empty() && !(SONY_MIN_KNOTS..=SONY_MAX_KNOTS).contains(&v.len()) {
                        return bad(format!(
                            "{name} の節点の数 {} は {SONY_MIN_KNOTS}〜{SONY_MAX_KNOTS} の外です",
                            v.len()
                        ));
                    }
                }
                if s.ca_red.len() != s.ca_blue.len() {
                    return bad(format!(
                        "色収差の赤（{}）と青（{}）の節点の数が違います",
                        s.ca_red.len(),
                        s.ca_blue.len()
                    ));
                }
            }
            Self::DngOpcode(d) => {
                let finite = |v: f64| v.is_finite() && v.abs() <= MAX_DNG_COEFFICIENT;
                let center_ok = |c: [f64; 2]| {
                    c.iter().all(|v| {
                        v.is_finite() && (DNG_CENTER_RANGE.0..=DNG_CENTER_RANGE.1).contains(v)
                    })
                };
                if let Some(w) = &d.warp {
                    if !(w.planes.len() == 1 || w.planes.len() == 3) {
                        return bad(format!(
                            "WarpRectilinear の面の数 {} は 1 か 3 ではありません",
                            w.planes.len()
                        ));
                    }
                    for p in &w.planes {
                        if !p.kr.iter().chain(&p.kt).all(|&v| finite(v)) {
                            return bad("WarpRectilinear の係数が不正です".to_owned());
                        }
                        if p.kr[0] <= 0.0 {
                            return bad(format!(
                                "WarpRectilinear の kr0（{}）が正ではありません",
                                p.kr[0]
                            ));
                        }
                        let (lo, hi) = DNG_WARP_RADIAL_RANGE;
                        if let Some(rho) = dng_range_samples()
                            .find(|&rho| !(lo..=hi).contains(&dng_warp_radial(&p.kr, rho)))
                        {
                            return bad(format!(
                                "WarpRectilinear の倍率（ρ = {rho:.3} で {:.4}）が {lo}〜{hi} の外です",
                                dng_warp_radial(&p.kr, rho)
                            ));
                        }
                    }
                    // 3 つの面なら、R・B の面と G の面の比（倍率色収差の取る位置の倍率。genzo-pipeline の
                    // ステージ 7）が 1 に近いこと。
                    if let [r, g, b] = &w.planes[..] {
                        for (name, p) in [("R", r), ("B", b)] {
                            if let Some(rho) = dng_range_samples().find(|&rho| {
                                let ratio =
                                    dng_warp_radial(&p.kr, rho) / dng_warp_radial(&g.kr, rho);
                                (ratio - 1.0).abs() > MAX_CA_RATIO_DEVIATION
                            }) {
                                return bad(format!(
                                    "WarpRectilinear の {name} と G の面の比（倍率色収差。ρ = {rho:.3} で {:.4}）が 1 から {} % を超えて離れています",
                                    dng_warp_radial(&p.kr, rho) / dng_warp_radial(&g.kr, rho),
                                    MAX_CA_RATIO_DEVIATION * 100.0
                                ));
                            }
                        }
                    }
                    if !center_ok(w.center) {
                        return bad(format!(
                            "WarpRectilinear の光学中心 {:?} が範囲の外です",
                            w.center
                        ));
                    }
                }
                if let Some(v) = &d.vignette {
                    if !v.k.iter().all(|&k| finite(k)) {
                        return bad("FixVignetteRadial の係数が不正です".to_owned());
                    }
                    if !center_ok(v.center) {
                        return bad(format!(
                            "FixVignetteRadial の光学中心 {:?} が範囲の外です",
                            v.center
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// 画面・CLI に出す短い説明（例: `"Sony の ARW（歪曲・周辺減光・色収差）"`）。
    pub fn describe(&self) -> String {
        let source = match self {
            Self::SonyArw(_) => "Sony の ARW",
            Self::DngOpcode(_) => "DNG の opcode",
        };
        let mut kinds = Vec::new();
        if self.has_distortion() {
            kinds.push("歪曲");
        }
        if self.has_vignetting() {
            kinds.push("周辺減光");
        }
        if self.has_chromatic_aberration() {
            kinds.push("色収差");
        }
        format!("{source}（{}）", kinds.join("・"))
    }
}

// ---------------------------------------------------------------------------
// 読み取り
// ---------------------------------------------------------------------------

/// 位置を指定して読める入力（ファイルかメモリ上のデータ）。
trait ReadAt {
    /// 全体の大きさ（バイト）。
    fn size(&self) -> u64;
    /// `offset` から `buf` の長さだけ読む（足りなければエラー）。
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), RawError>;
}

struct SliceSource<'a>(&'a [u8]);

impl ReadAt for SliceSource<'_> {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), RawError> {
        let start = usize::try_from(offset).map_err(|_| truncated())?;
        let end = start.checked_add(buf.len()).ok_or_else(truncated)?;
        let src = self.0.get(start..end).ok_or_else(truncated)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

struct FileSource {
    file: File,
    size: u64,
}

impl ReadAt for FileSource {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), RawError> {
        let end = offset.checked_add(buf.len() as u64).ok_or_else(truncated)?;
        if end > self.size {
            return Err(truncated());
        }
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buf)?;
        Ok(())
    }
}

fn truncated() -> RawError {
    RawError::Decode("レンズ補正の情報を読む途中でファイルの終わりを超えました".to_owned())
}

fn malformed(what: &str) -> RawError {
    RawError::Decode(format!("レンズ補正の情報を読めません（{what}）"))
}

/// ファイルから埋め込みのレンズ補正の情報を読む。TIFF でないファイル・情報がないファイルは `Ok(None)`。
pub fn read_lens_correction(
    path: impl AsRef<Path>,
) -> Result<Option<LensCorrectionData>, RawError> {
    let file = File::open(path.as_ref())?;
    let size = file.metadata()?.len();
    parse(&mut FileSource { file, size })
}

/// メモリ上のデータから埋め込みのレンズ補正の情報を読む（[`read_lens_correction`] と同じ）。
pub fn read_lens_correction_bytes(data: &[u8]) -> Result<Option<LensCorrectionData>, RawError> {
    parse(&mut SliceSource(data))
}

/// TIFF の項目。
#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    /// 値のある位置（4 バイト以下なら項目の中の位置）。
    value_pos: u64,
}

/// 読んだ IFD の要約。
#[derive(Debug, Default)]
struct IfdInfo {
    /// IFD0 の連鎖（0）か、SubIFD（1 以上）か。
    depth: u32,
    new_subfile_type: Option<u32>,
    entries: Vec<Entry>,
}

impl IfdInfo {
    fn find(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

/// TIFF の型の 1 つの値のバイト数（知らない型は `None`）。
fn type_size(typ: u16) -> Option<u64> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

struct Tiff<'a> {
    src: &'a mut dyn ReadAt,
    little: bool,
}

impl Tiff<'_> {
    fn bytes(&mut self, offset: u64, len: u64) -> Result<Vec<u8>, RawError> {
        let len = usize::try_from(len).map_err(|_| truncated())?;
        let mut buf = vec![0u8; len];
        self.src.read_at(offset, &mut buf)?;
        Ok(buf)
    }

    fn u16_at(&mut self, offset: u64) -> Result<u16, RawError> {
        let mut b = [0u8; 2];
        self.src.read_at(offset, &mut b)?;
        Ok(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32_at(&mut self, offset: u64) -> Result<u32, RawError> {
        let mut b = [0u8; 4];
        self.src.read_at(offset, &mut b)?;
        Ok(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    /// `offset` の IFD を読む（項目と次の IFD の位置）。
    fn read_ifd(&mut self, offset: u64) -> Result<(Vec<Entry>, u32), RawError> {
        let n = usize::from(self.u16_at(offset)?);
        if n == 0 || n > MAX_IFD_ENTRIES {
            return Err(malformed("IFD の項目の数"));
        }
        let table = self.bytes(offset + 2, 12 * n as u64 + 4)?;
        let little = self.little;
        let rd16 = |b: &[u8]| read_u16(b, little);
        let rd32 = |b: &[u8]| read_u32(b, little);
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let e = &table[12 * i..12 * i + 12];
            let (tag, typ, count) = (rd16(&e[0..2]), rd16(&e[2..4]), rd32(&e[4..8]));
            let Some(size) = type_size(typ) else {
                continue;
            };
            let bytes = size * u64::from(count);
            let value_pos = if bytes <= 4 {
                offset + 2 + 12 * i as u64 + 8
            } else {
                u64::from(rd32(&e[8..12]))
            };
            entries.push(Entry {
                tag,
                typ,
                count,
                value_pos,
            });
        }
        let next = rd32(&table[12 * n..12 * n + 4]);
        Ok((entries, next))
    }

    /// 整数の値の列（SHORT・SSHORT・LONG・SLONG・IFD）。数は `max` まで。
    fn ints(&mut self, e: &Entry, max: u32) -> Result<Vec<i64>, RawError> {
        if e.count > max {
            return Err(malformed("値の数が多すぎる"));
        }
        let size = type_size(e.typ).ok_or_else(|| malformed("型"))?;
        let raw = self.bytes(e.value_pos, size * u64::from(e.count))?;
        let little = self.little;
        let mut out = Vec::with_capacity(e.count as usize);
        for c in raw.chunks_exact(size as usize) {
            let v = match (e.typ, size) {
                (3, 2) => i64::from(read_u16(c, little)),
                (8, 2) => i64::from(read_u16(c, little) as i16),
                (4 | 13, 4) => i64::from(read_u32(c, little)),
                (9, 4) => i64::from(read_u32(c, little) as i32),
                _ => return Err(malformed("整数でない型")),
            };
            out.push(v);
        }
        Ok(out)
    }
}

fn read_u16(b: &[u8], little: bool) -> u16 {
    let a = [b[0], b[1]];
    if little {
        u16::from_le_bytes(a)
    } else {
        u16::from_be_bytes(a)
    }
}

fn read_u32(b: &[u8], little: bool) -> u32 {
    let a = [b[0], b[1], b[2], b[3]];
    if little {
        u32::from_le_bytes(a)
    } else {
        u32::from_be_bytes(a)
    }
}

/// TIFF の構造をたどって補正の情報を取り出す（モジュールの doc の表）。
fn parse(src: &mut dyn ReadAt) -> Result<Option<LensCorrectionData>, RawError> {
    if src.size() < 8 {
        return Ok(None);
    }
    let mut head = [0u8; 4];
    src.read_at(0, &mut head)?;
    let little = match head {
        [b'I', b'I', 42, 0] => true,
        [b'M', b'M', 0, 42] => false,
        _ => return Ok(None),
    };
    let mut t = Tiff { src, little };
    let first = t.u32_at(4)?;
    let ifds = walk(&mut t, u64::from(first))?;
    let is_dng = ifds.first().is_some_and(|ifd| ifd.find(0xC612).is_some());

    if is_dng && let Some(dng) = read_dng(&mut t, &ifds)? {
        return Ok(Some(LensCorrectionData::DngOpcode(dng)));
    }
    let sony = read_sony(&mut t, &ifds)?;
    Ok(sony.map(LensCorrectionData::SonyArw))
}

/// IFD0 の連鎖と SubIFDs をたどる（上限つき）。順序: IFD0、IFD0 の SubIFDs（深さ順）、IFD1 …
fn walk(t: &mut Tiff<'_>, first: u64) -> Result<Vec<IfdInfo>, RawError> {
    let mut out = Vec::new();
    let mut visited = HashSet::new();
    let mut chain = Some(first);
    while let Some(off) = chain.take() {
        if off == 0 || !visited.insert(off) || out.len() >= MAX_IFDS {
            break;
        }
        let (entries, next) = t.read_ifd(off)?;
        let info = ifd_info(t, entries, 0);
        let subs = sub_ifds(t, &info)?;
        out.push(info);
        let mut stack: Vec<(u64, u32)> = subs.into_iter().rev().map(|o| (o, 1)).collect();
        while let Some((sub, depth)) = stack.pop() {
            if sub == 0 || !visited.insert(sub) || out.len() >= MAX_IFDS {
                continue;
            }
            let (entries, _) = t.read_ifd(sub)?;
            let info = ifd_info(t, entries, depth);
            if depth < MAX_DEPTH {
                for s in sub_ifds(t, &info)?.into_iter().rev() {
                    stack.push((s, depth + 1));
                }
            }
            out.push(info);
        }
        chain = Some(u64::from(next));
    }
    Ok(out)
}

fn ifd_info(t: &mut Tiff<'_>, entries: Vec<Entry>, depth: u32) -> IfdInfo {
    let mut info = IfdInfo {
        depth,
        new_subfile_type: None,
        entries,
    };
    if let Some(e) = info.find(0x00FE).copied() {
        info.new_subfile_type = t
            .ints(&e, 1)
            .ok()
            .and_then(|v| v.first().copied())
            .and_then(|v| u32::try_from(v).ok());
    }
    info
}

/// SubIFDs（0x014A）の位置。
fn sub_ifds(t: &mut Tiff<'_>, info: &IfdInfo) -> Result<Vec<u64>, RawError> {
    let Some(e) = info.find(0x014A).copied() else {
        return Ok(Vec::new());
    };
    let v = t.ints(&e, MAX_SUB_IFDS)?;
    Ok(v.into_iter()
        .filter_map(|o| u64::try_from(o).ok())
        .collect())
}

/// IFD の優先の順（RAW のデータの IFD（NewSubFileType = 0 の SubIFD）→ SubIFD → IFD0 → その他）。
fn preference(info: &IfdInfo, index: usize) -> (u8, usize) {
    let raw = info.new_subfile_type == Some(0);
    let rank = if info.depth > 0 {
        if raw { 0 } else { 1 }
    } else if index == 0 {
        2
    } else {
        3
    };
    (rank, index)
}

fn ordered(ifds: &[IfdInfo]) -> Vec<&IfdInfo> {
    let mut v: Vec<(usize, &IfdInfo)> = ifds.iter().enumerate().collect();
    v.sort_by_key(|(i, info)| preference(info, *i));
    v.into_iter().map(|(_, i)| i).collect()
}

/// Sony のタグ（モジュールの doc の表）。どれもなければ `None`。
fn read_sony(t: &mut Tiff<'_>, ifds: &[IfdInfo]) -> Result<Option<SonyLensCorrection>, RawError> {
    let order = ordered(ifds);
    let find = |tag: u16| order.iter().find_map(|i| i.find(tag).copied());
    let (dist, vig, ca) = (
        find(TAG_SONY_DISTORTION),
        find(TAG_SONY_VIGNETTING),
        find(TAG_SONY_CHROMATIC_ABERRATION),
    );
    if dist.is_none() && vig.is_none() && ca.is_none() {
        return Ok(None);
    }
    let max = 2 * SONY_MAX_KNOTS as u32 + 1;
    let mut out = SonyLensCorrection::default();
    let knots = |values: &[i64], per: usize| -> Option<Vec<Vec<i16>>> {
        let n_total = usize::try_from(*values.first()?).ok()?;
        if n_total % per != 0 {
            return None;
        }
        let n = n_total / per;
        if !(SONY_MIN_KNOTS..=SONY_MAX_KNOTS).contains(&n) || values.len() < n_total + 1 {
            return None;
        }
        let body = &values[1..=n_total];
        Some(
            body.chunks(n)
                .map(|c| c.iter().map(|&v| v as i16).collect())
                .collect(),
        )
    };
    if let Some(e) = dist
        && let Some(mut v) = knots(&t.ints(&e, max)?, 1)
    {
        out.distortion = v.remove(0);
    }
    if let Some(e) = vig
        && let Some(mut v) = knots(&t.ints(&e, max)?, 1)
    {
        out.vignetting = v.remove(0);
    }
    if let Some(e) = ca
        && let Some(mut v) = knots(&t.ints(&e, max)?, 2)
    {
        out.ca_blue = v.remove(1);
        out.ca_red = v.remove(0);
    }
    Ok(Some(out))
}

/// DNG の opcode（モジュールの doc の表）。補正の opcode がなければ `None`。
fn read_dng(t: &mut Tiff<'_>, ifds: &[IfdInfo]) -> Result<Option<DngLensCorrection>, RawError> {
    let order = ordered(ifds);
    let mut out = DngLensCorrection::default();
    if let Some(e) = order.iter().find_map(|i| i.find(TAG_OPCODE_LIST3).copied()) {
        let list = opcode_bytes(t, &e)?;
        let parsed = parse_opcode_list(&list)?;
        out.warp = parsed.warp;
        out.vignette = parsed.vignette;
    }
    if out.vignette.is_none()
        && let Some(e) = order.iter().find_map(|i| i.find(TAG_OPCODE_LIST2).copied())
    {
        let list = opcode_bytes(t, &e)?;
        out.vignette = parse_opcode_list(&list)?.vignette;
    }
    Ok((out.warp.is_some() || out.vignette.is_some()).then_some(out))
}

fn opcode_bytes(t: &mut Tiff<'_>, e: &Entry) -> Result<Vec<u8>, RawError> {
    let size = type_size(e.typ).ok_or_else(|| malformed("opcode の列の型"))? * u64::from(e.count);
    if size > MAX_OPCODE_LIST_BYTES {
        return Err(malformed("opcode の列が大きすぎる"));
    }
    t.bytes(e.value_pos, size)
}

fn be_u32(b: &[u8], at: usize) -> Result<u32, RawError> {
    let s = b
        .get(at..at + 4)
        .ok_or_else(|| malformed("opcode の長さ"))?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be_f64(b: &[u8], at: usize) -> Result<f64, RawError> {
    let s = b
        .get(at..at + 8)
        .ok_or_else(|| malformed("opcode の長さ"))?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(f64::from_be_bytes(a))
}

/// opcode の列（ビッグエンディアン）から WarpRectilinear・FixVignetteRadial を取り出す（最初のもの）。
pub fn parse_opcode_list(list: &[u8]) -> Result<DngLensCorrection, RawError> {
    let mut out = DngLensCorrection::default();
    let count = be_u32(list, 0)?;
    if count > MAX_OPCODES {
        return Err(malformed("opcode の数が多すぎる"));
    }
    let mut at = 4usize;
    for _ in 0..count {
        let id = be_u32(list, at)?;
        let size = be_u32(list, at + 12)? as usize;
        let start = at + 16;
        let end = start
            .checked_add(size)
            .filter(|&e| e <= list.len())
            .ok_or_else(|| malformed("opcode の大きさ"))?;
        let p = &list[start..end];
        match id {
            OPCODE_WARP_RECTILINEAR if out.warp.is_none() => {
                let planes = be_u32(p, 0)? as usize;
                if !(planes == 1 || planes == 3) {
                    return Err(malformed("WarpRectilinear の面の数"));
                }
                if p.len() < 4 + 8 * (6 * planes + 2) {
                    return Err(malformed("WarpRectilinear の大きさ"));
                }
                let mut v = Vec::with_capacity(planes);
                for k in 0..planes {
                    let base = 4 + 8 * 6 * k;
                    let c = |i: usize| be_f64(p, base + 8 * i);
                    v.push(DngWarpPlane {
                        kr: [c(0)?, c(1)?, c(2)?, c(3)?],
                        kt: [c(4)?, c(5)?],
                    });
                }
                let cbase = 4 + 8 * 6 * planes;
                out.warp = Some(DngWarpRectilinear {
                    planes: v,
                    center: [be_f64(p, cbase)?, be_f64(p, cbase + 8)?],
                });
            }
            OPCODE_FIX_VIGNETTE_RADIAL if out.vignette.is_none() => {
                if p.len() < 8 * 7 {
                    return Err(malformed("FixVignetteRadial の大きさ"));
                }
                let c = |i: usize| be_f64(p, 8 * i);
                out.vignette = Some(DngVignetteRadial {
                    k: [c(0)?, c(1)?, c(2)?, c(3)?, c(4)?],
                    center: [c(5)?, c(6)?],
                });
            }
            _ => {}
        }
        at = end;
    }
    Ok(out)
}

/// WarpRectilinear・FixVignetteRadial を opcode の列（ビッグエンディアン。DNG の OpcodeList3 の値）に
/// する（テスト用の合成 DNG の書き出し。[`crate::dng::DngOptions::lens_correction`]）。
pub fn encode_opcode_list(d: &DngLensCorrection) -> Vec<u8> {
    let mut ops: Vec<(u32, Vec<u8>)> = Vec::new();
    if let Some(w) = &d.warp {
        let mut p = (w.planes.len() as u32).to_be_bytes().to_vec();
        for plane in &w.planes {
            for v in plane.kr.iter().chain(&plane.kt) {
                p.extend_from_slice(&v.to_be_bytes());
            }
        }
        for v in w.center {
            p.extend_from_slice(&v.to_be_bytes());
        }
        ops.push((OPCODE_WARP_RECTILINEAR, p));
    }
    if let Some(v) = &d.vignette {
        let mut p = Vec::new();
        for k in v.k.iter().chain(&v.center) {
            p.extend_from_slice(&k.to_be_bytes());
        }
        ops.push((OPCODE_FIX_VIGNETTE_RADIAL, p));
    }
    let mut out = (ops.len() as u32).to_be_bytes().to_vec();
    for (id, p) in ops {
        out.extend_from_slice(&id.to_be_bytes());
        // DNG の仕様の版（1.3.0.0）・フラグ（0 = 必須）・バイト数。
        out.extend_from_slice(&0x0103_0000u32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(p.len() as u32).to_be_bytes());
        out.extend_from_slice(&p);
    }
    out
}

/// Sony のタグの値（先頭の個数を含む SSHORT の配列）を作る（テスト用の合成のファイルの書き出し）。
/// 空なら `None`。色収差は `[2n, 赤 n 個, 青 n 個]`。
pub fn sony_tag_values(s: &SonyLensCorrection) -> [(u16, Option<Vec<i16>>); 3] {
    let with_count = |v: &[i16]| {
        (!v.is_empty()).then(|| {
            let mut out = vec![v.len() as i16];
            out.extend_from_slice(v);
            out
        })
    };
    let ca = (!s.ca_red.is_empty()).then(|| {
        let mut out = vec![(s.ca_red.len() + s.ca_blue.len()) as i16];
        out.extend_from_slice(&s.ca_red);
        out.extend_from_slice(&s.ca_blue);
        out
    });
    [
        (TAG_SONY_DISTORTION, with_count(&s.distortion)),
        (TAG_SONY_VIGNETTING, with_count(&s.vignetting)),
        (TAG_SONY_CHROMATIC_ABERRATION, ca),
    ]
}

#[cfg(test)]
mod tests;
