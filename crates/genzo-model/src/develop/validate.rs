//! 現像設定の検証（`validate`）と丸め（`normalized`）。
//!
//! - `validate` は、範囲外・NaN・無限大・不正な形（点が 1 つだけのカーブなど）を、
//!   どの項目か（`"tone.highlights"`、`"tone_curve.luma[2].x"` など）とともにすべて報告する。
//! - `normalized` は、同じ規則で値を有効な範囲に丸めた設定を返す。パイプラインと
//!   ハッシュ（`develop_hash`）は丸めた設定を使う。

use std::fmt;

use super::DevelopSettings;
use super::params::{
    AspectRatio, ColorParams, CropRect, CurvePoint, DataRef, DetailParams, Geometry,
    LensCorrection, LocalAdjustment, LocalDelta, MAX_ASPECT_TERM, MAX_CURVE_POINTS, MAX_MASKS,
    MIN_CROP_SIZE, MaskShape, ParametricCurve, RenderDeps, ToneCurve, ToneParams, ValueRange,
    WhiteBalance, ranges,
};
use crate::{CURRENT_PROCESS_VERSION, CURRENT_SCHEMA_VERSION};

/// 1 つの項目の問題の種類。
#[derive(Debug, Clone, PartialEq)]
pub enum IssueKind {
    /// NaN または無限大。
    NotFinite,
    /// 範囲外。
    OutOfRange {
        /// 値。
        value: f64,
        /// 最小値。
        min: f64,
        /// 最大値。
        max: f64,
    },
    /// その他の不正な形。
    Invalid(String),
}

/// 1 つの項目の問題。
#[derive(Debug, Clone, PartialEq)]
pub struct FieldIssue {
    /// 項目の位置（例: `"tone.highlights"`、`"tone_curve.luma[2].x"`）。
    pub field: String,
    /// 問題の種類。
    pub kind: IssueKind,
}

impl fmt::Display for FieldIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            IssueKind::NotFinite => write!(f, "{}: 有限の数値ではありません", self.field),
            IssueKind::OutOfRange { value, min, max } => {
                write!(f, "{}: {value} は範囲外です（{min}〜{max}）", self.field)
            }
            IssueKind::Invalid(reason) => write!(f, "{}: {reason}", self.field),
        }
    }
}

/// 現像設定の検証エラー。見つかった問題をすべて持つ。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub struct ValidationError {
    /// 見つかった問題（1 件以上）。
    pub issues: Vec<FieldIssue>,
}

