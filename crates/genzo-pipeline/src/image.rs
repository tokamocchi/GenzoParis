//! パイプラインの画像の型（docs/04_architecture.md の 2.1 節・2.6 節・7.1 節）。
//!
//! | 型 | 内容 | 色の契約（2.6 節） |
//! |---|---|---|
//! | [`Roi`] | 画素の範囲（左上 x, y と幅・高さ）。画像の外にはみ出してよい（タイルの余白） | — |
//! | [`CfaImage`] | ベイヤー配列の f32 の画像と飽和の印。S 座標のどの範囲かを持つ | B0（ステージ 3 の後は B1 の値の CFA） |
//! | [`RgbImage`] | RGB の f32 の画像（インターリーブ `[f32; 3]`）。どの境界の値かを持つ | B1〜B5（[`ColorContract`]） |
//! | [`GrayImage`] | 1 チャンネルの f32 の画像（ガイド・マスク） | — |
//! | [`ImageTile`] | RGB の画像と、それが覆う範囲（7.1 節の `ImageTile`） | 画像に従う |
//!
//! 画素の値はすべて f32（2.3 節「浮動小数点は 32bit で統一」）。画素数の上限は genzo-raw の
//! [`genzo_raw::MAX_PIXELS`]（壊れた入力による巨大な確保を防ぐ）。

use genzo_raw::{CfaColor, CfaPattern, MAX_PIXELS};

use crate::border::mirror_index;
use crate::contract::ColorContract;
use crate::error::{PipelineError, Result};
use crate::sanitize::{sanitize_rgb, sanitize_slice};

/// 画素の範囲（左上の座標と幅・高さ）。
///
/// 座標は、その範囲を使う場所の画素の格子（S 座標のフル解像度、簡易処理の半分の解像度、
/// C 座標の出力の解像度など）で表す。タイルの余白のために画像の外（負の座標や、幅・高さを
/// 超える座標）にはみ出してよい。はみ出した部分の値は鏡映で決める（[`crate::border`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Roi {
    /// 左端の x。
    pub x: i32,
    /// 上端の y。
    pub y: i32,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
}

