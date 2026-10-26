//! スポット修正（DEV-21。修復・コピースタンプ。処理バージョン 2）の型（docs/04_architecture.md の 2.1 節
//! 「スポット修正」・2.5 節。設計は docs/retouch_design.md）。
//!
//! 1 つのスポット（[`RetouchSpot`]）は、消したい場所（点 1 つなら円、2 つ以上なら点を順に結んだ線。電線など）と、
//! 写してくる場所（コピー元。消したい場所を [`RetouchSpot::offset`] だけずらした場所）からなる。パイプライン
//! （genzo-pipeline の `retouch`）は、段階 A1 の出力（センサー処理の後の作業色空間の画像）に、スポットを順に
//! 当てる。
//!
//! # 座標
//!
//! 位置・大きさ・ずれは **O 座標**（RAW の向きを反映した画像（既定の切り抜きの範囲）の長辺を 1 とする座標。
//! 左上が (0, 0)。genzo-pipeline の `coords` の O）で表す。マスク（G 座標。回転・歪曲補正の後）と違い、
//! 角度補正・レンズの歪曲補正・切り抜きを後から変えても、スポットは写っているもの（ゴミ・電線）の上から
//! ずれない。プレビューと書き出しの解像度にもよらない。
//!
//! # JSON の形
//!
//! ```json
//! {"retouch": [
//!   {"mode": "heal", "points": [[0.41, 0.22]], "size": 0.01, "offset": [0.03, 0.0]},
//!   {"mode": "clone", "points": [[0.1, 0.5], [0.6, 0.45]], "size": 0.004, "feather": 30, "offset": [0.0, 0.012]}
//! ]}
//! ```
//!
//! スポットがないときは `retouch` を JSON に書かない（項目を足す前の設定とハッシュが同じ）。

use serde::{Deserialize, Serialize};

/// スポットの数の上限（**仮置き**）。
pub const MAX_RETOUCH_SPOTS: usize = 1000;
/// 1 つのスポットの点の数の上限（**仮置き**。ブラシの 1 本のストロークと同じ）。
pub const MAX_RETOUCH_POINTS: usize = 5000;
/// 設定の全体のスポットの点の数の上限（**仮置き**。ブラシと同じ。履歴の 1 件ごとに設定の全体を保存するため）。
pub const MAX_TOTAL_RETOUCH_POINTS: usize = 100_000;
/// 線のスポットの長さ（点を順に結んだ折れ線の長さ。O 座標で、長辺の何倍か）の上限（**仮置き**）。写真を
/// 斜めに横切る線（長さ 1.2 ほど）の数倍。画像の外の端どうしを往復する点の列のような極端に長い線で、修復の
/// 計算の時間とメモリが際限なく増えないようにする。
pub const MAX_RETOUCH_STROKE_LENGTH: f32 = 4.0;

/// スポットの直し方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetouchMode {
    /// 修復（コピー元の模様を写し、明るさと色をまわりになじませる）。
    #[default]
    Heal,
    /// コピースタンプ（コピー元をそのまま写す。縁だけぼかす）。
    Clone,
}

/// スポット修正 1 つ分（DEV-21）。位置・大きさ・ずれは O 座標（モジュールの doc）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetouchSpot {
    /// 有効か。
    pub enabled: bool,
    /// 直し方。
    pub mode: RetouchMode,
    /// 消したい場所の点の列（O 座標の `[x, y]`）。1 点なら円、2 点以上なら点を順に結んだ線（太さは大きさの 2 倍）。
    pub points: Vec<[f32; 2]>,
    /// 大きさ（半径。O 座標。[`super::ranges::RETOUCH_SIZE`]）。
    pub size: f32,
    /// ぼかし（0〜100。縁から内側のこの割合の幅で、なめらかに元の画像に戻す）。
    pub feather: f32,
    /// 不透明度（0〜100）。
    pub opacity: f32,
    /// コピー元のずれ（O 座標の `[dx, dy]`。コピー元 = 消したい場所 + ずれ）。
    pub offset: [f32; 2],
}

impl RetouchSpot {
    /// 大きさの既定値（長辺の 1%）。
    pub const DEFAULT_SIZE: f32 = 0.01;
    /// ぼかしの既定値。
    pub const DEFAULT_FEATHER: f32 = 50.0;
    /// 不透明度の既定値。
    pub const DEFAULT_OPACITY: f32 = 100.0;

    /// 点 `points` のスポット（有効・修復・既定の大きさ・ずれ `offset`）。
    pub fn new(points: Vec<[f32; 2]>, offset: [f32; 2]) -> Self {
        Self {
            points,
            offset,
            ..Self::default()
        }
    }

    /// 結果に影響する（有効・点がある・不透明度が 0 でない・ずれが 0 でない）か。
    ///
    /// ずれが 0 なら、コピー元が消したい場所そのものなので、修復・コピースタンプのどちらも元の画像のまま。
    pub fn is_active(&self) -> bool {
        self.enabled && !self.points.is_empty() && self.opacity > 0.0 && self.offset != [0.0, 0.0]
    }

    /// 線（2 点以上）か。
    pub fn is_stroke(&self) -> bool {
        self.points.len() >= 2
    }

    /// 点を順に結んだ折れ線の長さ（O 座標。円なら 0）。
    pub fn stroke_length(&self) -> f64 {
        self.points
            .windows(2)
            .map(|w| f64::from(w[1][0] - w[0][0]).hypot(f64::from(w[1][1] - w[0][1])))
            .sum()
    }
}

impl Default for RetouchSpot {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: RetouchMode::Heal,
            points: Vec::new(),
            size: Self::DEFAULT_SIZE,
            feather: Self::DEFAULT_FEATHER,
            opacity: Self::DEFAULT_OPACITY,
            offset: [0.0, 0.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_forms_and_defaults() {
        let s: RetouchSpot =
            serde_json::from_str(r#"{"points": [[0.4, 0.2]], "offset": [0.03, 0]}"#).unwrap();
        assert!(s.enabled);
        assert_eq!(s.mode, RetouchMode::Heal);
        assert_eq!(s.size, RetouchSpot::DEFAULT_SIZE);
        assert_eq!(s.feather, 50.0);
        assert_eq!(s.opacity, 100.0);
        assert!(s.is_active() && !s.is_stroke());
        assert_eq!(
            serde_json::to_value(RetouchMode::Clone).unwrap(),
            serde_json::json!("clone")
        );
        let line: RetouchSpot = serde_json::from_str(
            r#"{"mode": "clone", "points": [[0.1, 0.5], [0.6, 0.45]], "offset": [0, 0.01]}"#,
        )
        .unwrap();
        assert!(line.is_stroke() && line.mode == RetouchMode::Clone);
    }

    #[test]
    fn inactive_spots() {
        let base = RetouchSpot::new(vec![[0.5, 0.5]], [0.02, 0.0]);
        assert!(base.is_active());
        for f in [
            |s: &mut RetouchSpot| s.enabled = false,
            |s: &mut RetouchSpot| s.points.clear(),
            |s: &mut RetouchSpot| s.opacity = 0.0,
            |s: &mut RetouchSpot| s.offset = [0.0, 0.0],
        ] {
            let mut s = base.clone();
            f(&mut s);
            assert!(!s.is_active(), "{s:?}");
        }
    }
}
