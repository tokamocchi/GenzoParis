//! LibRaw から読み取った値を、この crate の型（[`RawImage`]・[`PhotoMetadata`]）に変換する。
//!
//! LibRaw に依存しない純粋な関数だけを置き、機能フラグ `libraw` がなくてもテストできるようにする。
//! LibRaw の値の意味は、LibRaw 0.21 のドキュメント（API-datastruct）とソース
//! （`src/metadata/tiff.cpp`・`identify.cpp`・`src/decoders/unpack.cpp`・
//! `src/utils/utils_dcraw.cpp`）で確かめた範囲で書く。実際の ARW での確認は PoC-2 で行う。
#![cfg_attr(not(feature = "libraw"), allow(dead_code))]

use genzo_model::{CaptureInfo, GpsCoord, Orientation, PhotoMetadata};

use crate::decode::{
    CamXyzSource, CaptureTimeSource, DecodedRaw, DngColorMatrixInfo, RawDetails, WbSource,
};
use crate::thumbnail::{EmbeddedThumbnail, ThumbnailFormat, jpeg_dimensions};
use crate::types::{MAX_WB_COEFF, validate_matrix};
use crate::{CfaColor, CfaPattern, MAX_PIXELS, RawError, RawImage};

/// DNG の CalibrationIlluminant の D65（EXIF の LightSource の 21。LibRaw の `LIBRAW_WBI_D65`）。
pub(crate) const ILLUMINANT_D65: u32 = 21;

/// LibRaw の `filters` のうち、特殊な配置を表す値の上限（これ未満は 2 × 2 のベイヤーではない。
/// LibRaw の API-datastruct の `filters` の説明: 1 は Leaf の 16 × 16、9 は X-Trans）。
const FILTERS_SPECIAL_MAX: u32 = 1000;

/// LibRaw の 4 色の番号（`COLOR()` の戻り値）。3 色の RAW では cdesc が "RGBG" で、
/// 0 = R、1 = G、2 = B、3 = 2 つ目の G。
const LR_RED: i32 = 0;
const LR_GREEN: i32 = 1;
const LR_BLUE: i32 = 2;
const LR_GREEN2: i32 = 3;

/// LibRaw の寸法と CFA（シムの `genzo_lr_sizes`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SizesSnapshot {
    pub raw_width: u32,
    pub raw_height: u32,
    pub width: u32,
    pub height: u32,
    pub top_margin: u32,
    pub left_margin: u32,
    pub raw_pitch: u32,
    pub filters: u32,
    pub colors: i32,
    pub dng_version: u32,
    pub is_foveon: bool,
    pub fuji_rotated: bool,
    /// `COLOR(row, col)`（有効画素の座標。8 行 × 2 列）。
    pub cfa: [[i32; 2]; 8],
    pub cdesc: String,
}

/// DNG の色の情報（`imgdata.color.dng_color[i]`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DngColorSnapshot {
    pub illuminant: u32,
    pub color_matrix: [[f32; 3]; 4],
    pub calibration: [[f32; 4]; 4],
}

/// LibRaw の色の情報（シムの `genzo_lr_color`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColorSnapshot {
    pub black: u32,
    pub maximum: u32,
    /// `cblack` の全体（`LIBRAW_CBLACK_SIZE` 個）。
    pub cblack: Vec<u32>,
    pub linear_max: [i64; 4],
    pub cam_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    pub cam_xyz: [[f32; 3]; 4],
    pub rgb_cam: [[f32; 4]; 3],
    pub dng_color: [DngColorSnapshot; 2],
    pub dng_analog_balance: [f32; 4],
    pub raw_bps: u32,
}

/// LibRaw の GPS（`imgdata.other.parsed_gps`）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct GpsSnapshot {
    pub parsed: bool,
    /// 度・分・秒。
    pub latitude: [f32; 3],
    pub longitude: [f32; 3],
    pub latitude_ref: u8,
    pub longitude_ref: u8,
}

/// LibRaw の撮影情報（シムの `genzo_lr_meta`）。
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct MetaSnapshot {
    pub make: Option<String>,
    pub model: Option<String>,
    pub exif_make: Option<String>,
    pub exif_model: Option<String>,
    pub lens: Option<String>,
    pub makernotes_lens: Option<String>,
    pub iso_speed: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub timestamp: i64,
    pub timestamp_local: Option<String>,
    pub datetime_original: Option<String>,
    pub subsec_time_original: Option<String>,
    pub offset_time_original: Option<String>,
    pub gps: GpsSnapshot,
    pub flip: i32,
    pub width: u32,
    pub height: u32,
}

/// 展開に使ったデコーダの情報。
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct DecoderInfo {
    pub decoder_id: String,
    pub unpack_function: String,
    pub data_error_count: i32,
}

/// 有効画素と RAW のバッファの寸法を検証する（展開の前後で使う）。
pub(crate) fn check_dimensions(s: &SizesSnapshot) -> Result<(), RawError> {
    if s.width == 0 || s.height == 0 {
        return Err(RawError::InvalidDimensions {
            width: s.width,
            height: s.height,
        });
    }
    for (w, h) in [(s.width, s.height), (s.raw_width, s.raw_height)] {
        let pixels = u64::from(w) * u64::from(h);
        if pixels > MAX_PIXELS {
            return Err(RawError::TooManyPixels {
                pixels,
                max: MAX_PIXELS,
            });
        }
    }
    if u64::from(s.left_margin) + u64::from(s.width) > u64::from(s.raw_width)
        || u64::from(s.top_margin) + u64::from(s.height) > u64::from(s.raw_height)
    {
        return Err(RawError::Decode(format!(
            "有効画素の範囲（{}×{}、余白 左 {}・上 {}）が RAW のバッファ（{}×{}）からはみ出しています",
            s.width, s.height, s.left_margin, s.top_margin, s.raw_width, s.raw_height
        )));
    }
    Ok(())
}

