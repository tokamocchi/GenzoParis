//! 自動の遠近補正（Upright 相当。DEV-16）: 画像の直線を検出し、垂直・水平の遠近補正と角度補正の値を求める。
//!
//! 結果は設定の値（genzo-model の `Transform::vertical`・`horizontal` と `Geometry::angle_deg`）として書き込み、
//! 描画は値だけを見る（[`crate::perspective`]）。そのため解析のアルゴリズムを変えても、保存した写真の見え方は
//! 変わらない（IQ-08）。
//!
//! # 方法（AI の仮決定。値は仮置き）
//!
//! 入力は歪曲補正の後の枠（D。[`crate::coords`]。回転・遠近補正の前）の輝度の画像（長辺
//! [`ANALYSIS_LONG_EDGE`] px 以下。[`crate::engine::Engine::analyze_upright`] が段階 A1 のプレビューから作る）。
//!
//! 1. **エッジ**: 輝度の対数（[`crate::sanitize::safe_log2`]。明るさの違いによらず縁を拾う）を 5 タップのガウスで
//!    ぼかし、Sobel の勾配の大きさが平均の [`EDGE_THRESHOLD_FACTOR`] 倍以上で、勾配の向きで極大（細線化）の画素を
//!    エッジにする。
//! 2. **直線の候補**（勾配の向きつきの Hough 変換）: エッジの画素ごとに、法線の向き θ（勾配の向き。0.5 度刻み）と
//!    中心からの距離 ρ（1 px 刻み）の表に、θ の隣の区間も含めて投票する。極大で票が長辺の [`MIN_VOTES_FRACTION`]
//!    以上の区間を候補にする。
//! 3. **線分**: 候補ごとに、法線の向きが ±[`SUPPORT_THETA_DEG`]・距離が ±[`SUPPORT_RHO_PX`] のエッジの画素を集め、
//!    線に沿って並べて、すき間が [`MAX_GAP_PX`] 以下で最も長い連なりを取る。長さが長辺の
//!    [`MIN_SEGMENT_FRACTION`] 以上なら、画素の主成分で直線を当てはめて線分にする（直線からの画素のずれの
//!    2 乗平均平方根が [`MAX_FIT_RMS_PX`] より大きい候補は、別の線をまたいだ偽の線分として捨てる。使った画素は、
//!    ほかの候補に使わない）。
//! 4. **分類**: 線分の向きが垂直から ±[`GROUP_HALF_WIDTH_DEG`] 以内なら「垂直の線」、水平から同じ範囲なら
//!    「水平の線」。どちらでもない線（斜め）は使わない。
//! 5. **当てはめ**: 傾け直しの角度（x 軸のまわり φ・y 軸のまわり ψ。[`crate::perspective::tilt_rotation`]）と
//!    角度補正 θ を、変換した後の垂直の線が垂直に、水平の線が水平になるように求める。評価は線分の長さで重みを
//!    付けた Cauchy の損失（尺度 [`LOSS_SCALE_DEG`]）の和で、Nelder–Mead 法で最小にする（決定的。乱数は使わない）。
//!    残差が大きい線分（中央値の 3 倍かつ [`OUTLIER_MIN_DEG`] 以上）を除いて当てはめ直す（[`OUTLIER_ROUNDS`] 回）。
//!    - 水平（[`UprightMode::Level`]）: θ だけ（φ = ψ = 0）。
//!    - 垂直（[`UprightMode::Vertical`]）: θ と φ（ψ = 0）。
//!    - 完全（[`UprightMode::Full`]）: θ・φ・ψ。水平の線が足りなければ ψ = 0。
//! 6. 結果を設定の値にする: `angle_deg = θ`（±45 度に収める）、`vertical = −φ / TILT_MAX_DEG × 100`、
//!    `horizontal = ψ / TILT_MAX_DEG × 100`（±100 に収める。合成の上限は描画の側で縮める）。
//!
//! 線分が足りないとき（モードに要る向きの線分の長さの合計が、対角線の半分の [`MIN_GROUP_SUPPORT`] 倍に届かない・
//! 本数が [`MIN_GROUP_LINES`] に届かない）は [`PipelineError::InvalidArgument`]（画面は「直線が見つかりません」と
//! 知らせる）。
//!
//! Lightroom Classic の Upright のアルゴリズムは公開されておらず、ここは一般的な方法（勾配の向きつきの Hough 変換と
//! 消失点に基づく当てはめ。darktable の perspective correction モジュールの考え方に近い）で作った **AI の仮決定**。
//! 実写での精度は人の確認待ち（implementation_status の判断の表）。

use rayon::prelude::*;

use crate::error::{PipelineError, Result};
use crate::image::GrayImage;
use crate::perspective::{TILT_MAX_DEG, TILT_TOTAL_MAX_DEG, tilt_rotation};
use crate::sanitize::safe_log2;