impl ValidationError {
    /// 問題のあった項目の位置を返す。
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        self.issues.iter().map(|i| i.field.as_str())
    }

    /// `field` に問題があったか。
    pub fn has_field(&self, field: &str) -> bool {
        self.fields().any(|f| f == field)
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("現像設定の値が不正です: ")?;
        for (i, issue) in self.issues.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

/// 問題を集める。
#[derive(Default)]
struct Checker {
    issues: Vec<FieldIssue>,
}

impl Checker {
    fn push(&mut self, field: impl Into<String>, kind: IssueKind) {
        self.issues.push(FieldIssue {
            field: field.into(),
            kind,
        });
    }

    fn invalid(&mut self, field: impl Into<String>, reason: impl Into<String>) {
        self.push(field, IssueKind::Invalid(reason.into()));
    }

    fn range(&mut self, field: impl Into<String>, value: f32, range: ValueRange) {
        if !value.is_finite() {
            self.push(field, IssueKind::NotFinite);
        } else if !range.contains(value) {
            self.push(
                field,
                IssueKind::OutOfRange {
                    value: f64::from(value),
                    min: f64::from(range.min),
                    max: f64::from(range.max),
                },
            );
        }
    }

    fn range_u32(&mut self, field: impl Into<String>, value: u32, min: u32, max: u32) {
        if value < min || value > max {
            self.push(
                field,
                IssueKind::OutOfRange {
                    value: f64::from(value),
                    min: f64::from(min),
                    max: f64::from(max),
                },
            );
        }
    }
}

/// 0 を既定値とする値を丸める。
fn clamp0(value: &mut f32, range: ValueRange) {
    *value = range.clamp_or(*value, 0.0);
}

impl DevelopSettings {
    /// 値を検証する。範囲外・NaN・無限大・不正な形があれば、すべての問題を返す。
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut c = Checker::default();
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            c.invalid(
                "schema_version",
                format!(
                    "{} です（メモリ上の設定は {CURRENT_SCHEMA_VERSION} に移行済みである必要があります）",
                    self.schema_version
                ),
            );
        }
        c.range_u32(
            "process_version",
            self.process_version,
            1,
            CURRENT_PROCESS_VERSION,
        );
        check_white_balance(&self.white_balance, &mut c);
        c.range("exposure_ev", self.exposure_ev, ranges::EXPOSURE_EV);
        c.range("contrast", self.contrast, ranges::SLIDER);
        self.tone.check(&mut c);
        self.tone_curve.check(&mut c);
        self.color.check(&mut c);
        self.geometry.check(&mut c);
        self.lens.check(&mut c);
        self.detail.check(&mut c);
        if self.masks.len() > MAX_MASKS {
            c.invalid(
                "masks",
                format!(
                    "マスクが多すぎます（{} 個。上限 {MAX_MASKS}）",
                    self.masks.len()
                ),
            );
        }
        for (i, m) in self.masks.iter().enumerate() {
            m.check(&format!("masks[{i}]"), &mut c);
        }
        self.render_deps.check(&mut c);
        if c.issues.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { issues: c.issues })
        }
    }

    /// 値を有効な範囲に丸めた設定を返す。
    ///
    /// - 範囲外の値は範囲の端に、NaN は既定値に、無限大は範囲の端に置き換える。-0.0 は 0.0 にする。
    /// - `schema_version` は現在の値にし、`process_version` は 1〜現在の値に丸める。
    /// - トーンカーブの点は、有限でない点を除き、座標を 0〜1 に丸めて x の昇順に並べ、
    ///   同じ x の点は後のものを残す。上限を超えた点は捨て、1 点だけ残ったカーブは空（恒等）にする。
    /// - 切り抜きは、各軸で左右（上下）が逆なら入れ替え、幅（高さ）が最小値より小さければ
    ///   その軸を全体に戻す。
    /// - 縦横比の項が 0 や上限を超える場合は、指定なし（自由）にする。
    /// - マスクは上限を超えた分を捨てる。
    /// - SHA-256 の 16 進数は小文字にする。
    ///
    /// 丸めた結果は必ず [`validate`](Self::validate) を通る（render_deps の SHA-256 の形が
    /// 不正な場合を除く）。
    pub fn normalized(&self) -> DevelopSettings {
        let mut s = self.clone();
        s.schema_version = CURRENT_SCHEMA_VERSION;
        s.process_version = s.process_version.clamp(1, CURRENT_PROCESS_VERSION);
        normalize_white_balance(&mut s.white_balance);
        clamp0(&mut s.exposure_ev, ranges::EXPOSURE_EV);
        clamp0(&mut s.contrast, ranges::SLIDER);
        s.tone.normalize();
        s.tone_curve.normalize();
        s.color.normalize();
        s.geometry.normalize();
        s.lens.normalize();
        s.detail.normalize();
        s.masks.truncate(MAX_MASKS);
        for m in &mut s.masks {
            m.normalize();
        }
        s.render_deps.normalize();
        s
    }
}

fn check_white_balance(wb: &WhiteBalance, c: &mut Checker) {
    if let WhiteBalance::Custom {
        temperature_k,
        tint,
    } = *wb
    {
        c.range(
            "white_balance.custom.temperature_k",
            temperature_k,
            ranges::TEMPERATURE_K,
        );
        c.range("white_balance.custom.tint", tint, ranges::TINT);
    }
}

fn normalize_white_balance(wb: &mut WhiteBalance) {
    if let WhiteBalance::Custom {
        temperature_k,
        tint,
    } = wb
    {
        // 色温度の NaN は、晴天に相当する値に置き換える。
        *temperature_k = ranges::TEMPERATURE_K.clamp_or(*temperature_k, 5500.0);
        clamp0(tint, ranges::TINT);
    }
}