/// LibRaw の `COLOR()` の 2 × 2 の値から CFA の配列を作る。
///
/// 3 色・2 × 2 の周期のベイヤー配列だけを受け付ける。`filters == 0`（CFA のない RAW）、
/// `filters < 1000`（X-Trans など）、Fuji の斜めの配置、4 色の CFA は [`RawError::Unsupported`]。
/// 2 つの緑は、LibRaw の番号（1 と 3）ではなく位置で区別する（R と同じ行が G1）。
pub(crate) fn cfa_from_libraw(s: &SizesSnapshot) -> Result<CfaPattern, RawError> {
    if s.is_foveon {
        return Err(RawError::Unsupported(
            "Foveon のセンサーの RAW には対応していません".to_owned(),
        ));
    }
    if s.filters == 0 {
        return Err(RawError::Unsupported(
            "CFA のない RAW（リニア DNG など）には対応していません".to_owned(),
        ));
    }
    if s.filters < FILTERS_SPECIAL_MAX {
        let kind = match s.filters {
            9 => "X-Trans の 6 × 6 の配列",
            1 => "Leaf の 16 × 16 の配列",
            _ => "特殊な CFA の配列",
        };
        return Err(RawError::Unsupported(format!(
            "{kind}には対応していません（filters = {}）",
            s.filters
        )));
    }
    if s.fuji_rotated {
        return Err(RawError::Unsupported(
            "Fuji の斜めの画素配列には対応していません".to_owned(),
        ));
    }
    if s.colors != 3 || !s.cdesc.starts_with("RGB") {
        return Err(RawError::Unsupported(format!(
            "3 色（RGB）の CFA ではありません（colors = {}、cdesc = {:?}）",
            s.colors, s.cdesc
        )));
    }
    // filters は 8 行 × 2 列の周期を表せるため、2 行の周期になっているかを確かめる。
    for (row, cols) in s.cfa.iter().enumerate() {
        if *cols != s.cfa[row % 2] {
            return Err(RawError::Unsupported(format!(
                "CFA の配列が 2 × 2 の周期になっていません（{:?}）",
                s.cfa
            )));
        }
    }
    let idx = [s.cfa[0], s.cfa[1]];
    let count = |v: &[i32]| idx.iter().flatten().filter(|c| v.contains(c)).count();
    if count(&[LR_RED]) != 1 || count(&[LR_BLUE]) != 1 || count(&[LR_GREEN, LR_GREEN2]) != 2 {
        return Err(RawError::Unsupported(format!(
            "R・G・G・B のベイヤー配列ではありません（{idx:?}）"
        )));
    }
    let red_row = idx
        .iter()
        .position(|row| row.contains(&LR_RED))
        .expect("R の数は 1 であることを確かめた");
    let mut colors = [[CfaColor::Red; 2]; 2];
    for (y, row) in idx.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            colors[y][x] = match c {
                LR_RED => CfaColor::Red,
                LR_BLUE => CfaColor::Blue,
                _ if y == red_row => CfaColor::Green1,
                _ => CfaColor::Green2,
            };
        }
    }
    CfaPattern::from_colors(colors).map_err(|e| RawError::Unsupported(e.to_string()))
}

/// 最大公約数。
fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// 最小公倍数（a, b は 1 以上）。
fn lcm(a: usize, b: usize) -> usize {
    a / gcd(a, b) * b
}

/// 黒レベルを CFA の位置ごとに合成する（R, G1, B, G2 の順）。
///
/// LibRaw の黒レベルは 3 つの部分の和（API-datastruct の `black`・`cblack`）:
/// - `black`: 全体に共通の値
/// - `cblack[0..4]`: LibRaw の色の番号（`COLOR()` の値）ごとの値
/// - `cblack[4]` × `cblack[5]` の繰り返しのパターン `cblack[6..]`（有効画素の座標で
///   `cblack[6 + (row % cblack[4]) * cblack[5] + col % cblack[5]]`。LibRaw の
///   `subtract_black_internal` と同じ添字）
///
/// パターンが 2 × 2 の周期で表せない（CFA の位置ごとの 4 つの値にならない）場合は
/// [`RawError::Unsupported`]。`cfa_idx` は有効画素の (row, col) = (0..2, 0..2) の `COLOR()`。
pub(crate) fn black_levels(
    c: &ColorSnapshot,
    cfa_idx: [[i32; 2]; 2],
    cfa: CfaPattern,
) -> Result<[f32; 4], RawError> {
    let dim = |i: usize| c.cblack.get(i).copied().unwrap_or(0) as usize;
    let (ph, pw) = (dim(4), dim(5));
    let has_pattern = ph > 0 && pw > 0;
    let fits = ph
        .checked_mul(pw)
        .and_then(|n| n.checked_add(6))
        .is_some_and(|n| n <= c.cblack.len());
    if has_pattern && !fits {
        return Err(RawError::Unsupported(format!(
            "黒レベルのパターン（{ph} × {pw}）が LibRaw の配列に収まりません"
        )));
    }
    let pattern = |row: usize, col: usize| -> u64 {
        if has_pattern {
            u64::from(c.cblack[6 + (row % ph) * pw + col % pw])
        } else {
            0
        }
    };
    if has_pattern {
        for row in 0..lcm(ph, 2) {
            for col in 0..lcm(pw, 2) {
                if pattern(row, col) != pattern(row % 2, col % 2) {
                    return Err(RawError::Unsupported(format!(
                        "黒レベルのパターン（{ph} × {pw}）が CFA の 2 × 2 の周期で表せません"
                    )));
                }
            }
        }
    }
    let mut out = [0.0f32; 4];
    for (y, row) in cfa_idx.iter().enumerate() {
        for (x, &lr) in row.iter().enumerate() {
            let per_color = usize::try_from(lr)
                .ok()
                .filter(|&i| i < 4)
                .and_then(|i| c.cblack.get(i))
                .copied()
                .ok_or_else(|| {
                    RawError::Unsupported(format!("CFA の色の番号が不正です（{lr}）"))
                })?;
            let total = u64::from(c.black) + u64::from(per_color) + pattern(y, x);
            out[cfa.color_at(x, y).index()] = total as f32;
        }
    }
    Ok(out)
}

/// LibRaw の色の番号の順（R, G, B, G2）の 4 つの値を、CFA の位置の順（R, G1, B, G2）に並べ替える。
///
/// LibRaw の G2（番号 3）が 0 以下なら G（番号 1）の値で補う（`cam_mul[3] == 0` は「G と同じ」の意味）。
fn reorder_by_position(values: [f32; 4], cfa_idx: [[i32; 2]; 2], cfa: CfaPattern) -> [f32; 4] {
    let mut v = values;
    if !(v[3].is_finite() && v[3] > 0.0) {
        v[3] = v[1];
    }
    let mut out = [0.0f32; 4];
    for (y, row) in cfa_idx.iter().enumerate() {
        for (x, &lr) in row.iter().enumerate() {
            if let Some(&value) = usize::try_from(lr).ok().and_then(|i| v.get(i)) {
                out[cfa.color_at(x, y).index()] = value;
            }
        }
    }
    out
}

