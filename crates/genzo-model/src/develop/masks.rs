//! ローカル補正（マスク。DEV-18・DEV-19。v1。処理バージョン 2）の型（docs/04_architecture.md の 2.1 節
//! 「ローカル補正」・2.5 節）。
//!
//! 1 つのマスク（[`LocalAdjustment`]）は、部品（[`MaskComponent`]）を順に合成した 0〜1 の重みと、調整値の差分
//! （[`LocalDelta`]）からなる。パイプライン（genzo-pipeline の `local`）は、画素ごとに「マスクの重み × 差分」を
//! すべてのマスクについて足した値を、ステージ 10〜14 の全体の値に足して使う。
//!
//! # 座標
//!
//! 位置・半径・ブラシの大きさは 04 の 2.7 節の **G 座標**（ジオメトリ補正後・切り抜く前の全体の外接矩形の長辺を 1
//! とする座標。左上が (0, 0)）で表す。[`super::CropRect`]（各軸の割合）とは違って縦横の尺度が同じなので、
//! `rx == ry` の円形マスクは画像の縦横比によらず円になる。切り抜きを変えてもマスクの位置は変わらず、プレビューと
//! 書き出しの解像度にもよらない。
//!
//! # 部品の合成（[`MaskMode`]）
//!
//! 重み `m` を、最初の部品が「加える」なら 0、それ以外なら 1（画像全体）から始め、部品の値 `c`（反転なら `1 − c`）
//! を順に当てる: 加える `m = max(m, c)`、除く `m = m · (1 − c)`、交差 `m = m · c`。範囲マスク（色域・輝度）は
//! 交差の部品として使う（DEV-19）。
//!
//! # JSON の形
//!
//! ```json
//! {"enabled": true, "name": "空", "components": [
//!   {"mode": "add", "invert": false, "shape": {"linear": {"x0": 0.5, "y0": 0.0, "x1": 0.5, "y1": 0.3}}},
//!   {"mode": "intersect", "shape": {"luminance_range": {"min": 50, "max": 100, "smoothness": 50}}}
//! ], "delta": {"exposure_ev": -0.5, "dehaze": 20}}
//! ```
//!
//! finish-detail までの仮の形（`"mask": {...}` に形が 1 つ。円形の反転は形の中）は、読み込みで部品 1 つの形に
//! 読み替える（[`super::migrate`]）。

use serde::{Deserialize, Serialize};

/// ローカル補正（マスク）の数の上限。
pub const MAX_MASKS: usize = 100;
/// 1 つのマスクの部品の数の上限（**仮置き**）。
pub const MAX_MASK_COMPONENTS: usize = 32;
/// 1 つのブラシの部品のストロークの数の上限（**仮置き**）。
pub const MAX_BRUSH_STROKES: usize = 1000;
/// 1 本のストロークの点の数の上限（**仮置き**）。
pub const MAX_STROKE_POINTS: usize = 5000;
/// 設定の全体のブラシの点の数の上限（**仮置き**。JSON で約 2 MB。履歴の 1 件ごとに設定の全体を保存するため）。
pub const MAX_TOTAL_BRUSH_POINTS: usize = 100_000;
/// 色域の範囲の見本の色の数の上限（Lightroom Classic と同じ 5 つ。**AI の理解。要確認**）。
pub const MAX_COLOR_SAMPLES: usize = 5;
/// マスクの名前の長さの上限（文字）。
pub const MAX_MASK_NAME_CHARS: usize = 100;

/// 部品の合成のしかた（モジュールの doc の「部品の合成」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMode {
    /// 加える（和。`max`）。
    #[default]
    Add,
    /// 除く（`m · (1 − c)`）。
    Subtract,
    /// 交差（積。範囲マスクで選択範囲を絞る）。
    Intersect,
}

impl MaskMode {
    /// 部品を当てる（`m` は今の重み、`c` は部品の値。どちらも 0〜1）。
    #[inline]
    pub fn combine(self, m: f32, c: f32) -> f32 {
        match self {
            MaskMode::Add => m.max(c),
            MaskMode::Subtract => m * (1.0 - c),
            MaskMode::Intersect => m * c,
        }
    }