/// 解析に使う画像の長辺の上限（px。**仮置き**: 1024。A1 のプレビュー（長辺 2560）を縮小して作る）。
pub const ANALYSIS_LONG_EDGE: u32 = 1024;
/// エッジとみなす勾配の大きさ（平均に対する倍率。**仮置き**）。
pub const EDGE_THRESHOLD_FACTOR: f32 = 2.0;
/// Hough 変換の θ の刻み（度）。
pub const THETA_STEP_DEG: f64 = 0.5;
/// 直線の候補にする票の下限（長辺に対する割合。**仮置き**）。
pub const MIN_VOTES_FRACTION: f64 = 0.02;
/// 線分の画素を集めるときの法線の向きの許容（度）。
pub const SUPPORT_THETA_DEG: f64 = 1.5;
/// 線分の画素を集めるときの距離の許容（px）。
pub const SUPPORT_RHO_PX: f64 = 1.5;
/// 線分の中のすき間の上限（px。これより空けば別の連なり）。
pub const MAX_GAP_PX: f64 = 6.0;
/// 線分の長さの下限（長辺に対する割合。**仮置き**）。
pub const MIN_SEGMENT_FRACTION: f64 = 0.03;
/// 当てはめた直線からの画素のずれ（2 乗平均平方根。px）の上限。太い線の 2 つの縁をまたいでつないだ候補
/// （わずかに傾いた偽の線分）を除く（**仮置き**）。
pub const MAX_FIT_RMS_PX: f64 = 0.6;
/// 調べる直線の候補の上限。
pub const MAX_PEAKS: usize = 400;
/// 返す線分の上限（長い順）。
pub const MAX_SEGMENTS: usize = 200;
/// 垂直・水平の線とみなす向きの幅（度。**仮置き**: 遠近で傾いた垂直線を拾い、斜めの線を除く）。
pub const GROUP_HALF_WIDTH_DEG: f64 = 30.0;
/// 当てはめの Cauchy の損失の尺度（度）。
pub const LOSS_SCALE_DEG: f64 = 1.5;
/// 外れ値とみなす残差の下限（度）。
pub const OUTLIER_MIN_DEG: f64 = 1.5;
/// 外れ値を除いて当てはめ直す回数。
pub const OUTLIER_ROUNDS: usize = 2;
/// 向きの組ごとの線分の長さの合計の下限（対角線の半分に対する倍率。**仮置き**）。
pub const MIN_GROUP_SUPPORT: f64 = 0.25;
/// 向きの組ごとの線分の本数の下限。
pub const MIN_GROUP_LINES: usize = 2;
/// 解析できる画像の最小の寸法（px）。
pub const MIN_IMAGE_SIZE: u32 = 64;
/// 角度補正の上限（度。genzo-model の `ranges::ANGLE_DEG`）。
const ANGLE_LIMIT_DEG: f64 = 45.0;
/// Nelder–Mead 法の繰り返しの上限。
const NM_MAX_ITER: usize = 600;

/// 自動の補正の種類（Lightroom Classic の Upright の「水平」「垂直」「完全」に相当）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UprightMode {
    /// 角度補正だけ（水平・垂直の線が傾かないように回す）。
    Level,
    /// 角度補正と垂直方向の遠近補正（垂直の線を平行にする）。
    Vertical,
    /// 角度補正と垂直・水平の遠近補正。
    Full,
}

/// 検出した線分（D の画素の座標。[`detect_lines`] は解析の画像の座標で返し、呼ぶ側が尺度を直す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineSegment {
    /// 端点。
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl LineSegment {
    /// 長さ。
    pub fn length(&self) -> f64 {
        (self.x1 - self.x0).hypot(self.y1 - self.y0)
    }

    /// 向き（度。0〜180 未満。x 軸から y が下向きの画面の座標で測る）。
    pub fn angle_deg(&self) -> f64 {
        let a = (self.y1 - self.y0).atan2(self.x1 - self.x0).to_degrees();
        a.rem_euclid(180.0)
    }

    /// 座標を倍率 `(kx, ky)` で直したもの。
    pub fn scaled(&self, kx: f64, ky: f64) -> LineSegment {
        LineSegment {
            x0: self.x0 * kx,
            y0: self.y0 * ky,
            x1: self.x1 * kx,
            y1: self.y1 * ky,
        }
    }
}

/// 自動の補正の結果（設定に書く値）。
#[derive(Debug, Clone, PartialEq)]
pub struct UprightAnalysis {
    /// 種類。
    pub mode: UprightMode,
    /// 角度補正（度。±45。genzo-model の `Geometry::angle_deg`）。
    pub angle_deg: f32,
    /// 垂直方向の遠近補正（−100〜+100。`Transform::vertical`）。水平モードでは 0。
    pub vertical: f32,
    /// 水平方向の遠近補正（−100〜+100。`Transform::horizontal`）。完全モード以外では 0。
    pub horizontal: f32,
    /// 使った垂直の線の数（外れ値を除いた後）。
    pub vertical_lines: usize,
    /// 使った水平の線の数（外れ値を除いた後）。
    pub horizontal_lines: usize,
    /// 当てはめの後の残差（度。長さで重みを付けた 2 乗平均平方根）。
    pub residual_deg: f32,
    /// 検出した線分（D の画素の座標。画面に重ねて確かめるためのもの）。
    pub segments: Vec<LineSegment>,
}

// ---------------------------------------------------------------------------
// 線分の検出
// ---------------------------------------------------------------------------