impl Roi {
    /// 範囲を作る。
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// 画像全体（左上 (0, 0)）。
    pub const fn full(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    /// 右端の次の x（`x + width`）。
    pub fn x_end(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    /// 下端の次の y（`y + height`）。
    pub fn y_end(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// 画素がないか。
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// 画素数。
    pub const fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// 上下左右に `margin` 画素ずつ広げる（タイルの余白。7.1 節の `input_roi`）。
    pub fn expand(&self, margin: u32) -> Roi {
        self.expand_xy(margin, margin)
    }

    /// 左右に `mx`、上下に `my` 画素ずつ広げる。座標が i32 の範囲を超える場合は飽和させる。
    pub fn expand_xy(&self, mx: u32, my: u32) -> Roi {
        let x = (i64::from(self.x) - i64::from(mx)).max(i64::from(i32::MIN));
        let y = (i64::from(self.y) - i64::from(my)).max(i64::from(i32::MIN));
        let w = (u64::from(self.width) + 2 * u64::from(mx)).min(u64::from(u32::MAX));
        let h = (u64::from(self.height) + 2 * u64::from(my)).min(u64::from(u32::MAX));
        Roi::new(x as i32, y as i32, w as u32, h as u32)
    }

    /// 上下左右を `margin` 画素ずつ狭める。空になるなら `None`。
    pub fn shrink(&self, margin: u32) -> Option<Roi> {
        let m2 = u64::from(margin) * 2;
        if u64::from(self.width) <= m2 || u64::from(self.height) <= m2 {
            return None;
        }
        let x = i64::from(self.x) + i64::from(margin);
        let y = i64::from(self.y) + i64::from(margin);
        Some(Roi::new(
            i32::try_from(x).ok()?,
            i32::try_from(y).ok()?,
            (u64::from(self.width) - m2) as u32,
            (u64::from(self.height) - m2) as u32,
        ))
    }

    /// 2 つの範囲の共通部分。重ならなければ `None`。
    pub fn intersect(&self, other: &Roi) -> Option<Roi> {
        let x0 = i64::from(self.x).max(i64::from(other.x));
        let y0 = i64::from(self.y).max(i64::from(other.y));
        let x1 = self.x_end().min(other.x_end());
        let y1 = self.y_end().min(other.y_end());
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Roi::new(
            x0 as i32,
            y0 as i32,
            (x1 - x0) as u32,
            (y1 - y0) as u32,
        ))
    }

    /// `other` をすべて含むか（空の `other` は常に含む）。
    pub fn contains_roi(&self, other: &Roi) -> bool {
        other.is_empty()
            || (i64::from(self.x) <= i64::from(other.x)
                && i64::from(self.y) <= i64::from(other.y)
                && other.x_end() <= self.x_end()
                && other.y_end() <= self.y_end())
    }

    /// 画素 (x, y) を含むか。
    pub fn contains(&self, x: i64, y: i64) -> bool {
        i64::from(self.x) <= x && x < self.x_end() && i64::from(self.y) <= y && y < self.y_end()
    }

    /// 幅 `width`・高さ `height` の画像の内側に収まるか。
    pub fn is_inside(&self, width: u32, height: u32) -> bool {
        Roi::full(width, height).contains_roi(self)
    }

    /// 左上の座標を (dx, dy) だけずらす。
    pub fn translate(&self, dx: i32, dy: i32) -> Roi {
        Roi::new(
            self.x.saturating_add(dx),
            self.y.saturating_add(dy),
            self.width,
            self.height,
        )
    }

    /// 座標と寸法を 2 倍にした範囲（半分の解像度の格子の範囲 → フル解像度の格子の範囲。
    /// 2 × 2 の簡易処理の入力の範囲）。
    pub fn doubled(&self) -> Roi {
        let x = (i64::from(self.x) * 2).clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        let y = (i64::from(self.y) * 2).clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        Roi::new(
            x as i32,
            y as i32,
            self.width.saturating_mul(2),
            self.height.saturating_mul(2),
        )
    }

    /// `outer` の中での左上の位置（`outer` がこの範囲を含むときだけ）。
    pub fn offset_in(&self, outer: &Roi) -> Option<(usize, usize)> {
        if !outer.contains_roi(self) {
            return None;
        }
        Some((
            (i64::from(self.x) - i64::from(outer.x)) as usize,
            (i64::from(self.y) - i64::from(outer.y)) as usize,
        ))
    }

    /// 幅 `width`・高さ `height` の画像を、一辺 `tile` 画素のタイルに分ける（左上から行の順。
    /// 右端・下端のタイルは小さくなる）。`tile = 0` か画像が空なら空。
    pub fn tiles(width: u32, height: u32, tile: u32) -> Vec<Roi> {
        if tile == 0 || width == 0 || height == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut y = 0u32;
        while y < height {
            let h = tile.min(height - y);
            let mut x = 0u32;
            while x < width {
                let w = tile.min(width - x);
                out.push(Roi::new(x as i32, y as i32, w, h));
                x += w;
            }
            y += h;
        }
        out
    }
}

/// 寸法を検証し、画素数を返す（0 や上限超えはエラー）。
pub(crate) fn checked_len(width: u32, height: u32) -> Result<usize> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || pixels > MAX_PIXELS {
        return Err(PipelineError::InvalidDimensions { width, height });
    }
    usize::try_from(pixels).map_err(|_| PipelineError::InvalidDimensions { width, height })
}

/// RGB の f32 の画像（インターリーブの `[f32; 3]`、行優先）。
///
/// どの境界の値か（[`ColorContract`]。B1 のカメラ RGB、B2 の作業色空間など）を持つ。
/// 寸法は 1 × 1 以上。
#[derive(Debug, Clone, PartialEq)]
pub struct RgbImage {
    width: u32,
    height: u32,
    contract: ColorContract,
    data: Vec<[f32; 3]>,
}

impl RgbImage {
    /// 0 で埋めた画像。
    pub fn new(width: u32, height: u32, contract: ColorContract) -> Result<Self> {
        Self::filled(width, height, contract, [0.0; 3])
    }