    /// 最初の部品がこの合成のときの、重みの初めの値（加えるなら 0、除く・交差なら 1 = 画像全体）。
    #[inline]
    pub fn initial(self) -> f32 {
        match self {
            MaskMode::Add => 0.0,
            MaskMode::Subtract | MaskMode::Intersect => 1.0,
        }
    }
}

/// ブラシのストローク 1 本（DEV-18）。
///
/// 点の列（G 座標）を順に結んだ折れ線からの距離 `d` で、大きさ（半径）`size` とぼかしから覆い方 `cov`（0〜1）を
/// 決め、ストロークごとに重みに当てる: 描くなら `m + max(0, 密度 − m) · 流量 · cov`（密度までしか上がらない）、
/// 消すなら `m · (1 − 密度 · 流量 · cov)`。式は genzo-pipeline の `local::brush`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushStroke {
    /// 消しゴム（重みを減らす）か。
    pub erase: bool,
    /// 大きさ（半径。G 座標。[`super::ranges::BRUSH_SIZE`]）。
    pub size: f32,
    /// ぼかし（0〜100。100 で中心から外へなめらかに 0 になる）。
    pub feather: f32,
    /// 流量（0〜100。1 本のストロークで密度までの差のどれだけを塗るか）。
    pub flow: f32,
    /// 密度（0〜100。描くときの重みの上限・消すときの強さ）。
    pub density: f32,
    /// 点の列（G 座標の `[x, y]`）。1 点なら円。
    pub points: Vec<[f32; 2]>,
}

impl BrushStroke {
    /// 大きさの既定値。
    pub const DEFAULT_SIZE: f32 = 0.05;
    /// ぼかしの既定値。
    pub const DEFAULT_FEATHER: f32 = 50.0;
    /// 流量・密度の既定値。
    pub const DEFAULT_FLOW: f32 = 100.0;
    /// 密度の既定値。
    pub const DEFAULT_DENSITY: f32 = 100.0;
}

impl Default for BrushStroke {
    fn default() -> Self {
        Self {
            erase: false,
            size: Self::DEFAULT_SIZE,
            feather: Self::DEFAULT_FEATHER,
            flow: Self::DEFAULT_FLOW,
            density: Self::DEFAULT_DENSITY,
            points: Vec::new(),
        }
    }
}

/// 色域の範囲の見本の色（スポイトで選んだ点。DEV-19）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorSample {
    /// 選んだ点の x（G 座標。画面に印を出すためだけに使う）。
    pub x: f32,
    /// 選んだ点の y（G 座標）。
    pub y: f32,
    /// 選んだ点のまわりの平均の色の OKLab（ステージ 9 の出力。露光量の前のシーンリニアの値から求める）。
    pub lab: [f32; 3],
}

/// マスクの部品の形（DEV-18・DEV-19）。位置・半径は G 座標（モジュールの doc）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskShape {
    /// 線形グラデーション。(x0, y0) を通り (x1 − x0, y1 − y0) に垂直な線で 100%、(x1, y1) を通る線で 0%（間は
    /// smoothstep）。
    Linear {
        /// 始点の x。
        x0: f32,
        /// 始点の y。
        y0: f32,
        /// 終点の x。
        x1: f32,
        /// 終点の y。
        y1: f32,
    },
    /// 円形グラデーション（楕円の内側で 100%）。
    Radial {
        /// 中心の x。
        cx: f32,
        /// 中心の y。
        cy: f32,
        /// 横方向の半径。
        rx: f32,
        /// 縦方向の半径。
        ry: f32,
        /// 回転（度。時計回り）。
        angle_deg: f32,
        /// ぼかし（0〜100。楕円の縁から内側のこの割合の範囲で 0 に向かう）。
        #[serde(default = "default_feather")]
        feather: f32,
    },
    /// ブラシ（ストロークを順に塗る）。
    Brush {
        /// ストローク（塗った順）。
        #[serde(default)]
        strokes: Vec<BrushStroke>,
    },
    /// 色域の範囲（見本の色に近い色。DEV-19）。
    ColorRange {
        /// 見本の色（1〜[`MAX_COLOR_SAMPLES`] 個。どれかに近ければ選ぶ）。
        #[serde(default)]
        samples: Vec<ColorSample>,
        /// 許容量（0〜100。大きいほど離れた色まで選ぶ）。
        #[serde(default = "default_range_amount")]
        amount: f32,
    },
    /// 輝度の範囲（表示の明るさの範囲。DEV-19）。
    LuminanceRange {
        /// 範囲の下端（0〜100。表示の明るさを sRGB の伝達関数で符号化した値の百分率）。
        #[serde(default)]
        min: f32,
        /// 範囲の上端（0〜100）。
        #[serde(default = "default_luminance_max")]
        max: f32,
        /// 滑らかさ（0〜100。範囲の外へなめらかに 0 にする幅）。
        #[serde(default = "default_smoothness")]
        smoothness: f32,
    },
}