impl ToneParams {
    fn check(&self, c: &mut Checker) {
        c.range("tone.highlights", self.highlights, ranges::SLIDER);
        c.range("tone.shadows", self.shadows, ranges::SLIDER);
        c.range("tone.whites", self.whites, ranges::SLIDER);
        c.range("tone.blacks", self.blacks, ranges::SLIDER);
    }

    fn normalize(&mut self) {
        for v in [
            &mut self.highlights,
            &mut self.shadows,
            &mut self.whites,
            &mut self.blacks,
        ] {
            clamp0(v, ranges::SLIDER);
        }
    }
}

impl ParametricCurve {
    fn check(&self, c: &mut Checker) {
        let p = "tone_curve.parametric";
        c.range(format!("{p}.highlights"), self.highlights, ranges::SLIDER);
        c.range(format!("{p}.lights"), self.lights, ranges::SLIDER);
        c.range(format!("{p}.darks"), self.darks, ranges::SLIDER);
        c.range(format!("{p}.shadows"), self.shadows, ranges::SLIDER);
    }

    fn normalize(&mut self) {
        for v in [
            &mut self.highlights,
            &mut self.lights,
            &mut self.darks,
            &mut self.shadows,
        ] {
            clamp0(v, ranges::SLIDER);
        }
    }
}

impl ToneCurve {
    fn check(&self, c: &mut Checker) {
        self.parametric.check(c);
        for (name, points) in self.point_curves() {
            check_points(&format!("tone_curve.{name}"), points, c);
        }
    }

    fn normalize(&mut self) {
        self.parametric.normalize();
        for points in self.point_curves_mut() {
            normalize_points(points);
        }
    }
}

fn check_points(path: &str, points: &[CurvePoint], c: &mut Checker) {
    if points.len() == 1 {
        c.invalid(
            path,
            "点が 1 つだけのカーブは使えません（空にするか、2 点以上にしてください）",
        );
    }
    if points.len() > MAX_CURVE_POINTS {
        c.invalid(
            path,
            format!(
                "点が多すぎます（{} 点。上限 {MAX_CURVE_POINTS}）",
                points.len()
            ),
        );
    }
    for (i, p) in points.iter().enumerate() {
        c.range(format!("{path}[{i}].x"), p.x, ranges::CURVE_COORD);
        c.range(format!("{path}[{i}].y"), p.y, ranges::CURVE_COORD);
    }
    for (i, w) in points.windows(2).enumerate() {
        // NaN を含む組は上の範囲の検査で報告済み。
        if w[0].x.is_finite() && w[1].x.is_finite() && w[1].x <= w[0].x {
            c.invalid(
                format!("{path}[{}].x", i + 1),
                "x は前の点より大きくなければなりません（x の昇順・重複なし）",
            );
        }
    }
}

fn normalize_points(points: &mut Vec<CurvePoint>) {
    points.retain(|p| p.x.is_finite() && p.y.is_finite());
    for p in points.iter_mut() {
        p.x = ranges::CURVE_COORD.clamp_or(p.x, 0.0);
        p.y = ranges::CURVE_COORD.clamp_or(p.y, 0.0);
    }
    // 安定ソートなので、同じ x の点は元の順を保つ。重複は後の点を残す。
    points.sort_by(|a, b| a.x.total_cmp(&b.x));
    let mut deduped: Vec<CurvePoint> = Vec::with_capacity(points.len());
    for p in points.drain(..) {
        match deduped.last_mut() {
            Some(last) if last.x == p.x => *last = p,
            _ => deduped.push(p),
        }
    }
    deduped.truncate(MAX_CURVE_POINTS);
    if deduped.len() == 1 {
        deduped.clear();
    }
    *points = deduped;
}

impl ColorParams {
    fn check(&self, c: &mut Checker) {
        c.range("color.saturation", self.saturation, ranges::SLIDER);
        c.range("color.vibrance", self.vibrance, ranges::SLIDER);
    }

    fn normalize(&mut self) {
        clamp0(&mut self.saturation, ranges::SLIDER);
        clamp0(&mut self.vibrance, ranges::SLIDER);
    }
}

