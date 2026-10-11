//! プレビュー・サムネイルのキャッシュ（B5）の JPEG を作る（PRV-01、04 の 4 章・2.6 節の B5）。
//!
//! 取り込み直後に、RAW の埋め込み JPEG や RAW 以外の元の画像（JPEG / PNG / TIFF）、動画の代表
//! フレームから、L0（長辺 320px）・L1（長辺 2560px）のキャッシュを作る。手順:
//!
//! 1. 入力の ICC プロファイル（なければ sRGB）で、画素をリニア BT.2020（作業色空間）に変換する
//!    （2.6 節「RAW 以外の入力」。lcms2）。
//! 2. リニアな値のまま、面積平均で長辺 N px に縮小する（[`crate::resize`]。拡大はしない）。
//! 3. Display P3 のリニアな値にして、IEC 61966-2-1 の伝達関数で符号化し、8bit にする（B5）。
//! 4. 向き（Orientation）を画素に反映する。
//! 5. Display P3 の ICC プロファイル（v4、[`CACHE_ICC_VERSION`]）を埋め込んだ JPEG にする。Exif は
//!    入れない（向きは反映済み）。
//!
//! 色域: 入力が sRGB・Display P3 なら、P3 の色域に収まる。Adobe RGB などで P3 の外になる色だけ、
//! 各成分を 0〜1 に収める（**仮置きの簡易な扱い**。2.6 節の色域の圧縮（ステージ 17）は現像の
//! 経路で行うもので、ここでは埋め込み JPEG という仕上がった画像の一時的な表示に使うため）。

use genzo_color::space::working_to;
use genzo_color::transfer::srgb_decode;
use genzo_color::{
    IccProfile, IccTransform, IccVersion, RenderingIntent, RgbColorSpace, StandardProfile,
};
use genzo_model::{ExportSize, Orientation};
use serde::{Deserialize, Serialize};

use crate::buffer::{DynRgbImage, RgbImage, RgbImage8, Sample};
use crate::decode::{DecodedImage, decode_image_bytes};
use crate::error::{MediaError, Result};
use crate::jpeg::encode_jpeg;
use crate::orientation::apply_orientation;
use crate::resize::AreaDownscaler;

/// キャッシュに埋め込む ICC プロファイルのバージョン（仮置き）。
///
/// v4 は伝達関数を区分関数（'para'）で正確に表せて、プロファイルが小さい（数百バイト）。L0 は
/// 50 万件を保存する（SCL-03）ので小ささを優先した。表示側（WebView・ビューポートの 3D LUT）は
/// v4 を読める前提（PoC-1 で確認する）。
pub const CACHE_ICC_VERSION: IccVersion = IccVersion::V4_3;

/// 作るキャッシュの大きさと品質。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheSpec {
    /// 長辺の画素数（これより小さい入力は拡大しない）。
    pub long_edge: u32,
    /// JPEG の品質（1〜100）。
    pub quality: u8,
}

impl CacheSpec {
    /// L0（サムネイル）: 長辺 320px、品質 80（04 の 4 章の表。品質は「80 前後」の仮置き）。
    pub const L0_THUMBNAIL: Self = Self {
        long_edge: 320,
        quality: 80,
    };

    /// L1（標準プレビュー）: 長辺 2560px、品質 85（04 の 4 章の表。品質は「85 前後」の仮置き）。
    pub const L1_PREVIEW: Self = Self {
        long_edge: 2560,
        quality: 85,
    };

    /// 値を確認する。
    pub fn validate(&self) -> Result<()> {
        if !(1..=ExportSize::MAX_LONG_EDGE).contains(&self.long_edge) {
            return Err(MediaError::invalid_argument(format!(
                "長辺は 1〜{}（{}）",
                ExportSize::MAX_LONG_EDGE,
                self.long_edge
            )));
        }
        if !(1..=100).contains(&self.quality) {
            return Err(MediaError::invalid_argument(format!(
                "JPEG の品質は 1〜100（{}）",
                self.quality
            )));
        }
        Ok(())
    }
}