fn default_feather() -> f32 {
    50.0
}

fn default_range_amount() -> f32 {
    50.0
}

fn default_luminance_max() -> f32 {
    100.0
}

fn default_smoothness() -> f32 {
    50.0
}

impl MaskShape {
    /// 範囲マスク（画素の色・明るさで決まる部品）か。
    pub fn is_range(&self) -> bool {
        matches!(
            self,
            MaskShape::ColorRange { .. } | MaskShape::LuminanceRange { .. }
        )
    }

    /// ブラシの点の数。
    pub fn brush_points(&self) -> usize {
        match self {
            MaskShape::Brush { strokes } => strokes.iter().map(|s| s.points.len()).sum(),
            _ => 0,
        }
    }
}

/// マスクの部品（形と合成のしかた）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaskComponent {
    /// 合成のしかた（既定は加える）。
    #[serde(default)]
    pub mode: MaskMode,
    /// 部品の値を反転する（`1 − c`）。
    #[serde(default)]
    pub invert: bool,
    /// 形。
    pub shape: MaskShape,
}

impl MaskComponent {
    /// 加える部品。
    pub fn add(shape: MaskShape) -> Self {
        Self {
            mode: MaskMode::Add,
            invert: false,
            shape,
        }
    }
}

/// ローカル補正の調整値の差分（DEV-18。画素ごとに「マスクの重み × 差分」を全体の値に足す。04 の 2.1 節）。
///
/// どれも全体のスライダーと同じ単位（露光量は EV、ほかは −100〜+100）。色温度・色かぶりは全体の WB（ステージ 3）
/// とは別に、ステージ 10 で作業色空間の RGB に掛ける相対的な補正。シャープは全体の適用量に足す（負の値はぼかす）。
/// ノイズ軽減（ステージ 6 は段階 A1）は持たない（決定事項ログ）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalDelta {
    /// 色温度（−100〜+100。正で暖かく（黄）、負で冷たく（青））。
    pub temperature: f32,
    /// 色かぶり補正（−100〜+100。正でマゼンタ、負で緑）。
    pub tint: f32,
    /// 露光量（EV）。
    pub exposure_ev: f32,
    /// コントラスト。
    pub contrast: f32,
    /// ハイライト。
    pub highlights: f32,
    /// シャドウ。
    pub shadows: f32,
    /// 白レベル。
    pub whites: f32,
    /// 黒レベル。
    pub blacks: f32,
    /// テクスチャ。
    pub texture: f32,
    /// 明瞭度。
    pub clarity: f32,
    /// かすみの除去。
    pub dehaze: f32,
    /// 彩度。
    pub saturation: f32,
    /// シャープ（−100〜+100。全体の適用量に足す。負の値はぼかす）。
    pub sharpness: f32,
}

impl LocalDelta {
    /// 項目の数。
    pub const LEN: usize = 13;

    /// すべての差分が 0 か。
    pub fn is_zero(&self) -> bool {
        self.values().iter().all(|&v| v == 0.0)
    }

    /// 値の並び（[`LocalDelta::NAMES`] と同じ順。genzo-pipeline の `LocalParam` の番号）。
    pub fn values(&self) -> [f32; Self::LEN] {
        [
            self.temperature,
            self.tint,
            self.exposure_ev,
            self.contrast,
            self.highlights,
            self.shadows,
            self.whites,
            self.blacks,
            self.texture,
            self.clarity,
            self.dehaze,
            self.saturation,
            self.sharpness,
        ]
    }

    /// 値の並びの変更できる参照（[`LocalDelta::values`] と同じ順）。
    pub fn values_mut(&mut self) -> [&mut f32; Self::LEN] {
        [
            &mut self.temperature,
            &mut self.tint,
            &mut self.exposure_ev,
            &mut self.contrast,
            &mut self.highlights,
            &mut self.shadows,
            &mut self.whites,
            &mut self.blacks,
            &mut self.texture,
            &mut self.clarity,
            &mut self.dehaze,
            &mut self.saturation,
            &mut self.sharpness,
        ]
    }

