//! 透かし（EXP-03。docs/04_architecture.md の 2.4 節「書き出しの仕上げ」・6.4 節）。
//!
//! 書き出しの縮小・出力シャープの後に、出力の色空間に符号化した 16bit の画素へ重ねる。
//!
//! | 処理 | 関数 | 動く場所 |
//! |---|---|---|
//! | 大きさ・余白・位置（書き出す画像の短辺に対する割合から画素に） | [`WatermarkGeometry::new`]・[`place`] | 本体 |
//! | 画像の透かし（PNG・JPEG・TIFF。透明度を保ち、埋め込みの ICC プロファイルで出力の色空間へ） | [`image_watermark`] | ワーカー（SEC-05） |
//! | 文字の透かし（利用者の OS のフォントのファイルを skrifa で読み、自前のラスタライザで描く） | [`text_watermark`] | ワーカー（SEC-05） |
//! | 合成（`out = dst × (1 − a) + mark × a`、a = アルファ × 不透明度。出力の色空間の符号化した値で） | [`composite`] | 本体 |
//!
//! 透かしの画像・フォントは利用者が選んだファイルで、写真と同じく信頼できない入力として扱う（デコード・ICC の解析・
//! フォントの解析はワーカーで行い、本体は出来上がった RGBA を合成するだけ。決定事項ログ No.167）。フォントはアプリに
//! 同梱しない・ダウンロードしない（決定事項ログ No.167）。
//!
//! # 文字
//!
//! - 改行で複数の行（[`genzo_model::Watermark::MAX_TEXT_LINES`] まで）。行は位置の横の揃え（左・中央・右）で揃える。
//! - 字形は文字ごとに `cmap` で引き、送り幅（`hmtx`）で並べる。カーニング・合字・字形の置き換え（OpenType の GSUB・
//!   GPOS）・縦書き・右から左の文字は扱わない（アラビア文字・インドの文字などは正しく並ばない）。フォントにない文字は
//!   `.notdef`（多くのフォントで四角）で描き、[`TextWatermark::missing`] で知らせる。
//! - 大きさは文字の大きさ（em。`size_px`）。形の外接矩形が `max_width` × `max_height` に収まらなければ、縦横比を
//!   保って縮める（[`TextWatermark::shrunk`]）。ヒンティングはしない。
//! - ラスタライズ: 字形の輪郭（直線・2 次・3 次のベジェ曲線を線分に分ける）を、1 画素を [`TEXT_SUBSAMPLES`] 本の
//!   横の線で調べ、各線で非ゼロ回転数の規則で塗る区間の横の長さ（画素の中での割合まで）を足して、覆われた割合にする。
//! - フォントの解析は skrifa（Google Fonts の fontations。決定事項ログ No.173。以前は ttf-parser）。輪郭は TrueType
//!   （`glyf`。合成の字形を含む）・CFF・CFF2 を、ヒンティングなし・フォントの単位のまま読む。可変フォントは既定の形
//!   （軸を動かさない）。ビットマップ・カラーの字形（絵文字など）は描かない。`glyf` の字形は FreeType と同じく、
//!   `hmtx` の左の余白に合わせて横に置く（字形の xMin と左の余白が違うフォントでは、ttf-parser のころより
//!   その差だけ横にずれる。確かめたフォントでは 1 単位）。
//! - フォントは信頼できない入力なので、読む前に必須の表（`head`・`hhea`・`maxp`）を確かめ（`open_font`）、
//!   輪郭の要素の数（[`MAX_TEXT_OUTLINE_ELEMENTS`]）と線分の数（[`MAX_TEXT_EDGES`]）に上限を置く。輪郭を読めない
//!   字形は描かない（形がないものとして扱う）。
//!
//! # 画像
//!
//! - 幅を `size_px` にし（高さは縦横比から）、`max_width` × `max_height` に収まらなければ縮める。小さい画像は拡大する。
//! - 色は、埋め込みの ICC プロファイル（ない・使えなければ sRGB。写真と同じ解釈）からリニア BT.2020 に変え、アルファを
//!   掛けた値で縮小・拡大（三角形の重み。縮小は倍率の幅）してから、出力の色空間に変えて符号化する（色域の外は切り
//!   そろえる）。Exif の向きは反映する。

use std::collections::HashMap;

use genzo_color::space::working_to;
use genzo_color::transfer::srgb_decode_f32;
use genzo_color::{IccTransform, IccVersion, RenderingIntent, RgbColorSpace, StandardProfile};
use genzo_model::{Align, Orientation, OutputColorSpace, RgbColor, Watermark, WatermarkAnchor};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::{LocationRef, Size};
use skrifa::raw::tables::cmap::{CmapSubtable, PlatformId};
use skrifa::raw::types::Version16Dot16;
use skrifa::raw::{CollectionRef, ReadError, TableProvider};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::decode::decode_image_rgba_bytes;
use crate::error::{MediaError, Result};
use crate::orientation::source_coord;

/// 透かしの画像のファイルの大きさの上限（バイト。仮置き）。
pub const MAX_WATERMARK_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
/// 透かしの画像の画素数の上限（仮置き。ロゴなどの用途には十分）。
pub const MAX_WATERMARK_SOURCE_PIXELS: u64 = 64_000_000;
/// フォントのファイルの大きさの上限（バイト。仮置き。日本語のフォントのコレクションでも数十 MB）。
pub const MAX_FONT_FILE_BYTES: u64 = 128 * 1024 * 1024;
/// 出来上がった透かしの画素数の上限（仮置き。RGBA 16bit で 320 MB）。
pub const MAX_WATERMARK_PIXELS: u64 = 40_000_000;
/// 文字のラスタライズで 1 画素を調べる横の線の数。
pub const TEXT_SUBSAMPLES: usize = 16;
/// 文字の透かしの字形の輪郭の要素（移動・直線・曲線・閉じる）の数の上限（すべての字形の合計。同じ字形は 1 回だけ
/// 数える。仮置き）。壊れた・悪意のあるフォントで記憶を使い尽くさないため。1 字形の要素の数は、Linux のフォントで
/// 多くて 211（IPA ゴシック）〜594（DejaVu Sans）で、200 文字がすべて違う字形でも十分に収まる。
pub const MAX_TEXT_OUTLINE_ELEMENTS: u64 = 1_000_000;
/// 文字の透かしのラスタライズの線分の数の上限（曲線を分けた後。仮置き。1 本 20 バイトで 40 MB）。
pub const MAX_TEXT_EDGES: u64 = 2_000_000;
/// 曲線を線分に分けるときの許容差（画素）。
const FLATTEN_TOLERANCE: f32 = 0.2;

/// 書き出す画像の大きさから決めた、透かしの大きさ・余白（画素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatermarkGeometry {
    /// 大きさ（文字は文字の大きさ（em）、画像は幅。短辺 × 大きさ %。1 以上）。
    pub size_px: u32,
    /// 余白（短辺 × 余白 %）。
    pub margin_px: u32,
    /// 透かしの幅の上限（画像の幅 − 左右の余白。1 以上）。
    pub max_width: u32,
    /// 透かしの高さの上限（画像の高さ − 上下の余白。1 以上）。
    pub max_height: u32,
}