/// 5 タップのガウス（[1, 4, 6, 4, 1] / 16）で横・縦にぼかす（端は鏡映）。
fn blur5(image: &GrayImage) -> GrayImage {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let k = [
        1.0f32 / 16.0,
        4.0 / 16.0,
        6.0 / 16.0,
        4.0 / 16.0,
        1.0 / 16.0,
    ];
    let src = image.data();
    let reflect = |i: i64, n: usize| -> usize {
        let n = n as i64;
        let mut i = i;
        if n == 1 {
            return 0;
        }
        while i < 0 || i >= n {
            if i < 0 {
                i = -i;
            }
            if i >= n {
                i = 2 * (n - 1) - i;
            }
        }
        i as usize
    };
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        for (x, out) in row.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (j, kj) in k.iter().enumerate() {
                acc += kj * s[reflect(x as i64 + j as i64 - 2, w)];
            }
            *out = acc;
        }
    });
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (j, kj) in k.iter().enumerate() {
                acc += kj * tmp[reflect(y as i64 + j as i64 - 2, h) * w + x];
            }
            *o = acc;
        }
    });
    GrayImage::from_vec(image.width(), image.height(), out).expect("寸法は入力と同じ")
}

/// エッジの画素（解析の画像の座標と、法線の向き（ラジアン。0〜π））。
#[derive(Debug, Clone, Copy)]
struct EdgePixel {
    x: f32,
    y: f32,
    theta: f32,
}

/// 輝度の画像からエッジの画素を求める（モジュールの doc の 1）。
fn detect_edges(luma: &GrayImage) -> Vec<EdgePixel> {
    let (w, h) = (luma.width() as usize, luma.height() as usize);
    if w < 3 || h < 3 {
        return Vec::new();
    }
    let log = GrayImage::from_vec(
        luma.width(),
        luma.height(),
        luma.data().par_iter().map(|&v| safe_log2(v)).collect(),
    )
    .expect("寸法は入力と同じ");
    let smooth = blur5(&log);
    let s = smooth.data();
    let at = |x: usize, y: usize| s[y * w + x];
    // Sobel。
    let mut gx = vec![0.0f32; w * h];
    let mut gy = vec![0.0f32; w * h];
    gx.par_chunks_mut(w)
        .zip(gy.par_chunks_mut(w))
        .enumerate()
        .for_each(|(y, (rx, ry))| {
            if y == 0 || y + 1 >= h {
                return;
            }
            for x in 1..w - 1 {
                let (a, b, c) = (at(x - 1, y - 1), at(x, y - 1), at(x + 1, y - 1));
                let (d, f) = (at(x - 1, y), at(x + 1, y));
                let (g, hh, i) = (at(x - 1, y + 1), at(x, y + 1), at(x + 1, y + 1));
                rx[x] = (c + 2.0 * f + i) - (a + 2.0 * d + g);
                ry[x] = (g + 2.0 * hh + i) - (a + 2.0 * b + c);
            }
        });
    let mag: Vec<f32> = gx
        .par_iter()
        .zip(gy.par_iter())
        .map(|(&a, &b)| a.hypot(b))
        .collect();
    let interior = ((w - 2) * (h - 2)) as f64;
    let mean = mag.iter().map(|&m| f64::from(m)).sum::<f64>() / interior.max(1.0);
    let threshold = (mean as f32 * EDGE_THRESHOLD_FACTOR).max(1e-6);
    // 勾配の向きで極大の画素だけ（細線化）。
    let rows: Vec<Vec<EdgePixel>> = (1..h - 1)
        .into_par_iter()
        .map(|y| {
            let mut out = Vec::new();
            for x in 1..w - 1 {
                let m = mag[y * w + x];
                if m < threshold {
                    continue;
                }
                let (dx, dy) = (gx[y * w + x], gy[y * w + x]);
                // 勾配の向きを 4 方向に丸める。
                let ang = dy.atan2(dx).rem_euclid(std::f32::consts::PI);
                let sector = ((ang / (std::f32::consts::PI / 4.0)).round() as i32).rem_euclid(4);
                let (ox, oy) = match sector {
                    0 => (1i64, 0i64),
                    1 => (1, 1),
                    2 => (0, 1),
                    _ => (-1, 1),
                };
                let n1 = mag[((y as i64 + oy) as usize) * w + (x as i64 + ox) as usize];
                let n2 = mag[((y as i64 - oy) as usize) * w + (x as i64 - ox) as usize];
                if m >= n1 && m >= n2 {
                    out.push(EdgePixel {
                        x: x as f32,
                        y: y as f32,
                        theta: ang,
                    });
                }
            }
            out
        })
        .collect();
    rows.into_iter().flatten().collect()
}

/// 主成分で直線を当てはめた線分（点の重心と向き、向きに沿った端）と、直線からの点のずれの 2 乗平均平方根（px）。
fn fit_segment(points: &[(f32, f32)]) -> Option<(LineSegment, f64)> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let (mx, my) = points.iter().fold((0.0, 0.0), |a, p| {
        (a.0 + f64::from(p.0) / n, a.1 + f64::from(p.1) / n)
    });
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in points {
        let (dx, dy) = (f64::from(p.0) - mx, f64::from(p.1) - my);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (dx, dy) = (ang.cos(), ang.sin());
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut perp2 = 0.0;
    for p in points {
        let (px, py) = (f64::from(p.0) - mx, f64::from(p.1) - my);
        let t = px * dx + py * dy;
        let d = -px * dy + py * dx;
        perp2 += d * d;
        lo = lo.min(t);
        hi = hi.max(t);
    }
    if hi <= lo || !(hi - lo).is_finite() {
        return None;
    }
    Some((
        LineSegment {
            x0: mx + dx * lo,
            y0: my + dy * lo,
            x1: mx + dx * hi,
            y1: my + dy * hi,
        },
        (perp2 / n).sqrt(),
    ))
}

