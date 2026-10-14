//! 3D LUT のファイル（.cube）の読み込み（DEV-17 の独自のルック。docs/04_architecture.md の 2.1 節のステージ 16 の
//! 最初）。
//!
//! 利用者が用意した 3D LUT（Adobe・Resolve などが書き出す `.cube` の形）を読み、四面体補間（[`Lut3d`]。画面の
//! 3D LUT と同じ）で当てる。**LUT は同梱しない**（利用者が取り込んだものだけを使う）。
//!
//! # 読む形
//!
//! - 1 行に 1 つの指定。`#` から行の終わりまでは注釈。空の行は飛ばす。UTF-8（先頭の BOM は無視する）。
//! - `TITLE "…"`（表示の名前）、`LUT_3D_SIZE N`（格子の数。[`MIN_CUBE_SIZE`]〜[`MAX_CUBE_SIZE`]）、
//!   `DOMAIN_MIN r g b`・`DOMAIN_MAX r g b`（入力の範囲。既定は 0 と 1）、`LUT_3D_INPUT_RANGE lo hi`（Resolve の形。
//!   3 つのチャンネルに同じ範囲）。キーワードの大文字・小文字は区別しない。
//! - 値の行（3 つの数）が N³ 行。並びは R が最も速く変わる（[`Lut3d`] と同じ）。
//! - **1D LUT（`LUT_1D_SIZE`）には対応しない**（エラー）。知らないキーワードの行もエラーにする（値の行と
//!   取り違えないため）。
//!
//! # 当て方（[`CubeLut::apply`]）
//!
//! 入力（符号化した値）を `(x − DOMAIN_MIN) · (1 / (DOMAIN_MAX − DOMAIN_MIN))` で 0〜1 の格子の位置にし、0〜1 に
//! 収めて四面体補間する。入力の色空間（どの原色・伝達関数を想定した LUT か）は、使う側（genzo-pipeline の
//! `finish::look`）が決める。

use std::fmt;

use crate::error::{ColorError, Result};
use crate::lut::Lut3d;

/// 格子の数の下限。
pub const MIN_CUBE_SIZE: usize = 2;
/// 格子の数の上限（**仮置き**: 65³ × 12 バイト ≒ 3.3 MB。よく使われる 17・33・64・65 を含む）。
pub const MAX_CUBE_SIZE: usize = 65;
/// ファイルの大きさの上限（バイト。**仮置き**: 65³ 行 × 1 行 40 バイト程度より十分大きい値）。
pub const MAX_CUBE_BYTES: usize = 32 << 20;
/// 表示の名前の文字数の上限（超えた分は切る）。
pub const MAX_TITLE_CHARS: usize = 256;

/// 読み込んだ 3D LUT。
#[derive(Clone, PartialEq)]
pub struct CubeLut {
    /// 表示の名前（`TITLE`）。
    pub title: Option<String>,
    /// 入力の範囲の下限（`DOMAIN_MIN`）。
    pub domain_min: [f32; 3],
    /// 入力の範囲の上限（`DOMAIN_MAX`）。
    pub domain_max: [f32; 3],
    lut: Lut3d,
    /// `1 / (DOMAIN_MAX − DOMAIN_MIN)`（f32。GPU 版にも同じ値を渡す）。
    domain_scale: [f32; 3],
}

impl fmt::Debug for CubeLut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CubeLut")
            .field("title", &self.title)
            .field("size", &self.lut.size())
            .field("domain_min", &self.domain_min)
            .field("domain_max", &self.domain_max)
            .finish_non_exhaustive()
    }
}

fn cube_error(line: Option<usize>, reason: impl Into<String>) -> ColorError {
    ColorError::CubeInvalid {
        line,
        reason: reason.into(),
    }
}

impl CubeLut {
    /// 格子の数・値（R が最も速く変わる並び）・入力の範囲から作る。範囲の上限が下限以下・有限でない、格子の数が
    /// 範囲の外、値の数が合わない・有限でない値があればエラー。
    pub fn new(
        title: Option<String>,
        size: usize,
        data: Vec<[f32; 3]>,
        domain_min: [f32; 3],
        domain_max: [f32; 3],
    ) -> Result<Self> {
        if !(MIN_CUBE_SIZE..=MAX_CUBE_SIZE).contains(&size) {
            return Err(cube_error(
                None,
                format!(
                    "LUT_3D_SIZE {size} には対応していません（{MIN_CUBE_SIZE}〜{MAX_CUBE_SIZE}）"
                ),
            ));
        }
        if data.len() != size * size * size {
            return Err(cube_error(
                None,
                format!(
                    "値の行の数が合いません（{} 行。{} 行が必要）",
                    data.len(),
                    size * size * size
                ),
            ));
        }
        if data.iter().flatten().any(|v| !v.is_finite()) {
            return Err(cube_error(None, "有限でない値があります"));
        }
        let mut domain_scale = [1.0f32; 3];
        for k in 0..3 {
            let (lo, hi) = (domain_min[k], domain_max[k]);
            if !(lo.is_finite() && hi.is_finite() && hi > lo) {
                return Err(cube_error(
                    None,
                    format!("入力の範囲が不正です（DOMAIN_MIN {lo}、DOMAIN_MAX {hi}）"),
                ));
            }
            domain_scale[k] = (1.0 / (f64::from(hi) - f64::from(lo))) as f32;
        }
        let title = title.map(|t| t.chars().take(MAX_TITLE_CHARS).collect::<String>());
        Ok(Self {
            title: title.filter(|t| !t.trim().is_empty()),
            domain_min,
            domain_max,
            lut: Lut3d::from_data(size, data)?,
            domain_scale,
        })
    }