impl WatermarkGeometry {
    /// `width` × `height` の画像に `w` を付けるときの大きさ（四捨五入）。
    pub fn new(w: &Watermark, width: u32, height: u32) -> Self {
        let short = u64::from(width.min(height));
        let pct = |p: u8| ((short * u64::from(p) + 50) / 100) as u32;
        let margin = pct(w.margin);
        Self {
            size_px: pct(w.size).max(1),
            margin_px: margin,
            max_width: width.saturating_sub(margin.saturating_mul(2)).max(1),
            max_height: height.saturating_sub(margin.saturating_mul(2)).max(1),
        }
    }
}

/// 透かし（`mark` の寸法）を置く左上の位置（`image` の寸法の中。中央は余白を使わない）。
pub fn place(
    anchor: WatermarkAnchor,
    image: (u32, u32),
    mark: (u32, u32),
    margin: u32,
) -> (i64, i64) {
    let pos = |align: Align, total: u32, size: u32| -> i64 {
        let (total, size, margin) = (i64::from(total), i64::from(size), i64::from(margin));
        match align {
            Align::Start => margin,
            Align::Center => (total - size).div_euclid(2),
            Align::End => total - margin - size,
        }
    };
    (
        pos(anchor.horizontal(), image.0, mark.0),
        pos(anchor.vertical(), image.1, mark.1),
    )
}

/// 透かしの画素（R, G, B, A の 16bit。RGB は出力の色空間に符号化した値、アルファは乗算していない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatermarkBitmap {
    width: u32,
    height: u32,
    rgba: Vec<u16>,
}