impl Geometry {
    fn check(&self, c: &mut Checker) {
        self.crop.check(c);
        c.range("geometry.angle_deg", self.angle_deg, ranges::ANGLE_DEG);
        if let Some(AspectRatio::Ratio { w, h }) = self.aspect {
            c.range_u32("geometry.aspect.ratio.w", w, 1, MAX_ASPECT_TERM);
            c.range_u32("geometry.aspect.ratio.h", h, 1, MAX_ASPECT_TERM);
        }
    }

    fn normalize(&mut self) {
        self.crop.normalize();
        clamp0(&mut self.angle_deg, ranges::ANGLE_DEG);
        if let Some(AspectRatio::Ratio { w, h }) = self.aspect {
            let ok = |v: u32| (1..=MAX_ASPECT_TERM).contains(&v);
            if !ok(w) || !ok(h) {
                self.aspect = None;
            }
        }
    }
}

impl CropRect {
    fn check(&self, c: &mut Checker) {
        let r = ranges::CROP_COORD;
        c.range("geometry.crop.left", self.left, r);
        c.range("geometry.crop.top", self.top, r);
        c.range("geometry.crop.right", self.right, r);
        c.range("geometry.crop.bottom", self.bottom, r);
        let all_finite = [self.left, self.top, self.right, self.bottom]
            .iter()
            .all(|v| v.is_finite());
        if all_finite {
            if self.width() < MIN_CROP_SIZE {
                c.invalid(
                    "geometry.crop",
                    format!("幅が {MIN_CROP_SIZE} より小さいか、left と right が逆です"),
                );
            }
            if self.height() < MIN_CROP_SIZE {
                c.invalid(
                    "geometry.crop",
                    format!("高さが {MIN_CROP_SIZE} より小さいか、top と bottom が逆です"),
                );
            }
        }
    }

    fn normalize(&mut self) {
        let (left, right) = normalize_crop_axis(self.left, self.right);
        let (top, bottom) = normalize_crop_axis(self.top, self.bottom);
        *self = CropRect {
            left,
            top,
            right,
            bottom,
        };
    }
}

/// 切り抜きの 1 つの軸を丸める。
fn normalize_crop_axis(start: f32, end: f32) -> (f32, f32) {
    let r = ranges::CROP_COORD;
    let a = r.clamp_or(start, 0.0);
    let b = r.clamp_or(end, 1.0);
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    if b - a < MIN_CROP_SIZE {
        (0.0, 1.0)
    } else {
        (a, b)
    }
}

impl LensCorrection {
    fn check(&self, c: &mut Checker) {
        c.range(
            "lens.distortion_scale",
            self.distortion_scale,
            ranges::LENS_SCALE,
        );
        c.range(
            "lens.vignetting_scale",
            self.vignetting_scale,
            ranges::LENS_SCALE,
        );
    }

    fn normalize(&mut self) {
        let d = LensCorrection::default();
        self.distortion_scale =
            ranges::LENS_SCALE.clamp_or(self.distortion_scale, d.distortion_scale);
        self.vignetting_scale =
            ranges::LENS_SCALE.clamp_or(self.vignetting_scale, d.vignetting_scale);
    }
}

impl DetailParams {
    fn check(&self, c: &mut Checker) {
        let s = &self.sharpening;
        c.range("detail.sharpening.amount", s.amount, ranges::SHARPEN_AMOUNT);
        c.range("detail.sharpening.radius", s.radius, ranges::SHARPEN_RADIUS);
        c.range("detail.sharpening.detail", s.detail, ranges::AMOUNT_0_100);
        c.range("detail.sharpening.masking", s.masking, ranges::AMOUNT_0_100);
        let n = &self.noise_reduction;
        c.range(
            "detail.noise_reduction.luminance",
            n.luminance,
            ranges::AMOUNT_0_100,
        );
        c.range(
            "detail.noise_reduction.color",
            n.color,
            ranges::AMOUNT_0_100,
        );
    }

