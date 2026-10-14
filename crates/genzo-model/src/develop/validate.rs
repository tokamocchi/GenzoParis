//! 現像設定の検証（`validate`）と丸め（`normalized`）。
//!
//! - `validate` は、範囲外・NaN・無限大・不正な形（点が 1 つだけのカーブなど）を、
//!   どの項目か（`"tone.highlights"`、`"tone_curve.luma[2].x"` など）とともにすべて報告する。
//! - `normalized` は、同じ規則で値を有効な範囲に丸めた設定を返す。パイプラインと
//!   ハッシュ（`develop_hash`）は丸めた設定を使う。

use std::fmt;

use super::DevelopSettings;
use super::masks::{
    BrushStroke, LocalAdjustment, LocalDelta, MAX_BRUSH_STROKES, MAX_COLOR_SAMPLES,
    MAX_MASK_COMPONENTS, MAX_MASK_NAME_CHARS, MAX_MASKS, MAX_STROKE_POINTS, MAX_TOTAL_BRUSH_POINTS,
    MaskShape,
};
use super::params::{
    AspectRatio, BlackWhite, ColorGrading, ColorMixer, ColorParams, CropRect, CurvePoint, DataRef,
    DetailParams, EffectsParams, Geometry, GradingWheel, Grain, HighlightRecovery, HueBand,
    LensCorrection, MAX_ASPECT_TERM, MAX_CURVE_POINTS, MIN_CROP_SIZE, NoiseReduction,
    ParametricCurve, PostCropVignette, PresenceParams, ProfileSettings, RenderDeps, Sharpening,
    ToneCurve, ToneParams, ValueRange, WhiteBalance, is_valid_profile_id, ranges,
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
        self.profile.check(&mut c);
        check_white_balance(&self.white_balance, &mut c);
        self.highlight_recovery.check(&mut c);
        c.range("exposure_ev", self.exposure_ev, ranges::EXPOSURE_EV);
        c.range("contrast", self.contrast, ranges::SLIDER);
        self.tone.check(&mut c);
        self.presence.check(&mut c);
        self.tone_curve.check(&mut c);
        self.color.check(&mut c);
        self.geometry.check(&mut c);
        self.lens.check(&mut c);
        self.detail.check(&mut c);
        self.effects.check(&mut c);
        if self.masks.len() > MAX_MASKS {
            c.invalid(
                "masks",
                format!(
                    "マスクが多すぎます（{} 個。上限 {MAX_MASKS}）",
                    self.masks.len()
                ),
            );
        }
        let points: usize = self.masks.iter().map(LocalAdjustment::brush_points).sum();
        if points > MAX_TOTAL_BRUSH_POINTS {
            c.invalid(
                "masks",
                format!("ブラシの点が多すぎます（{points} 個。上限 {MAX_TOTAL_BRUSH_POINTS}）"),
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
    ///   **注意**: 未知の（新しい）処理バージョンも黙って現在の値に丸めるので、別のアルゴリズムで
    ///   描いてしまう。描画する側は、丸める前の値を確かめて未知の版をエラーにすること
    ///   （genzo-pipeline の `ProcessVersion::from_u32` がそうしている。04 の 2.5 節・7.1 節）。
    /// - トーンカーブの点は、有限でない点を除き、座標を 0〜1 に丸めて x の昇順に並べ、
    ///   同じ x の点は後のものを残す。上限を超えた点は捨て、1 点だけ残ったカーブは空（恒等）にする。
    /// - 切り抜きは、各軸で左右（上下）が逆なら入れ替え、幅（高さ）が最小値より小さければ
    ///   その軸を全体に戻す。
    /// - 縦横比の項が 0 や上限を超える場合は、指定なし（自由）にする。
    /// - マスクは上限を超えた分を捨てる。
    /// - カメラプロファイル・ルックの ID が不正（空・長すぎる・制御文字・前後の空白）なら、指定なしにする。
    /// - SHA-256 の 16 進数は小文字にする。
    ///
    /// 丸めた結果は必ず [`validate`](Self::validate) を通る（render_deps の SHA-256 の形が
    /// 不正な場合を除く）。
    pub fn normalized(&self) -> DevelopSettings {
        let mut s = self.clone();
        s.schema_version = CURRENT_SCHEMA_VERSION;
        s.process_version = s.process_version.clamp(1, CURRENT_PROCESS_VERSION);
        s.profile.normalize();
        normalize_white_balance(&mut s.white_balance);
        s.highlight_recovery.normalize();
        clamp0(&mut s.exposure_ev, ranges::EXPOSURE_EV);
        clamp0(&mut s.contrast, ranges::SLIDER);
        s.tone.normalize();
        s.presence.normalize();
        s.tone_curve.normalize();
        s.color.normalize();
        s.geometry.normalize();
        s.lens.normalize();
        s.detail.normalize();
        s.effects.normalize();
        s.masks.truncate(MAX_MASKS);
        let mut budget = MAX_TOTAL_BRUSH_POINTS;
        for m in &mut s.masks {
            m.normalize(&mut budget);
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

impl PresenceParams {
    fn check(&self, c: &mut Checker) {
        c.range("presence.texture", self.texture, ranges::SLIDER);
        c.range("presence.clarity", self.clarity, ranges::SLIDER);
        c.range("presence.dehaze", self.dehaze, ranges::SLIDER);
    }

    fn normalize(&mut self) {
        for v in [&mut self.texture, &mut self.clarity, &mut self.dehaze] {
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
        self.mixer.check(c);
        self.grading.check(c);
        self.black_white.check(c);
    }

    fn normalize(&mut self) {
        clamp0(&mut self.saturation, ranges::SLIDER);
        clamp0(&mut self.vibrance, ranges::SLIDER);
        self.mixer.normalize();
        self.grading.normalize();
        self.black_white.normalize();
    }
}

impl ColorMixer {
    fn check(&self, c: &mut Checker) {
        for band in HueBand::ALL {
            let a = self.get(band);
            let p = format!("color.mixer.{}", band.key());
            c.range(format!("{p}.hue"), a.hue, ranges::SLIDER);
            c.range(format!("{p}.saturation"), a.saturation, ranges::SLIDER);
            c.range(format!("{p}.luminance"), a.luminance, ranges::SLIDER);
        }
    }

    fn normalize(&mut self) {
        for band in HueBand::ALL {
            let a = self.get_mut(band);
            clamp0(&mut a.hue, ranges::SLIDER);
            clamp0(&mut a.saturation, ranges::SLIDER);
            clamp0(&mut a.luminance, ranges::SLIDER);
        }
    }
}

impl GradingWheel {
    fn check(&self, path: &str, c: &mut Checker) {
        c.range(format!("{path}.hue"), self.hue, ranges::HUE_DEG);
        c.range(
            format!("{path}.saturation"),
            self.saturation,
            ranges::AMOUNT_0_100,
        );
        c.range(format!("{path}.luminance"), self.luminance, ranges::SLIDER);
    }

    fn normalize(&mut self) {
        clamp0(&mut self.hue, ranges::HUE_DEG);
        clamp0(&mut self.saturation, ranges::AMOUNT_0_100);
        clamp0(&mut self.luminance, ranges::SLIDER);
    }
}

impl ColorGrading {
    fn check(&self, c: &mut Checker) {
        self.shadows.check("color.grading.shadows", c);
        self.midtones.check("color.grading.midtones", c);
        self.highlights.check("color.grading.highlights", c);
        self.global.check("color.grading.global", c);
        c.range(
            "color.grading.blending",
            self.blending,
            ranges::AMOUNT_0_100,
        );
        c.range("color.grading.balance", self.balance, ranges::SLIDER);
    }

    fn normalize(&mut self) {
        for w in [
            &mut self.shadows,
            &mut self.midtones,
            &mut self.highlights,
            &mut self.global,
        ] {
            w.normalize();
        }
        self.blending = ranges::AMOUNT_0_100.clamp_or(self.blending, Self::DEFAULT_BLENDING);
        clamp0(&mut self.balance, ranges::SLIDER);
    }
}

impl BlackWhite {
    fn check(&self, c: &mut Checker) {
        for band in HueBand::ALL {
            c.range(
                format!("color.black_white.mix.{}", band.key()),
                self.mix.get(band),
                ranges::SLIDER,
            );
        }
    }

    fn normalize(&mut self) {
        for band in HueBand::ALL {
            clamp0(self.mix.get_mut(band), ranges::SLIDER);
        }
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
        for (name, v) in [
            ("lens.distortion", self.distortion),
            ("lens.vignetting_amount", self.vignetting_amount),
            ("lens.ca_red_cyan", self.ca_red_cyan),
            ("lens.ca_blue_yellow", self.ca_blue_yellow),
        ] {
            c.range(name, v, ranges::LENS_MANUAL);
        }
        c.range(
            "lens.vignetting_midpoint",
            self.vignetting_midpoint,
            ranges::LENS_MIDPOINT,
        );
    }

    fn normalize(&mut self) {
        let d = LensCorrection::default();
        self.distortion_scale =
            ranges::LENS_SCALE.clamp_or(self.distortion_scale, d.distortion_scale);
        self.vignetting_scale =
            ranges::LENS_SCALE.clamp_or(self.vignetting_scale, d.vignetting_scale);
        for v in [
            &mut self.distortion,
            &mut self.vignetting_amount,
            &mut self.ca_red_cyan,
            &mut self.ca_blue_yellow,
        ] {
            *v = ranges::LENS_MANUAL.clamp_or(*v, 0.0);
        }
        self.vignetting_midpoint =
            ranges::LENS_MIDPOINT.clamp_or(self.vignetting_midpoint, d.vignetting_midpoint);
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
        for (name, v) in [
            ("detail.noise_reduction.luminance", n.luminance),
            (
                "detail.noise_reduction.luminance_detail",
                n.luminance_detail,
            ),
            (
                "detail.noise_reduction.luminance_contrast",
                n.luminance_contrast,
            ),
            ("detail.noise_reduction.color", n.color),
            ("detail.noise_reduction.color_detail", n.color_detail),
        ] {
            c.range(name, v, ranges::AMOUNT_0_100);
        }
    }

    fn normalize(&mut self) {
        let s = &mut self.sharpening;
        clamp0(&mut s.amount, ranges::SHARPEN_AMOUNT);
        s.radius = ranges::SHARPEN_RADIUS.clamp_or(s.radius, Sharpening::DEFAULT_RADIUS);
        s.detail = ranges::AMOUNT_0_100.clamp_or(s.detail, Sharpening::DEFAULT_DETAIL);
        clamp0(&mut s.masking, ranges::AMOUNT_0_100);
        self.noise_reduction.normalize();
    }
}

impl ProfileSettings {
    fn check(&self, c: &mut Checker) {
        for (field, id) in [
            ("profile.camera", &self.camera),
            ("profile.look", &self.look),
        ] {
            if let Some(id) = id
                && !is_valid_profile_id(id)
            {
                c.invalid(
                    field,
                    format!(
                        "ID は 1〜{} 文字で、制御文字・前後の空白を含まない文字列で指定してください",
                        super::params::MAX_PROFILE_ID_CHARS
                    ),
                );
            }
        }
        c.range(
            "profile.look_amount",
            self.look_amount,
            ranges::AMOUNT_0_100,
        );
    }

    fn normalize(&mut self) {
        for id in [&mut self.camera, &mut self.look] {
            if id.as_deref().is_some_and(|s| !is_valid_profile_id(s)) {
                *id = None;
            }
        }
        self.look_amount =
            ranges::AMOUNT_0_100.clamp_or(self.look_amount, Self::DEFAULT_LOOK_AMOUNT);
    }
}

impl EffectsParams {
    fn check(&self, c: &mut Checker) {
        let v = &self.vignette;
        c.range("effects.vignette.amount", v.amount, ranges::SLIDER);
        c.range(
            "effects.vignette.midpoint",
            v.midpoint,
            ranges::AMOUNT_0_100,
        );
        c.range("effects.vignette.roundness", v.roundness, ranges::SLIDER);
        c.range("effects.vignette.feather", v.feather, ranges::AMOUNT_0_100);
        c.range(
            "effects.vignette.highlights",
            v.highlights,
            ranges::AMOUNT_0_100,
        );
        let g = &self.grain;
        c.range("effects.grain.amount", g.amount, ranges::AMOUNT_0_100);
        c.range("effects.grain.size", g.size, ranges::AMOUNT_0_100);
        c.range("effects.grain.roughness", g.roughness, ranges::AMOUNT_0_100);
    }

    fn normalize(&mut self) {
        let r = ranges::AMOUNT_0_100;
        let v = &mut self.vignette;
        clamp0(&mut v.amount, ranges::SLIDER);
        v.midpoint = r.clamp_or(v.midpoint, PostCropVignette::DEFAULT_MIDPOINT);
        clamp0(&mut v.roundness, ranges::SLIDER);
        v.feather = r.clamp_or(v.feather, PostCropVignette::DEFAULT_FEATHER);
        clamp0(&mut v.highlights, r);
        let g = &mut self.grain;
        clamp0(&mut g.amount, r);
        g.size = r.clamp_or(g.size, Grain::DEFAULT_SIZE);
        g.roughness = r.clamp_or(g.roughness, Grain::DEFAULT_ROUGHNESS);
    }
}

impl NoiseReduction {
    fn normalize(&mut self) {
        let r = ranges::AMOUNT_0_100;
        clamp0(&mut self.luminance, r);
        self.luminance_detail = r.clamp_or(self.luminance_detail, Self::DEFAULT_DETAIL);
        clamp0(&mut self.luminance_contrast, r);
        clamp0(&mut self.color, r);
        self.color_detail = r.clamp_or(self.color_detail, Self::DEFAULT_DETAIL);
    }
}

impl HighlightRecovery {
    fn check(&self, c: &mut Checker) {
        c.range(
            "highlight_recovery.amount",
            self.amount,
            ranges::AMOUNT_0_100,
        );
    }

    fn normalize(&mut self) {
        self.amount = ranges::AMOUNT_0_100.clamp_or(self.amount, Self::DEFAULT_AMOUNT);
    }
}

impl LocalAdjustment {
    fn check(&self, path: &str, c: &mut Checker) {
        let chars = self.name.chars().count();
        if chars > MAX_MASK_NAME_CHARS {
            c.invalid(
                format!("{path}.name"),
                format!("名前が長すぎます（{chars} 文字。上限 {MAX_MASK_NAME_CHARS}）"),
            );
        }
        if self.name.chars().any(char::is_control) {
            c.invalid(format!("{path}.name"), "名前に制御文字があります");
        }
        if self.components.len() > MAX_MASK_COMPONENTS {
            c.invalid(
                format!("{path}.components"),
                format!(
                    "部品が多すぎます（{} 個。上限 {MAX_MASK_COMPONENTS}）",
                    self.components.len()
                ),
            );
        }
        for (i, comp) in self.components.iter().enumerate() {
            comp.shape
                .check(&format!("{path}.components[{i}].shape"), c);
        }
        self.delta.check(&format!("{path}.delta"), c);
    }

    /// 丸める（ブラシの点の数の残り `budget` を使い、減らす）。
    fn normalize(&mut self, budget: &mut usize) {
        if self.name.chars().count() > MAX_MASK_NAME_CHARS
            || self.name.chars().any(char::is_control)
        {
            self.name = self
                .name
                .chars()
                .filter(|ch| !ch.is_control())
                .take(MAX_MASK_NAME_CHARS)
                .collect();
        }
        self.components.truncate(MAX_MASK_COMPONENTS);
        for comp in &mut self.components {
            comp.shape.normalize(budget);
        }
        self.delta.normalize();
    }
}

impl MaskShape {
    fn check(&self, p: &str, c: &mut Checker) {
        match self {
            MaskShape::Linear { x0, y0, x1, y1 } => {
                let p = format!("{p}.linear");
                c.range(format!("{p}.x0"), *x0, ranges::MASK_COORD);
                c.range(format!("{p}.y0"), *y0, ranges::MASK_COORD);
                c.range(format!("{p}.x1"), *x1, ranges::MASK_COORD);
                c.range(format!("{p}.y1"), *y1, ranges::MASK_COORD);
            }
            MaskShape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle_deg,
                feather,
            } => {
                let p = format!("{p}.radial");
                c.range(format!("{p}.cx"), *cx, ranges::MASK_COORD);
                c.range(format!("{p}.cy"), *cy, ranges::MASK_COORD);
                c.range(format!("{p}.rx"), *rx, ranges::MASK_RADIUS);
                c.range(format!("{p}.ry"), *ry, ranges::MASK_RADIUS);
                c.range(format!("{p}.angle_deg"), *angle_deg, ranges::MASK_ANGLE_DEG);
                c.range(format!("{p}.feather"), *feather, ranges::AMOUNT_0_100);
            }
            MaskShape::Brush { strokes } => {
                let p = format!("{p}.brush");
                if strokes.len() > MAX_BRUSH_STROKES {
                    c.invalid(
                        format!("{p}.strokes"),
                        format!(
                            "ストロークが多すぎます（{} 本。上限 {MAX_BRUSH_STROKES}）",
                            strokes.len()
                        ),
                    );
                }
                for (i, s) in strokes.iter().enumerate() {
                    let q = format!("{p}.strokes[{i}]");
                    c.range(format!("{q}.size"), s.size, ranges::BRUSH_SIZE);
                    c.range(format!("{q}.feather"), s.feather, ranges::AMOUNT_0_100);
                    c.range(format!("{q}.flow"), s.flow, ranges::AMOUNT_0_100);
                    c.range(format!("{q}.density"), s.density, ranges::AMOUNT_0_100);
                    if s.points.is_empty() {
                        c.invalid(format!("{q}.points"), "点がありません");
                    }
                    if s.points.len() > MAX_STROKE_POINTS {
                        c.invalid(
                            format!("{q}.points"),
                            format!(
                                "点が多すぎます（{} 個。上限 {MAX_STROKE_POINTS}）",
                                s.points.len()
                            ),
                        );
                    }
                    // 点はたくさんあるので、問題のある最初の点だけを報告する。
                    if let Some((k, [x, y])) = s.points.iter().enumerate().find(|(_, [x, y])| {
                        !(ranges::MASK_COORD.contains(*x) && ranges::MASK_COORD.contains(*y))
                    }) {
                        c.range(format!("{q}.points[{k}][0]"), *x, ranges::MASK_COORD);
                        c.range(format!("{q}.points[{k}][1]"), *y, ranges::MASK_COORD);
                    }
                }
            }
            MaskShape::ColorRange { samples, amount } => {
                let p = format!("{p}.color_range");
                if samples.is_empty() {
                    c.invalid(format!("{p}.samples"), "見本の色がありません");
                }
                if samples.len() > MAX_COLOR_SAMPLES {
                    c.invalid(
                        format!("{p}.samples"),
                        format!(
                            "見本の色が多すぎます（{} 個。上限 {MAX_COLOR_SAMPLES}）",
                            samples.len()
                        ),
                    );
                }
                for (i, s) in samples.iter().enumerate() {
                    let q = format!("{p}.samples[{i}]");
                    c.range(format!("{q}.x"), s.x, ranges::MASK_COORD);
                    c.range(format!("{q}.y"), s.y, ranges::MASK_COORD);
                    c.range(format!("{q}.lab[0]"), s.lab[0], ranges::MASK_LAB_L);
                    c.range(format!("{q}.lab[1]"), s.lab[1], ranges::MASK_LAB_AB);
                    c.range(format!("{q}.lab[2]"), s.lab[2], ranges::MASK_LAB_AB);
                }
                c.range(format!("{p}.amount"), *amount, ranges::AMOUNT_0_100);
            }
            MaskShape::LuminanceRange {
                min,
                max,
                smoothness,
            } => {
                let p = format!("{p}.luminance_range");
                c.range(format!("{p}.min"), *min, ranges::AMOUNT_0_100);
                c.range(format!("{p}.max"), *max, ranges::AMOUNT_0_100);
                c.range(format!("{p}.smoothness"), *smoothness, ranges::AMOUNT_0_100);
                if min > max {
                    c.invalid(
                        format!("{p}.min"),
                        format!("下端 {min} が上端 {max} より大きい"),
                    );
                }
            }
        }
    }

    /// 丸める。ブラシの点は、有限でない点を除いて範囲に丸め、上限（本数・1 本の点の数・全体の点の数 `budget`）を
    /// 超えた分を除く（点のなくなったストロークも除く）。色域の範囲の見本の色は上限まで。
    fn normalize(&mut self, budget: &mut usize) {
        match self {
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
            } => {
                clamp0(cx, ranges::MASK_COORD);
                clamp0(cy, ranges::MASK_COORD);
                clamp0(rx, ranges::MASK_RADIUS);
                clamp0(ry, ranges::MASK_RADIUS);
                clamp0(angle_deg, ranges::MASK_ANGLE_DEG);
                *feather = ranges::AMOUNT_0_100.clamp_or(*feather, 50.0);
            }
            MaskShape::Brush { strokes } => {
                strokes.truncate(MAX_BRUSH_STROKES);
                for s in strokes.iter_mut() {
                    s.size = ranges::BRUSH_SIZE.clamp_or(s.size, BrushStroke::DEFAULT_SIZE);
                    s.feather =
                        ranges::AMOUNT_0_100.clamp_or(s.feather, BrushStroke::DEFAULT_FEATHER);
                    s.flow = ranges::AMOUNT_0_100.clamp_or(s.flow, BrushStroke::DEFAULT_FLOW);
                    s.density =
                        ranges::AMOUNT_0_100.clamp_or(s.density, BrushStroke::DEFAULT_DENSITY);
                    s.points.retain(|[x, y]| x.is_finite() && y.is_finite());
                    let keep = s.points.len().min(MAX_STROKE_POINTS).min(*budget);
                    s.points.truncate(keep);
                    *budget -= keep;
                    for [x, y] in &mut s.points {
                        clamp0(x, ranges::MASK_COORD);
                        clamp0(y, ranges::MASK_COORD);
                    }
                }
                strokes.retain(|s| !s.points.is_empty());
            }
            MaskShape::ColorRange { samples, amount } => {
                samples.truncate(MAX_COLOR_SAMPLES);
                for s in samples.iter_mut() {
                    clamp0(&mut s.x, ranges::MASK_COORD);
                    clamp0(&mut s.y, ranges::MASK_COORD);
                    clamp0(&mut s.lab[0], ranges::MASK_LAB_L);
                    clamp0(&mut s.lab[1], ranges::MASK_LAB_AB);
                    clamp0(&mut s.lab[2], ranges::MASK_LAB_AB);
                }
                *amount = ranges::AMOUNT_0_100.clamp_or(*amount, 50.0);
            }
            MaskShape::LuminanceRange {
                min,
                max,
                smoothness,
            } => {
                clamp0(min, ranges::AMOUNT_0_100);
                *max = ranges::AMOUNT_0_100.clamp_or(*max, 100.0);
                *smoothness = ranges::AMOUNT_0_100.clamp_or(*smoothness, 50.0);
                if *min > *max {
                    std::mem::swap(min, max);
                }
            }
        }
    }
}

impl LocalDelta {
    fn check(&self, path: &str, c: &mut Checker) {
        for (name, v) in LocalDelta::NAMES.iter().zip(self.values()) {
            let range = if *name == "exposure_ev" {
                ranges::EXPOSURE_EV
            } else {
                ranges::SLIDER
            };
            c.range(format!("{path}.{name}"), v, range);
        }
    }

    fn normalize(&mut self) {
        for (name, v) in LocalDelta::NAMES.iter().zip(self.values_mut()) {
            let range = if *name == "exposure_ev" {
                ranges::EXPOSURE_EV
            } else {
                ranges::SLIDER
            };
            clamp0(v, range);
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
        if let Some(p) = &self.profile {
            p.check("render_deps.profile.sha256", c);
        }
        if let Some(l) = &self.look {
            l.check("render_deps.look.sha256", c);
        }
    }

    pub(crate) fn normalize(&mut self) {
        self.camera_profile.normalize();
        for d in [&mut self.lens_profile, &mut self.profile, &mut self.look]
            .into_iter()
            .flatten()
        {
            d.normalize();
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
    use crate::develop::masks::{MaskComponent, MaskMode};
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
            delta: LocalDelta {
                exposure_ev: 9.0,
                ..Default::default()
            },
            ..LocalAdjustment::new(MaskComponent::add(MaskShape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 3.0,
                ry: 0.2,
                angle_deg: 0.0,
                feather: 50.0,
            }))
        });
        assert_eq!(
            issue_fields(&s),
            [
                "masks[0].components[0].shape.radial.rx",
                "masks[0].delta.exposure_ev"
            ]
        );
        s.normalized().validate().unwrap();
    }

    #[test]
    fn brush_and_range_masks_are_checked_and_normalized() {
        let mut s = DevelopSettings::default();
        let stroke = BrushStroke {
            size: 0.0,
            points: vec![[0.1, 0.2], [f32::NAN, 0.3], [5.0, 0.4]],
            ..Default::default()
        };
        let mut m = LocalAdjustment::new(MaskComponent::add(MaskShape::Brush {
            strokes: vec![stroke, BrushStroke::default()],
        }));
        m.components.push(MaskComponent {
            mode: MaskMode::Intersect,
            invert: false,
            shape: MaskShape::ColorRange {
                samples: vec![],
                amount: 120.0,
            },
        });
        m.components.push(MaskComponent {
            mode: MaskMode::Intersect,
            invert: true,
            shape: MaskShape::LuminanceRange {
                min: 80.0,
                max: 20.0,
                smoothness: 50.0,
            },
        });
        m.name = "空\u{7}".to_owned();
        m.delta.dehaze = 30.0;
        s.masks.push(m);
        assert_eq!(
            issue_fields(&s),
            [
                "masks[0].name",
                "masks[0].components[0].shape.brush.strokes[0].size",
                "masks[0].components[0].shape.brush.strokes[0].points[1][0]",
                "masks[0].components[0].shape.brush.strokes[1].points",
                "masks[0].components[1].shape.color_range.samples",
                "masks[0].components[1].shape.color_range.amount",
                "masks[0].components[2].shape.luminance_range.min",
            ]
        );
        let n = s.normalized();
        let nm = &n.masks[0];
        assert_eq!(nm.name, "空");
        let MaskShape::Brush { strokes } = &nm.components[0].shape else {
            panic!()
        };
        // 有限でない点を除き、範囲に丸め、点のないストロークを除く。
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].points, vec![[0.1, 0.2], [2.0, 0.4]]);
        assert_eq!(strokes[0].size, ranges::BRUSH_SIZE.min);
        assert_eq!(
            nm.components[2].shape,
            MaskShape::LuminanceRange {
                min: 20.0,
                max: 80.0,
                smoothness: 50.0
            }
        );
        // 見本の色のない色域の範囲は、丸めても残る（検証のエラーのまま。画面は見本を選んでから作る）。
        assert_eq!(
            issue_fields(&n),
            ["masks[0].components[1].shape.color_range.samples"]
        );
    }

    #[test]
    fn brush_point_limits_are_enforced() {
        let mut s = DevelopSettings::default();
        let stroke = |n: usize| BrushStroke {
            points: vec![[0.5, 0.5]; n],
            ..Default::default()
        };
        let strokes: Vec<_> = (0..30).map(|_| stroke(MAX_STROKE_POINTS)).collect();
        s.masks
            .push(LocalAdjustment::new(MaskComponent::add(MaskShape::Brush {
                strokes,
            })));
        assert_eq!(
            s.masks[0].brush_points(),
            30 * MAX_STROKE_POINTS,
            "上限を超える"
        );
        assert_eq!(issue_fields(&s), ["masks"]);
        let n = s.normalized();
        assert_eq!(n.masks[0].brush_points(), MAX_TOTAL_BRUSH_POINTS);
        n.validate().unwrap();
        // 1 本の点の上限。
        let mut t = DevelopSettings::default();
        t.masks
            .push(LocalAdjustment::new(MaskComponent::add(MaskShape::Brush {
                strokes: vec![stroke(MAX_STROKE_POINTS + 1)],
            })));
        assert_eq!(
            issue_fields(&t),
            ["masks[0].components[0].shape.brush.strokes[0].points"]
        );
        assert_eq!(t.normalized().masks[0].brush_points(), MAX_STROKE_POINTS);
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