/// 輝度の画像（D の枠を一定の倍率で縮小したもの。長辺は [`ANALYSIS_LONG_EDGE`] 程度）から線分を検出する
/// （モジュールの doc の 1〜3）。座標は `luma` の画素（画素の中心が +0.5 ではなく、画素の番号そのもの）。
/// 長い順に [`MAX_SEGMENTS`] 本まで。
pub fn detect_lines(luma: &GrayImage) -> Result<Vec<LineSegment>> {
    let (w, h) = (luma.width(), luma.height());
    if w < MIN_IMAGE_SIZE || h < MIN_IMAGE_SIZE {
        return Err(PipelineError::InvalidArgument(format!(
            "画像が小さすぎて直線を検出できません（{w} × {h}）"
        )));
    }
    let edges = detect_edges(luma);
    let long = f64::from(w.max(h));
    let (cx, cy) = (f64::from(w) * 0.5, f64::from(h) * 0.5);
    let half_diag = 0.5 * f64::from(w).hypot(f64::from(h));
    let n_theta = (180.0 / THETA_STEP_DEG).round() as usize;
    let rho_off = half_diag.ceil() as i64 + 1;
    let n_rho = (2 * rho_off + 1) as usize;
    let theta_of = |b: usize| (b as f64 + 0.5) * THETA_STEP_DEG.to_radians();
    // 2. 勾配の向きつきの Hough 変換（θ の隣の区間にも投票する。θ は 0 ↔ π で折り返す。ρ は投票先の区間の θ で
    //    計算するので、折り返しても符号を変えない（区間の最初と最後の θ は π 近く違い、同じ直線の ρ は符号が逆になる
    //    が、それは投票先の θ で計算した値にすでに入っている））。
    let mut votes = vec![0u32; n_theta * n_rho];
    let mut by_theta: Vec<Vec<usize>> = vec![Vec::new(); n_theta];
    for (i, e) in edges.iter().enumerate() {
        let b = ((f64::from(e.theta) / THETA_STEP_DEG.to_radians()) as i64)
            .rem_euclid(n_theta as i64) as usize;
        by_theta[b].push(i);
        for db in -1i64..=1 {
            let bb = (b as i64 + db).rem_euclid(n_theta as i64);
            let th = theta_of(bb as usize);
            let rho = (f64::from(e.x) - cx) * th.cos() + (f64::from(e.y) - cy) * th.sin();
            let r = rho.round() as i64 + rho_off;
            if (0..n_rho as i64).contains(&r) {
                votes[bb as usize * n_rho + r as usize] += 1;
            }
        }
    }
    let min_votes = ((long * MIN_VOTES_FRACTION).round() as u32).max(8);
    // 極大（θ の隣は ρ の符号の反転を含めて見る）。
    let vote_at = |b: i64, r: i64| -> u32 {
        let (mut b, mut r) = (b, r - rho_off);
        if b < 0 {
            b += n_theta as i64;
            r = -r;
        } else if b >= n_theta as i64 {
            b -= n_theta as i64;
            r = -r;
        }
        let r = r + rho_off;
        if (0..n_rho as i64).contains(&r) {
            votes[b as usize * n_rho + r as usize]
        } else {
            0
        }
    };
    let mut peaks: Vec<(u32, usize, i64)> = Vec::new();
    for b in 0..n_theta {
        for r in 0..n_rho {
            let v = votes[b * n_rho + r];
            if v < min_votes {
                continue;
            }
            let mut is_max = true;
            'outer: for db in -1i64..=1 {
                for dr in -1i64..=1 {
                    if db == 0 && dr == 0 {
                        continue;
                    }
                    let u = vote_at(b as i64 + db, r as i64 + dr);
                    // 同じ票数は番号の小さい方を残す。
                    if u > v || (u == v && (db < 0 || (db == 0 && dr < 0))) {
                        is_max = false;
                        break 'outer;
                    }
                }
            }
            if is_max {
                peaks.push((v, b, r as i64 - rho_off));
            }
        }
    }
    peaks.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    peaks.truncate(MAX_PEAKS);
    // 3. 候補ごとに線分を取り出す。
    let support_bins = (SUPPORT_THETA_DEG / THETA_STEP_DEG).ceil() as i64;
    let min_len = long * MIN_SEGMENT_FRACTION;
    let mut used = vec![false; edges.len()];
    let mut segments: Vec<LineSegment> = Vec::new();
    for &(_, b, rho_bin) in &peaks {
        let th = theta_of(b);
        let (c, s) = (th.cos(), th.sin());
        let rho_p = rho_bin as f64;
        let mut members: Vec<(f64, usize)> = Vec::new();
        for db in -support_bins..=support_bins {
            // 区間の折り返しをまたいでも、距離は候補の区間の θ（c, s）で測るので符号を変えない。
            let bb = (b as i64 + db).rem_euclid(n_theta as i64);
            for &i in &by_theta[bb as usize] {
                if used[i] {
                    continue;
                }
                let e = &edges[i];
                let (dx, dy) = (f64::from(e.x) - cx, f64::from(e.y) - cy);
                let rho = dx * c + dy * s;
                if (rho - rho_p).abs() > SUPPORT_RHO_PX {
                    continue;
                }
                let t = -dx * s + dy * c;
                members.push((t, i));
            }
        }
        if members.len() < 2 {
            continue;
        }
        members.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        // すき間が MAX_GAP_PX 以下の最も長い連なり。
        let (mut best_lo, mut best_hi, mut best_len) = (0usize, 0usize, 0.0f64);
        let mut lo = 0usize;
        for i in 1..=members.len() {
            let broken = i == members.len() || members[i].0 - members[i - 1].0 > MAX_GAP_PX;
            if broken {
                let len = members[i - 1].0 - members[lo].0;
                if len > best_len {
                    best_len = len;
                    best_lo = lo;
                    best_hi = i;
                }
                lo = i;
            }
        }
        if best_len < min_len {
            continue;
        }
        let run = &members[best_lo..best_hi];
        let points: Vec<(f32, f32)> = run.iter().map(|&(_, i)| (edges[i].x, edges[i].y)).collect();
        let Some((seg, rms)) = fit_segment(&points) else {
            continue;
        };
        if seg.length() < min_len || rms > MAX_FIT_RMS_PX {
            continue;
        }
        for &(_, i) in run {
            used[i] = true;
        }
        segments.push(seg);
    }
    segments.sort_by(|a, b| b.length().total_cmp(&a.length()));
    segments.truncate(MAX_SEGMENTS);
    Ok(segments)
}

