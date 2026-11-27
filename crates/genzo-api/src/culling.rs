//! 選別支援（ORG-06）: 似た写真のグループと、その中でピントの甘い（ぼけ・ブレの見込みの）写真を見つける。
//!
//! AI を使わない手法で始める（後から置き換えられるよう、判定はこのモジュールに閉じる）。**何も変えない**
//! （評価・フラグ・スタックは変えない。結果を見て、利用者が選ぶ・スタックにまとめる）。
//!
//! # 方式（どれも **仮置き**。合成の画像で決めた。実写で見直す）
//!
//! - 入力は、カタログのサムネイル（L0。長辺 320 の JPEG。元のファイルは開かない）。サムネイルのない写真・動画は
//!   飛ばす。**サムネイルの解像度での判定なので粗い**: 大きなぼけ・ブレは分かるが、等倍で分かる程度の僅かな
//!   ピントのずれは分からない。
//! - **似た写真**: サムネイルの輝度を 9 × 8 に縮めて、横に隣り合う画素の大小を 64 ビットにしたもの（dHash）の
//!   違うビットの数（ハミング距離）が [`CullingOptions::max_distance`] 以下で、撮影日時の差が
//!   [`CullingOptions::max_gap_seconds`] 以下の、撮影日時の順に続く写真をグループにする（1 つ前の写真と比べる）。
//!   撮影日時のない写真はグループにしない。
//! - **シャープさ**: サムネイルの輝度のラプラシアン（4 近傍）の絶対値の平均。構図が違う写真の間では比べられない
//!   （空の多い写真は小さくなる）ので、**同じグループの中でだけ比べる**: いちばん大きい写真を「いちばんシャープ」
//!   とし、その [`CullingOptions::soft_ratio`] 倍に満たない写真を「甘い見込み」にする。
//! - サムネイルは、このアプリのワーカーが作った JPEG なので、本体で読む。

use std::collections::HashSet;

use genzo_media::{DynRgbImage, decode_image_bytes};
use genzo_model::{AssetKind, VariantId};

use crate::core::Core;
use crate::error::ApiError;
use crate::jobs::{JobCtx, spawn_job};
use crate::types::{CullingGroup, CullingOptions, CullingReport, JobKind, JobResult};

/// 1 度に調べる写真の数の上限（**仮置き**: 20,000）。
pub const MAX_CULLING_VARIANTS: usize = 20_000;

/// 似た写真とみなす dHash の違いの上限として指定できる範囲（ビット）。
pub const MAX_CULLING_DISTANCE: u32 = 32;

/// 撮影日時の差の上限として指定できる範囲（秒）。
pub const MAX_CULLING_GAP_SECONDS: u32 = 3600;

/// 1 枚の特徴。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Features {
    /// dHash（9 × 8 の輝度の、横の大小）。
    pub hash: u64,
    /// シャープさ（輝度のラプラシアンの絶対値の平均。0〜255 の輝度で）。
    pub sharpness: f32,
}

/// RGB（8bit。R, G, B, … の並び）の輝度（Rec. 709 の重み。0〜255）。
fn luminance(rgb: &[u8]) -> Vec<f32> {
    rgb.chunks_exact(3)
        .map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
        .collect()
}

/// 輝度の画像（`w` × `h`）から特徴を求める。小さすぎる画像（3 × 3 未満）は `None`。
pub(crate) fn features_of(lum: &[f32], w: usize, h: usize) -> Option<Features> {
    if w < 3 || h < 3 || lum.len() != w * h {
        return None;
    }
    // dHash: 9 × 8 の面積平均。
    let (gw, gh) = (9usize, 8usize);
    let mut grid = [0f32; 72];
    for gy in 0..gh {
        let (y0, y1) = (gy * h / gh, ((gy + 1) * h / gh).max(gy * h / gh + 1));
        for gx in 0..gw {
            let (x0, x1) = (gx * w / gw, ((gx + 1) * w / gw).max(gx * w / gw + 1));
            let mut sum = 0f32;
            for y in y0..y1.min(h) {
                sum += lum[y * w + x0..y * w + x1.min(w)].iter().sum::<f32>();
            }
            let n = (y1.min(h) - y0) * (x1.min(w) - x0);
            grid[gy * gw + gx] = sum / n.max(1) as f32;
        }
    }
    let mut hash = 0u64;
    for gy in 0..gh {
        for gx in 0..gw - 1 {
            hash = (hash << 1) | u64::from(grid[gy * gw + gx] > grid[gy * gw + gx + 1]);
        }
    }
    // シャープさ: 内側の画素の 4 近傍のラプラシアンの絶対値の平均。
    let mut sum = 0f64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = lum[y * w + x];
            let l = 4.0 * c
                - lum[y * w + x - 1]
                - lum[y * w + x + 1]
                - lum[(y - 1) * w + x]
                - lum[(y + 1) * w + x];
            sum += f64::from(l.abs());
        }
    }
    Some(Features {
        hash,
        sharpness: (sum / ((w - 2) * (h - 2)) as f64) as f32,
    })
}