/// キャッシュの JPEG。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheJpeg {
    /// JPEG のバイト列（Display P3 の ICC プロファイル付き）。
    pub bytes: Vec<u8>,
    /// 幅（向きを反映した後）。
    pub width: u32,
    /// 高さ（向きを反映した後）。
    pub height: u32,
}

/// キャッシュに埋め込む Display P3 のプロファイル。
pub fn cache_icc_profile() -> Result<IccProfile> {
    crate::profiles::standard(StandardProfile::DisplayP3, CACHE_ICC_VERSION)
}

/// 符号化された入力 → リニア BT.2020 の変換。
enum Linearizer {
    /// 行列とトーンカーブ（matrix-shaper）のプロファイル。変換は「チャンネルごとのトーンカーブ →
    /// 行列」なので、`f(r, g, b) = f(r, 0, 0) + f(0, g, 0) + f(0, 0, b) − 2·f(0, 0, 0)` が成り立つ。
    /// チャンネルごとの表（値の段階の数 × 3）を lcms2 で作っておき、表を引いて足すだけにする。
    Separable {
        tables: [Vec<[f32; 3]>; 3],
        black2: [f32; 3],
    },
    /// それ以外（LUT のプロファイルなど）: 画素ごとに lcms2 で変換する。
    PerPixel { transform: IccTransform },
}

/// 表で計算した結果と lcms2 の結果の差の許容値（リニアの値。1 が白）。
///
/// lcms2 の浮動小数点の計算の丸め（1e-6 程度）より十分大きく、8bit の 1 段階（暗部で約 3e-4）より
/// 小さい値。
const SEPARABLE_TOLERANCE: f32 = 1e-4;

impl Linearizer {
    fn new<T: Sample>(source: &IccProfile) -> Result<Self> {
        let lin = crate::profiles::standard(StandardProfile::LinearBt2020, IccVersion::V4_3)?;
        let transform = IccTransform::new(source, &lin, RenderingIntent::RelativeColorimetric)?;
        let levels = usize::from(T::MAX) + 1;
        let max = f32::from(T::MAX);
        let mut tables: [Vec<[f32; 3]>; 3] = Default::default();
        for (c, table) in tables.iter_mut().enumerate() {
            let input: Vec<[f32; 3]> = (0..levels)
                .map(|v| {
                    let mut px = [0.0; 3];
                    px[c] = v as f32 / max;
                    px
                })
                .collect();
            let mut out = vec![[0.0; 3]; levels];
            transform.transform(&input, &mut out)?;
            *table = out;
        }
        let black = tables[0][0];
        let sep = Self::Separable {
            tables,
            black2: black.map(|v| 2.0 * v),
        };
        // 表の和で計算できるか（matrix-shaper として変換されるか）を、格子の色で確かめる。
        let grid = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0].map(|v: f32| (v * max).round() as u32);
        let mut probe_in = Vec::with_capacity(grid.len().pow(3));
        for &r in &grid {
            for &g in &grid {
                for &b in &grid {
                    probe_in.push([r, g, b]);
                }
            }
        }
        let normalized: Vec<[f32; 3]> =
            probe_in.iter().map(|p| p.map(|v| v as f32 / max)).collect();
        let mut expected = vec![[0.0; 3]; normalized.len()];
        transform.transform(&normalized, &mut expected)?;
        let separable_ok = probe_in.iter().zip(&expected).all(|(p, e)| {
            let got = sep.lookup(*p);
            got.iter()
                .zip(e)
                .all(|(a, b)| (a - b).abs() <= SEPARABLE_TOLERANCE)
        });
        Ok(if separable_ok {
            sep
        } else {
            Self::PerPixel { transform }
        })
    }

    /// 表の和（Separable のときだけ使う）。
    fn lookup(&self, px: [u32; 3]) -> [f32; 3] {
        match self {
            Self::Separable { tables, black2 } => {
                let r = tables[0][px[0] as usize];
                let g = tables[1][px[1] as usize];
                let b = tables[2][px[2] as usize];
                [
                    r[0] + g[0] + b[0] - black2[0],
                    r[1] + g[1] + b[1] - black2[1],
                    r[2] + g[2] + b[2] - black2[2],
                ]
            }
            Self::PerPixel { .. } => unreachable!("lookup は Separable のときだけ使う"),
        }
    }

    /// 1 行（幅 × 3 要素）をリニア BT.2020 にする。
    fn row<T: Sample>(&self, row: &[T], out: &mut [[f32; 3]]) -> Result<()> {
        match self {
            Self::Separable { .. } => {
                for (o, px) in out.iter_mut().zip(row.chunks_exact(3)) {
                    *o = self.lookup([px[0].to_u32(), px[1].to_u32(), px[2].to_u32()]);
                }
                Ok(())
            }
            Self::PerPixel { transform } => {
                for (o, px) in out.iter_mut().zip(row.chunks_exact(3)) {
                    *o = [
                        px[0].to_unit_f32(),
                        px[1].to_unit_f32(),
                        px[2].to_unit_f32(),
                    ];
                }
                transform.transform_in_place(out);
                Ok(())
            }
        }
    }

    #[cfg(test)]
    fn is_separable(&self) -> bool {
        matches!(self, Self::Separable { .. })
    }

    /// 画素ごとに変換する版を作る（テストで 2 つの経路を比べるため）。
    #[cfg(test)]
    fn per_pixel(source: &IccProfile) -> Result<Self> {
        let lin = crate::profiles::standard(StandardProfile::LinearBt2020, IccVersion::V4_3)?;
        Ok(Self::PerPixel {
            transform: IccTransform::new(source, &lin, RenderingIntent::RelativeColorimetric)?,
        })
    }
}