    /// 項目の JSON の名前（[`LocalDelta::values`] と同じ順）。
    pub const NAMES: [&'static str; Self::LEN] = [
        "temperature",
        "tint",
        "exposure_ev",
        "contrast",
        "highlights",
        "shadows",
        "whites",
        "blacks",
        "texture",
        "clarity",
        "dehaze",
        "saturation",
        "sharpness",
    ];
}

/// ローカル補正（マスク）1 つ分（DEV-18・DEV-19）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAdjustment {
    /// 有効か。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 名前（画面の一覧に出す。空なら「マスク 1」などの番号）。
    #[serde(default)]
    pub name: String,
    /// 部品（合成の順。モジュールの doc）。
    #[serde(default)]
    pub components: Vec<MaskComponent>,
    /// 調整値の差分。
    #[serde(default)]
    pub delta: LocalDelta,
}

impl LocalAdjustment {
    /// 部品 `component` 1 つのマスク（有効・名前なし・差分 0）。
    pub fn new(component: MaskComponent) -> Self {
        Self {
            enabled: true,
            name: String::new(),
            components: vec![component],
            delta: LocalDelta::default(),
        }
    }

    /// 結果に影響しない（無効・部品がない・差分がすべて 0）か。
    pub fn is_neutral(&self) -> bool {
        !self.enabled || self.components.is_empty() || self.delta.is_zero()
    }

    /// ブラシの点の数。
    pub fn brush_points(&self) -> usize {
        self.components.iter().map(|c| c.shape.brush_points()).sum()
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_forms_and_defaults() {
        let adj: LocalAdjustment = serde_json::from_str(
            r#"{"components": [{"shape": {"linear": {"x0": 0, "y0": 0, "x1": 0, "y1": 1}}}]}"#,
        )
        .unwrap();
        assert!(adj.enabled);
        assert_eq!(adj.name, "");
        assert_eq!(adj.components[0].mode, MaskMode::Add);
        assert!(!adj.components[0].invert);
        assert!(adj.delta.is_zero());
        assert!(adj.is_neutral());
        let s: MaskShape = serde_json::from_str(r#"{"luminance_range": {"min": 20}}"#).unwrap();
        assert_eq!(
            s,
            MaskShape::LuminanceRange {
                min: 20.0,
                max: 100.0,
                smoothness: 50.0
            }
        );
        assert!(s.is_range());
        let s: MaskShape =
            serde_json::from_str(r#"{"brush": {"strokes": [{"points": [[0.1, 0.2]]}]}}"#).unwrap();
        let MaskShape::Brush { strokes } = &s else {
            panic!("{s:?}")
        };
        assert_eq!(strokes[0].size, BrushStroke::DEFAULT_SIZE);
        assert_eq!(strokes[0].flow, 100.0);
        assert_eq!(s.brush_points(), 1);
        assert_eq!(
            serde_json::to_value(MaskMode::Intersect).unwrap(),
            serde_json::json!("intersect")
        );
    }

    #[test]
    fn combine_follows_the_documented_rules() {
        assert_eq!(MaskMode::Add.combine(0.3, 0.6), 0.6);
        assert_eq!(MaskMode::Add.combine(0.7, 0.6), 0.7);
        assert!((MaskMode::Subtract.combine(0.8, 0.25) - 0.6).abs() < 1e-7);
        assert!((MaskMode::Intersect.combine(0.8, 0.5) - 0.4).abs() < 1e-7);
        assert_eq!(MaskMode::Add.initial(), 0.0);
        assert_eq!(MaskMode::Intersect.initial(), 1.0);
    }

    #[test]
    fn delta_values_follow_the_names() {
        let mut d = LocalDelta::default();
        for (i, v) in d.values_mut().into_iter().enumerate() {
            *v = i as f32 + 1.0;
        }
        let json = serde_json::to_value(d).unwrap();
        for (i, name) in LocalDelta::NAMES.iter().enumerate() {
            assert_eq!(json[name], serde_json::json!(i as f32 + 1.0), "{name}");
        }
        assert_eq!(d.values()[2], d.exposure_ev);
        assert!(!d.is_zero());
    }
}