/// 係数を G1 を 1 として正規化する。すべてが正の有限の値で上限以内でなければ `None`。
fn normalize_wb(v: [f32; 4]) -> Option<[f32; 4]> {
    let g = v[1];
    if !(g.is_finite() && g > 0.0) {
        return None;
    }
    let n = v.map(|x| x / g);
    n.iter()
        .all(|&x| x.is_finite() && x > 0.0 && x <= MAX_WB_COEFF)
        .then_some(n)
}

/// 撮影時の WB の係数（R, G1, B, G2 の順、G1 = 1）を決める。
///
/// 1. `cam_mul`（撮影時の WB。LibRaw の API-datastruct: "White balance coefficients (as shot)"）
/// 2. 1 が使えなければ `pre_mul`（昼光の WB。"daylight balance"）
/// 3. どちらも使えなければ (1, 1, 1, 1)
///
/// 2 と 3 は仮置きの代わりの値で、[`WbSource`] に記録する。
pub(crate) fn as_shot_wb(
    c: &ColorSnapshot,
    cfa_idx: [[i32; 2]; 2],
    cfa: CfaPattern,
) -> ([f32; 4], WbSource) {
    if let Some(wb) = normalize_wb(reorder_by_position(c.cam_mul, cfa_idx, cfa)) {
        return (wb, WbSource::AsShot);
    }
    if let Some(wb) = normalize_wb(reorder_by_position(c.pre_mul, cfa_idx, cfa)) {
        return (wb, WbSource::Daylight);
    }
    ([1.0; 4], WbSource::Unity)
}

/// 3 × 3 の行列がすべて 0 か（LibRaw は行列がない場合に 0 のままにする）。
fn is_zero_matrix(m: &[[f32; 3]; 3]) -> bool {
    m.iter().flatten().all(|&v| v == 0.0)
}

/// 4 × 3 の行列の上の 3 行。
fn upper3(m: &[[f32; 3]; 4]) -> [[f32; 3]; 3] {
    [m[0], m[1], m[2]]
}

/// DNG の ColorMatrix に CameraCalibration と AnalogBalance を掛ける（XYZ → カメラ）。
///
/// DNG の仕様（1.4 の 6 章 "Mapping Camera Color Space to CIE XYZ Space"）の
/// XYZ → カメラ = AB × CC × CM。LibRaw の `parse_tiff_ifd` の `cam_xyz` の計算と同じ。
/// CameraCalibration がすべて 0（記録がない）なら単位行列、AnalogBalance が正でなければ 1 とする。
fn dng_xyz_to_camera(dc: &DngColorSnapshot, analog_balance: [f32; 4]) -> [[f32; 3]; 3] {
    let cm = upper3(&dc.color_matrix);
    let cal_zero = dc.calibration[..3]
        .iter()
        .all(|row| row[..3].iter().all(|&v| v == 0.0));
    let cc = |r: usize, k: usize| -> f64 {
        if cal_zero {
            if r == k { 1.0 } else { 0.0 }
        } else {
            f64::from(dc.calibration[r][k])
        }
    };
    let mut out = [[0.0f32; 3]; 3];
    for (r, out_row) in out.iter_mut().enumerate() {
        let ab = analog_balance[r];
        let ab = if ab.is_finite() && ab > 0.0 {
            f64::from(ab)
        } else {
            1.0
        };
        for (col, value) in out_row.iter_mut().enumerate() {
            let sum: f64 = (0..3).map(|k| cc(r, k) * f64::from(cm[k][col])).sum();
            *value = (ab * sum) as f32;
        }
    }
    out
}

/// XYZ（D65）→ カメラ RGB の行列を選ぶ（04 の 2.6 節）。
///
/// - DNG: CalibrationIlluminant が D65（21）の ColorMatrix（AB × CC を掛けたもの）。
///   LibRaw の `imgdata.color.cam_xyz` は DNG の ColorMatrix からは設定されない（LibRaw 0.21 の
///   `parse_tiff_ifd` は ColorMatrix を `cmatrix`・`rgb_cam` の計算にだけ使い、`cam_xyz` は
///   内蔵の表（`adobe_coeff`）からだけ設定する）ため、DNG では `dng_color` を使う。
/// - DNG で D65 の行列がない場合と DNG 以外: LibRaw の `cam_xyz`（内蔵の表。4 × 3 のうち 3 × 3）。
///   LibRaw の表の注記は「All matrices are from Adobe DNG Converter unless otherwise noted」で、
///   dcraw 以来の慣習では D65 の行列（Adobe の ColorMatrix2）とされるが、どの光源の行列かは
///   LibRaw のドキュメントに明記がない。**PoC-2 で α7 IV / α7C の ARW と DNG で確認する。**
/// - どちらも使えない（すべて 0・正則でない）場合は `None`。
pub(crate) fn select_cam_xyz(
    is_dng: bool,
    c: &ColorSnapshot,
) -> (Option<[[f32; 3]; 3]>, CamXyzSource) {
    if is_dng {
        for (i, dc) in c.dng_color.iter().enumerate() {
            if dc.illuminant != ILLUMINANT_D65 || is_zero_matrix(&upper3(&dc.color_matrix)) {
                continue;
            }
            let m = dng_xyz_to_camera(dc, c.dng_analog_balance);
            if validate_matrix(&m).is_ok() {
                return (
                    Some(m),
                    CamXyzSource::DngColorMatrix {
                        index: i as u8 + 1,
                        illuminant: ILLUMINANT_D65 as u16,
                    },
                );
            }
        }
    }
    let m = upper3(&c.cam_xyz);
    if !is_zero_matrix(&m) && validate_matrix(&m).is_ok() {
        return (Some(m), CamXyzSource::LibRawTable);
    }
    (None, CamXyzSource::None)
}