// ---------------------------------------------------------------------------
// 当てはめ
// ---------------------------------------------------------------------------

/// 向きの組。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Vertical,
    Horizontal,
}

/// 当てはめに使う線分（中心を原点・対角線の半分を 1 とした座標）。
#[derive(Debug, Clone, Copy)]
struct Observation {
    p0: (f64, f64),
    p1: (f64, f64),
    weight: f64,
    group: Group,
}

/// 角度の差を (−90, 90] 度に折り返す。
fn wrap_deg(d: f64) -> f64 {
    let mut d = d.rem_euclid(180.0);
    if d > 90.0 {
        d -= 180.0;
    }
    d
}

/// 線分の向きの、組の目標（垂直か水平）からのずれ（度）。
fn deviation_deg(angle_deg: f64, group: Group) -> f64 {
    match group {
        Group::Vertical => wrap_deg(angle_deg - 90.0),
        Group::Horizontal => wrap_deg(angle_deg),
    }
}

/// 当てはめのパラメータ（度）: 角度補正 θ、x 軸のまわりの傾き φ、y 軸のまわりの傾き ψ。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fit {
    theta: f64,
    phi: f64,
    psi: f64,
}

impl Fit {
    /// 線分を変換した後の向きのずれ（度）。
    fn residual(&self, o: &Observation) -> f64 {
        let rot = tilt_rotation(self.phi, self.psi);
        let (c, s) = self.theta.to_radians().sin_cos();
        let (c, s) = (s, c);
        let map = |p: (f64, f64)| -> (f64, f64) {
            let (x, y) = rot.apply(p.0, p.1);
            // 画面の座標の回転（coords の G = R·d）。
            (c * x + s * y, -s * x + c * y)
        };
        let a = map(o.p0);
        let b = map(o.p1);
        let ang = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
        deviation_deg(ang, o.group)
    }

    /// 損失（長さで重みを付けた Cauchy の損失の和。範囲の外には罰を足す）。
    fn loss(&self, obs: &[Observation]) -> f64 {
        let mut sum = 0.0;
        for o in obs {
            let d = self.residual(o) / LOSS_SCALE_DEG;
            sum += o.weight * (1.0 + d * d).ln();
        }
        let over_tilt = (self.phi.hypot(self.psi) - TILT_TOTAL_MAX_DEG).max(0.0);
        let over_angle = (self.theta.abs() - ANGLE_LIMIT_DEG).max(0.0);
        sum + 1e3 * (over_tilt * over_tilt + over_angle * over_angle)
    }
}