    fn normalize(&mut self) {
        let s = &mut self.sharpening;
        clamp0(&mut s.amount, ranges::SHARPEN_AMOUNT);
        s.radius = ranges::SHARPEN_RADIUS.clamp_or(s.radius, 1.0);
        clamp0(&mut s.detail, ranges::AMOUNT_0_100);
        clamp0(&mut s.masking, ranges::AMOUNT_0_100);
        let n = &mut self.noise_reduction;
        clamp0(&mut n.luminance, ranges::AMOUNT_0_100);
        clamp0(&mut n.color, ranges::AMOUNT_0_100);
    }
}

impl LocalAdjustment {
    fn check(&self, path: &str, c: &mut Checker) {
        match self.mask {
            MaskShape::Linear { x0, y0, x1, y1 } => {
                let p = format!("{path}.mask.linear");
                c.range(format!("{p}.x0"), x0, ranges::MASK_COORD);
                c.range(format!("{p}.y0"), y0, ranges::MASK_COORD);
                c.range(format!("{p}.x1"), x1, ranges::MASK_COORD);
                c.range(format!("{p}.y1"), y1, ranges::MASK_COORD);
            }
            MaskShape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle_deg,
                feather,
                invert: _,
            } => {
                let p = format!("{path}.mask.radial");
                c.range(format!("{p}.cx"), cx, ranges::MASK_COORD);
                c.range(format!("{p}.cy"), cy, ranges::MASK_COORD);
                c.range(format!("{p}.rx"), rx, ranges::MASK_RADIUS);
                c.range(format!("{p}.ry"), ry, ranges::MASK_RADIUS);
                c.range(format!("{p}.angle_deg"), angle_deg, ranges::MASK_ANGLE_DEG);
                c.range(format!("{p}.feather"), feather, ranges::AMOUNT_0_100);
            }
        }
        self.delta.check(&format!("{path}.delta"), c);
    }

    fn normalize(&mut self) {
        match &mut self.mask {
            MaskShape::Linear { x0, y0, x1, y1 } => {
                for v in [x0, y0, x1, y1] {
                    clamp0(v, ranges::MASK_COORD);
                }
            }
            MaskShape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle_deg,
                feather,
                invert: _,
            } => {
                clamp0(cx, ranges::MASK_COORD);
                clamp0(cy, ranges::MASK_COORD);
                clamp0(rx, ranges::MASK_RADIUS);
                clamp0(ry, ranges::MASK_RADIUS);
                clamp0(angle_deg, ranges::MASK_ANGLE_DEG);
                clamp0(feather, ranges::AMOUNT_0_100);
            }
        }
        self.delta.normalize();
    }
}

impl LocalDelta {
    fn check(&self, path: &str, c: &mut Checker) {
        c.range(
            format!("{path}.exposure_ev"),
            self.exposure_ev,
            ranges::EXPOSURE_EV,
        );
        for (name, v) in [
            ("contrast", self.contrast),
            ("highlights", self.highlights),
            ("shadows", self.shadows),
            ("whites", self.whites),
            ("blacks", self.blacks),
            ("saturation", self.saturation),
        ] {
            c.range(format!("{path}.{name}"), v, ranges::SLIDER);
        }
    }

    fn normalize(&mut self) {
        clamp0(&mut self.exposure_ev, ranges::EXPOSURE_EV);
        for v in [
            &mut self.contrast,
            &mut self.highlights,
            &mut self.shadows,
            &mut self.whites,
            &mut self.blacks,
            &mut self.saturation,
        ] {
            clamp0(v, ranges::SLIDER);
        }
    }
}

impl RenderDeps {
    fn check(&self, c: &mut Checker) {
        self.camera_profile
            .check("render_deps.camera_profile.sha256", c);
        if let Some(lens) = &self.lens_profile {
            lens.check("render_deps.lens_profile.sha256", c);
        }
    }

    pub(crate) fn normalize(&mut self) {
        self.camera_profile.normalize();
        if let Some(lens) = &mut self.lens_profile {
            lens.normalize();
        }
    }
}

impl DataRef {
    fn check(&self, field: &str, c: &mut Checker) {
        if !is_sha256_hex_or_empty(&self.sha256) {
            c.invalid(
                field,
                "SHA-256 は小文字の 16 進数 64 文字（未定なら空文字列）で指定してください",
            );
        }
    }