/// リニアな値 → IEC 61966-2-1 で符号化した 8bit の値（`round(encode(v) × 255)`）を、べき乗を
/// 計算せずに求める。
///
/// 8bit の値 k になるリニアな値の範囲は `decode((k − 0.5)/255) ≤ v < decode((k + 0.5)/255)`
/// （伝達関数は単調増加）なので、255 個のしきい値の二分探索で決まる。0 以下・NaN は 0、
/// 1 以上は 255（色域外を 0〜1 に収めるのと同じ）。
struct Srgb8Encoder {
    thresholds: [f32; 255],
}

impl Srgb8Encoder {
    fn new() -> Self {
        let mut thresholds = [0.0; 255];
        for (k, t) in thresholds.iter_mut().enumerate() {
            *t = srgb_decode((k as f64 + 0.5) / 255.0) as f32;
        }
        Self { thresholds }
    }

    fn shared() -> &'static Self {
        static ENCODER: std::sync::OnceLock<Srgb8Encoder> = std::sync::OnceLock::new();
        ENCODER.get_or_init(Self::new)
    }

    fn encode(&self, v: f32) -> u8 {
        // NaN との比較は常に偽なので、NaN は 0 になる。
        self.thresholds.partition_point(|&t| t <= v) as u8
    }
}

fn render_typed<T: Sample>(
    img: &RgbImage<T>,
    source: &IccProfile,
    orientation: Orientation,
    long_edge: u32,
) -> Result<RgbImage8> {
    let (w, h) = img.dimensions();
    // 長辺は向きによらないので、記録された向きのまま縮小してから向きを反映する（軽い）。
    let (dw, dh) = ExportSize::LongEdge(long_edge).fit(w, h);
    let lin = Linearizer::new::<T>(source)?;
    let mut ds = AreaDownscaler::new(w, h, dw, dh)?;
    let mut row = vec![[0.0_f32; 3]; w as usize];
    for src_row in img.rows() {
        lin.row(src_row, &mut row)?;
        ds.push_row(&row)?;
    }
    let to_p3 = working_to(RgbColorSpace::DisplayP3).to_f32();
    let encoder = Srgb8Encoder::shared();
    let mut data = Vec::with_capacity(dw as usize * dh as usize * 3);
    for px in ds.finish()? {
        let p3 = to_p3.apply(px);
        for v in p3 {
            // 色域外は 0〜1 に収める（モジュールの説明の「仮置きの簡易な扱い」）。
            data.push(encoder.encode(v));
        }
    }
    let out = RgbImage8::from_raw(dw, dh, data)?;
    Ok(apply_orientation(&out, orientation))
}