/// Nelder–Mead 法（決定的）。`free` は動かすパラメータ（θ・φ・ψ の順）、`init` は初期値。
fn nelder_mead(init: Fit, free: [bool; 3], obs: &[Observation]) -> Fit {
    let idx: Vec<usize> = (0..3).filter(|&i| free[i]).collect();
    let n = idx.len();
    if n == 0 {
        return init;
    }
    let to_fit = |x: &[f64]| -> Fit {
        let mut f = init;
        for (k, &i) in idx.iter().enumerate() {
            match i {
                0 => f.theta = x[k],
                1 => f.phi = x[k],
                _ => f.psi = x[k],
            }
        }
        f
    };
    let value = |x: &[f64]| to_fit(x).loss(obs);
    let x0: Vec<f64> = idx
        .iter()
        .map(|&i| match i {
            0 => init.theta,
            1 => init.phi,
            _ => init.psi,
        })
        .collect();
    let step = |i: usize| if i == 0 { 1.0 } else { 5.0 };
    let mut simplex: Vec<(Vec<f64>, f64)> = Vec::with_capacity(n + 1);
    simplex.push((x0.clone(), value(&x0)));
    for k in 0..n {
        let mut x = x0.clone();
        x[k] += step(idx[k]);
        let v = value(&x);
        simplex.push((x, v));
    }
    for _ in 0..NM_MAX_ITER {
        simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
        let best = &simplex[0];
        let worst = &simplex[n];
        let spread = simplex
            .iter()
            .map(|(x, _)| {
                x.iter()
                    .zip(&best.0)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f64::max)
            })
            .fold(0.0, f64::max);
        if spread < 1e-5 && (worst.1 - best.1).abs() < 1e-12 {
            break;
        }
        // 重心（最悪の点を除く）。
        let mut centroid = vec![0.0; n];
        for (x, _) in &simplex[..n] {
            for (c, v) in centroid.iter_mut().zip(x) {
                *c += v / n as f64;
            }
        }
        let worst_x = simplex[n].0.clone();
        let worst_v = simplex[n].1;
        let point = |t: f64| -> Vec<f64> {
            centroid
                .iter()
                .zip(&worst_x)
                .map(|(c, w)| c + t * (c - w))
                .collect()
        };
        let xr = point(1.0);
        let vr = value(&xr);
        if vr < simplex[0].1 {
            let xe = point(2.0);
            let ve = value(&xe);
            simplex[n] = if ve < vr { (xe, ve) } else { (xr, vr) };
        } else if vr < simplex[n - 1].1 {
            simplex[n] = (xr, vr);
        } else {
            let (xc, vc) = if vr < worst_v {
                let x = point(0.5);
                let v = value(&x);
                (x, v)
            } else {
                let x = point(-0.5);
                let v = value(&x);
                (x, v)
            };
            if vc < worst_v.min(vr) {
                simplex[n] = (xc, vc);
            } else {
                // 縮小。
                let best_x = simplex[0].0.clone();
                for entry in simplex.iter_mut().skip(1) {
                    let x: Vec<f64> = entry
                        .0
                        .iter()
                        .zip(&best_x)
                        .map(|(a, b)| b + 0.5 * (a - b))
                        .collect();
                    let v = value(&x);
                    *entry = (x, v);
                }
            }
        }
    }
    simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
    to_fit(&simplex[0].0)
}

/// 中央値（空なら 0）。
fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        0.5 * (values[n / 2 - 1] + values[n / 2])
    }
}