    /// 同じ値で埋めた画像。
    pub fn filled(
        width: u32,
        height: u32,
        contract: ColorContract,
        value: [f32; 3],
    ) -> Result<Self> {
        let len = checked_len(width, height)?;
        Ok(Self {
            width,
            height,
            contract,
            data: vec![value; len],
        })
    }

    /// 画素の列から作る（長さは `width * height`）。
    pub fn from_vec(
        width: u32,
        height: u32,
        contract: ColorContract,
        data: Vec<[f32; 3]>,
    ) -> Result<Self> {
        let len = checked_len(width, height)?;
        if data.len() != len {
            return Err(PipelineError::DataLengthMismatch {
                expected: len,
                actual: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            contract,
            data,
        })
    }

    /// 画素ごとの関数から作る（`f(x, y)`）。
    pub fn from_fn(
        width: u32,
        height: u32,
        contract: ColorContract,
        mut f: impl FnMut(u32, u32) -> [f32; 3],
    ) -> Result<Self> {
        let len = checked_len(width, height)?;
        let mut data = Vec::with_capacity(len);
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Self::from_vec(width, height, contract, data)
    }

    /// 幅。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さ。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 寸法（幅, 高さ）。
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 画素数。
    pub fn pixel_count(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// 色の契約（2.6 節）。
    pub fn contract(&self) -> ColorContract {
        self.contract
    }

    /// 色の契約を変える（色空間を変換したステージが使う。例: ステージ 8 で B1 → B2）。
    pub fn set_contract(&mut self, contract: ColorContract) {
        self.contract = contract;
    }

    /// 画素の列（行優先）。
    pub fn data(&self) -> &[[f32; 3]] {
        &self.data
    }

    /// 画素の列（変更用）。
    pub fn data_mut(&mut self) -> &mut [[f32; 3]] {
        &mut self.data
    }

    /// 画素の列を取り出す。
    pub fn into_data(self) -> Vec<[f32; 3]> {
        self.data
    }

    /// チャンネルを並べた f32 の列（R, G, B, R, G, B, …）。
    pub fn as_flat(&self) -> &[f32] {
        self.data.as_flattened()
    }

    /// 画素 (x, y)。範囲外なら `None`。
    pub fn pixel(&self, x: u32, y: u32) -> Option<[f32; 3]> {
        (x < self.width && y < self.height)
            .then(|| self.data[y as usize * self.width as usize + x as usize])
    }

    /// 画素 (x, y)（変更用）。範囲外なら `None`。
    pub fn pixel_mut(&mut self, x: u32, y: u32) -> Option<&mut [f32; 3]> {
        if x < self.width && y < self.height {
            Some(&mut self.data[y as usize * self.width as usize + x as usize])
        } else {
            None
        }
    }

    /// 行 y（範囲外ならパニック）。
    pub fn row(&self, y: u32) -> &[[f32; 3]] {
        let w = self.width as usize;
        &self.data[y as usize * w..(y as usize + 1) * w]
    }

    /// 範囲 `roi`（画像の内側）を切り出す。
    pub fn crop(&self, roi: Roi) -> Result<RgbImage> {
        if roi.is_empty() || !roi.is_inside(self.width, self.height) {
            return Err(PipelineError::InvalidRoi {
                roi,
                reason: "切り出す範囲が画像の内側にない",
            });
        }
        let (x0, y0) = (roi.x as usize, roi.y as usize);
        let w = self.width as usize;
        let mut data = Vec::with_capacity(roi.pixel_count() as usize);
        for y in y0..y0 + roi.height as usize {
            data.extend_from_slice(&self.data[y * w + x0..y * w + x0 + roi.width as usize]);
        }
        RgbImage::from_vec(roi.width, roi.height, self.contract, data)
    }

    /// NaN・無限大を 0 に置き換え、置き換えた値の数を返す（2.6 節）。
    pub fn sanitize(&mut self) -> u64 {
        sanitize_rgb(&mut self.data)
    }
}

/// 1 チャンネルの f32 の画像（行優先）。ガイド（2.7 節）やマスクに使う。寸法は 1 × 1 以上。
#[derive(Debug, Clone, PartialEq)]
pub struct GrayImage {
    width: u32,
    height: u32,
    data: Vec<f32>,
}

impl GrayImage {
    /// 同じ値で埋めた画像。
    pub fn filled(width: u32, height: u32, value: f32) -> Result<Self> {
        let len = checked_len(width, height)?;
        Ok(Self {
            width,
            height,
            data: vec![value; len],
        })
    }

    /// 値の列から作る（長さは `width * height`）。
    pub fn from_vec(width: u32, height: u32, data: Vec<f32>) -> Result<Self> {
        let len = checked_len(width, height)?;
        if data.len() != len {
            return Err(PipelineError::DataLengthMismatch {
                expected: len,
                actual: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// 画素ごとの関数から作る（`f(x, y)`）。
    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> f32) -> Result<Self> {
        let len = checked_len(width, height)?;
        let mut data = Vec::with_capacity(len);
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Self::from_vec(width, height, data)
    }

    /// 幅。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さ。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 値の列（行優先）。
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// 値の列（変更用）。
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }

    /// 画素 (x, y)。範囲外なら `None`。
    pub fn pixel(&self, x: u32, y: u32) -> Option<f32> {
        (x < self.width && y < self.height)
            .then(|| self.data[y as usize * self.width as usize + x as usize])
    }

    /// NaN・無限大を 0 に置き換え、置き換えた値の数を返す（2.6 節）。
    pub fn sanitize(&mut self) -> u64 {
        sanitize_slice(&mut self.data)
    }
}

/// ベイヤー配列（CFA）の f32 の画像と飽和の印（2.6 節の B0。ステージ 3 の後は B1 の値）。
///
/// - [`roi`](Self::roi): この画像が S 座標（センサーのフル解像度の画素）のどの範囲か。タイルの
///   余白のために画像の外にはみ出してよい（はみ出した部分は鏡映で作った値。[`crate::border`]）。
/// - [`pattern`](Self::pattern): この画像の左上の画素 (0, 0) から見た色の並び（RAW の配列を
///   `roi` の左上の偶奇だけずらしたもの）。
/// - 飽和の印: 正規化（ステージ 2）で白レベル以上だった画素（2.6 節「白レベルを超えた（飽和した）
///   画素には印を付ける」）。WB（ステージ 3）とハイライト復元（ステージ 4、v1）が使う。
#[derive(Debug, Clone, PartialEq)]
pub struct CfaImage {
    roi: Roi,
    pattern: CfaPattern,
    data: Vec<f32>,
    saturated: Vec<bool>,
}

impl CfaImage {
    /// 値と飽和の印から作る（長さはどちらも `roi.width * roi.height`）。
    pub fn new(
        roi: Roi,
        pattern: CfaPattern,
        data: Vec<f32>,
        saturated: Vec<bool>,
    ) -> Result<Self> {
        let len = checked_len(roi.width, roi.height)?;
        for actual in [data.len(), saturated.len()] {
            if actual != len {
                return Err(PipelineError::DataLengthMismatch {
                    expected: len,
                    actual,
                });
            }
        }
        Ok(Self {
            roi,
            pattern,
            data,
            saturated,
        })
    }

    /// 画素ごとの関数から作る（`f(x, y)` は画像の中の座標。飽和の印はなし）。テスト用。
    pub fn from_fn(
        roi: Roi,
        pattern: CfaPattern,
        mut f: impl FnMut(u32, u32) -> f32,
    ) -> Result<Self> {
        let len = checked_len(roi.width, roi.height)?;
        let mut data = Vec::with_capacity(len);
        for y in 0..roi.height {
            for x in 0..roi.width {
                data.push(f(x, y));
            }
        }
        Self::new(roi, pattern, data, vec![false; len])
    }

    /// S 座標での範囲。
    pub fn roi(&self) -> Roi {
        self.roi
    }

    /// 幅。
    pub fn width(&self) -> u32 {
        self.roi.width
    }

    /// 高さ。
    pub fn height(&self) -> u32 {
        self.roi.height
    }

    /// 左上の画素 (0, 0) から見た色の並び。
    pub fn pattern(&self) -> CfaPattern {
        self.pattern
    }

    /// 画像の中の画素 (x, y) の色。
    pub fn color_at(&self, x: u32, y: u32) -> CfaColor {
        self.pattern.color_at(x as usize, y as usize)
    }

    /// 値の列（行優先）。
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// 値の列（変更用）。
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }

    /// 飽和の印の列（行優先）。
    pub fn saturated(&self) -> &[bool] {
        &self.saturated
    }

    /// 値と飽和の印の列（変更用）。
    pub fn parts_mut(&mut self) -> (&mut [f32], &mut [bool]) {
        (&mut self.data, &mut self.saturated)
    }

    /// 画像の中の画素 (x, y) の値。範囲外なら `None`。
    pub fn value(&self, x: u32, y: u32) -> Option<f32> {
        (x < self.roi.width && y < self.roi.height)
            .then(|| self.data[y as usize * self.roi.width as usize + x as usize])
    }

    /// 画像の中の画素 (x, y) が飽和しているか。範囲外なら `false`。
    pub fn is_saturated(&self, x: u32, y: u32) -> bool {
        x < self.roi.width
            && y < self.roi.height
            && self.saturated[y as usize * self.roi.width as usize + x as usize]
    }

    /// 飽和の印の数。
    pub fn saturated_count(&self) -> usize {
        self.saturated.iter().filter(|&&s| s).count()
    }

    /// S 座標の範囲 `roi`（この画像の範囲の内側）を切り出す。色の並びは切り出した左上に合わせる。
    pub fn window(&self, roi: Roi) -> Result<CfaImage> {
        let Some((ox, oy)) = roi.offset_in(&self.roi) else {
            return Err(PipelineError::InvalidRoi {
                roi,
                reason: "CFA の画像の範囲の外",
            });
        };
        let w = self.roi.width as usize;
        let len = checked_len(roi.width, roi.height)?;
        let mut data = Vec::with_capacity(len);
        let mut saturated = Vec::with_capacity(len);
        for y in oy..oy + roi.height as usize {
            let s = y * w + ox;
            data.extend_from_slice(&self.data[s..s + roi.width as usize]);
            saturated.extend_from_slice(&self.saturated[s..s + roi.width as usize]);
        }
        CfaImage::new(roi, self.pattern.shifted(ox, oy), data, saturated)
    }

    /// NaN・無限大を 0 に置き換え、置き換えた値の数を返す（2.6 節）。
    pub fn sanitize(&mut self) -> u64 {
        sanitize_slice(&mut self.data)
    }
}

/// RGB の画像と、それが覆う範囲（7.1 節の `ImageTile`）。
///
/// `roi` はそのステージの画素の格子での範囲で、`image` の寸法は `roi` の幅・高さと同じ。
/// センサーのステージ（[`crate::sensor`]）では S 座標（2 × 2 の簡易処理では半分の解像度の格子）、
/// ステージ 9 より後では C の格子（[`crate::finish::geometry`]。G の格子のうち切り抜きの範囲）。
#[derive(Debug, Clone, PartialEq)]
pub struct ImageTile {
    /// このタイルが覆う範囲。
    pub roi: Roi,
    /// 画素（寸法は `roi` と同じ）。
    pub image: RgbImage,
}

impl ImageTile {
    /// 0 で埋めたタイル。
    pub fn new(roi: Roi, contract: ColorContract) -> Result<Self> {
        Ok(Self {
            roi,
            image: RgbImage::new(roi.width, roi.height, contract)?,
        })
    }

    /// 画像と範囲から作る（寸法が一致すること）。
    pub fn from_image(roi: Roi, image: RgbImage) -> Result<Self> {
        if image.dimensions() != (roi.width, roi.height) {
            return Err(PipelineError::InvalidRoi {
                roi,
                reason: "範囲と画像の寸法が一致しない",
            });
        }
        Ok(Self { roi, image })
    }

    /// 色の契約。
    pub fn contract(&self) -> ColorContract {
        self.image.contract()
    }

    /// 格子の座標 (x, y) の画素（タイルの範囲外なら `None`）。
    pub fn at(&self, x: i64, y: i64) -> Option<[f32; 3]> {
        if !self.roi.contains(x, y) {
            return None;
        }
        let lx = (x - i64::from(self.roi.x)) as u32;
        let ly = (y - i64::from(self.roi.y)) as u32;
        self.image.pixel(lx, ly)
    }

    /// 範囲 `roi`（このタイルの範囲の内側）を切り出す。
    pub fn crop(&self, roi: Roi) -> Result<ImageTile> {
        let Some((ox, oy)) = roi.offset_in(&self.roi) else {
            return Err(PipelineError::InvalidRoi {
                roi,
                reason: "タイルの範囲の外",
            });
        };
        let image = self
            .image
            .crop(Roi::new(ox as i32, oy as i32, roi.width, roi.height))?;
        Ok(ImageTile { roi, image })
    }

    /// 幅 `grid_width`・高さ `grid_height` の画像の一部であるこのタイルから、範囲 `roi`（画像の外に
    /// はみ出してよい）を鏡映で取り出す（2.7 節）。`roi` の画素を鏡映で折り返した位置が、すべて
    /// このタイルの範囲に入っていること（[`crate::border::mirror_cover`] で求めた範囲を覆うこと）。
    pub fn gather_mirror(&self, roi: Roi, grid_width: u32, grid_height: u32) -> Result<ImageTile> {
        if grid_width == 0 || grid_height == 0 {
            return Err(PipelineError::InvalidDimensions {
                width: grid_width,
                height: grid_height,
            });
        }
        if roi.is_inside(grid_width, grid_height) && self.roi.contains_roi(&roi) {
            return self.crop(roi);
        }
        let mut data = Vec::with_capacity(checked_len(roi.width, roi.height)?);
        for y in 0..roi.height {
            let my = mirror_index(i64::from(roi.y) + i64::from(y), grid_height);
            for x in 0..roi.width {
                let mx = mirror_index(i64::from(roi.x) + i64::from(x), grid_width);
                let p = self
                    .at(i64::from(mx), i64::from(my))
                    .ok_or(PipelineError::InvalidRoi {
                        roi,
                        reason: "鏡映で参照する画素がタイルの範囲にない",
                    })?;
                data.push(p);
            }
        }
        Ok(ImageTile {
            roi,
            image: RgbImage::from_vec(roi.width, roi.height, self.contract(), data)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roi_basic_operations() {
        let r = Roi::new(2, 3, 4, 5);
        assert_eq!(r.x_end(), 6);
        assert_eq!(r.y_end(), 8);
        assert_eq!(r.pixel_count(), 20);
        assert_eq!(r.expand(2), Roi::new(0, 1, 8, 9));
        assert_eq!(r.expand(2).shrink(2), Some(r));
        assert_eq!(r.shrink(2), None);
        assert_eq!(Roi::new(0, 0, 5, 5).shrink(2), Some(Roi::new(2, 2, 1, 1)));
        assert_eq!(r.translate(-3, 1), Roi::new(-1, 4, 4, 5));
        assert_eq!(r.doubled(), Roi::new(4, 6, 8, 10));
        assert!(r.contains(2, 3) && r.contains(5, 7));
        assert!(!r.contains(6, 3) && !r.contains(2, 8) && !r.contains(1, 3));
        assert!(Roi::new(0, 0, 0, 3).is_empty());
    }

    #[test]
    fn roi_intersection_and_containment() {
        let a = Roi::new(0, 0, 10, 10);
        let b = Roi::new(-3, 5, 6, 20);
        assert_eq!(a.intersect(&b), Some(Roi::new(0, 5, 3, 5)));
        assert_eq!(a.intersect(&Roi::new(10, 0, 3, 3)), None);
        assert!(a.contains_roi(&Roi::new(2, 2, 8, 8)));
        assert!(!a.contains_roi(&Roi::new(2, 2, 9, 8)));
        assert!(a.contains_roi(&Roi::new(50, 50, 0, 0)));
        assert!(Roi::new(1, 1, 3, 3).is_inside(4, 4));
        assert!(!Roi::new(-1, 1, 3, 3).is_inside(4, 4));
        assert_eq!(Roi::new(3, 4, 2, 2).offset_in(&a), Some((3, 4)));
        assert_eq!(b.offset_in(&a), None);
    }

    #[test]
    fn roi_extreme_values_saturate() {
        let r = Roi::new(i32::MIN + 1, 0, u32::MAX - 1, 1).expand(10);
        assert_eq!(r.x, i32::MIN);
        assert_eq!(r.width, u32::MAX);
        assert!(Roi::new(i32::MAX, 0, 10, 10).shrink(1).is_none());
    }

    #[test]
    fn tiles_cover_the_image_exactly_once() {
        for (w, h, t) in [(10, 7, 4), (8, 8, 8), (1, 1, 16), (17, 3, 5)] {
            let tiles = Roi::tiles(w, h, t);
            let mut seen = vec![0u8; (w * h) as usize];
            for r in &tiles {
                assert!(r.is_inside(w, h) && !r.is_empty());
                assert!(r.width <= t && r.height <= t);
                for y in r.y..r.y + r.height as i32 {
                    for x in r.x..r.x + r.width as i32 {
                        seen[(y as u32 * w + x as u32) as usize] += 1;
                    }
                }
            }
            assert!(seen.iter().all(|&c| c == 1), "{w}×{h} / {t}");
        }
        assert!(Roi::tiles(10, 10, 0).is_empty());
        assert!(Roi::tiles(0, 10, 4).is_empty());
    }

    #[test]
    fn rgb_image_construction_and_access() {
        let img = RgbImage::from_fn(3, 2, ColorContract::B2Working, |x, y| {
            [x as f32, y as f32, 7.0]
        })
        .unwrap();
        assert_eq!(img.dimensions(), (3, 2));
        assert_eq!(img.pixel(2, 1), Some([2.0, 1.0, 7.0]));
        assert_eq!(img.pixel(3, 0), None);
        assert_eq!(img.row(1)[0], [0.0, 1.0, 7.0]);
        assert_eq!(img.as_flat().len(), 18);
        assert_eq!(img.contract(), ColorContract::B2Working);
        let c = img.crop(Roi::new(1, 0, 2, 2)).unwrap();
        assert_eq!(
            c.data(),
            &[
                [1.0, 0.0, 7.0],
                [2.0, 0.0, 7.0],
                [1.0, 1.0, 7.0],
                [2.0, 1.0, 7.0]
            ]
        );
        assert!(img.crop(Roi::new(2, 0, 2, 2)).is_err());
        assert!(img.crop(Roi::new(0, 0, 0, 2)).is_err());
    }

    #[test]
    fn rgb_image_rejects_bad_dimensions() {
        assert!(matches!(
            RgbImage::new(0, 3, ColorContract::B1CameraRgb),
            Err(PipelineError::InvalidDimensions { .. })
        ));
        assert!(matches!(
            RgbImage::new(20_000, 10_001, ColorContract::B1CameraRgb),
            Err(PipelineError::InvalidDimensions { .. })
        ));
        assert!(matches!(
            RgbImage::from_vec(2, 2, ColorContract::B1CameraRgb, vec![[0.0; 3]; 3]),
            Err(PipelineError::DataLengthMismatch {
                expected: 4,
                actual: 3
            })
        ));
        assert!(GrayImage::from_vec(2, 2, vec![0.0; 5]).is_err());
    }

    #[test]
    fn sanitize_counts_values() {
        let mut img =
            RgbImage::filled(2, 1, ColorContract::B2Working, [f32::NAN, 1.0, 2.0]).unwrap();
        assert_eq!(img.sanitize(), 2);
        assert_eq!(img.data(), &[[0.0, 1.0, 2.0]; 2]);
        let mut g = GrayImage::from_vec(2, 1, vec![f32::INFINITY, 0.5]).unwrap();
        assert_eq!(g.sanitize(), 1);
        assert_eq!(g.pixel(0, 0), Some(0.0));
    }

    #[test]
    fn cfa_window_shifts_the_pattern() {
        let cfa = CfaImage::from_fn(Roi::new(10, 20, 6, 4), CfaPattern::RGGB, |x, y| {
            (y * 10 + x) as f32
        })
        .unwrap();
        assert_eq!(cfa.color_at(0, 0), CfaColor::Red);
        let w = cfa.window(Roi::new(11, 21, 3, 2)).unwrap();
        assert_eq!(w.pattern(), CfaPattern::BGGR);
        assert_eq!(w.value(0, 0), Some(11.0));
        assert_eq!(w.value(2, 1), Some(23.0));
        assert_eq!(w.color_at(0, 0), CfaColor::Blue);
        assert!(cfa.window(Roi::new(9, 20, 2, 2)).is_err());
        assert_eq!(cfa.saturated_count(), 0);
        assert!(!cfa.is_saturated(100, 0));
    }

    #[test]
    fn cfa_lengths_are_checked() {
        let roi = Roi::new(0, 0, 2, 2);
        assert!(CfaImage::new(roi, CfaPattern::RGGB, vec![0.0; 4], vec![false; 3]).is_err());
        assert!(CfaImage::new(roi, CfaPattern::RGGB, vec![0.0; 3], vec![false; 4]).is_err());
        assert!(CfaImage::new(roi, CfaPattern::RGGB, vec![0.0; 4], vec![false; 4]).is_ok());
    }

    #[test]
    fn tile_gather_mirror() {
        // 幅 4・高さ 3 の画像全体のタイル。画素 (x, y) の R = 10y + x。
        let img = RgbImage::from_fn(4, 3, ColorContract::B2Working, |x, y| {
            [(10 * y + x) as f32, 0.0, 0.0]
        })
        .unwrap();
        let tile = ImageTile::from_image(Roi::full(4, 3), img).unwrap();
        let g = tile.gather_mirror(Roi::new(-2, -1, 8, 5), 4, 3).unwrap();
        let reds: Vec<f32> = g.image.data().iter().map(|p| p[0]).collect();
        // x: −2 −1 0 1 2 3 4 5 → 2 1 0 1 2 3 2 1、y: −1 0 1 2 3 → 1 0 1 2 1。
        let xs = [2, 1, 0, 1, 2, 3, 2, 1];
        let ys = [1, 0, 1, 2, 1];
        let expect: Vec<f32> = ys
            .iter()
            .flat_map(|&y| xs.iter().map(move |&x| (10 * y + x) as f32))
            .collect();
        assert_eq!(reds, expect);
        assert_eq!(g.roi, Roi::new(-2, -1, 8, 5));
        // 内側ならただの切り出し。
        let c = tile.gather_mirror(Roi::new(1, 1, 2, 2), 4, 3).unwrap();
        assert_eq!(c.at(2, 2), Some([22.0, 0.0, 0.0]));
        // 参照先がタイルにない。
        let part = tile.crop(Roi::new(2, 0, 2, 3)).unwrap();
        assert!(part.gather_mirror(Roi::new(-1, 0, 2, 1), 4, 3).is_err());
        assert!(ImageTile::from_image(Roi::new(0, 0, 3, 3), tile.image.clone()).is_err());
    }
}