/// 撮影日時の順に並べた写真（撮影日時（UTC のミリ秒）と特徴）を、似た写真のグループにする（2 枚以上のものだけ）。
/// 戻り値は、グループごとの `items` の添字（撮影日時の順）。
pub(crate) fn group_similar(
    items: &[(i64, Features)],
    max_gap_ms: i64,
    max_distance: u32,
) -> Vec<Vec<usize>> {
    let mut groups = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for i in 0..items.len() {
        let joins = current.last().is_some_and(|&p| {
            items[i].0 - items[p].0 <= max_gap_ms
                && (items[i].1.hash ^ items[p].1.hash).count_ones() <= max_distance
        });
        if !joins {
            if current.len() >= 2 {
                groups.push(std::mem::take(&mut current));
            }
            current.clear();
        }
        current.push(i);
    }
    if current.len() >= 2 {
        groups.push(current);
    }
    groups
}

fn check_options(o: &CullingOptions) -> Result<(), ApiError> {
    if o.max_distance > MAX_CULLING_DISTANCE {
        return Err(ApiError::InvalidArgument(format!(
            "似た写真とみなす違いは 0〜{MAX_CULLING_DISTANCE} で指定してください"
        )));
    }
    if o.max_gap_seconds == 0 || o.max_gap_seconds > MAX_CULLING_GAP_SECONDS {
        return Err(ApiError::InvalidArgument(format!(
            "撮影日時の差は 1〜{MAX_CULLING_GAP_SECONDS} 秒で指定してください"
        )));
    }
    if !(o.soft_ratio > 0.0 && o.soft_ratio < 1.0) {
        return Err(ApiError::InvalidArgument(
            "甘いとみなす割合は 0 より大きく 1 より小さい値で指定してください".to_owned(),
        ));
    }
    Ok(())
}

fn run(
    ctx: &JobCtx<'_>,
    variant_ids: &[VariantId],
    options: &CullingOptions,
) -> Result<CullingReport, ApiError> {
    let inner = ctx.inner;
    let mut report = CullingReport {
        options: *options,
        ..CullingReport::default()
    };
    let total = variant_ids.len() as u64;
    ctx.progress(0, total);
    // (撮影日時, variant, 特徴)
    let mut analyzed: Vec<(i64, VariantId, Features)> = Vec::new();
    for (n, chunk) in variant_ids.chunks(200).enumerate() {
        if ctx.is_cancelled() {
            report.cancelled = true;
            return Ok(report);
        }
        let summaries = inner.with_catalog(|c| c.variant_summaries(chunk))?;
        report.missing += (chunk.len() - summaries.len()) as u64;
        for s in summaries {
            if s.kind == AssetKind::Video {
                report.skipped_videos += 1;
                continue;
            }
            let Some(utc) = s.capture.utc else {
                report.without_time += 1;
                continue;
            };
            let Some(thumb) = inner.with_cache(|c| c.thumbs.get(s.variant_id))? else {
                report.without_thumbnail += 1;
                continue;
            };
            let features = decode_image_bytes(&thumb.jpeg).ok().and_then(|d| {
                let rgb = match &d.pixels {
                    DynRgbImage::Rgb8(i) => i.clone(),
                    other => other.to_rgb8(),
                };
                let (w, h) = rgb.dimensions();
                features_of(&luminance(rgb.as_raw()), w as usize, h as usize)
            });
            match features {
                Some(f) => analyzed.push((utc.timestamp_millis(), s.variant_id, f)),
                None => report.without_thumbnail += 1,
            }
        }
        ctx.progress(((n + 1) * 200).min(variant_ids.len()) as u64, total);
    }
    report.analyzed = analyzed.len() as u64;
    // 撮影日時の順（同じ時刻は variant の順）。
    analyzed.sort_by_key(|(t, v, _)| (*t, *v));
    let items: Vec<(i64, Features)> = analyzed.iter().map(|(t, _, f)| (*t, *f)).collect();
    let groups = group_similar(
        &items,
        i64::from(options.max_gap_seconds) * 1000,
        options.max_distance,
    );
    for g in groups {
        let best_at = g
            .iter()
            .copied()
            .max_by(|a, b| {
                items[*a]
                    .1
                    .sharpness
                    .total_cmp(&items[*b].1.sharpness)
                    // 同じなら、先に撮った写真。
                    .then(b.cmp(a))
            })
            .expect("2 枚以上");
        let best = items[best_at].1.sharpness;
        let soft: Vec<VariantId> = g
            .iter()
            .filter(|&&i| i != best_at && items[i].1.sharpness < best * options.soft_ratio)
            .map(|&i| analyzed[i].1)
            .collect();
        report.grouped += g.len() as u64;
        report.soft += soft.len() as u64;
        report.groups.push(CullingGroup {
            variant_ids: g.iter().map(|&i| analyzed[i].1).collect(),
            sharpest: analyzed[best_at].1,
            soft,
        });
    }
    Ok(report)
}

