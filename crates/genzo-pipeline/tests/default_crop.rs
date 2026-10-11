//! 既定の切り抜き（RAW の DefaultCrop。実機の ARW の不具合 A-2）の結合テスト。
//!
//! α7 IV の APS-C の ARW では、LibRaw の有効画素の右端 40 列が詰め物（2 列左の値の繰り返し）で、
//! 下から 3 行目の B が異常な値になっており、既定の切り抜きを使わない現像では出力の端に筋の帯と
//! 線が出た。ここでは同じ形の合成の RAW で、次を確かめる（docs/04_architecture.md の 2.7 節）。
//!
//! - 既定の切り抜きの外の詰め物・異常な行は、出力に影響しない（デモザイクとステージ 9 の近傍より
//!   離れている場合）。既定の切り抜きを使わないと影響する（不具合の再現）。
//! - 既定の切り抜きの範囲の出力は、切り抜きのない現像の同じ位置の画素と同じ（ステージ 9 は平行移動
//!   だけで、補間は恒等）。向きを反映した場合も同じ画素の並べ替え。
//! - 利用者の切り抜き・回転は既定の切り抜きの範囲を基準にする。

mod common;

use common::*;
use genzo_model::{CropRect, DevelopSettings, FileId, Orientation, RenderQuality};
use genzo_pipeline::{NeverCancel, PhotoSource, RegionRender, SourceId, SourceImage};
use genzo_raw::{PixelRect, RawImage};

/// 有効画素の寸法（偶数）と、詰め物の列・異常な行の数。
const W: u32 = 136;
const H: u32 = 92;
const PAD_COLS: u32 = 10;
/// 既定の切り抜き（詰め物の列から 16 列、異常な行から 21 行離す。実機の α7 IV では 44 列・27 行）。
///
/// 出力の 1 画素が依存する RAW の範囲は、RCD の余白 10 画素（`RCD_MARGIN`）＋ 飽和の切りそろえの
/// 半径 2 ＋ ステージ 9 のバイキュービックのタップ 2 画素なので、それより離す。幅は 4 の倍数（利用者の
/// 切り抜きの割合 0.25 が整数の画素になるように）。
const CROP: PixelRect = PixelRect::new(6, 4, 104, 64);

fn source(raw: RawImage, file_id: i64) -> PhotoSource {
    PhotoSource::new(
        SourceId::new(FileId::new(file_id), 1),
        SourceImage::Raw(std::sync::Arc::new(raw)),
    )
    .unwrap()
}

/// 実機の α7 IV の APS-C の ARW と同じ形の、壊れた端を持つ RAW（右端の詰め物の列と、下から 3 行目の
/// 異常な行）。`clean` は端を壊す前の RAW。
fn raws_with_bad_edges(orientation: Orientation) -> (RawImage, RawImage) {
    let mut clean = raw_oriented(&busy_scene(W, H), orientation);
    clean.default_crop = Some(CROP);
    let mut bad = clean.clone();
    let w = W as usize;
    for y in 0..H as usize {
        for x in (W - PAD_COLS) as usize..w {
            // 2 列左の値の繰り返し（実機の詰め物と同じ形）。
            bad.data[y * w + x] = bad.data[y * w + x - 2];
        }
    }
    // 下から 3 行目を明るくする（実機では B が 12% 高い。ここでは全体を 1.5 倍）。
    let y = (H - 3) as usize;
    for x in 0..w {
        let v = &mut bad.data[y * w + x];
        *v = ((f32::from(*v) - 512.0) * 1.5 + 512.0).min(16383.0) as u16;
    }
    clean.validate().unwrap();
    bad.validate().unwrap();
    (clean, bad)
}

fn render(src: &PhotoSource, s: &DevelopSettings) -> RegionRender {
    engine(64, 48)
        .render_full_resolution(src, s, None, &NeverCancel)
        .unwrap()
}

/// 既定の切り抜きの外の詰め物・異常な行は、出力に影響しない。既定の切り抜きを使わない現像
/// （修正の前の動作）では、出力の端が壊れる（不具合の再現）。
#[test]
fn bad_edges_outside_the_default_crop_do_not_reach_the_output() {
    for orientation in [Orientation::Normal, Orientation::Rotate270Cw] {
        let (clean, bad) = raws_with_bad_edges(orientation);
        let s = DevelopSettings::default();
        let a = render(&source(clean.clone(), 1), &s);
        let b = render(&source(bad.clone(), 2), &s);
        // 出力の寸法は既定の切り抜きの寸法（向きを反映）。
        let expected = if orientation.swaps_dimensions() {
            (CROP.height, CROP.width)
        } else {
            (CROP.width, CROP.height)
        };
        assert_eq!(a.full_size, expected, "{orientation:?}");
        assert_eq!(a.b3.image.dimensions(), expected);
        assert!(
            a.b3.image.data() == b.b3.image.data(),
            "{orientation:?}: 既定の切り抜きの外の値が出力に影響した"
        );

        // 再現: 既定の切り抜きを使わない（有効画素の全体を出力する）と、端が壊れる。
        let (mut clean_full, mut bad_full) = (clean, bad);
        clean_full.default_crop = None;
        bad_full.default_crop = None;
        let a = render(&source(clean_full, 3), &s);
        let b = render(&source(bad_full, 4), &s);
        assert_eq!(
            a.full_size,
            if orientation.swaps_dimensions() {
                (H, W)
            } else {
                (W, H)
            }
        );
        assert!(
            a.b3.image.data() != b.b3.image.data(),
            "{orientation:?}: 詰め物の列が出力に出ない（再現できていない）"
        );
    }
}