/// B5 の画素（Display P3、IEC 61966-2-1 の伝達関数、8bit、向きを反映済み）を作る。
///
/// - `pixels`: 入力の画素（ファイルに記録された向きのまま）。
/// - `source`: 入力の色を解釈する ICC プロファイル（なければ sRGB を渡す）。
/// - `orientation`: 画素に反映する向き。
/// - `long_edge`: 長辺の画素数（入力より大きければ拡大しない）。
pub fn render_cache_rgb(
    pixels: &DynRgbImage,
    source: &IccProfile,
    orientation: Orientation,
    long_edge: u32,
) -> Result<RgbImage8> {
    if long_edge == 0 {
        return Err(MediaError::invalid_argument("長辺は 1 以上"));
    }
    match pixels {
        DynRgbImage::Rgb8(i) => render_typed(i, source, orientation, long_edge),
        DynRgbImage::Rgb16(i) => render_typed(i, source, orientation, long_edge),
    }
}

/// B5 のキャッシュの JPEG（Display P3 の ICC プロファイル付き）を作る。
pub fn render_cache_jpeg(
    pixels: &DynRgbImage,
    source: &IccProfile,
    orientation: Orientation,
    spec: CacheSpec,
) -> Result<CacheJpeg> {
    spec.validate()?;
    let rgb = render_cache_rgb(pixels, source, orientation, spec.long_edge)?;
    let icc = cache_icc_profile()?;
    let bytes = encode_jpeg(&rgb, spec.quality, Some(icc.as_bytes()), None)?;
    Ok(CacheJpeg {
        bytes,
        width: rgb.width(),
        height: rgb.height(),
    })
}

/// デコード済みの画像から B5 のキャッシュの JPEG を作る。
///
/// 向きは `orientation` が `Some` ならその値（RAW の埋め込み JPEG では RAW の向きを渡す。埋め込み
/// JPEG 自体には向きが記録されていないことが多いため）、`None` なら画像の Exif の向き。
pub fn cache_jpeg_from_decoded(
    decoded: &DecodedImage,
    orientation: Option<Orientation>,
    spec: CacheSpec,
) -> Result<CacheJpeg> {
    let source = decoded.profile.to_icc()?;
    render_cache_jpeg(
        &decoded.pixels,
        &source,
        orientation.unwrap_or(decoded.metadata.orientation),
        spec,
    )
}