    pub(crate) fn normalize(&mut self) {
        self.sha256.make_ascii_lowercase();
    }
}

/// 空文字列、または小文字の 16 進数 64 文字か。
fn is_sha256_hex_or_empty(s: &str) -> bool {
    s.is_empty()
        || (s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::params::{AspectRatio, CurvePoint, WbPreset};

    fn issue_fields(s: &DevelopSettings) -> Vec<String> {
        s.validate()
            .unwrap_err()
            .issues
            .into_iter()
            .map(|i| i.field)
            .collect()
    }

    #[test]
    fn default_settings_are_valid() {
        DevelopSettings::default().validate().unwrap();
    }

    #[test]
    fn out_of_range_and_nan_are_reported_with_field_names() {
        let mut s = DevelopSettings {
            exposure_ev: 6.0,
            contrast: f32::NAN,
            ..Default::default()
        };
        s.tone.highlights = -101.0;
        s.color.vibrance = f32::INFINITY;
        s.geometry.angle_deg = 46.0;
        let err = s.validate().unwrap_err();
        assert_eq!(
            err.fields().collect::<Vec<_>>(),
            [
                "exposure_ev",
                "contrast",
                "tone.highlights",
                "color.vibrance",
                "geometry.angle_deg"
            ]
        );
        assert_eq!(err.issues[1].kind, IssueKind::NotFinite);
        assert_eq!(
            err.issues[0].kind,
            IssueKind::OutOfRange {
                value: 6.0,
                min: -5.0,
                max: 5.0
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("exposure_ev"), "{msg}");
        assert!(msg.contains("tone.highlights"), "{msg}");
    }

    #[test]
    fn boundary_values_are_valid() {
        let mut s = DevelopSettings {
            exposure_ev: -5.0,
            contrast: 100.0,
            white_balance: WhiteBalance::Custom {
                temperature_k: 2000.0,
                tint: 150.0,
            },
            ..Default::default()
        };
        s.tone.blacks = -100.0;
        s.geometry.angle_deg = 45.0;
        s.geometry.crop = CropRect {
            left: 0.0,
            top: 0.0,
            right: MIN_CROP_SIZE,
            bottom: 1.0,
        };
        s.validate().unwrap();
    }

    #[test]
    fn white_balance_custom_is_checked() {
        let s = DevelopSettings {
            white_balance: WhiteBalance::Custom {
                temperature_k: 1000.0,
                tint: f32::NAN,
            },
            ..Default::default()
        };
        assert_eq!(
            issue_fields(&s),
            [
                "white_balance.custom.temperature_k",
                "white_balance.custom.tint"
            ]
        );
        let n = s.normalized();
        assert_eq!(
            n.white_balance,
            WhiteBalance::Custom {
                temperature_k: 2000.0,
                tint: 0.0
            }
        );
        n.validate().unwrap();
        // プリセットと撮影時の値には数値がない。
        let p = DevelopSettings {
            white_balance: WhiteBalance::Preset(WbPreset::Shade),
            ..Default::default()
        };
        p.validate().unwrap();
    }

    #[test]
    fn versions_are_checked() {
        let s = DevelopSettings {
            process_version: 0,
            ..Default::default()
        };
        assert_eq!(issue_fields(&s), ["process_version"]);
        let s = DevelopSettings {
            process_version: CURRENT_PROCESS_VERSION + 1,
            schema_version: 0,
            ..Default::default()
        };
        assert_eq!(issue_fields(&s), ["schema_version", "process_version"]);
        let n = s.normalized();
        assert_eq!(n.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(n.process_version, CURRENT_PROCESS_VERSION);
    }

    #[test]
    fn curve_points_are_checked() {
        let mut s = DevelopSettings::default();
        s.tone_curve.luma = vec![CurvePoint::new(0.5, 0.5)];
        s.tone_curve.red = vec![
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.6, 1.2),
            CurvePoint::new(0.6, 0.7),
            CurvePoint::new(0.3, f32::NAN),
        ];
        assert_eq!(
            issue_fields(&s),
            [
                "tone_curve.luma",
                "tone_curve.red[1].y",
                "tone_curve.red[3].y",
                "tone_curve.red[2].x",
                "tone_curve.red[3].x",
            ]
        );
        let n = s.normalized();
        assert!(n.tone_curve.luma.is_empty());
        assert_eq!(
            n.tone_curve.red,
            vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(0.6, 0.7)]
        );
        n.validate().unwrap();
    }

    #[test]
    fn too_many_curve_points_are_truncated() {
        let mut s = DevelopSettings::default();
        s.tone_curve.blue = (0..100)
            .map(|i| {
                let v = i as f32 / 99.0;
                CurvePoint::new(v, v)
            })
            .collect();
        assert_eq!(issue_fields(&s), ["tone_curve.blue"]);
        let n = s.normalized();
        assert_eq!(n.tone_curve.blue.len(), MAX_CURVE_POINTS);
        n.validate().unwrap();
    }

    #[test]
    fn crop_is_checked_and_normalized() {
        let mut s = DevelopSettings::default();
        s.geometry.crop = CropRect {
            left: 0.8,
            top: -0.1,
            right: 0.2,
            bottom: 0.5,
        };
        let fields = issue_fields(&s);
        assert_eq!(fields, ["geometry.crop.top", "geometry.crop"]);
        let n = s.normalized();
        assert_eq!(
            n.geometry.crop,
            CropRect {
                left: 0.2,
                top: 0.0,
                right: 0.8,
                bottom: 0.5
            }
        );
        // 幅が小さすぎる軸は全体に戻す。
        s.geometry.crop = CropRect {
            left: 0.5,
            top: 0.1,
            right: 0.5,
            bottom: 0.9,
        };
        let n = s.normalized();
        assert_eq!(
            n.geometry.crop,
            CropRect {
                left: 0.0,
                top: 0.1,
                right: 1.0,
                bottom: 0.9
            }
        );
        n.validate().unwrap();
    }

    #[test]
    fn aspect_ratio_terms_are_checked() {
        let mut s = DevelopSettings::default();
        s.geometry.aspect = Some(AspectRatio::Ratio { w: 0, h: 2 });
        assert_eq!(issue_fields(&s), ["geometry.aspect.ratio.w"]);
        assert_eq!(s.normalized().geometry.aspect, None);
        s.geometry.aspect = Some(AspectRatio::Ratio { w: 16, h: 9 });
        s.validate().unwrap();
        assert_eq!(s.normalized().geometry.aspect, s.geometry.aspect);
    }

    #[test]
    fn masks_are_checked() {
        let mut s = DevelopSettings::default();
        s.masks.push(LocalAdjustment {
            enabled: true,
            mask: MaskShape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 3.0,
                ry: 0.2,
                angle_deg: 0.0,
                feather: 50.0,
                invert: false,
            },
            delta: LocalDelta {
                exposure_ev: 9.0,
                ..Default::default()
            },
        });
        assert_eq!(
            issue_fields(&s),
            ["masks[0].mask.radial.rx", "masks[0].delta.exposure_ev"]
        );
        s.normalized().validate().unwrap();
    }

    #[test]
    fn sha256_format_is_checked() {
        let mut s = DevelopSettings::default();
        s.render_deps.camera_profile = DataRef::new("sony-ilce-7m4", "abc");
        assert_eq!(issue_fields(&s), ["render_deps.camera_profile.sha256"]);
        s.render_deps.camera_profile = DataRef::new("sony-ilce-7m4", "AB".repeat(32));
        assert!(s.validate().is_err());
        let n = s.normalized();
        assert_eq!(n.render_deps.camera_profile.sha256, "ab".repeat(32));
        n.validate().unwrap();
    }

    #[test]
    fn normalized_is_idempotent_and_removes_negative_zero() {
        let mut s = DevelopSettings {
            exposure_ev: -0.0,
            ..Default::default()
        };
        s.tone.shadows = 1e9;
        let n1 = s.normalized();
        assert!(n1.exposure_ev.is_sign_positive());
        assert_eq!(n1.tone.shadows, 100.0);
        assert_eq!(n1.normalized(), n1);
    }
}