    /// `.cube` のファイルの中身（テキスト）を読む（モジュールの doc の形）。
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > MAX_CUBE_BYTES {
            return Err(cube_error(
                None,
                format!("ファイルが大きすぎます（{MAX_CUBE_BYTES} バイトまで）"),
            ));
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut title = None;
        let mut size: Option<usize> = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut data: Vec<[f32; 3]> = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line_no = i + 1;
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let first = parts.next().unwrap_or("");
            let numbers = |rest: &str, n: usize| -> Result<Vec<f32>> {
                let v: Vec<&str> = rest.split_whitespace().collect();
                if v.len() != n {
                    return Err(cube_error(
                        Some(line_no),
                        format!("数が {n} 個ではありません"),
                    ));
                }
                v.iter()
                    .map(|s| {
                        s.parse::<f32>()
                            .ok()
                            .filter(|x| x.is_finite())
                            .ok_or_else(|| {
                                cube_error(Some(line_no), format!("数ではありません: {s}"))
                            })
                    })
                    .collect()
            };
            let rest = line[first.len()..].trim();
            if first.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') {
                let Some(n) = size else {
                    return Err(cube_error(
                        Some(line_no),
                        "LUT_3D_SIZE より前に値の行があります",
                    ));
                };
                if data.len() >= n * n * n {
                    return Err(cube_error(Some(line_no), "値の行が多すぎます"));
                }
                let v = numbers(line, 3)?;
                data.push([v[0], v[1], v[2]]);
                continue;
            }
            if !data.is_empty() {
                return Err(cube_error(
                    Some(line_no),
                    format!("値の行の途中に指定があります: {first}"),
                ));
            }
            match first.to_ascii_uppercase().as_str() {
                "TITLE" => {
                    let t = rest.trim();
                    let t = t
                        .strip_prefix('"')
                        .and_then(|t| t.strip_suffix('"'))
                        .unwrap_or(t);
                    title = Some(t.to_owned());
                }
                "LUT_3D_SIZE" => {
                    let n: usize = rest.parse().map_err(|_| {
                        cube_error(
                            Some(line_no),
                            format!("LUT_3D_SIZE が数ではありません: {rest}"),
                        )
                    })?;
                    if !(MIN_CUBE_SIZE..=MAX_CUBE_SIZE).contains(&n) {
                        return Err(cube_error(
                            Some(line_no),
                            format!(
                                "LUT_3D_SIZE {n} には対応していません（{MIN_CUBE_SIZE}〜{MAX_CUBE_SIZE}）"
                            ),
                        ));
                    }
                    if size.replace(n).is_some() {
                        return Err(cube_error(Some(line_no), "LUT_3D_SIZE が 2 つあります"));
                    }
                    data.reserve_exact(n * n * n);
                }
                "LUT_1D_SIZE" => {
                    return Err(cube_error(
                        Some(line_no),
                        "1D LUT（LUT_1D_SIZE）には対応していません（3D LUT だけ）",
                    ));
                }
                "DOMAIN_MIN" => {
                    let v = numbers(rest, 3)?;
                    domain_min = [v[0], v[1], v[2]];
                }
                "DOMAIN_MAX" => {
                    let v = numbers(rest, 3)?;
                    domain_max = [v[0], v[1], v[2]];
                }
                "LUT_3D_INPUT_RANGE" => {
                    let v = numbers(rest, 2)?;
                    domain_min = [v[0]; 3];
                    domain_max = [v[1]; 3];
                }
                other => {
                    return Err(cube_error(
                        Some(line_no),
                        format!("知らない指定です: {other}"),
                    ));
                }
            }
        }
        let Some(n) = size else {
            return Err(cube_error(None, "LUT_3D_SIZE がありません"));
        };
        Self::new(title, n, data, domain_min, domain_max)
    }

    /// 格子の数。
    pub fn size(&self) -> usize {
        self.lut.size()
    }

    /// 格子の値（[`Lut3d`]）。
    pub fn lut(&self) -> &Lut3d {
        &self.lut
    }

    /// `1 / (DOMAIN_MAX − DOMAIN_MIN)`（f32）。
    pub fn domain_scale(&self) -> [f32; 3] {
        self.domain_scale
    }

    /// 入力の範囲が既定（0〜1）か。
    pub fn has_default_domain(&self) -> bool {
        self.domain_min == [0.0; 3] && self.domain_max == [1.0; 3]
    }

    /// 1 画素（符号化した値）に当てる: `(x − DOMAIN_MIN) · scale` を 0〜1 に収めて四面体補間（[`Lut3d::apply`]）。
    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p = [0, 1, 2].map(|k| (rgb[k] - self.domain_min[k]) * self.domain_scale[k]);
        self.lut.apply(p)
    }

    /// `.cube` のテキスト（**テスト・検証用**。このアプリは LUT を書き出す機能を持たないが、テストの中で
    /// LUT のファイルを作るために使う）。
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        if let Some(t) = &self.title {
            s.push_str(&format!("TITLE \"{t}\"\n"));
        }
        s.push_str(&format!("LUT_3D_SIZE {}\n", self.size()));
        if !self.has_default_domain() {
            let [a, b, c] = self.domain_min;
            s.push_str(&format!("DOMAIN_MIN {a} {b} {c}\n"));
            let [a, b, c] = self.domain_max;
            s.push_str(&format!("DOMAIN_MAX {a} {b} {c}\n"));
        }
        for v in self.lut.data() {
            s.push_str(&format!("{} {} {}\n", v[0], v[1], v[2]));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(n: usize) -> Vec<[f32; 3]> {
        Lut3d::identity(n).unwrap().data().to_vec()
    }

    #[test]
    fn parses_the_common_form() {
        let text = "\u{feff}# 注釈\nTITLE \"テストのルック\"\nLUT_3D_SIZE 2\n\n0 0 0\n1 0 0 # 赤\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let c = CubeLut::parse(text).unwrap();
        assert_eq!(c.title.as_deref(), Some("テストのルック"));
        assert_eq!(c.size(), 2);
        assert!(c.has_default_domain());
        assert_eq!(c.apply([0.25, 0.5, 0.75]), [0.25, 0.5, 0.75]);
        // 書いて読み直しても同じ。
        let back = CubeLut::parse(&c.to_text()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn domain_scales_the_input() {
        let mut text = String::from("LUT_3D_SIZE 3\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n");
        for v in identity(3) {
            text.push_str(&format!("{} {} {}\n", v[0], v[1], v[2]));
        }
        let c = CubeLut::parse(&text).unwrap();
        // 1.0 は範囲の真ん中（0.5）。
        let out = c.apply([1.0, 0.5, 2.0]);
        for (o, e) in out.iter().zip([0.5, 0.25, 1.0]) {
            assert!((o - e).abs() < 1e-6, "{out:?}");
        }
        assert_eq!(CubeLut::parse(&c.to_text()).unwrap(), c);
        // Resolve の形。
        let text = text.replace(
            "DOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2",
            "LUT_3D_INPUT_RANGE 0 2",
        );
        assert_eq!(CubeLut::parse(&text).unwrap().domain_max, [2.0; 3]);
    }

    #[test]
    fn malformed_files_are_rejected_with_line_numbers() {
        let cases: [(&str, &str); 9] = [
            ("0 0 0\n", "LUT_3D_SIZE より前"),
            ("LUT_3D_SIZE 1\n", "対応していません"),
            ("LUT_3D_SIZE 99\n", "対応していません"),
            ("LUT_1D_SIZE 16\n", "1D LUT"),
            ("LUT_3D_SIZE 2\n0 0\n", "3 個ではありません"),
            ("LUT_3D_SIZE 2\n0 0 x\n", "数ではありません"),
            ("LUT_3D_SIZE 2\n0 0 0\n", "行の数が合いません"),
            ("FOO 1\n", "知らない指定"),
            (
                "LUT_3D_SIZE 2\nDOMAIN_MIN 1 1 1\nDOMAIN_MAX 0 0 0\n",
                "行の数が合いません",
            ),
        ];
        for (text, want) in cases {
            let e = CubeLut::parse(text).unwrap_err().to_string();
            assert!(e.contains(want), "{text:?}: {e}");
        }
        let e = CubeLut::parse("LUT_3D_SIZE 2\n0 0 0\nTITLE \"x\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("3 行目"), "{e}");
        assert!(CubeLut::parse("").is_err());
        let mut text = String::from("LUT_3D_SIZE 2\nDOMAIN_MIN 1 1 1\nDOMAIN_MAX 0 0 0\n");
        for v in identity(2) {
            text.push_str(&format!("{} {} {}\n", v[0], v[1], v[2]));
        }
        assert!(
            CubeLut::parse(&text)
                .unwrap_err()
                .to_string()
                .contains("入力の範囲")
        );
        let mut text = String::from("LUT_3D_SIZE 2\n");
        for _ in 0..9 {
            text.push_str("0 0 0\n");
        }
        assert!(
            CubeLut::parse(&text)
                .unwrap_err()
                .to_string()
                .contains("多すぎ")
        );
    }
}