/// LibRaw の `flip`（dcraw の向きのビット）から EXIF の向きにする。
///
/// LibRaw（dcraw）は EXIF の Orientation（1〜8）を `"50132467"[o & 7] - '0'` で `flip` に
/// している（`src/metadata/tiff.cpp`）。その逆の対応。範囲外の値は [`Orientation::Normal`]。
pub(crate) fn orientation_from_flip(flip: i32) -> Orientation {
    match flip {
        1 => Orientation::FlipHorizontal,
        2 => Orientation::FlipVertical,
        3 => Orientation::Rotate180,
        4 => Orientation::Transpose,
        5 => Orientation::Rotate270Cw,
        6 => Orientation::Rotate90Cw,
        7 => Orientation::Transverse,
        _ => Orientation::Normal,
    }
}

/// 度・分・秒と、方角の記号（正の向き・負の向き）から 10 進数の度にする。
fn dms_to_degrees(dms: [f32; 3], reference: u8, positive: u8, negative: u8) -> Option<f64> {
    if !dms.iter().all(|v| v.is_finite() && *v >= 0.0) {
        return None;
    }
    let deg = f64::from(dms[0]) + f64::from(dms[1]) / 60.0 + f64::from(dms[2]) / 3600.0;
    match reference.to_ascii_uppercase() {
        r if r == positive => Some(deg),
        r if r == negative => Some(-deg),
        _ => None,
    }
}

/// LibRaw の GPS から座標を作る。緯度・経度の方角の記号（N/S、E/W）がなければ `None`。
pub(crate) fn gps_from_snapshot(g: &GpsSnapshot) -> Option<GpsCoord> {
    if !g.parsed {
        return None;
    }
    let lat = dms_to_degrees(g.latitude, g.latitude_ref, b'N', b'S')?;
    let lon = dms_to_degrees(g.longitude, g.longitude_ref, b'E', b'W')?;
    GpsCoord::new(lat, lon)
}

/// EXIF の日時の文字列が意味のある値か（空・すべて 0 の「不明」の値でない）。
fn has_datetime(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit() && c != '0')
}

/// 撮影日時（元の文字列とオフセット）を決める。
///
/// 1. EXIF の DateTimeOriginal（シムが LibRaw の EXIF のコールバックで読んだ元の文字列）。
///    SubSecTimeOriginal があれば `".123"` の形で付ける。オフセットは OffsetTimeOriginal。
/// 2. 1 がなければ、LibRaw の `timestamp` を同じプロセスのローカル時刻に戻した文字列
///    （LibRaw は日時を `mktime` で time_t にしているため。DateTimeOriginal 以外のタグ
///    （DateTime など）から読んだ値の場合もある。夏時間の切り替わりの時刻は正しく戻らない）。
pub(crate) fn capture_info(m: &MetaSnapshot) -> (CaptureInfo, CaptureTimeSource) {
    if let Some(dt) = m.datetime_original.as_deref().filter(|s| has_datetime(s)) {
        let subsec = m
            .subsec_time_original
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
        let datetime = match subsec {
            Some(frac) if !dt.contains('.') => format!("{dt}.{frac}"),
            _ => dt.to_owned(),
        };
        let offset = m
            .offset_time_original
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        return (
            CaptureInfo {
                datetime: Some(datetime),
                offset,
            },
            CaptureTimeSource::ExifDateTimeOriginal,
        );
    }
    if m.timestamp > 0
        && let Some(local) = m.timestamp_local.as_deref().filter(|s| has_datetime(s))
    {
        return (
            CaptureInfo {
                datetime: Some(local.to_owned()),
                offset: None,
            },
            CaptureTimeSource::LibRawTimestamp,
        );
    }
    (CaptureInfo::default(), CaptureTimeSource::None)
}

/// 正の有限の値なら `Some`。
fn positive(v: f32) -> Option<f32> {
    (v.is_finite() && v > 0.0).then_some(v)
}

/// 撮影情報を作る。
///
/// - メーカー・機種: IFD の Make / Model の元の文字列（例: `"SONY"`）。なければ LibRaw が
///   正規化した名前（例: `"Sony"`）。
/// - レンズ: LibRaw の `lens.Lens`（EXIF の LensModel など）、なければメーカーノートのレンズ名。
/// - 寸法: LibRaw の有効画素（`sizes.width`・`height`。向きを適用する前）。
pub(crate) fn photo_metadata(m: &MetaSnapshot) -> (PhotoMetadata, CaptureTimeSource) {
    let (capture, source) = capture_info(m);
    let iso = positive(m.iso_speed).map(|v| v.round().min(u32::MAX as f32) as u32);
    let meta = PhotoMetadata {
        make: m.exif_make.clone().or_else(|| m.make.clone()),
        model: m.exif_model.clone().or_else(|| m.model.clone()),
        lens: m.lens.clone().or_else(|| m.makernotes_lens.clone()),
        iso,
        aperture: positive(m.aperture),
        shutter_s: positive(m.shutter),
        focal_mm: positive(m.focal_len),
        width: (m.width > 0).then_some(m.width),
        height: (m.height > 0).then_some(m.height),
        orientation: orientation_from_flip(m.flip),
        gps: gps_from_snapshot(&m.gps),
        capture,
    };
    (meta, source)
}

/// 展開した値から [`DecodedRaw`] を作り、[`RawImage::validate`] で検証する。
pub(crate) fn build_decoded(
    s: &SizesSnapshot,
    c: &ColorSnapshot,
    m: &MetaSnapshot,
    data: Vec<u16>,
    decoder: DecoderInfo,
) -> Result<DecodedRaw, RawError> {
    check_dimensions(s)?;
    let cfa = cfa_from_libraw(s)?;
    let cfa_idx = [s.cfa[0], s.cfa[1]];
    let black_level = black_levels(c, cfa_idx, cfa)?;
    let (as_shot_wb, wb_source) = as_shot_wb(c, cfa_idx, cfa);
    let is_dng = s.dng_version != 0;
    let (cam_xyz, cam_xyz_source) = select_cam_xyz(is_dng, c);
    let (metadata, capture_time_source) = photo_metadata(m);
    let image = RawImage {
        width: s.width,
        height: s.height,
        cfa,
        data,
        black_level,
        white_level: c.maximum as f32,
        as_shot_wb,
        cam_xyz,
        metadata,
    };
    image.validate()?;
    let dng_color_matrices = c
        .dng_color
        .iter()
        .enumerate()
        .filter(|(_, dc)| !is_zero_matrix(&upper3(&dc.color_matrix)))
        .map(|(i, dc)| DngColorMatrixInfo {
            index: i as u8 + 1,
            illuminant: u16::try_from(dc.illuminant).unwrap_or(u16::MAX),
            matrix: upper3(&dc.color_matrix),
        })
        .collect();
    let details = RawDetails {
        decoder_id: decoder.decoder_id,
        unpack_function: decoder.unpack_function,
        data_error_count: decoder.data_error_count,
        is_dng,
        raw_width: s.raw_width,
        raw_height: s.raw_height,
        left_margin: s.left_margin,
        top_margin: s.top_margin,
        raw_pitch_bytes: s.raw_pitch,
        raw_bits_per_sample: c.raw_bps,
        libraw_black: c.black,
        libraw_cblack: [c.cblack[0], c.cblack[1], c.cblack[2], c.cblack[3]],
        black_pattern_size: [c.cblack[4], c.cblack[5]],
        linear_max: c.linear_max,
        cam_mul: c.cam_mul,
        pre_mul: c.pre_mul,
        libraw_cam_xyz: c.cam_xyz,
        rgb_cam: c.rgb_cam,
        dng_color_matrices,
        cam_xyz_source,
        wb_source,
        timestamp: (m.timestamp > 0).then_some(m.timestamp),
        capture_time_source,
        libraw_make: m.make.clone(),
        libraw_model: m.model.clone(),
    };
    Ok(DecodedRaw { image, details })
}