/// 線分（D の画素の座標。D の寸法は `oriented_size`）から、モード `mode` の補正の値を求める
/// （モジュールの doc の 4〜6）。線分が足りなければ [`PipelineError::InvalidArgument`]。
pub fn analyze(
    segments: &[LineSegment],
    oriented_size: (u32, u32),
    mode: UprightMode,
) -> Result<UprightAnalysis> {
    let (ow, oh) = (f64::from(oriented_size.0), f64::from(oriented_size.1));
    if !(ow > 0.0 && oh > 0.0) {
        return Err(PipelineError::InvalidDimensions {
            width: oriented_size.0,
            height: oriented_size.1,
        });
    }
    let (cx, cy) = (ow * 0.5, oh * 0.5);
    let r = 0.5 * ow.hypot(oh);
    // 4. 分類。
    let mut obs: Vec<Observation> = Vec::new();
    for s in segments {
        let ang = s.angle_deg();
        let group = if deviation_deg(ang, Group::Vertical).abs() <= GROUP_HALF_WIDTH_DEG {
            Group::Vertical
        } else if deviation_deg(ang, Group::Horizontal).abs() <= GROUP_HALF_WIDTH_DEG {
            Group::Horizontal
        } else {
            continue;
        };
        let weight = s.length() / r;
        if !(weight.is_finite() && weight > 0.0) {
            continue;
        }
        obs.push(Observation {
            p0: ((s.x0 - cx) / r, (s.y0 - cy) / r),
            p1: ((s.x1 - cx) / r, (s.y1 - cy) / r),
            weight,
            group,
        });
    }
    let support = |obs: &[Observation], g: Group| -> (usize, f64) {
        obs.iter()
            .filter(|o| o.group == g)
            .fold((0usize, 0.0), |a, o| (a.0 + 1, a.1 + o.weight))
    };
    let enough = |obs: &[Observation], g: Group| -> bool {
        let (n, w) = support(obs, g);
        n >= MIN_GROUP_LINES && w >= MIN_GROUP_SUPPORT
    };
    let has_v = enough(&obs, Group::Vertical);
    let has_h = enough(&obs, Group::Horizontal);
    let need = match mode {
        UprightMode::Level => has_v || has_h,
        UprightMode::Vertical | UprightMode::Full => has_v,
    };
    if !need {
        return Err(PipelineError::InvalidArgument(match mode {
            UprightMode::Level => {
                "水平・垂直の直線が見つからないため、自動で補正できません".to_owned()
            }
            UprightMode::Vertical | UprightMode::Full => {
                "垂直の直線が見つからないため、自動で遠近補正できません".to_owned()
            }
        }));
    }
    // 動かすパラメータ。
    let free = match mode {
        UprightMode::Level => [true, false, false],
        UprightMode::Vertical => [true, has_v, false],
        UprightMode::Full => [true, has_v, has_h],
    };
    // 使う線分: 水平モードは両方、垂直・完全は組が足りている方。
    let mut used: Vec<Observation> = obs
        .iter()
        .copied()
        .filter(|o| match o.group {
            Group::Vertical => has_v,
            Group::Horizontal => has_h,
        })
        .collect();
    // 初期値: 角度は長さで重みを付けたずれの平均の逆。
    let init_theta = {
        let (mut sum, mut wsum) = (0.0, 0.0);
        for o in &used {
            let ang = (o.p1.1 - o.p0.1).atan2(o.p1.0 - o.p0.0).to_degrees();
            sum += o.weight * deviation_deg(ang, o.group);
            wsum += o.weight;
        }
        if wsum > 0.0 { sum / wsum } else { 0.0 }
    };
    let mut fit = Fit {
        theta: init_theta.clamp(-ANGLE_LIMIT_DEG, ANGLE_LIMIT_DEG),
        phi: 0.0,
        psi: 0.0,
    };
    // 5. 当てはめと外れ値の除去。
    for round in 0..=OUTLIER_ROUNDS {
        fit = nelder_mead(fit, free, &used);
        if round == OUTLIER_ROUNDS {
            break;
        }
        let mut res: Vec<f64> = used.iter().map(|o| fit.residual(o).abs()).collect();
        let med = median(&mut res.clone());
        let cut = (3.0 * med).max(OUTLIER_MIN_DEG);
        let kept: Vec<Observation> = used
            .iter()
            .copied()
            .filter(|o| fit.residual(o).abs() <= cut)
            .collect();
        res.clear();
        // 組の支持が足りなくなるほどは除かない。
        let ok = |g: Group, needed: bool| !needed || enough(&kept, g);
        if kept.len() == used.len()
            || !(ok(Group::Vertical, free[1]) && ok(Group::Horizontal, free[2]))
        {
            break;
        }
        used = kept;
    }
    let (mut sq, mut wsum) = (0.0, 0.0);
    for o in &used {
        let d = fit.residual(o);
        sq += o.weight * d * d;
        wsum += o.weight;
    }
    let residual = if wsum > 0.0 { (sq / wsum).sqrt() } else { 0.0 };
    let (v_n, _) = support(&used, Group::Vertical);
    let (h_n, _) = support(&used, Group::Horizontal);
    // 6. 設定の値。
    let round_to = |v: f64, step: f64| (v / step).round() * step;
    let angle = round_to(fit.theta.clamp(-ANGLE_LIMIT_DEG, ANGLE_LIMIT_DEG), 0.01);
    let vertical = round_to((-fit.phi / TILT_MAX_DEG * 100.0).clamp(-100.0, 100.0), 0.1);
    let horizontal = round_to((fit.psi / TILT_MAX_DEG * 100.0).clamp(-100.0, 100.0), 0.1);
    let zero = |v: f64| if v == 0.0 { 0.0 } else { v };
    Ok(UprightAnalysis {
        mode,
        angle_deg: zero(angle) as f32,
        vertical: zero(vertical) as f32,
        horizontal: zero(horizontal) as f32,
        vertical_lines: v_n,
        horizontal_lines: h_n,
        residual_deg: residual as f32,
        segments: segments.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use genzo_model::Transform;

    use super::*;
    use crate::perspective::Perspective;

    /// 格子（垂直・水平の線）の画像を、変形 `t` の **逆**（傾いたカメラで撮った状態）で描く。線は暗く、地は明るい。
    fn tilted_grid(w: u32, h: u32, t: &Transform) -> GrayImage {
        let p = Perspective::from_transform((w, h), t).unwrap();
        // 画素 (x, y) の値: 補正後の座標 d_to_p に写した先が格子の線の上なら暗い。
        // つまりこの画像は「補正すると格子になる」= 傾いて写った格子。
        let spacing = f64::from(w.min(h)) / 6.0;
        GrayImage::from_fn(w, h, |x, y| {
            let (px, py) = p.d_to_p.apply(f64::from(x) + 0.5, f64::from(y) + 0.5);
            let on_v = ((px / spacing).round() * spacing - px).abs() < 1.2;
            let on_h = ((py / spacing).round() * spacing - py).abs() < 1.2;
            if on_v || on_h { 0.02 } else { 0.5 }
        })
        .unwrap()
    }

    #[test]
    fn detects_the_lines_of_an_axis_aligned_grid() {
        let img = tilted_grid(400, 300, &Transform::default());
        let lines = detect_lines(&img).unwrap();
        assert!(lines.len() >= 8, "{}", lines.len());
        for l in &lines {
            let d = deviation_deg(l.angle_deg(), Group::Vertical)
                .abs()
                .min(deviation_deg(l.angle_deg(), Group::Horizontal).abs());
            assert!(d < 0.5, "{l:?} {d}");
        }
        let a = analyze(&lines, (400, 300), UprightMode::Full).unwrap();
        assert!(a.angle_deg.abs() < 0.2, "{a:?}");
        assert!(a.vertical.abs() < 2.0 && a.horizontal.abs() < 2.0, "{a:?}");
    }

    #[test]
    fn level_recovers_a_rotation() {
        // 回転した格子: 変形の逆で描けないので、座標を回して描く。
        let (w, h) = (400u32, 300u32);
        let deg = 3.0f64;
        let (s, c) = deg.to_radians().sin_cos();
        let spacing = 50.0;
        let img = GrayImage::from_fn(w, h, |x, y| {
            let (dx, dy) = (f64::from(x) + 0.5 - 200.0, f64::from(y) + 0.5 - 150.0);
            // 画面の座標で反時計回りに deg 回した格子（補正は −deg … ではなく、coords の向きで +deg）。
            let (px, py) = (c * dx + s * dy, -s * dx + c * dy);
            let on_v = ((px / spacing).round() * spacing - px).abs() < 1.2;
            let on_h = ((py / spacing).round() * spacing - py).abs() < 1.2;
            if on_v || on_h { 0.02 } else { 0.5 }
        })
        .unwrap();
        let lines = detect_lines(&img).unwrap();
        let a = analyze(&lines, (w, h), UprightMode::Level).unwrap();
        assert_eq!((a.vertical, a.horizontal), (0.0, 0.0));
        assert!((f64::from(a.angle_deg) - deg).abs() < 0.3, "{a:?}");
    }

    #[test]
    fn vertical_and_full_recover_the_tilt() {
        for (v, hz, mode) in [
            (-30.0f32, 0.0f32, UprightMode::Vertical),
            (25.0, 0.0, UprightMode::Vertical),
            (-30.0, 20.0, UprightMode::Full),
        ] {
            // 傾いて写った格子を作る: 設定 t で「補正すると格子になる」画像。解析は −t ではなく t を返すべき。
            let t = Transform {
                vertical: v,
                horizontal: hz,
                ..Default::default()
            };
            let img = tilted_grid(480, 360, &t);
            let lines = detect_lines(&img).unwrap();
            let a = analyze(&lines, (480, 360), mode).unwrap();
            assert!((a.vertical - v).abs() < 4.0, "{mode:?} {v} {hz}: {a:?}");
            if mode == UprightMode::Full {
                assert!((a.horizontal - hz).abs() < 4.0, "{a:?}");
            } else {
                assert_eq!(a.horizontal, 0.0);
            }
            assert!(a.angle_deg.abs() < 0.5, "{a:?}");
            assert!(a.residual_deg < 1.0, "{a:?}");
            assert!(a.vertical_lines >= 2);
        }
    }

    /// 垂直の線の勾配の向きは 0 と π の境目にある。わずかに傾けて、縁の画素の向きが境目の両側（区間の最初と最後）に
    /// 分かれても、同じ直線として検出する（境目をまたぐ投票と画素の集め方。数学関数の丸めが環境で違っても同じ結果に
    /// なるように）。
    #[test]
    fn vertical_lines_across_the_angle_wrap_are_detected() {
        let (w, h) = (400u32, 300u32);
        for deg in [-0.2f64, -0.05, 0.05, 0.2] {
            let (s, c) = deg.to_radians().sin_cos();
            let spacing = 50.0;
            let img = GrayImage::from_fn(w, h, |x, y| {
                let (dx, dy) = (f64::from(x) + 0.5 - 200.0, f64::from(y) + 0.5 - 150.0);
                let px = c * dx + s * dy;
                let on_v = ((px / spacing).round() * spacing - px).abs() < 1.2;
                if on_v { 0.02 } else { 0.5 }
            })
            .unwrap();
            let lines = detect_lines(&img).unwrap();
            let vertical: Vec<_> = lines
                .iter()
                .filter(|l| deviation_deg(l.angle_deg(), Group::Vertical).abs() < 1.0)
                .collect();
            // 線は 7 本（x = 200 ± 50k）。縁は線ごとに 2 本なので、少なくとも線の数だけは長い線分になる。
            assert!(vertical.len() >= 7, "{deg}: {}", vertical.len());
            // 画像の端（x = 0 の線）の近くを除けば、線は画像の高さの大部分にわたってつながる。
            for l in vertical
                .iter()
                .filter(|l| l.x0.min(l.x1) > 5.0 && l.x0.max(l.x1) < f64::from(w) - 5.0)
            {
                assert!(l.length() > 250.0, "{deg}: {l:?}");
            }
            let a = analyze(&lines, (w, h), UprightMode::Vertical).unwrap();
            assert!((f64::from(a.angle_deg) - deg).abs() < 0.1, "{deg}: {a:?}");
        }
    }

    #[test]
    fn flat_images_have_no_lines() {
        let img = GrayImage::filled(300, 200, 0.3).unwrap();
        let lines = detect_lines(&img).unwrap();
        assert!(lines.is_empty());
        let err = analyze(&lines, (300, 200), UprightMode::Vertical).unwrap_err();
        assert!(matches!(err, PipelineError::InvalidArgument(_)));
        // 小さすぎる画像。
        assert!(detect_lines(&GrayImage::filled(10, 10, 0.3).unwrap()).is_err());
    }

    #[test]
    fn wrap_and_deviation() {
        assert_eq!(wrap_deg(0.0), 0.0);
        assert_eq!(wrap_deg(179.0), -1.0);
        assert_eq!(wrap_deg(-91.0), 89.0);
        assert_eq!(deviation_deg(92.0, Group::Vertical), 2.0);
        assert_eq!(deviation_deg(88.0, Group::Vertical), -2.0);
        assert_eq!(deviation_deg(177.0, Group::Horizontal), -3.0);
    }
}