/// 符号化された画像（埋め込み JPEG、元の JPEG / PNG / TIFF）から B5 のキャッシュの JPEG を作る。
///
/// 信頼できない入力をデコードするので、ワーカープロセスで呼ぶ（SEC-05）。
pub fn cache_jpeg_from_encoded(
    bytes: &[u8],
    orientation: Option<Orientation>,
    spec: CacheSpec,
) -> Result<CacheJpeg> {
    spec.validate()?;
    let decoded = decode_image_bytes(bytes)?;
    cache_jpeg_from_decoded(&decoded, orientation, spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RgbImage16;
    use crate::decode::SourceProfile;
    use genzo_color::space::{SRGB_TO_XYZ, XYZ_TO_DISPLAY_P3};
    use genzo_color::transfer::srgb_decode;

    fn srgb() -> IccProfile {
        IccProfile::standard(StandardProfile::Srgb).unwrap()
    }

    /// sRGB の 8bit の色を、行列と伝達関数で Display P3 の 8bit にした値（基準）。
    fn srgb8_to_p3_8(c: [u8; 3]) -> [f64; 3] {
        let lin = c.map(|v| srgb_decode(f64::from(v) / 255.0));
        let xyz = SRGB_TO_XYZ.apply(lin);
        let p3 = XYZ_TO_DISPLAY_P3.apply(xyz);
        p3.map(|v| genzo_color::transfer::srgb_encode(v.clamp(0.0, 1.0)) * 255.0)
    }

    fn decode_jpeg(bytes: &[u8]) -> crate::buffer::RgbImage8 {
        crate::decode::decode_image_bytes(bytes)
            .unwrap()
            .pixels
            .to_rgb8()
    }

    #[test]
    fn threshold_encoder_matches_the_transfer_function() {
        // しきい値の二分探索が、伝達関数を計算して四捨五入した値と一致する（境界のごく近くの
        // 丸め誤差による ±1 の違いだけを許す）。
        let enc = Srgb8Encoder::new();
        let n = 1_000_000;
        let mut mismatches = 0;
        for i in 0..=n {
            let v = i as f32 / n as f32;
            let reference = u8::from_unit_f32(genzo_color::transfer::srgb_encode_f32(v));
            let got = enc.encode(v);
            if got != reference {
                assert_eq!(got.abs_diff(reference), 1, "{v}");
                mismatches += 1;
            }
        }
        assert!(mismatches < 50, "{mismatches}");
        // 各段階のちょうど中央のリニアな値（f64 で計算した基準）。
        for k in 0..=255u32 {
            let v = srgb_decode(f64::from(k) / 255.0) as f32;
            assert_eq!(u32::from(enc.encode(v)), k);
        }
        assert_eq!(enc.encode(-1.0), 0);
        assert_eq!(enc.encode(f32::NAN), 0);
        assert_eq!(enc.encode(2.0), 255);
        assert_eq!(enc.encode(f32::INFINITY), 255);
    }

    #[test]
    fn srgb_primaries_match_published_display_p3_values() {
        // CSS Color 4 などで示される、sRGB の原色を Display P3 で表した値（小数 4 桁）:
        // 赤 (1, 0, 0) は color(display-p3 0.9175 0.2003 0.1386)（8bit で (234, 51, 35)）、
        // 緑 (0, 1, 0) は color(display-p3 0.4584 0.9853 0.2983)、
        // 青 (0, 0, 1) は color(display-p3 0 0 0.9596)。
        for (c, published) in [
            ([255u8, 0, 0], [0.9175, 0.2003, 0.1386]),
            ([0, 255, 0], [0.4584, 0.9853, 0.2983]),
            ([0, 0, 255], [0.0, 0.0, 0.9596]),
        ] {
            let img = DynRgbImage::Rgb8(RgbImage8::from_fn(8, 8, |_, _| c).unwrap());
            let rgb = render_cache_rgb(&img, &srgb(), Orientation::Normal, 8).unwrap();
            let px = rgb.pixel(3, 3).unwrap();
            for (got, expected) in px.iter().zip(published) {
                assert!(
                    (f64::from(*got) - expected * 255.0).abs() <= 1.0,
                    "{c:?}: {px:?}"
                );
            }
            // 同じ値を、行列と伝達関数の計算でも確かめる。
            let reference = srgb8_to_p3_8(c);
            for (got, e) in px.iter().zip(reference) {
                assert!(
                    (f64::from(*got) - e).abs() <= 0.6,
                    "{c:?}: {px:?} vs {reference:?}"
                );
            }
        }
    }

    #[test]
    fn colors_through_jpeg_match_reference() {
        for c in [
            [200u8, 100, 50],
            [20, 180, 220],
            [128, 128, 128],
            [255, 255, 255],
            [0, 0, 0],
        ] {
            let img = DynRgbImage::Rgb8(RgbImage8::from_fn(16, 16, |_, _| c).unwrap());
            let jpeg = render_cache_jpeg(
                &img,
                &srgb(),
                Orientation::Normal,
                CacheSpec {
                    long_edge: 16,
                    quality: 100,
                },
            )
            .unwrap();
            let decoded = crate::decode::decode_image_bytes(&jpeg.bytes).unwrap();
            // P3 のプロファイルが埋め込まれている。
            assert_eq!(
                decoded.profile,
                SourceProfile::Embedded(cache_icc_profile().unwrap())
            );
            assert_eq!(decoded.profile.to_icc().unwrap().version(), (4, 3));
            let px = decoded.pixels.to_rgb8().pixel(8, 8).unwrap();
            let e = srgb8_to_p3_8(c);
            for k in 0..3 {
                // JPEG（品質 100）の YCbCr の丸めの分を見込む。
                assert!(
                    (f64::from(px[k]) - e[k]).abs() <= 2.5,
                    "{c:?}: {px:?} vs {e:?}"
                );
            }
        }
    }

    #[test]
    fn embedded_profile_maps_pixels_back_to_the_original_colors() {
        // 埋め込んだ ICC（Display P3）で画素を解釈し直すと、元の sRGB の色に戻る（画素の符号化と
        // 埋め込んだプロファイルが食い違っていないこと。B5 を表示するときの 3D LUT と同じ経路）。
        for c in [[200u8, 100, 50], [20, 180, 220], [90, 90, 90], [255, 0, 0]] {
            let img = DynRgbImage::Rgb8(RgbImage8::from_fn(16, 16, |_, _| c).unwrap());
            let jpeg = render_cache_jpeg(
                &img,
                &srgb(),
                Orientation::Normal,
                CacheSpec {
                    long_edge: 16,
                    quality: 100,
                },
            )
            .unwrap();
            let decoded = crate::decode::decode_image_bytes(&jpeg.bytes).unwrap();
            let embedded = decoded.profile.to_icc().unwrap();
            let t = IccTransform::new(&embedded, &srgb(), RenderingIntent::RelativeColorimetric)
                .unwrap();
            let px = decoded.pixels.to_rgb8().pixel(8, 8).unwrap();
            let mut back = [[0.0f32; 3]];
            t.transform(&[px.map(|v| f32::from(v) / 255.0)], &mut back)
                .unwrap();
            for k in 0..3 {
                // P3 の 8bit への量子化と JPEG（品質 100）の丸めの分を見込む。
                assert!(
                    (f64::from(back[0][k]) * 255.0 - f64::from(c[k])).abs() <= 2.5,
                    "{c:?}: {px:?} → {back:?}"
                );
            }
        }
    }

    #[test]
    fn downscaling_averages_in_linear_light() {
        // 1 画素の白黒の市松模様を 1/2 にすると、リニアで 0.5 → 符号化して 188（P3 と sRGB は
        // 白色点と伝達関数が同じなので、無彩色は同じ値）。
        let img = DynRgbImage::Rgb8(
            RgbImage8::from_fn(
                64,
                32,
                |x, y| if (x + y) % 2 == 0 { [0; 3] } else { [255; 3] },
            )
            .unwrap(),
        );
        let rgb = render_cache_rgb(&img, &srgb(), Orientation::Normal, 32).unwrap();
        assert_eq!(rgb.dimensions(), (32, 16));
        for px in rgb.as_raw().chunks_exact(3) {
            for v in px {
                assert!(v.abs_diff(188) <= 1, "{px:?}");
            }
        }
    }

    #[test]
    fn orientation_is_applied_after_downscale() {
        // 左半分が赤、右半分が青の 40×20。向き 6（時計回りに 90 度）→ 上半分が赤、下半分が青の 10×20。
        let img = DynRgbImage::Rgb8(
            RgbImage8::from_fn(
                40,
                20,
                |x, _| if x < 20 { [255, 0, 0] } else { [0, 0, 255] },
            )
            .unwrap(),
        );
        let rgb = render_cache_rgb(&img, &srgb(), Orientation::Rotate90Cw, 20).unwrap();
        assert_eq!(rgb.dimensions(), (10, 20));
        let top = rgb.pixel(5, 2).unwrap();
        let bottom = rgb.pixel(5, 17).unwrap();
        assert!(top[0] > 200 && top[2] < 60, "{top:?}");
        assert!(bottom[2] > 200 && bottom[0] < 60, "{bottom:?}");
    }

    #[test]
    fn sizes_follow_long_edge_and_never_upscale() {
        let img = DynRgbImage::Rgb8(RgbImage8::new(300, 200).unwrap());
        let j =
            render_cache_jpeg(&img, &srgb(), Orientation::Normal, CacheSpec::L0_THUMBNAIL).unwrap();
        assert_eq!((j.width, j.height), (300, 200));
        let j = render_cache_jpeg(
            &img,
            &srgb(),
            Orientation::Rotate270Cw,
            CacheSpec {
                long_edge: 64,
                quality: 80,
            },
        )
        .unwrap();
        // 200 × 64 / 300 = 42.67 → 43。向き 8 で縦長になる。
        assert_eq!((j.width, j.height), (43, 64));
        let d = decode_jpeg(&j.bytes);
        assert_eq!(d.dimensions(), (43, 64));
    }

    #[test]
    fn spec_validation() {
        CacheSpec::L0_THUMBNAIL.validate().unwrap();
        CacheSpec::L1_PREVIEW.validate().unwrap();
        assert!(
            CacheSpec {
                long_edge: 0,
                quality: 80
            }
            .validate()
            .is_err()
        );
        assert!(
            CacheSpec {
                long_edge: 100,
                quality: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            CacheSpec {
                long_edge: 70_000,
                quality: 80
            }
            .validate()
            .is_err()
        );
        let img = DynRgbImage::Rgb8(RgbImage8::new(2, 2).unwrap());
        assert!(render_cache_rgb(&img, &srgb(), Orientation::Normal, 0).is_err());
    }

    #[test]
    fn separable_tables_match_per_pixel_transform() {
        // matrix-shaper のプロファイル（v2 の表のトーンカーブ、v4 の区分関数、Adobe RGB のガンマ）で、
        // 表の和の計算が lcms2 の画素ごとの変換と一致する。
        let pixels: Vec<[u8; 3]> = (0..500u32)
            .map(|i| {
                let h = i.wrapping_mul(2_654_435_761);
                [(h >> 8) as u8, (h >> 16) as u8, (h >> 24) as u8]
            })
            .collect();
        for (kind, version) in [
            (StandardProfile::Srgb, IccVersion::V2_4),
            (StandardProfile::Srgb, IccVersion::V4_3),
            (StandardProfile::AdobeRgb1998, IccVersion::V4_3),
            (StandardProfile::DisplayP3, IccVersion::V2_4),
        ] {
            let src = IccProfile::standard_with_version(kind, version).unwrap();
            let lin = Linearizer::new::<u8>(&src).unwrap();
            assert!(lin.is_separable(), "{kind:?} {version:?}");
            let row: Vec<u8> = pixels.iter().flatten().copied().collect();
            let mut ours = vec![[0.0; 3]; pixels.len()];
            lin.row(&row, &mut ours).unwrap();
            let t = IccTransform::new(
                &src,
                &IccProfile::standard(StandardProfile::LinearBt2020).unwrap(),
                RenderingIntent::RelativeColorimetric,
            )
            .unwrap();
            let input: Vec<[f32; 3]> = pixels
                .iter()
                .map(|p| p.map(|v| f32::from(v) / 255.0))
                .collect();
            let mut theirs = vec![[0.0; 3]; pixels.len()];
            t.transform(&input, &mut theirs).unwrap();
            for (a, b) in ours.iter().zip(&theirs) {
                for k in 0..3 {
                    assert!((a[k] - b[k]).abs() < 2e-5, "{kind:?}: {a:?} vs {b:?}");
                }
            }
        }
    }

    #[test]
    fn per_pixel_path_matches_separable_path() {
        // LUT のプロファイルで使う画素ごとの経路も、同じ結果になる（8bit と 16bit）。
        for kind in [StandardProfile::AdobeRgb1998, StandardProfile::DisplayP3] {
            let src = IccProfile::standard(kind).unwrap();
            let row8: Vec<u8> = (0..300u32).map(|i| (i * 37 % 256) as u8).collect();
            let row16: Vec<u16> = (0..300u32).map(|i| (i * 7919 % 65536) as u16).collect();
            let mut a = vec![[0.0; 3]; 100];
            let mut b = vec![[0.0; 3]; 100];
            Linearizer::new::<u8>(&src)
                .unwrap()
                .row(&row8, &mut a)
                .unwrap();
            Linearizer::per_pixel(&src)
                .unwrap()
                .row(&row8, &mut b)
                .unwrap();
            for (x, y) in a.iter().zip(&b) {
                for k in 0..3 {
                    assert!((x[k] - y[k]).abs() < 2e-5, "{kind:?}: {x:?} vs {y:?}");
                }
            }
            Linearizer::new::<u16>(&src)
                .unwrap()
                .row(&row16, &mut a)
                .unwrap();
            Linearizer::per_pixel(&src)
                .unwrap()
                .row(&row16, &mut b)
                .unwrap();
            for (x, y) in a.iter().zip(&b) {
                for k in 0..3 {
                    assert!((x[k] - y[k]).abs() < 2e-5, "{kind:?} 16bit: {x:?} vs {y:?}");
                }
            }
        }
    }

    #[test]
    fn sixteen_bit_input_and_adobe_rgb_source() {
        // Adobe RGB の 16bit の中間の灰色は、P3 でも同じ明るさの灰色（白色点が同じ D65）。
        let adobe = IccProfile::standard(StandardProfile::AdobeRgb1998).unwrap();
        let v = 0.5_f64;
        let encoded16 = (v * 65535.0).round() as u16;
        let img = DynRgbImage::Rgb16(RgbImage16::from_fn(4, 4, |_, _| [encoded16; 3]).unwrap());
        let rgb = render_cache_rgb(&img, &adobe, Orientation::Normal, 4).unwrap();
        let linear = genzo_color::transfer::adobe_rgb_decode(v);
        let expected = genzo_color::transfer::srgb_encode(linear) * 255.0;
        for px in rgb.as_raw().chunks_exact(3) {
            for &c in px {
                assert!(
                    (f64::from(c) - expected).abs() <= 1.0,
                    "{px:?} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn from_encoded_uses_exif_orientation_or_override() {
        use crate::exif_write::{ByteOrder, Entry, ExifIfds, Value, encode_exif_tiff, tag};
        // 左が赤・右が青の 8×4 の JPEG に、向き 6 の Exif を入れる。
        let img =
            RgbImage8::from_fn(8, 4, |x, _| if x < 4 { [255, 0, 0] } else { [0, 0, 255] }).unwrap();
        let exif = encode_exif_tiff(
            &ExifIfds {
                ifd0: vec![Entry {
                    tag: tag::ORIENTATION,
                    value: Value::Short(vec![6]),
                }],
                exif: vec![],
                gps: None,
            },
            ByteOrder::Big,
        )
        .unwrap();
        let jpeg = encode_jpeg(&img, 95, None, Some(&exif)).unwrap();
        let spec = CacheSpec {
            long_edge: 8,
            quality: 95,
        };
        let from_exif = cache_jpeg_from_encoded(&jpeg, None, spec).unwrap();
        assert_eq!((from_exif.width, from_exif.height), (4, 8));
        let d = decode_jpeg(&from_exif.bytes);
        assert!(d.pixel(2, 1).unwrap()[0] > 200, "上が赤");
        // RAW の向き（ここでは 1）で上書きする。
        let overridden = cache_jpeg_from_encoded(&jpeg, Some(Orientation::Normal), spec).unwrap();
        assert_eq!((overridden.width, overridden.height), (8, 4));
        assert!(cache_jpeg_from_encoded(b"broken", None, spec).is_err());
    }
}