impl Core {
    /// 選別支援（ORG-06）: 選んだ写真から、似た写真のグループと、その中でピントの甘い見込みの写真を見つける
    /// （P3 のジョブ。何も変えない。ジョブの ID を返す。結果は [`JobResult::CullingAssist`]。モジュールの doc）。
    ///
    /// エラー: 空・[`MAX_CULLING_VARIANTS`] を超える・設定が範囲外なら [`ApiError::InvalidArgument`]。
    pub fn culling_assist(
        &self,
        variant_ids: &[VariantId],
        options: &CullingOptions,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        check_options(options)?;
        let mut seen = HashSet::new();
        let ids: Vec<VariantId> = variant_ids
            .iter()
            .copied()
            .filter(|v| seen.insert(*v))
            .collect();
        if ids.len() < 2 {
            return Err(ApiError::InvalidArgument(
                "選別支援には、写真を 2 枚以上選んでください".to_owned(),
            ));
        }
        if ids.len() > MAX_CULLING_VARIANTS {
            return Err(ApiError::InvalidArgument(format!(
                "一度に調べられるのは {MAX_CULLING_VARIANTS} 枚までです（{} 枚が選ばれています）",
                ids.len()
            )));
        }
        let options = *options;
        Ok(spawn_job(
            inner,
            JobKind::CullingAssist,
            "選別支援",
            move |ctx| run(ctx, &ids, &options).map(JobResult::CullingAssist),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 縞模様（周期 `period` 画素）の輝度の画像。`blur` 回、横に 3 画素の平均をかける。
    fn stripes(w: usize, h: usize, period: usize, blur: usize) -> Vec<f32> {
        let mut img: Vec<f32> = (0..w * h)
            .map(|i| {
                if ((i % w) / period).is_multiple_of(2) {
                    40.0
                } else {
                    200.0
                }
            })
            .collect();
        for _ in 0..blur {
            let src = img.clone();
            for y in 0..h {
                for x in 1..w - 1 {
                    img[y * w + x] =
                        (src[y * w + x - 1] + src[y * w + x] + src[y * w + x + 1]) / 3.0;
                }
            }
        }
        img
    }

    #[test]
    fn blurred_images_keep_their_hash_but_lose_sharpness() {
        let (w, h) = (90, 64);
        let sharp = features_of(&stripes(w, h, 10, 0), w, h).unwrap();
        let soft = features_of(&stripes(w, h, 10, 6), w, h).unwrap();
        assert!(
            soft.sharpness < sharp.sharpness * 0.5,
            "{soft:?} / {sharp:?}"
        );
        assert!(
            (sharp.hash ^ soft.hash).count_ones() <= 4,
            "{sharp:?} / {soft:?}"
        );
        // 別の模様は、ハッシュが大きく違う。
        let other = features_of(&stripes(w, h, 23, 0), w, h).unwrap();
        assert!((sharp.hash ^ other.hash).count_ones() > 10, "{other:?}");
        // 一様な画像はシャープさ 0。小さすぎる画像は求めない。
        assert_eq!(
            features_of(&vec![128.0; w * h], w, h).unwrap().sharpness,
            0.0
        );
        assert_eq!(features_of(&[1.0; 4], 2, 2), None);
    }

    #[test]
    fn groups_are_runs_of_close_and_similar_photos() {
        let f = |hash: u64| Features {
            hash,
            sharpness: 1.0,
        };
        let items = [
            (0, f(0b0000)),
            (1_000, f(0b0001)),
            (2_000, f(0b0011)),
            // 時刻が離れている。
            (60_000, f(0b0011)),
            // 模様が違う。
            (61_000, f(u64::MAX)),
            (62_000, f(u64::MAX - 1)),
            // 1 枚だけのものはグループにしない。
            (200_000, f(0b0101)),
        ];
        assert_eq!(
            group_similar(&items, 5_000, 2),
            vec![vec![0, 1, 2], vec![4, 5]]
        );
        // 違いを許さなければ、同じハッシュのものだけ。
        assert!(group_similar(&items, 5_000, 0).is_empty());
        assert!(group_similar(&[], 5_000, 2).is_empty());
    }

    #[test]
    fn options_out_of_range_are_rejected() {
        assert!(check_options(&CullingOptions::default()).is_ok());
        for o in [
            CullingOptions {
                max_distance: MAX_CULLING_DISTANCE + 1,
                ..Default::default()
            },
            CullingOptions {
                max_gap_seconds: 0,
                ..Default::default()
            },
            CullingOptions {
                soft_ratio: 1.0,
                ..Default::default()
            },
            CullingOptions {
                soft_ratio: f32::NAN,
                ..Default::default()
            },
        ] {
            assert!(check_options(&o).is_err(), "{o:?}");
        }
    }
}