/// 既定の切り抜きの範囲の出力は、切り抜きのない現像の同じ位置の画素とビット単位で同じ（回転・利用者の
/// 切り抜き・ガイドを使う設定がない場合）。向き 8 では画素の並べ替え。
#[test]
fn default_crop_selects_the_same_pixels_as_the_full_render() {
    let s = DevelopSettings::default();
    for (crop, orientation) in [
        (CROP, Orientation::Normal),
        // 奇数の原点。
        (PixelRect::new(7, 5, 101, 71), Orientation::Normal),
        (CROP, Orientation::Rotate270Cw),
    ] {
        let mut raw = raw_oriented(&busy_scene(W, H), Orientation::Normal);
        let full = render(&source(raw.clone(), 10), &s);
        raw.default_crop = Some(crop);
        raw.metadata.orientation = orientation;
        let cropped = render(&source(raw, 11), &s);
        let (cw, ch) = cropped.b3.image.dimensions();
        for v in 0..ch {
            for u in 0..cw {
                // O の画素 (u, v) に当たる既定の切り抜きの中の位置 (x, y)。
                let (x, y) = match orientation {
                    Orientation::Normal => (u, v),
                    // 向き 8（反時計回りに 90 度。0 行目が左辺）: O の (u, v) は S の (W − 1 − v, u)。
                    Orientation::Rotate270Cw => (crop.width - 1 - v, u),
                    _ => unreachable!(),
                };
                let p = cropped.b3.image.data()[(v * cw + u) as usize];
                let q = full.b3.image.data()[((crop.y + y) * W + crop.x + x) as usize];
                assert_eq!(p, q, "{crop:?} {orientation:?} ({u}, {v})");
            }
        }
    }
}

/// 利用者の切り抜き（`CropRect`）と回転は、既定の切り抜きの範囲（G の「画像全体」）を基準にする。
#[test]
fn user_crop_and_rotation_are_relative_to_the_default_crop() {
    let (clean, _) = raws_with_bad_edges(Orientation::Normal);
    let src = source(clean.clone(), 20);
    let mut s = DevelopSettings::default();
    s.geometry.crop = CropRect {
        left: 0.25,
        top: 0.0,
        right: 0.75,
        bottom: 1.0,
    };
    let half = render(&src, &s);
    // 幅は既定の切り抜きの幅の半分。
    assert_eq!(half.full_size, (CROP.width / 2, CROP.height));
    let whole = render(&src, &DevelopSettings::default());
    for y in 0..CROP.height {
        for x in 0..CROP.width / 2 {
            let p = half.b3.image.data()[(y * (CROP.width / 2) + x) as usize];
            let q = whole.b3.image.data()[(y * CROP.width + x + CROP.width / 4) as usize];
            assert_eq!(p, q, "({x}, {y})");
        }
    }
    // 回転しても、出力は既定の切り抜きの内側に制限される（詰め物の列を使わない）。
    let (_, bad) = raws_with_bad_edges(Orientation::Normal);
    let mut s = DevelopSettings::default();
    s.geometry.angle_deg = 4.0;
    let a = render(&src, &s);
    let b = render(&source(bad, 21), &s);
    assert!(a.b3.image.data() == b.b3.image.data());
    assert!(a.full_size.0 <= CROP.width && a.full_size.1 <= CROP.height);
}

/// プレビュー（段階 A1 は有効画素の全体）とガイドも、既定の切り抜きを基準にする。
#[test]
fn preview_uses_the_default_crop() {
    let (clean, bad) = raws_with_bad_edges(Orientation::Rotate270Cw);
    let mut s = strong_settings();
    s.geometry.angle_deg = -2.0;
    let e = engine(68, 48);
    let a = e
        .render_preview(&source(clean, 30), &s, RenderQuality::Final, 68)
        .unwrap();
    let e = engine(68, 48);
    let b = e
        .render_preview(&source(bad, 31), &s, RenderQuality::Final, 68)
        .unwrap();
    assert!(a.used_guide);
    // A1 は有効画素の全体（136 × 92）を長辺 68 に縮小したもの（密度 0.5）。出力は既定の切り抜き
    // （104 × 64。向き 8 で 64 × 104）を同じ密度で標本化したものを、回転で内側に制限した範囲。
    let (w, h) = a.b3.image.dimensions();
    assert!(
        w <= CROP.height / 2 + 1 && h <= CROP.width / 2 + 1,
        "{w} × {h}"
    );
    // 詰め物の列・異常な行の影響（RCD の余白 10 画素）は、縮小した A1 では既定の切り抜きの境界から
    // 3 画素以上離れているので、バイキュービック（2 画素）とガイドの双線形（1 画素）に入らない。
    assert!(a.b3.image.data() == b.b3.image.data());
}