/// LibRaw のサムネイルの形式（`LibRaw_thumbnail_formats`）の値。
pub(crate) mod libraw_thumb {
    pub(crate) const JPEG: i32 = 1;
    pub(crate) const BITMAP: i32 = 2;
}

/// LibRaw の埋め込みサムネイルから [`EmbeddedThumbnail`] を作り、検証する。
///
/// JPEG の寸法は、JPEG の SOF から読めればそれを、読めなければ LibRaw の値を使う。
/// ビットマップは 8bit の RGB（`colors == 3`）とグレー（`colors == 1`）だけを受け付ける。
pub(crate) fn thumbnail_from_libraw(
    format: i32,
    width: u32,
    height: u32,
    colors: i32,
    data: Vec<u8>,
) -> Result<EmbeddedThumbnail, RawError> {
    let thumb = match format {
        libraw_thumb::JPEG => {
            let (w, h) = jpeg_dimensions(&data).unwrap_or((width, height));
            EmbeddedThumbnail {
                format: ThumbnailFormat::Jpeg,
                width: w,
                height: h,
                data,
            }
        }
        libraw_thumb::BITMAP => {
            let format = match colors {
                3 => ThumbnailFormat::Rgb8,
                1 => ThumbnailFormat::Gray8,
                _ => {
                    return Err(RawError::Unsupported(format!(
                        "埋め込みサムネイルの色の数 {colors} には対応していません"
                    )));
                }
            };
            EmbeddedThumbnail {
                format,
                width,
                height,
                data,
            }
        }
        other => {
            let name = match other {
                0 => "不明",
                3 => "16bit のビットマップ",
                4 => "Kodak のレイヤー",
                5 => "Rollei",
                6 => "H.265",
                _ => "その他",
            };
            return Err(RawError::Unsupported(format!(
                "埋め込みサムネイルの形式（{name}、LibRaw の値 {other}）には対応していません"
            )));
        }
    };
    thumb.validate()?;
    Ok(thumb)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LibRaw の `filters` の値と、それが表す 2 × 2 の `COLOR()`（4 色化した後）。
    ///
    /// LibRaw の `identify()` は 3 色の CFA の `filters` に
    /// `filters |= ((filters >> 2 & 0x22222222) | (filters << 2 & 0x88888888)) & filters << 1`
    /// を適用し、片方の緑を 3 にする。ここではそれを再現して `COLOR()` の表を作る。
    fn colors_from_filters(filters: u32) -> [[i32; 2]; 8] {
        let f = filters
            | (((filters >> 2) & 0x2222_2222) | ((filters << 2) & 0x8888_8888)) & (filters << 1);
        let mut out = [[0; 2]; 8];
        for (row, cols) in out.iter_mut().enumerate() {
            for (col, v) in cols.iter_mut().enumerate() {
                *v = ((f >> ((((row << 1) & 14) | (col & 1)) << 1)) & 3) as i32;
            }
        }
        out
    }

    fn sizes(cfa: [[i32; 2]; 8]) -> SizesSnapshot {
        SizesSnapshot {
            raw_width: 120,
            raw_height: 80,
            width: 100,
            height: 60,
            top_margin: 10,
            left_margin: 20,
            raw_pitch: 240,
            filters: 0x9494_9494,
            colors: 3,
            dng_version: 0,
            is_foveon: false,
            fuji_rotated: false,
            cfa,
            cdesc: "RGBG".to_owned(),
        }
    }

    fn color() -> ColorSnapshot {
        let mut cblack = vec![0u32; 4104];
        cblack[..4].copy_from_slice(&[0, 0, 0, 0]);
        ColorSnapshot {
            black: 512,
            maximum: 16383,
            cblack,
            linear_max: [0; 4],
            cam_mul: [2400.0, 1024.0, 1600.0, 1024.0],
            pre_mul: [2.0, 1.0, 1.5, 0.0],
            cam_xyz: [
                [0.7, -0.2, -0.05],
                [-0.45, 1.25, 0.2],
                [-0.07, 0.15, 0.6],
                [0.0, 0.0, 0.0],
            ],
            rgb_cam: [[0.0; 4]; 3],
            dng_color: [DngColorSnapshot {
                illuminant: 0,
                color_matrix: [[0.0; 3]; 4],
                calibration: [[0.0; 4]; 4],
            }; 2],
            dng_analog_balance: [0.0; 4],
            raw_bps: 14,
        }
    }

    /// dcraw の公表されている `filters` の値（RGGB = 0x94949494 など）が、正しい配列になる。
    #[test]
    fn cfa_from_known_filters_values() {
        let cases = [
            (0x9494_9494, CfaPattern::RGGB),
            (0x1616_1616, CfaPattern::BGGR),
            (0x6161_6161, CfaPattern::GRBG),
            (0x4949_4949, CfaPattern::GBRG),
        ];
        for (filters, expected) in cases {
            let mut s = sizes(colors_from_filters(filters));
            s.filters = filters;
            assert_eq!(cfa_from_libraw(&s).unwrap(), expected, "{filters:#x}");
        }
    }

    #[test]
    fn greens_are_named_by_position_not_by_libraw_index() {
        // LibRaw の番号 1 と 3 を入れ替えても、R の行の緑が G1 になる。
        let mut s = sizes([[0, 3], [1, 2]].repeat(4).try_into().unwrap());
        assert_eq!(cfa_from_libraw(&s).unwrap(), CfaPattern::RGGB);
        s.cfa = [[1, 0], [2, 3]].repeat(4).try_into().unwrap();
        assert_eq!(cfa_from_libraw(&s).unwrap(), CfaPattern::GRBG);
        assert_eq!(
            CfaPattern::GRBG.color_at(0, 0),
            CfaColor::Green1,
            "GRBG の左上は R の行の緑"
        );
    }

    #[test]
    fn unsupported_cfas_are_rejected() {
        let base = sizes(colors_from_filters(0x9494_9494));
        let reject = |f: &dyn Fn(&mut SizesSnapshot)| {
            let mut s = base.clone();
            f(&mut s);
            assert!(
                matches!(cfa_from_libraw(&s), Err(RawError::Unsupported(_))),
                "{s:?}"
            );
        };
        reject(&|s| s.filters = 0);
        reject(&|s| s.filters = 9);
        reject(&|s| s.filters = 1);
        reject(&|s| s.is_foveon = true);
        reject(&|s| s.fuji_rotated = true);
        reject(&|s| s.colors = 4);
        reject(&|s| s.cdesc = "GMCY".to_owned());
        // 4 行目だけ配置が違う（2 行の周期ではない）。
        reject(&|s| s.cfa[3] = [2, 1]);
        // 緑が 3 つ。
        reject(&|s| s.cfa = [[1, 1], [3, 2]].repeat(4).try_into().unwrap());
        // 範囲外の番号。
        reject(&|s| s.cfa = [[0, 5], [1, 2]].repeat(4).try_into().unwrap());
    }

    #[test]
    fn dimensions_are_checked() {
        let s = sizes(colors_from_filters(0x9494_9494));
        check_dimensions(&s).unwrap();
        let mut t = s.clone();
        t.width = 0;
        assert!(matches!(
            check_dimensions(&t),
            Err(RawError::InvalidDimensions { .. })
        ));
        let mut t = s.clone();
        t.width = 101;
        assert!(matches!(check_dimensions(&t), Err(RawError::Decode(_))));
        let mut t = s.clone();
        t.top_margin = 21;
        assert!(check_dimensions(&t).is_err());
        let mut t = s;
        t.raw_width = 20_000;
        t.raw_height = 10_001;
        assert!(matches!(
            check_dimensions(&t),
            Err(RawError::TooManyPixels { .. })
        ));
    }

    #[test]
    fn black_levels_combine_three_parts() {
        let cfa = CfaPattern::RGGB;
        let idx = [[0, 1], [3, 2]];
        let mut c = color();
        c.black = 500;
        c.cblack[..4].copy_from_slice(&[1, 2, 3, 4]);
        // 2 × 2 のパターン（行優先）。
        c.cblack[4] = 2;
        c.cblack[5] = 2;
        c.cblack[6..10].copy_from_slice(&[10, 20, 30, 40]);
        let b = black_levels(&c, idx, cfa).unwrap();
        // R: (0,0) → 500 + cblack[0] + 10
        assert_eq!(b, [511.0, 522.0, 543.0, 534.0]);
    }

    #[test]
    fn black_level_pattern_of_one_by_one_and_larger_periods() {
        let cfa = CfaPattern::GBRG;
        let idx = [[1, 2], [0, 3]];
        let mut c = color();
        c.black = 0;
        // 1 × 1 のパターン: 全体に足す。
        c.cblack[4] = 1;
        c.cblack[5] = 1;
        c.cblack[6] = 7;
        assert_eq!(black_levels(&c, idx, cfa).unwrap(), [7.0; 4]);
        // 4 × 2 でも 2 × 2 の周期なら受け付ける。
        c.cblack[4] = 4;
        c.cblack[5] = 2;
        c.cblack[6..14].copy_from_slice(&[1, 2, 3, 4, 1, 2, 3, 4]);
        let b = black_levels(&c, idx, cfa).unwrap();
        // GBRG: (0,0) = G2、(1,0) = B、(0,1) = R、(1,1) = G1。
        assert_eq!(b, [3.0, 4.0, 2.0, 1.0]);
        // 2 × 2 の周期で表せないパターン。
        c.cblack[6..14].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(matches!(
            black_levels(&c, idx, cfa),
            Err(RawError::Unsupported(_))
        ));
        // 3 × 1 のパターンは 2 × 2 の周期にならない（値が違えば）。
        c.cblack[4] = 3;
        c.cblack[5] = 1;
        c.cblack[6..9].copy_from_slice(&[5, 5, 6]);
        assert!(black_levels(&c, idx, cfa).is_err());
        c.cblack[6..9].copy_from_slice(&[5, 5, 5]);
        assert_eq!(black_levels(&c, idx, cfa).unwrap(), [5.0; 4]);
        // 配列に収まらない大きさ。
        c.cblack[4] = 100;
        c.cblack[5] = 100;
        assert!(black_levels(&c, idx, cfa).is_err());
    }

    #[test]
    fn black_level_rejects_bad_color_index() {
        let c = color();
        assert!(black_levels(&c, [[0, 4], [1, 2]], CfaPattern::RGGB).is_err());
        assert!(black_levels(&c, [[0, -1], [1, 2]], CfaPattern::RGGB).is_err());
    }

    #[test]
    fn white_balance_uses_cam_mul_then_pre_mul() {
        let mut c = color();
        let idx = [[0, 1], [3, 2]];
        let (wb, src) = as_shot_wb(&c, idx, CfaPattern::RGGB);
        assert_eq!(src, WbSource::AsShot);
        assert_eq!(wb, [2400.0 / 1024.0, 1.0, 1600.0 / 1024.0, 1.0]);
        // cam_mul[3] == 0 は G と同じ。
        c.cam_mul = [2.0, 1.0, 1.5, 0.0];
        assert_eq!(
            as_shot_wb(&c, idx, CfaPattern::RGGB).0,
            [2.0, 1.0, 1.5, 1.0]
        );
        // 使えない cam_mul → pre_mul（昼光）。
        c.cam_mul = [0.0, 0.0, 0.0, 0.0];
        let (wb, src) = as_shot_wb(&c, idx, CfaPattern::RGGB);
        assert_eq!(src, WbSource::Daylight);
        assert_eq!(wb, [2.0, 1.0, 1.5, 1.0]);
        // どちらもない → 1。
        c.pre_mul = [f32::NAN, 1.0, 1.0, 1.0];
        assert_eq!(
            as_shot_wb(&c, idx, CfaPattern::RGGB),
            ([1.0; 4], WbSource::Unity)
        );
    }

    #[test]
    fn white_balance_greens_follow_position() {
        // LibRaw の番号 3 の緑が R の行にある場合、G1 は cam_mul[3] になる。
        let mut c = color();
        c.cam_mul = [2.0, 1.0, 1.5, 1.1];
        let idx = [[0, 3], [1, 2]];
        let (wb, _) = as_shot_wb(&c, idx, CfaPattern::RGGB);
        assert_eq!(wb[1], 1.0);
        assert!((wb[0] - 2.0 / 1.1).abs() < 1e-6);
        assert!((wb[3] - 1.0 / 1.1).abs() < 1e-6);
    }

    #[test]
    fn cam_xyz_prefers_dng_d65_matrix() {
        let mut c = color();
        let d65 = [[1.0, 0.1, 0.0], [0.0, 1.0, 0.2], [0.1, 0.0, 1.0], [0.0; 3]];
        let std_a = [[0.5, 0.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.0, 0.5], [0.0; 3]];
        c.dng_color[0] = DngColorSnapshot {
            illuminant: 17,
            color_matrix: std_a,
            calibration: [[0.0; 4]; 4],
        };
        c.dng_color[1] = DngColorSnapshot {
            illuminant: ILLUMINANT_D65,
            color_matrix: d65,
            calibration: [[0.0; 4]; 4],
        };
        let (m, src) = select_cam_xyz(true, &c);
        assert_eq!(m.unwrap(), upper3(&d65));
        assert_eq!(
            src,
            CamXyzSource::DngColorMatrix {
                index: 2,
                illuminant: 21
            }
        );
        // DNG でなければ LibRaw の表。
        let (m, src) = select_cam_xyz(false, &c);
        assert_eq!(m.unwrap(), upper3(&c.cam_xyz));
        assert_eq!(src, CamXyzSource::LibRawTable);
        // D65 がない DNG は LibRaw の表、表もなければ None。
        c.dng_color[1].illuminant = 23;
        assert_eq!(select_cam_xyz(true, &c).1, CamXyzSource::LibRawTable);
        c.cam_xyz = [[0.0; 3]; 4];
        assert_eq!(select_cam_xyz(true, &c), (None, CamXyzSource::None));
    }

    #[test]
    fn dng_matrix_applies_calibration_and_analog_balance() {
        let cm = [[1.0, 2.0, 3.0], [0.0, 1.0, 4.0], [5.0, 6.0, 0.0], [0.0; 3]];
        let mut cal = [[0.0f32; 4]; 4];
        cal[0][0] = 2.0;
        cal[1][1] = 1.0;
        cal[2][2] = 0.5;
        let dc = DngColorSnapshot {
            illuminant: ILLUMINANT_D65,
            color_matrix: cm,
            calibration: cal,
        };
        let m = dng_xyz_to_camera(&dc, [1.0, 3.0, 1.0, 0.0]);
        assert_eq!(m, [[2.0, 4.0, 6.0], [0.0, 3.0, 12.0], [2.5, 3.0, 0.0]]);
        // 記録のない CameraCalibration（すべて 0）と AnalogBalance は単位行列と 1。
        let dc0 = DngColorSnapshot {
            calibration: [[0.0; 4]; 4],
            ..dc
        };
        assert_eq!(dng_xyz_to_camera(&dc0, [0.0; 4]), upper3(&cm));
    }

    #[test]
    fn singular_or_zero_matrices_are_dropped() {
        let mut c = color();
        c.cam_xyz = [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 0.0], [0.0; 3]];
        assert_eq!(select_cam_xyz(false, &c), (None, CamXyzSource::None));
    }

    #[test]
    fn flip_maps_back_to_exif_orientation() {
        // dcraw の "50132467"[o & 7] の逆の対応。
        let table = b"50132467";
        for exif in 1..=8u16 {
            let flip = i32::from(table[usize::from(exif & 7)] - b'0');
            assert_eq!(
                orientation_from_flip(flip).to_exif(),
                exif as u8,
                "flip {flip}"
            );
        }
        assert_eq!(orientation_from_flip(-1), Orientation::Normal);
        assert_eq!(orientation_from_flip(99), Orientation::Normal);
    }

    #[test]
    fn gps_conversion() {
        // 東京駅付近: 35°40'52.2"N 139°46'0.6"E
        let g = GpsSnapshot {
            parsed: true,
            latitude: [35.0, 40.0, 52.2],
            longitude: [139.0, 46.0, 0.6],
            latitude_ref: b'N',
            longitude_ref: b'E',
        };
        let c = gps_from_snapshot(&g).unwrap();
        assert!((c.lat - (35.0 + 40.0 / 60.0 + 52.2 / 3600.0)).abs() < 1e-6);
        assert!((c.lon - (139.0 + 46.0 / 60.0 + 0.6 / 3600.0)).abs() < 1e-6);
        let south_west = GpsSnapshot {
            latitude_ref: b's',
            longitude_ref: b'W',
            ..g
        };
        let c = gps_from_snapshot(&south_west).unwrap();
        assert!(c.lat < 0.0 && c.lon < 0.0);
        // 方角の記号がない・解析されていない・範囲外は None。
        assert_eq!(
            gps_from_snapshot(&GpsSnapshot {
                latitude_ref: 0,
                ..g
            }),
            None
        );
        assert_eq!(gps_from_snapshot(&GpsSnapshot { parsed: false, ..g }), None);
        assert_eq!(
            gps_from_snapshot(&GpsSnapshot {
                latitude: [95.0, 0.0, 0.0],
                ..g
            }),
            None
        );
        assert_eq!(
            gps_from_snapshot(&GpsSnapshot {
                longitude: [-1.0, 0.0, 0.0],
                ..g
            }),
            None
        );
    }

    #[test]
    fn capture_info_prefers_exif_strings() {
        let m = MetaSnapshot {
            datetime_original: Some("2024:05:01 12:34:56".to_owned()),
            subsec_time_original: Some("123".to_owned()),
            offset_time_original: Some("+09:00".to_owned()),
            timestamp: 1_714_534_496,
            timestamp_local: Some("2024:05:01 03:34:56".to_owned()),
            ..Default::default()
        };
        let (info, src) = capture_info(&m);
        assert_eq!(src, CaptureTimeSource::ExifDateTimeOriginal);
        assert_eq!(info.datetime.as_deref(), Some("2024:05:01 12:34:56.123"));
        assert_eq!(info.offset.as_deref(), Some("+09:00"));
        // genzo-model の解析で読める形であること。
        let (local, _) =
            genzo_model::capture_time::parse_datetime(info.datetime.as_deref().unwrap())
                .unwrap()
                .unwrap();
        assert_eq!(local.and_utc().timestamp_subsec_millis(), 123);

        // 数字でない小数秒は付けない。
        let m2 = MetaSnapshot {
            subsec_time_original: Some("  ".to_owned()),
            ..m.clone()
        };
        assert_eq!(
            capture_info(&m2).0.datetime.as_deref(),
            Some("2024:05:01 12:34:56")
        );

        // DateTimeOriginal が「不明」の値なら timestamp を使う。
        let m3 = MetaSnapshot {
            datetime_original: Some("0000:00:00 00:00:00".to_owned()),
            ..m.clone()
        };
        let (info, src) = capture_info(&m3);
        assert_eq!(src, CaptureTimeSource::LibRawTimestamp);
        assert_eq!(info.datetime.as_deref(), Some("2024:05:01 03:34:56"));
        assert_eq!(info.offset, None);

        let m4 = MetaSnapshot::default();
        assert_eq!(
            capture_info(&m4),
            (CaptureInfo::default(), CaptureTimeSource::None)
        );
    }

    #[test]
    fn photo_metadata_fields() {
        let m = MetaSnapshot {
            make: Some("Sony".to_owned()),
            model: Some("ILCE-7M4".to_owned()),
            exif_make: Some("SONY".to_owned()),
            lens: None,
            makernotes_lens: Some("FE 24-70mm F2.8 GM II".to_owned()),
            iso_speed: 99.6,
            shutter: 0.004,
            aperture: 2.8,
            focal_len: 0.0,
            flip: 6,
            width: 7028,
            height: 4688,
            ..Default::default()
        };
        let (p, _) = photo_metadata(&m);
        assert_eq!(p.make.as_deref(), Some("SONY"));
        assert_eq!(p.model.as_deref(), Some("ILCE-7M4"));
        assert_eq!(p.lens.as_deref(), Some("FE 24-70mm F2.8 GM II"));
        assert_eq!(p.iso, Some(100));
        assert_eq!(p.shutter_s, Some(0.004));
        assert_eq!(p.aperture, Some(2.8));
        assert_eq!(p.focal_mm, None);
        assert_eq!(p.orientation, Orientation::Rotate90Cw);
        assert_eq!(p.display_size(), Some((4688, 7028)));
        let (p, _) = photo_metadata(&MetaSnapshot::default());
        assert_eq!(p, PhotoMetadata::default());
    }

    #[test]
    fn build_decoded_validates_and_records_details() {
        let s = sizes(colors_from_filters(0x9494_9494));
        let c = color();
        let m = MetaSnapshot::default();
        let data = vec![600u16; 100 * 60];
        let d = build_decoded(&s, &c, &m, data, DecoderInfo::default()).unwrap();
        assert_eq!(d.image.cfa, CfaPattern::RGGB);
        assert_eq!(d.image.black_level, [512.0; 4]);
        assert_eq!(d.image.white_level, 16383.0);
        assert_eq!(d.details.raw_width, 120);
        assert_eq!(d.details.left_margin, 20);
        assert_eq!(d.details.cam_xyz_source, CamXyzSource::LibRawTable);
        assert!(d.details.dng_color_matrices.is_empty());
        // 長さが合わない。
        let r = build_decoded(&s, &c, &m, vec![0; 10], DecoderInfo::default());
        assert!(matches!(r, Err(RawError::DataLengthMismatch { .. })));
        // 白レベルが 0。
        let mut c0 = c.clone();
        c0.maximum = 0;
        let r = build_decoded(&s, &c0, &m, vec![0; 6000], DecoderInfo::default());
        assert!(matches!(r, Err(RawError::InvalidWhiteLevel(_))));
        // 黒レベルが白レベル以上。
        let mut c1 = c;
        c1.black = 20_000;
        let r = build_decoded(&s, &c1, &m, vec![0; 6000], DecoderInfo::default());
        assert!(matches!(r, Err(RawError::InvalidBlackLevel { .. })));
    }

    #[test]
    fn thumbnails_from_libraw_values() {
        let rgb = thumbnail_from_libraw(libraw_thumb::BITMAP, 2, 2, 3, vec![1; 12]).unwrap();
        assert_eq!(rgb.format, ThumbnailFormat::Rgb8);
        let gray = thumbnail_from_libraw(libraw_thumb::BITMAP, 2, 2, 1, vec![1; 4]).unwrap();
        assert_eq!(gray.format, ThumbnailFormat::Gray8);
        assert!(thumbnail_from_libraw(libraw_thumb::BITMAP, 2, 2, 4, vec![1; 16]).is_err());
        assert!(thumbnail_from_libraw(libraw_thumb::BITMAP, 2, 2, 3, vec![1; 11]).is_err());
        assert!(matches!(
            thumbnail_from_libraw(0, 2, 2, 3, vec![1; 12]),
            Err(RawError::Unsupported(_))
        ));
        assert!(matches!(
            thumbnail_from_libraw(6, 2, 2, 3, vec![1; 12]),
            Err(RawError::Unsupported(_))
        ));
        // JPEG の寸法は SOF から読めなければ LibRaw の値を使う。SOI がなければエラー。
        let j = thumbnail_from_libraw(libraw_thumb::JPEG, 16, 8, 3, vec![0xFF, 0xD8, 0xFF, 0xD9])
            .unwrap();
        assert_eq!((j.width, j.height), (16, 8));
        assert!(thumbnail_from_libraw(libraw_thumb::JPEG, 16, 8, 3, vec![0, 0]).is_err());
    }
}