impl WatermarkBitmap {
    /// 寸法と画素から作る（長さ・寸法・画素数の上限を確かめる）。
    pub fn new(width: u32, height: u32, rgba: Vec<u16>) -> Result<Self> {
        let pixels = u64::from(width) * u64::from(height);
        if width == 0 || height == 0 {
            return Err(MediaError::invalid_image(format!(
                "透かしの寸法が不正（{width}×{height}）"
            )));
        }
        if pixels > MAX_WATERMARK_PIXELS {
            return Err(MediaError::TooLarge {
                what: "透かしの画素数",
                actual: pixels,
                max: MAX_WATERMARK_PIXELS,
            });
        }
        if rgba.len() as u64 != pixels * 4 {
            return Err(MediaError::invalid_image(format!(
                "透かしの画素の数が寸法と合わない（{} 個、{width}×{height}）",
                rgba.len()
            )));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// 幅。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さ。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 画素（R, G, B, A, …）。
    pub fn rgba(&self) -> &[u16] {
        &self.rgba
    }

    /// 画素を取り出す。
    pub fn into_raw(self) -> Vec<u16> {
        self.rgba
    }
}

/// 16bit の RGB（R, G, B, … の並び。出力の色空間に符号化した値）に透かしを重ねる（その場で置き換える）。`at` は
/// 透かしの左上の位置（画像の外にはみ出した部分は描かない）。`opacity` は 0〜1。
pub fn composite(
    dst: &mut [u16],
    width: u32,
    height: u32,
    mark: &WatermarkBitmap,
    at: (i64, i64),
    opacity: f32,
) -> Result<()> {
    if dst.len() as u64 != u64::from(width) * u64::from(height) * 3 {
        return Err(MediaError::invalid_argument(format!(
            "透かしを重ねる画像の画素の数が寸法と合わない（{} 個、{width}×{height}）",
            dst.len()
        )));
    }
    let opacity = if opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if opacity == 0.0 {
        return Ok(());
    }
    let (w, mw) = (width as usize, mark.width as usize);
    for my in 0..mark.height as usize {
        let y = at.1 + my as i64;
        if y < 0 || y >= i64::from(height) {
            continue;
        }
        let row = &mut dst[y as usize * w * 3..(y as usize + 1) * w * 3];
        let src = &mark.rgba[my * mw * 4..(my + 1) * mw * 4];
        for (mx, m) in src.chunks_exact(4).enumerate() {
            let x = at.0 + mx as i64;
            if x < 0 || x >= i64::from(width) || m[3] == 0 {
                continue;
            }
            let a = f32::from(m[3]) / 65535.0 * opacity;
            let p = &mut row[x as usize * 3..x as usize * 3 + 3];
            for c in 0..3 {
                let v = f32::from(p[c]) * (1.0 - a) + f32::from(m[c]) * a;
                p[c] = (v + 0.5).floor().clamp(0.0, 65535.0) as u16;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 色
// ---------------------------------------------------------------------------

fn rgb_space(space: OutputColorSpace) -> RgbColorSpace {
    match space {
        OutputColorSpace::Srgb => RgbColorSpace::Srgb,
        OutputColorSpace::DisplayP3 => RgbColorSpace::DisplayP3,
        OutputColorSpace::AdobeRgb => RgbColorSpace::AdobeRgb1998,
    }
}

/// リニアな値（0〜1 に切りそろえる）を符号化して 16bit にする。
fn encode16(space: RgbColorSpace, v: f32) -> u16 {
    let e = space.transfer_function().encode_f32(v.clamp(0.0, 1.0));
    (e * 65535.0 + 0.5).floor().clamp(0.0, 65535.0) as u16
}

/// sRGB（8bit）の色を、出力の色空間に符号化した 16bit の値にする（色域の外は切りそろえる）。
pub fn color_in(space: OutputColorSpace, color: RgbColor) -> [u16; 3] {
    let rgb = rgb_space(space);
    let lin = color.0.map(|v| srgb_decode_f32(f32::from(v) / 255.0));
    let m = rgb
        .from_xyz()
        .mul_mat(&RgbColorSpace::Srgb.to_xyz())
        .to_f32();
    m.apply(lin).map(|v| encode16(rgb, v))
}

// ---------------------------------------------------------------------------
// 画像の透かし
// ---------------------------------------------------------------------------

/// 元の寸法 `src` の画像を、幅 `width_px`（高さは縦横比から）にして `max` に収める寸法。
pub fn fit_image_size(src: (u32, u32), width_px: u32, max: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (f64::from(src.0.max(1)), f64::from(src.1.max(1)));
    let mut w = f64::from(width_px.max(1));
    let mut h = (w * sh / sw).max(1.0);
    let f = (f64::from(max.0) / w).min(f64::from(max.1) / h);
    if f < 1.0 {
        w *= f;
        h *= f;
    }
    (
        (w.round() as u32).clamp(1, max.0.max(1)),
        (h.round() as u32).clamp(1, max.1.max(1)),
    )
}

/// 透かしの画像（JPEG・PNG・TIFF のバイト列）から、幅 `width_px` の透かしを作る（モジュールの doc の「画像」）。
pub fn image_watermark(
    bytes: &[u8],
    width_px: u32,
    max_width: u32,
    max_height: u32,
    space: OutputColorSpace,
) -> Result<WatermarkBitmap> {
    let d = decode_image_rgba_bytes(bytes)?;
    let pixels = u64::from(d.width) * u64::from(d.height);
    if pixels > MAX_WATERMARK_SOURCE_PIXELS {
        return Err(MediaError::TooLarge {
            what: "透かしの画像の画素数",
            actual: pixels,
            max: MAX_WATERMARK_SOURCE_PIXELS,
        });
    }
    let (sw, sh, rgba) = orient_rgba(d.width, d.height, d.rgba, d.orientation);
    // 元の色 → リニア BT.2020（アルファを掛ける）。
    let source = d.profile.to_icc()?;
    let lin = crate::profiles::standard(StandardProfile::LinearBt2020, IccVersion::V4_3)?;
    let transform = IccTransform::new(&source, &lin, RenderingIntent::RelativeColorimetric)?;
    let mut premul: Vec<[f32; 4]> = Vec::with_capacity(rgba.len() / 4);
    const CHUNK: usize = 65_536;
    let mut buf_in = Vec::with_capacity(CHUNK);
    let mut buf_out = vec![[0.0f32; 3]; CHUNK];
    for chunk in rgba.chunks(CHUNK * 4) {
        buf_in.clear();
        buf_in.extend(chunk.chunks_exact(4).map(|p| {
            [
                f32::from(p[0]) / 65535.0,
                f32::from(p[1]) / 65535.0,
                f32::from(p[2]) / 65535.0,
            ]
        }));
        let out = &mut buf_out[..buf_in.len()];
        transform.transform(&buf_in, out)?;
        for (c, p) in out.iter().zip(chunk.chunks_exact(4)) {
            let a = f32::from(p[3]) / 65535.0;
            premul.push([c[0] * a, c[1] * a, c[2] * a, a]);
        }
    }
    let (dw, dh) = fit_image_size((sw, sh), width_px, (max_width, max_height));
    let pixels_out = u64::from(dw) * u64::from(dh);
    if pixels_out > MAX_WATERMARK_PIXELS {
        return Err(MediaError::TooLarge {
            what: "透かしの画素数",
            actual: pixels_out,
            max: MAX_WATERMARK_PIXELS,
        });
    }
    let resized = resample_rgba(&premul, sw, sh, dw, dh);
    let rgb = rgb_space(space);
    let m = working_to(rgb).to_f32();
    let mut out = Vec::with_capacity(resized.len() * 4);
    for p in &resized {
        let a = p[3].clamp(0.0, 1.0);
        let c = if a > 0.0 {
            m.apply([p[0] / a, p[1] / a, p[2] / a])
        } else {
            [0.0; 3]
        };
        out.extend_from_slice(&[
            encode16(rgb, c[0]),
            encode16(rgb, c[1]),
            encode16(rgb, c[2]),
            (a * 65535.0 + 0.5).floor() as u16,
        ]);
    }
    WatermarkBitmap::new(dw, dh, out)
}

/// RGBA に Exif の向きを反映する（寸法と画素）。
fn orient_rgba(w: u32, h: u32, rgba: Vec<u16>, o: Orientation) -> (u32, u32, Vec<u16>) {
    if o == Orientation::Normal {
        return (w, h, rgba);
    }
    let (dw, dh) = if o.swaps_dimensions() { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(rgba.len());
    for dy in 0..dh {
        for dx in 0..dw {
            let (sx, sy) = source_coord(o, dx, dy, w, h);
            let i = (sy as usize * w as usize + sx as usize) * 4;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    (dw, dh, out)
}

/// 1 方向の重み（出力の各画素の、入力の先頭の番号と重みの列）。三角形の重みで、縮小は倍率の幅に広げる。
fn axis_weights(src: u32, dst: u32) -> Vec<(usize, Vec<f32>)> {
    let scale = f64::from(src) / f64::from(dst);
    let half = scale.max(1.0);
    (0..dst)
        .map(|i| {
            let center = (f64::from(i) + 0.5) * scale - 0.5;
            let lo = (center - half).ceil().max(0.0) as i64;
            let hi = ((center + half).floor() as i64).min(i64::from(src) - 1);
            let mut ws: Vec<f32> = (lo..=hi)
                .map(|k| (1.0 - (k as f64 - center).abs() / half).max(0.0) as f32)
                .collect();
            let sum: f32 = ws.iter().sum();
            if sum > 0.0 {
                for w in &mut ws {
                    *w /= sum;
                }
                (lo.max(0) as usize, ws)
            } else {
                // 重みがすべて 0（倍率が極端なとき）: 最も近い画素。
                let k = center.round().clamp(0.0, f64::from(src - 1)) as usize;
                (k, vec![1.0])
            }
        })
        .collect()
}

/// アルファを掛けた RGBA（f32）を `dw` × `dh` にする（横、縦の順）。
fn resample_rgba(src: &[[f32; 4]], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<[f32; 4]> {
    if (sw, sh) == (dw, dh) {
        return src.to_vec();
    }
    let xs = axis_weights(sw, dw);
    let ys = axis_weights(sh, dh);
    let (sw, dw) = (sw as usize, dw as usize);
    let mut tmp = vec![[0.0f32; 4]; dw * sh as usize];
    for y in 0..sh as usize {
        let line = &src[y * sw..(y + 1) * sw];
        for (x, (start, ws)) in xs.iter().enumerate() {
            let mut acc = [0.0f32; 4];
            for (k, w) in ws.iter().enumerate() {
                let p = line[start + k];
                for c in 0..4 {
                    acc[c] += w * p[c];
                }
            }
            tmp[y * dw + x] = acc;
        }
    }
    let mut out = vec![[0.0f32; 4]; dw * dh as usize];
    for (y, (start, ws)) in ys.iter().enumerate() {
        for x in 0..dw {
            let mut acc = [0.0f32; 4];
            for (k, w) in ws.iter().enumerate() {
                let p = tmp[(start + k) * dw + x];
                for c in 0..4 {
                    acc[c] += w * p[c];
                }
            }
            out[y * dw + x] = acc;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 文字の透かし
// ---------------------------------------------------------------------------

/// 文字の透かしの結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextWatermark {
    /// 透かしの画素。
    pub bitmap: WatermarkBitmap,
    /// フォントにない文字（重ならないように。現れた順。`.notdef` で描いた）。
    pub missing: Vec<char>,
    /// 収めるために文字の大きさを縮めたか。
    pub shrunk: bool,
}

/// 並べた 1 つの字形（フォントの単位。y は上向き、基線からの値）。
struct Placed {
    glyph: GlyphId,
    /// 字形の原点の x（行の左端から）。
    x: f32,
    /// 行の番号。
    line: usize,
}

/// 輪郭の線分（画素の座標。y は下向き。`y0 < y1`、`dir` は元の向き）。
#[derive(Debug, Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    dir: i32,
}

/// 字形の輪郭の要素（フォントの単位。y は上向き）。
#[derive(Debug, Clone, Copy)]
enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// 読んだ字形の輪郭（同じ字形は 1 回だけ読む）。
#[derive(Debug, Default)]
struct GlyphOutline {
    segments: Vec<Segment>,
    /// 外接矩形（x の最小・y の最小・x の最大・y の最大。フォントの単位）。曲線の制御点を含む。置き換える前
    /// （ttf-parser の `glyph_bounding_box`）と同じく、`glyf` の字形の見出しの値ではなく輪郭から求める（見出しの値は
    /// 壊れていることがあるため）。輪郭がなければ `None`。
    bounds: Option<[f32; 4]>,
}

/// skrifa が読んだ輪郭を [`GlyphOutline`] に記録するペン。要素の数（すべての字形の合計）が
/// [`MAX_TEXT_OUTLINE_ELEMENTS`] を超えたら、記録をやめて数だけ数える。
struct OutlineRecorder<'a> {
    outline: &'a mut GlyphOutline,
    count: &'a mut u64,
}

impl OutlineRecorder<'_> {
    fn add(&mut self, segment: Segment, points: &[(f32, f32)]) {
        *self.count += 1;
        if *self.count > MAX_TEXT_OUTLINE_ELEMENTS {
            return;
        }
        for &(x, y) in points {
            let b = self.outline.bounds.get_or_insert([x, y, x, y]);
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        self.outline.segments.push(segment);
    }
}

impl OutlinePen for OutlineRecorder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.add(Segment::Move(x, y), &[(x, y)]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.add(Segment::Line(x, y), &[(x, y)]);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.add(Segment::Quad(cx0, cy0, x, y), &[(cx0, cy0), (x, y)]);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.add(
            Segment::Cubic(cx0, cy0, cx1, cy1, x, y),
            &[(cx0, cy0), (cx1, cy1), (x, y)],
        );
    }

    fn close(&mut self) {
        self.add(Segment::Close, &[]);
    }
}

/// 字形の輪郭を、画素の座標の線分にする（skrifa の [`OutlinePen`]。記録した輪郭を流し込む）。線分の数
/// （すべての字形の合計）が [`MAX_TEXT_EDGES`] を超えたら、線分を足すのをやめて数だけ数える。
struct EdgeBuilder<'a> {
    edges: &'a mut Vec<Edge>,
    count: &'a mut u64,
    /// フォントの単位 → 画素の倍率。
    scale: f32,
    /// 字形の原点の画素の座標。
    ox: f32,
    oy: f32,
    start: (f32, f32),
    cur: (f32, f32),
}

impl EdgeBuilder<'_> {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (self.ox + x * self.scale, self.oy - y * self.scale)
    }

    fn push(&mut self, to: (f32, f32)) {
        let (a, b) = (self.cur, to);
        if a.1 != b.1 {
            *self.count += 1;
            if *self.count <= MAX_TEXT_EDGES {
                let (dir, p, q) = if a.1 < b.1 { (1, a, b) } else { (-1, b, a) };
                self.edges.push(Edge {
                    x0: p.0,
                    y0: p.1,
                    x1: q.0,
                    y1: q.1,
                    dir,
                });
            }
        }
        self.cur = to;
    }
}

/// 曲線を線分に分ける数（制御点と弦の距離から）。
fn segments_for(dist: f32) -> usize {
    ((dist / FLATTEN_TOLERANCE).sqrt().ceil() as usize).clamp(1, 64)
}

impl OutlinePen for EdgeBuilder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.point(x, y);
        self.start = p;
        self.cur = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.point(x, y);
        self.push(p);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p0 = self.cur;
        let c = self.point(x1, y1);
        let p1 = self.point(x, y);
        let dist = (p0.0 - 2.0 * c.0 + p1.0).hypot(p0.1 - 2.0 * c.1 + p1.1);
        let n = segments_for(dist);
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            self.push((
                u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
                u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
            ));
        }
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p0 = self.cur;
        let c1 = self.point(x1, y1);
        let c2 = self.point(x2, y2);
        let p1 = self.point(x, y);
        let d1 = (p0.0 - 2.0 * c1.0 + c2.0).hypot(p0.1 - 2.0 * c1.1 + c2.1);
        let d2 = (c1.0 - 2.0 * c2.0 + p1.0).hypot(c1.1 - 2.0 * c2.1 + p1.1);
        let n = segments_for(d1.max(d2) * 1.5);
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.push((
                a * p0.0 + b * c1.0 + c * c2.0 + d * p1.0,
                a * p0.1 + b * c1.1 + c * c2.1 + d * p1.1,
            ));
        }
    }

    fn close(&mut self) {
        let s = self.start;
        self.push(s);
    }
}

/// 横の線 1 本の区間 `[xa, xb)` を、重み `w` で 1 行（`row`）に足す（端の画素は覆う長さの割合）。
fn add_span(row: &mut [f32], xa: f32, xb: f32, w: f32) {
    let width = row.len() as f32;
    let (xa, xb) = (xa.clamp(0.0, width), xb.clamp(0.0, width));
    if xb <= xa {
        return;
    }
    let ia = xa.floor() as usize;
    let ib = xb.floor() as usize;
    if ia == ib {
        row[ia.min(row.len() - 1)] += (xb - xa) * w;
        return;
    }
    row[ia] += (ia as f32 + 1.0 - xa) * w;
    for v in &mut row[ia + 1..ib] {
        *v += w;
    }
    if ib < row.len() {
        row[ib] += (xb - ib as f32) * w;
    }
}

/// 線分の集まりを、`width` × `height` の覆われた割合（0〜1）にする（非ゼロ回転数の規則）。
fn rasterize(edges: &[Edge], width: usize, height: usize) -> Vec<f32> {
    let mut cov = vec![0.0f32; width * height];
    let mut order: Vec<usize> = (0..edges.len()).collect();
    order.sort_by(|&a, &b| edges[a].y0.total_cmp(&edges[b].y0));
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    let w = 1.0 / TEXT_SUBSAMPLES as f32;
    for y in 0..height {
        let row = &mut cov[y * width..(y + 1) * width];
        for k in 0..TEXT_SUBSAMPLES {
            let sy = y as f32 + (k as f32 + 0.5) * w;
            while next < order.len() && edges[order[next]].y0 <= sy {
                active.push(order[next]);
                next += 1;
            }
            active.retain(|&i| edges[i].y1 > sy);
            xs.clear();
            for &i in &active {
                let e = &edges[i];
                if e.y0 <= sy {
                    let t = (sy - e.y0) / (e.y1 - e.y0);
                    xs.push((e.x0 + t * (e.x1 - e.x0), e.dir));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            let mut start = 0.0;
            for &(x, d) in &xs {
                let before = wind;
                wind += d;
                if before == 0 && wind != 0 {
                    start = x;
                } else if before != 0 && wind == 0 {
                    add_span(row, start, x, w);
                }
            }
        }
    }
    for v in &mut cov {
        *v = v.min(1.0);
    }
    cov
}

/// フォントのファイルの先頭の 4 バイト（TrueType・古い Mac の TrueType（`true`）・OpenType の CFF（`OTTO`）・
/// コレクション（`ttcf`））。
const FONT_MAGICS: [[u8; 4]; 4] = [[0, 1, 0, 0], *b"true", *b"OTTO", *b"ttcf"];

/// フォントを読めないときのエラー。`detail` の英語は、置き換える前（ttf-parser）のエラーの文言をそのまま使う
/// （利用者に見える文言を変えないため。決定事項ログ No.173）。
fn unreadable_font(detail: &str) -> MediaError {
    MediaError::decode(format!("フォントを読めません: {detail}"))
}

/// フォントのファイル（`data`）の `index` 番目のフォントを開き、em（フォントの単位。16〜16384）と返す。番号の誤りは
/// 引数の誤り（[`MediaError::invalid_argument`]）、そのほかは読めないフォント（[`MediaError::decode`]）。
///
/// 置き換える前（ttf-parser の `Face::parse`）と同じ順に確かめる: 先頭の 4 バイト → コレクションのフォントの数と
/// 番号（コレクションでなければ番号は 0 だけ） → 表の一覧（途中で切れていないか） → 必須の表（`head`: em が 16〜16384・`loca` の形式が 0 か
/// 1、`hhea`、`maxp`: 版が 0.5 か 1.0・字形が 1 つ以上）。表の中身のほかの部分は、使うときに skrifa が確かめる
/// （範囲の外を読まない。読めなければ形がないものとして扱う）。
fn open_font(data: &[u8], index: u32) -> Result<(FontRef<'_>, u16)> {
    let magic = data.get(..4).and_then(|m| <[u8; 4]>::try_from(m).ok());
    let Some(magic) = magic.filter(|m| FONT_MAGICS.contains(m)) else {
        return Err(unreadable_font("unknown magic"));
    };
    let collection = &magic == b"ttcf";
    if collection {
        // コレクションのヘッダ: 印（4 バイト）・版（4 バイト）・フォントの数（4 バイト）・各フォントの位置。
        let count = data
            .get(8..12)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_be_bytes);
        match count {
            None => return Err(unreadable_font("malformed font")),
            Some(n) if index >= n => {
                return Err(MediaError::invalid_argument(format!(
                    "フォントの番号 {index} はありません（このファイルのフォントは {n} 個。0〜{} で指定してください）",
                    n.saturating_sub(1)
                )));
            }
            Some(_) => {}
        }
    } else if index != 0 {
        return Err(MediaError::invalid_argument(format!(
            "このフォントのファイルはコレクション（.ttc）ではないため、番号は 0 にしてください（{index}）"
        )));
    }
    let font = if collection {
        CollectionRef::new(data).and_then(|c| c.get(index))
    } else {
        FontRef::new(data)
    }
    .map_err(|e| {
        unreadable_font(match e {
            ReadError::InvalidSfnt(_) => "unknown magic",
            _ => "malformed font",
        })
    })?;
    // read-fonts は表の一覧が途中で切れていても開ける（一覧が空になる）ので、数と合うかを確かめる。
    let dir = font.table_directory();
    if dir.table_records().len() != usize::from(dir.num_tables()) {
        return Err(unreadable_font("malformed font"));
    }
    let upem = font
        .head()
        .ok()
        .filter(|h| matches!(h.index_to_loc_format(), 0 | 1))
        .map(|h| h.units_per_em())
        .filter(|u| (16..=16384).contains(u))
        .ok_or_else(|| unreadable_font("the head table is missing or malformed"))?;
    if font.hhea().is_err() {
        return Err(unreadable_font("the hhea table is missing or malformed"));
    }
    let maxp_ok = font.maxp().is_ok_and(|m| {
        m.num_glyphs() != 0
            && [Version16Dot16::VERSION_0_5, Version16Dot16::VERSION_1_0].contains(&m.version())
    });
    if !maxp_ok {
        return Err(unreadable_font("the maxp table is missing or malformed"));
    }
    Ok((font, upem))
}

/// 文字から字形を引く `cmap` の表。置き換える前（ttf-parser の `glyph_index`）と同じ規則: Unicode の表（プラット
/// フォーム 0（Unicode）、プラットフォーム 3（Windows）の符号化 1（BMP）、プラットフォーム 3 の符号化 10 の形式 12・13）
/// を、ファイルの並びの順に調べる。記号のフォント（プラットフォーム 3 の符号化 0）・Mac の表は使わない。
fn unicode_subtables<'a>(font: &FontRef<'a>) -> Vec<CmapSubtable<'a>> {
    let Ok(cmap) = font.cmap() else {
        return Vec::new();
    };
    cmap.encoding_records()
        .iter()
        .filter_map(|r| {
            let s = r.subtable(cmap.offset_data()).ok()?;
            let unicode = match r.platform_id() {
                PlatformId::Unicode => true,
                PlatformId::Windows => {
                    r.encoding_id() == 1
                        || (r.encoding_id() == 10
                            && matches!(s, CmapSubtable::Format12(_) | CmapSubtable::Format13(_)))
                }
                _ => false,
            };
            unicode.then_some(s)
        })
        .collect()
}

/// 文字の字形（最初に見つかった表の字形。`.notdef`（0）はないものとして扱う）。
fn glyph_for(subtables: &[CmapSubtable<'_>], c: char) -> Option<GlyphId> {
    subtables
        .iter()
        .find_map(|s| s.map_codepoint(c).filter(|g| *g != GlyphId::NOTDEF))
}

/// 文字の透かしを作る（モジュールの doc の「文字」）。`font` はフォントのファイルの中身、`index` はコレクションの中の
/// 番号、`size_px` は文字の大きさ（em の画素）、`align` は行の揃え。
#[allow(clippy::too_many_arguments)]
pub fn text_watermark(
    font: &[u8],
    index: u32,
    text: &str,
    color: RgbColor,
    size_px: u32,
    align: Align,
    max_width: u32,
    max_height: u32,
    space: OutputColorSpace,
) -> Result<TextWatermark> {
    let (font, upem) = open_font(font, index)?;
    let upem = f32::from(upem);
    // ヒンティングなし・フォントの単位のまま・可変フォントは既定の形。
    let size = Size::unscaled();
    let location = LocationRef::default();
    // 行の高さ（OS/2 の USE_TYPO_METRICS の印があれば OS/2 の値、なければ hhea、hhea が 0 なら OS/2 の値。skrifa の
    // `Metrics`）。
    let metrics = font.metrics(size, location);
    let ascender = metrics.ascent;
    let descender = metrics.descent;
    let mut line_height = ascender - descender + metrics.leading;
    if line_height <= 0.0 {
        line_height = upem * 1.2;
    }
    let subtables = unicode_subtables(&font);
    let glyph_metrics = font.glyph_metrics(size, location);
    // 並べる（フォントの単位）。
    let mut placed = Vec::new();
    let mut missing: Vec<char> = Vec::new();
    let mut widths = Vec::new();
    for (line, s) in text.split('\n').enumerate() {
        let mut pen = 0.0f32;
        for c in s.chars().filter(|c| *c != '\r') {
            let glyph = match glyph_for(&subtables, c) {
                Some(g) => g,
                None => {
                    if !missing.contains(&c) {
                        missing.push(c);
                    }
                    GlyphId::NOTDEF
                }
            };
            placed.push(Placed {
                glyph,
                x: pen,
                line,
            });
            pen += glyph_metrics.advance_width(glyph).unwrap_or(0.0);
        }
        widths.push(pen);
    }
    // 字形の輪郭を読む（同じ字形は 1 回。読めない字形は形がないものとして扱う）。
    let outlines = font.outline_glyphs();
    let mut glyphs: HashMap<GlyphId, GlyphOutline> = HashMap::new();
    let mut elements = 0u64;
    for p in &placed {
        if glyphs.contains_key(&p.glyph) {
            continue;
        }
        let mut g = GlyphOutline::default();
        if let Some(outline) = outlines.get(p.glyph) {
            let mut recorder = OutlineRecorder {
                outline: &mut g,
                count: &mut elements,
            };
            if outline
                .draw(DrawSettings::unhinted(size, location), &mut recorder)
                .is_err()
            {
                g = GlyphOutline::default();
            }
        }
        if elements > MAX_TEXT_OUTLINE_ELEMENTS {
            return Err(MediaError::TooLarge {
                what: "透かしの文字の輪郭の要素の数",
                actual: elements,
                max: MAX_TEXT_OUTLINE_ELEMENTS,
            });
        }
        glyphs.insert(p.glyph, g);
    }
    let block = widths.iter().copied().fold(0.0f32, f32::max);
    let offset = |line: usize| -> f32 {
        let free = block - widths[line];
        match align {
            Align::Start => 0.0,
            Align::Center => free / 2.0,
            Align::End => free,
        }
    };
    // 形の外接矩形（フォントの単位。y は下向き、1 行目の上端から）。
    let mut ink: Option<[f32; 4]> = None;
    for p in &placed {
        if let Some(r) = glyphs.get(&p.glyph).and_then(|g| g.bounds) {
            let base = ascender + p.line as f32 * line_height;
            let x0 = offset(p.line) + p.x + r[0];
            let x1 = offset(p.line) + p.x + r[2];
            let y0 = base - r[3];
            let y1 = base - r[1];
            ink = Some(match ink {
                None => [x0, y0, x1, y1],
                Some(b) => [b[0].min(x0), b[1].min(y0), b[2].max(x1), b[3].max(y1)],
            });
        }
    }
    let Some(ink) = ink.filter(|b| b[2] > b[0] && b[3] > b[1]) else {
        return Err(MediaError::invalid_argument(
            "透かしの文字に描ける形がありません（空白だけ・フォントに形がない）",
        ));
    };
    let mut scale = size_px.max(1) as f32 / upem;
    let (ink_w, ink_h) = ((ink[2] - ink[0]) * scale, (ink[3] - ink[1]) * scale);
    let fit = (max_width as f32 / (ink_w + 2.0)).min(max_height as f32 / (ink_h + 2.0));
    let shrunk = fit < 1.0;
    if shrunk {
        scale *= fit;
    }
    // 画素の寸法（端に 1 画素の余裕）。
    let px0 = (ink[0] * scale).floor() - 1.0;
    let py0 = (ink[1] * scale).floor() - 1.0;
    let width = (((ink[2] * scale).ceil() + 1.0 - px0) as u32).clamp(1, max_width.max(1));
    let height = (((ink[3] * scale).ceil() + 1.0 - py0) as u32).clamp(1, max_height.max(1));
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_WATERMARK_PIXELS {
        return Err(MediaError::TooLarge {
            what: "透かしの画素数",
            actual: pixels,
            max: MAX_WATERMARK_PIXELS,
        });
    }
    let mut edges = Vec::new();
    let mut edge_count = 0u64;
    for p in &placed {
        let Some(g) = glyphs.get(&p.glyph) else {
            continue;
        };
        let base = ascender + p.line as f32 * line_height;
        let mut b = EdgeBuilder {
            edges: &mut edges,
            count: &mut edge_count,
            scale,
            ox: (offset(p.line) + p.x) * scale - px0,
            oy: base * scale - py0,
            start: (0.0, 0.0),
            cur: (0.0, 0.0),
        };
        for s in &g.segments {
            match *s {
                Segment::Move(x, y) => b.move_to(x, y),
                Segment::Line(x, y) => b.line_to(x, y),
                Segment::Quad(cx, cy, x, y) => b.quad_to(cx, cy, x, y),
                Segment::Cubic(cx0, cy0, cx1, cy1, x, y) => b.curve_to(cx0, cy0, cx1, cy1, x, y),
                Segment::Close => b.close(),
            }
            if *b.count > MAX_TEXT_EDGES {
                break;
            }
        }
        if edge_count > MAX_TEXT_EDGES {
            return Err(MediaError::TooLarge {
                what: "透かしの文字の輪郭の線分の数",
                actual: edge_count,
                max: MAX_TEXT_EDGES,
            });
        }
    }
    let cov = rasterize(&edges, width as usize, height as usize);
    let c = color_in(space, color);
    let mut rgba = Vec::with_capacity(cov.len() * 4);
    for a in cov {
        rgba.extend_from_slice(&[c[0], c[1], c[2], (a * 65535.0 + 0.5).floor() as u16]);
    }
    Ok(TextWatermark {
        bitmap: WatermarkBitmap::new(width, height, rgba)?,
        missing,
        shrunk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TEST_FONT_UNITS_PER_EM, test_font};
    use genzo_model::{RgbColor, WatermarkContent};
    use std::path::PathBuf;

    fn mark(size: u8, margin: u8) -> Watermark {
        Watermark {
            size,
            margin,
            ..Watermark::new(WatermarkContent::Image {
                path: PathBuf::from("/logo.png"),
            })
        }
    }

    #[test]
    fn geometry_uses_the_short_edge() {
        let g = WatermarkGeometry::new(&mark(20, 2), 6000, 4000);
        assert_eq!(g.size_px, 800);
        assert_eq!(g.margin_px, 80);
        assert_eq!((g.max_width, g.max_height), (5840, 3840));
        // 縦の写真でも同じ大きさ。
        let p = WatermarkGeometry::new(&mark(20, 2), 4000, 6000);
        assert_eq!((p.size_px, p.margin_px), (800, 80));
        // 小さい画像でも 1 画素以上。
        let t = WatermarkGeometry::new(&mark(1, 25), 3, 2);
        assert_eq!(t.size_px, 1);
        assert!(t.max_width >= 1 && t.max_height >= 1);
    }

    #[test]
    fn placement_follows_the_anchor() {
        let at = |a| place(a, (100, 50), (20, 10), 5);
        assert_eq!(at(WatermarkAnchor::TopLeft), (5, 5));
        assert_eq!(at(WatermarkAnchor::Top), (40, 5));
        assert_eq!(at(WatermarkAnchor::TopRight), (75, 5));
        assert_eq!(at(WatermarkAnchor::Left), (5, 20));
        assert_eq!(at(WatermarkAnchor::Center), (40, 20));
        assert_eq!(at(WatermarkAnchor::BottomRight), (75, 35));
        assert_eq!(at(WatermarkAnchor::Bottom), (40, 35));
        assert_eq!(at(WatermarkAnchor::BottomLeft), (5, 35));
    }

    #[test]
    fn composite_blends_with_opacity_and_clips() {
        let mut dst = vec![10_000u16; 4 * 3 * 3];
        let m = WatermarkBitmap::new(
            2,
            2,
            vec![
                60_000, 60_000, 60_000, 65_535, // 不透明
                60_000, 60_000, 60_000, 0, // 透明
                0, 0, 0, 32_768, // 半透明の黒
                60_000, 60_000, 60_000, 65_535,
            ],
        )
        .unwrap();
        composite(&mut dst, 4, 3, &m, (3, 2), 0.5).unwrap();
        let px = |x: usize, y: usize| dst[(y * 4 + x) * 3];
        // (3, 2) は不透明の画素を 50 %。はみ出した 3 つの画素は描かない。
        assert_eq!(px(3, 2), 35_000);
        assert_eq!(px(2, 2), 10_000);
        assert_eq!(px(0, 0), 10_000);
        let mut dst2 = vec![10_000u16; 4 * 3 * 3];
        composite(&mut dst2, 4, 3, &m, (0, 0), 1.0).unwrap();
        assert_eq!(dst2[0], 60_000);
        assert_eq!(dst2[3], 10_000, "透明の画素は変えない");
        // 半透明の黒（a ≈ 0.5）。
        assert_eq!(dst2[4 * 3], 5_000);
        assert!(composite(&mut dst2, 5, 3, &m, (0, 0), 1.0).is_err());
        assert!(WatermarkBitmap::new(2, 2, vec![0; 15]).is_err());
        assert!(WatermarkBitmap::new(0, 2, vec![]).is_err());
    }

    #[test]
    fn colors_are_converted_to_the_output_space() {
        assert_eq!(
            color_in(OutputColorSpace::Srgb, RgbColor::WHITE),
            [65_535; 3]
        );
        assert_eq!(
            color_in(OutputColorSpace::AdobeRgb, RgbColor([0, 0, 0])),
            [0; 3]
        );
        // sRGB の赤は Display P3 の中では少し内側。
        let red = color_in(OutputColorSpace::DisplayP3, RgbColor([255, 0, 0]));
        assert!(red[0] < 65_535 && red[1] > 0 && red[2] > 0, "{red:?}");
        assert_eq!(
            color_in(OutputColorSpace::Srgb, RgbColor([255, 0, 0])),
            [65_535, 0, 0]
        );
    }

    #[test]
    fn image_sizes_fit_the_box() {
        assert_eq!(fit_image_size((400, 200), 100, (1000, 1000)), (100, 50));
        // 小さい画像は拡大する。
        assert_eq!(fit_image_size((40, 20), 100, (1000, 1000)), (100, 50));
        // 収まらなければ縦横比を保って縮める。
        assert_eq!(fit_image_size((400, 200), 800, (300, 1000)), (300, 150));
        assert_eq!(fit_image_size((100, 400), 100, (1000, 200)), (50, 200));
    }

    fn png_rgba(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn image_watermark_keeps_alpha_and_colors() {
        // 左半分が不透明の白、右半分が透明。
        let png = png_rgba(40, 20, |x, _| {
            if x < 20 {
                [255, 255, 255, 255]
            } else {
                [255, 0, 0, 0]
            }
        });
        let m = image_watermark(&png, 20, 1000, 1000, OutputColorSpace::Srgb).unwrap();
        assert_eq!((m.width(), m.height()), (20, 10));
        let px = |x: u32, y: u32| {
            &m.rgba()[((y * 20 + x) * 4) as usize..((y * 20 + x) * 4 + 4) as usize]
        };
        assert!(px(2, 5)[3] > 65_000, "{:?}", px(2, 5));
        assert!(px(2, 5)[0] > 65_000);
        assert_eq!(px(17, 5)[3], 0);
        // 境目の画素は半透明で、透明な画素の色（赤）は混ざらない（アルファを掛けて縮める）。
        // 出力の 10 番目の画素は入力の 19〜22 番目（三角形の重み 0.25・0.75・0.75・0.25）で、不透明なのは 19 番目だけ。
        let edge = px(10, 5);
        assert_eq!(edge[3], 8_192, "{edge:?}");
        assert!(px(9, 5)[3] > 57_000, "{:?}", px(9, 5));
        assert!(edge[1] > 65_000 && edge[2] > 65_000, "{edge:?}");
        // 箱に収める。
        let small = image_watermark(&png, 200, 50, 50, OutputColorSpace::DisplayP3).unwrap();
        assert_eq!((small.width(), small.height()), (50, 25));
        // 壊れたファイル。
        assert!(image_watermark(b"not an image", 20, 100, 100, OutputColorSpace::Srgb).is_err());
    }

    #[test]
    fn rasterizer_fills_shapes_with_antialiasing() {
        // 2 × 2 の正方形（画素 1〜3）と、その中の穴（逆回りの 1 × 1）。
        let sq = |x0: f32, y0: f32, x1: f32, y1: f32, cw: bool| {
            let pts = if cw {
                [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
            } else {
                [(x0, y0), (x0, y1), (x1, y1), (x1, y0)]
            };
            let mut edges = Vec::new();
            for i in 0..4 {
                let (a, b) = (pts[i], pts[(i + 1) % 4]);
                if a.1 != b.1 {
                    let (dir, p, q) = if a.1 < b.1 { (1, a, b) } else { (-1, b, a) };
                    edges.push(Edge {
                        x0: p.0,
                        y0: p.1,
                        x1: q.0,
                        y1: q.1,
                        dir,
                    });
                }
            }
            edges
        };
        let cov = rasterize(&sq(1.0, 1.0, 3.0, 3.0, true), 4, 4);
        assert_eq!(cov[0], 0.0);
        assert!((cov[5] - 1.0).abs() < 1e-6 && (cov[10] - 1.0).abs() < 1e-6);
        // 半分の画素を覆う。
        let half = rasterize(&sq(0.5, 0.0, 2.0, 2.0, true), 3, 2);
        assert!((half[0] - 0.5).abs() < 1e-4, "{half:?}");
        assert!((half[1] - 1.0).abs() < 1e-4);
        // 穴（逆回り）は塗らない。同じ向きの重なりは 1 で止める。
        let mut e = sq(0.0, 0.0, 4.0, 4.0, true);
        e.extend(sq(1.0, 1.0, 3.0, 3.0, false));
        let hole = rasterize(&e, 4, 4);
        assert_eq!(hole[5], 0.0);
        assert!((hole[0] - 1.0).abs() < 1e-6);
        let mut e = sq(0.0, 0.0, 4.0, 4.0, true);
        e.extend(sq(1.0, 1.0, 3.0, 3.0, true));
        assert!((rasterize(&e, 4, 4)[5] - 1.0).abs() < 1e-6);
    }

    fn alpha_sum(m: &WatermarkBitmap) -> u64 {
        m.rgba().chunks_exact(4).map(|p| u64::from(p[3])).sum()
    }

    #[test]
    fn text_watermark_renders_glyphs_and_reports_missing() {
        let font = test_font();
        let t = text_watermark(
            &font,
            0,
            "AB",
            RgbColor([255, 0, 0]),
            100,
            Align::End,
            10_000,
            10_000,
            OutputColorSpace::Srgb,
        )
        .unwrap();
        assert!(t.missing.is_empty() && !t.shrunk);
        // テストのフォント: 「A」は幅 600・高さ 700 の正方形、「B」は三角形、送り幅 700（em 1000）。
        let m = &t.bitmap;
        assert!((130..=140).contains(&m.width()), "{}", m.width());
        assert!((70..=74).contains(&m.height()), "{}", m.height());
        assert!(m.rgba().chunks_exact(4).all(|p| p[0..3] == [65_535, 0, 0]));
        // 正方形の中は不透明、正方形と三角形の間は透明。
        let a = |x: u32, y: u32| m.rgba()[((y * m.width() + x) * 4 + 3) as usize];
        assert_eq!(a(30, 35), 65_535);
        assert_eq!(a(65, 35), 0);
        // 日本語の文字（テストのフォントは「あ」を A と同じ形にしてある）と、ない文字。
        let jp = text_watermark(
            &font,
            0,
            "あ漢",
            RgbColor::WHITE,
            50,
            Align::Start,
            10_000,
            10_000,
            OutputColorSpace::Srgb,
        )
        .unwrap();
        assert_eq!(jp.missing, vec!['漢']);
        // 2 行は 1 行より高い。大きさは文字の大きさに比例する。
        let one = text_watermark(
            &font,
            0,
            "A",
            RgbColor::WHITE,
            40,
            Align::Start,
            9999,
            9999,
            OutputColorSpace::Srgb,
        )
        .unwrap();
        let two = text_watermark(
            &font,
            0,
            "A\nA",
            RgbColor::WHITE,
            40,
            Align::Start,
            9999,
            9999,
            OutputColorSpace::Srgb,
        )
        .unwrap();
        assert!(two.bitmap.height() > one.bitmap.height() + 30);
        let big = text_watermark(
            &font,
            0,
            "A",
            RgbColor::WHITE,
            80,
            Align::Start,
            9999,
            9999,
            OutputColorSpace::Srgb,
        )
        .unwrap();
        let ratio = alpha_sum(&big.bitmap) as f64 / alpha_sum(&one.bitmap) as f64;
        assert!((ratio - 4.0).abs() < 0.3, "{ratio}");
        // 箱に収まらなければ縮める。
        let fitted = text_watermark(
            &font,
            0,
            "AB",
            RgbColor::WHITE,
            100,
            Align::Start,
            50,
            9999,
            OutputColorSpace::Srgb,
        )
        .unwrap();
        assert!(fitted.shrunk && fitted.bitmap.width() <= 50);
        // 空白だけ・壊れたフォント・ない番号。
        assert!(
            text_watermark(
                &font,
                0,
                "  ",
                RgbColor::WHITE,
                40,
                Align::Start,
                99,
                99,
                OutputColorSpace::Srgb
            )
            .is_err()
        );
        assert!(
            text_watermark(
                b"garbage",
                0,
                "A",
                RgbColor::WHITE,
                40,
                Align::Start,
                99,
                99,
                OutputColorSpace::Srgb
            )
            .is_err()
        );
        assert!(
            text_watermark(
                &font,
                3,
                "A",
                RgbColor::WHITE,
                40,
                Align::Start,
                99,
                99,
                OutputColorSpace::Srgb
            )
            .is_err()
        );
        assert_eq!(TEST_FONT_UNITS_PER_EM, 1000);
        // コレクション（.ttc）の番号。
        let ttc = crate::testing::test_font_collection(2);
        let r = |f: &[u8], i| {
            text_watermark(
                f,
                i,
                "A",
                RgbColor::WHITE,
                40,
                Align::Start,
                99,
                99,
                OutputColorSpace::Srgb,
            )
        };
        r(&ttc, 1).unwrap();
        let e = r(&ttc, 2).unwrap_err().to_string();
        assert!(e.contains("0〜1"), "{e}");
        let e = r(&font, 3).unwrap_err().to_string();
        assert!(e.contains("コレクション"), "{e}");
    }

    #[test]
    fn text_lines_follow_the_alignment() {
        let font = test_font();
        let render = |align| {
            text_watermark(
                &font,
                0,
                "AA\nA",
                RgbColor::WHITE,
                50,
                align,
                9999,
                9999,
                OutputColorSpace::Srgb,
            )
            .unwrap()
            .bitmap
        };
        // 2 行目（短い行）の形が、左揃えでは左に、右揃えでは右にある。
        let col_of_ink = |m: &WatermarkBitmap| {
            let y = m.height() - 5;
            (0..m.width())
                .find(|&x| m.rgba()[((y * m.width() + x) * 4 + 3) as usize] > 30_000)
                .unwrap()
        };
        let (l, c, r) = (
            render(Align::Start),
            render(Align::Center),
            render(Align::End),
        );
        assert!(col_of_ink(&l) < col_of_ink(&c) && col_of_ink(&c) < col_of_ink(&r));
    }

    /// テストのフォントの表の記録（表の一覧の中の位置・表の位置・長さ）。
    fn table_record(font: &[u8], tag: &[u8; 4]) -> (usize, usize, usize) {
        let n = usize::from(u16::from_be_bytes([font[4], font[5]]));
        let be32 = |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap()) as usize;
        (0..n)
            .map(|i| 12 + 16 * i)
            .find(|&r| &font[r..r + 4] == tag)
            .map(|r| (r, be32(r + 8), be32(r + 12)))
            .unwrap()
    }

    fn render_a(font: &[u8], index: u32, text: &str) -> Result<TextWatermark> {
        text_watermark(
            font,
            index,
            text,
            RgbColor::WHITE,
            40,
            Align::Start,
            99,
            99,
            OutputColorSpace::Srgb,
        )
    }

    #[test]
    fn broken_fonts_are_errors_with_the_same_messages() {
        let font = test_font();
        let err = |f: &[u8], i: u32| render_a(f, i, "A").unwrap_err().to_string();
        // 先頭の 4 バイト（空・短い・知らない印）。
        for bad in [&b""[..], b"tt", b"garbage", b"wOFF\0\0\0\0"] {
            assert!(
                err(bad, 0).ends_with("フォントを読めません: unknown magic"),
                "{bad:?}"
            );
        }
        // 表の一覧が途中で切れている。
        assert!(err(&font[..40], 0).ends_with("フォントを読めません: malformed font"));
        // 必須の表（head の em が 0・hhea が短い・maxp の字形が 0）。
        let (_, head, _) = table_record(&font, b"head");
        let mut f = font.clone();
        f[head + 18..head + 20].copy_from_slice(&0u16.to_be_bytes());
        assert!(err(&f, 0).ends_with("the head table is missing or malformed"));
        let (rec, _, _) = table_record(&font, b"hhea");
        let mut f = font.clone();
        f[rec + 12..rec + 16].copy_from_slice(&10u32.to_be_bytes());
        assert!(err(&f, 0).ends_with("the hhea table is missing or malformed"));
        let (_, maxp, _) = table_record(&font, b"maxp");
        let mut f = font.clone();
        f[maxp + 4..maxp + 6].copy_from_slice(&0u16.to_be_bytes());
        assert!(err(&f, 0).ends_with("the maxp table is missing or malformed"));
        // コレクションのヘッダが短い・フォントの数が巨大（パニックしない・確保しない）。
        assert!(err(b"ttcf\0\x01\0\0", 0).ends_with("malformed font"));
        let huge = b"ttcf\0\x01\0\0\xff\xff\xff\xff\0\0\0\x10";
        assert!(err(huge, 3).ends_with("malformed font"));
        let e = err(huge, u32::MAX);
        assert!(e.contains("4294967295 個"), "{e}");
        // コレクションの中のフォントの位置がファイルの外。
        let mut ttc = crate::testing::test_font_collection(2);
        ttc[16..20].copy_from_slice(&0x7fff_0000u32.to_be_bytes());
        assert!(err(&ttc, 1).ends_with("malformed font"));
        render_a(&ttc, 0, "A").unwrap();
    }

    #[test]
    fn unreadable_glyphs_have_no_shape() {
        // 「B」（字形 2）の終わりの位置（loca の 4 番目。空白（字形 3）の始まりでもある）を glyf の外にする: 「B」は
        // 形がないものとして扱い、「A」は描く。
        let mut font = test_font();
        let (_, loca, _) = table_record(&font, b"loca");
        font[loca + 12..loca + 16].copy_from_slice(&0x00ff_0000u32.to_be_bytes());
        let ab = render_a(&font, 0, "AB").unwrap();
        let a = render_a(&font, 0, "A").unwrap();
        assert_eq!(ab.bitmap, a.bitmap);
        assert!(ab.missing.is_empty());
        let e = render_a(&font, 0, "B").unwrap_err().to_string();
        assert!(e.contains("描ける形がありません"), "{e}");
    }

    #[test]
    fn outline_limits_stop_recording() {
        // 要素の数が上限に達した後は記録しない（数えるだけ）。
        let mut g = GlyphOutline::default();
        let mut count = MAX_TEXT_OUTLINE_ELEMENTS - 1;
        let mut r = OutlineRecorder {
            outline: &mut g,
            count: &mut count,
        };
        r.move_to(0.0, 0.0);
        r.line_to(10.0, 5.0);
        r.close();
        assert_eq!(count, MAX_TEXT_OUTLINE_ELEMENTS + 2);
        assert_eq!(g.segments.len(), 1);
        assert_eq!(g.bounds, Some([0.0, 0.0, 0.0, 0.0]));
        // 線分の数も同じ。
        let mut edges = Vec::new();
        let mut count = MAX_TEXT_EDGES - 1;
        let mut b = EdgeBuilder {
            edges: &mut edges,
            count: &mut count,
            scale: 1.0,
            ox: 0.0,
            oy: 0.0,
            start: (0.0, 0.0),
            cur: (0.0, 0.0),
        };
        b.move_to(0.0, 0.0);
        b.line_to(1.0, 1.0);
        b.line_to(2.0, 0.0);
        b.close();
        assert_eq!(count, MAX_TEXT_EDGES + 1);
        assert_eq!(edges.len(), 1);
    }
}
