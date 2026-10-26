use genzo_model::{
    AiMaskKind, BrushStroke, ColorSample, DataRef, DevelopSettings, Geometry, LocalAdjustment,
    LocalDelta, MaskComponent, MaskMode, MaskShape, Orientation,
};

use super::*;
use crate::coords::{CoordinateFrames, SensorGrid};
use crate::finish::FinishPlan;
use crate::image::RgbImage;
use crate::stage::StageContext;
use crate::{ColorContract, InputKind};

fn v2_settings() -> DevelopSettings {
    DevelopSettings {
        process_version: 2,
        ..Default::default()
    }
}

fn mask(shape: MaskShape, delta: LocalDelta) -> LocalAdjustment {
    LocalAdjustment {
        delta,
        ..LocalAdjustment::new(MaskComponent::add(shape))
    }
}

fn exposure(ev: f32) -> LocalDelta {
    LocalDelta {
        exposure_ev: ev,
        ..Default::default()
    }
}

fn radial(cx: f32, cy: f32, r: f32, feather: f32) -> MaskShape {
    MaskShape::Radial {
        cx,
        cy,
        rx: r,
        ry: r,
        angle_deg: 0.0,
        feather,
    }
}

/// 幅 `w`・高さ `h` の画像の、格子が `scale` 倍のジオメトリ（回転・切り抜きなし）。
fn geometry(w: u32, h: u32, scale: f64) -> GeometryPlan {
    let frames = CoordinateFrames::new(w, h, Orientation::Normal).unwrap();
    let gw = ((f64::from(w) * scale).round() as u32).max(1);
    let gh = ((f64::from(h) * scale).round() as u32).max(1);
    let grid = SensorGrid::new(gw, gh, (f64::from(w), f64::from(h))).unwrap();
    GeometryPlan::new(&Geometry::default(), frames, grid).unwrap()
}

fn none() -> RangeInputs {
    RangeInputs {
        chroma: [0.0; 2],
        luma: 0.0,
    }
}

#[test]
fn slots_follow_the_parameter_order_of_active_masks() {
    let mut s = v2_settings();
    assert_eq!(LocalSlots::for_settings(&s, ProcessVersion::V2), None);
    let mut d = LocalDelta {
        saturation: -30.0,
        temperature: 20.0,
        ..Default::default()
    };
    s.masks.push(mask(radial(0.5, 0.3, 0.2, 50.0), d));
    // 無効なマスク・差分 0 のマスク・部品のないマスクは数えない。
    d = exposure(1.0);
    let mut off = mask(radial(0.5, 0.3, 0.2, 50.0), d);
    off.enabled = false;
    s.masks.push(off);
    s.masks
        .push(mask(radial(0.5, 0.3, 0.2, 50.0), LocalDelta::default()));
    let mut empty = mask(
        radial(0.5, 0.3, 0.2, 50.0),
        LocalDelta {
            sharpness: 10.0,
            ..Default::default()
        },
    );
    empty.components.clear();
    s.masks.push(empty);
    let slots = LocalSlots::for_settings(&s, ProcessVersion::V2).unwrap();
    assert_eq!(slots.count(), 2);
    assert_eq!(slots.plane(LocalParam::Temperature), Some(0));
    assert_eq!(slots.plane(LocalParam::Saturation), Some(1));
    assert_eq!(slots.plane(LocalParam::Exposure), None);
    assert!(!slots.uses(LocalParam::Sharpness));
    assert_eq!(slots.to_u32()[LocalParam::Saturation.index()], 1);
    assert_eq!(slots.to_u32()[LocalParam::Tint.index()], u32::MAX);
    // 処理バージョン 1 では使わない。
    assert_eq!(LocalSlots::for_settings(&s, ProcessVersion::V1), None);
    for (i, p) in LocalParam::ALL.iter().enumerate() {
        assert_eq!(p.index(), i);
    }
    assert_eq!(LocalDelta::NAMES[LocalParam::Dehaze.index()], "dehaze");
}

#[test]
fn linear_and_radial_shapes() {
    let g = geometry(200, 100, 1.0);
    let lin = prepare_mask(
        &mask(
            MaskShape::Linear {
                x0: 0.2,
                y0: 0.0,
                x1: 0.6,
                y1: 0.0,
            },
            exposure(1.0),
        ),
        &g,
        None,
        None,
    );
    assert_eq!(lin.weight(0.1, 0.2, &none()), 1.0);
    assert_eq!(lin.weight(0.2, 0.3, &none()), 1.0);
    assert!((lin.weight(0.4, 0.1, &none()) - 0.5).abs() < 1e-6);
    assert_eq!(lin.weight(0.6, 0.1, &none()), 0.0);
    assert_eq!(lin.weight(0.9, 0.1, &none()), 0.0);
    // 長さ 0 の線形は何もしない。
    let zero = prepare_mask(
        &mask(
            MaskShape::Linear {
                x0: 0.3,
                y0: 0.3,
                x1: 0.3,
                y1: 0.3,
            },
            exposure(1.0),
        ),
        &g,
        None,
        None,
    );
    assert_eq!(zero.weight(0.3, 0.3, &none()), 0.0);
    // 円形: ぼかし 0 なら縁でちょうど切る。ぼかしで内側から 0 に向かう。
    let hard = prepare_mask(
        &mask(radial(0.5, 0.25, 0.1, 0.0), exposure(1.0)),
        &g,
        None,
        None,
    );
    assert_eq!(hard.weight(0.5, 0.25, &none()), 1.0);
    assert_eq!(hard.weight(0.59, 0.25, &none()), 1.0);
    assert_eq!(hard.weight(0.61, 0.25, &none()), 0.0);
    let soft = prepare_mask(
        &mask(radial(0.5, 0.25, 0.1, 50.0), exposure(1.0)),
        &g,
        None,
        None,
    );
    assert_eq!(soft.weight(0.54, 0.25, &none()), 1.0);
    let mid = soft.weight(0.575, 0.25, &none());
    assert!((mid - 0.5).abs() < 1e-5, "{mid}");
    // 回転した楕円（横の半径 0.2・縦 0.05 を 90° 回すと縦長）。
    let ellipse = prepare_mask(
        &mask(
            MaskShape::Radial {
                cx: 0.5,
                cy: 0.25,
                rx: 0.2,
                ry: 0.05,
                angle_deg: 90.0,
                feather: 0.0,
            },
            exposure(1.0),
        ),
        &g,
        None,
        None,
    );
    assert_eq!(ellipse.weight(0.5, 0.4, &none()), 1.0);
    assert_eq!(ellipse.weight(0.6, 0.25, &none()), 0.0);
}

#[test]
fn components_combine_and_invert() {
    let g = geometry(100, 100, 1.0);
    let mut m = mask(radial(0.3, 0.5, 0.1, 0.0), exposure(1.0));
    m.components
        .push(MaskComponent::add(radial(0.7, 0.5, 0.1, 0.0)));
    let p = prepare_mask(&m, &g, None, None);
    assert_eq!(p.weight(0.3, 0.5, &none()), 1.0);
    assert_eq!(p.weight(0.7, 0.5, &none()), 1.0);
    assert_eq!(p.weight(0.5, 0.5, &none()), 0.0);
    // 除く: 中心だけ穴をあける。
    m.components.push(MaskComponent {
        mode: MaskMode::Subtract,
        invert: false,
        shape: radial(0.3, 0.5, 0.03, 0.0),
    });
    let p = prepare_mask(&m, &g, None, None);
    assert_eq!(p.weight(0.3, 0.5, &none()), 0.0);
    assert_eq!(p.weight(0.36, 0.5, &none()), 1.0);
    // 交差だけのマスクは画像全体 ∩ 部品。反転で外側。
    let only = LocalAdjustment {
        components: vec![MaskComponent {
            mode: MaskMode::Intersect,
            invert: true,
            shape: radial(0.5, 0.5, 0.1, 0.0),
        }],
        ..mask(radial(0.0, 0.0, 0.0, 0.0), exposure(1.0))
    };
    let p = prepare_mask(&only, &g, None, None);
    assert_eq!(p.initial, 1.0);
    assert_eq!(p.weight(0.5, 0.5, &none()), 0.0);
    assert_eq!(p.weight(0.9, 0.9, &none()), 1.0);
}

#[test]
fn color_and_luminance_ranges() {
    let tone = SceneToDisplayParams::for_input(InputKind::SceneReferred);
    let red = [0.30f32, 0.05, 0.04];
    let lab = linear_bt2020_to_oklab(red);
    let g = geometry(100, 100, 1.0);
    let m = mask(
        MaskShape::ColorRange {
            samples: vec![ColorSample {
                x: 0.5,
                y: 0.5,
                lab,
            }],
            amount: 50.0,
        },
        exposure(1.0),
    );
    let p = prepare_mask(&m, &g, None, None);
    let at = |c: [f32; 3]| p.weight(0.5, 0.5, &range_inputs(c, 1.0, &tone));
    assert_eq!(at(red), 1.0);
    // 露光量を変えても（明るさだけが違う同じ色）選ぶ。
    assert_eq!(at(red.map(|v| v * 4.0)), 1.0);
    assert_eq!(at(red.map(|v| v * 0.25)), 1.0);
    // 色の違う色・無彩色は選ばない。
    assert_eq!(at([0.05, 0.3, 0.05]), 0.0);
    assert_eq!(at([0.2, 0.2, 0.2]), 0.0);
    // 許容量を上げると近い色まで選ぶ。
    let orange = [0.30f32, 0.12, 0.03];
    let narrow = at(orange);
    let wide_mask = mask(
        MaskShape::ColorRange {
            samples: vec![ColorSample {
                x: 0.5,
                y: 0.5,
                lab,
            }],
            amount: 100.0,
        },
        exposure(1.0),
    );
    let wide = prepare_mask(&wide_mask, &g, None, None).weight(
        0.5,
        0.5,
        &range_inputs(orange, 1.0, &tone),
    );
    assert!(wide > narrow, "{wide} {narrow}");
    // 輝度の範囲: 表示の明るさ（ステージ 15 のカーブの後の符号化した値）。
    let e = |y: f32| range_inputs([y; 3], 1.0, &tone).luma;
    assert!(
        e(0.0731) > 0.45 && e(0.0731) < 0.47,
        "中間グレーは sRGB で約 0.46: {}",
        e(0.0731)
    );
    assert_eq!(e(0.0), 0.0);
    assert!(e(100.0) <= 1.0 && e(100.0) > 0.999);
    let lum = |min: f32, max: f32, smoothness: f32| {
        prepare_mask(
            &mask(
                MaskShape::LuminanceRange {
                    min,
                    max,
                    smoothness,
                },
                exposure(1.0),
            ),
            &g,
            None,
            None,
        )
    };
    let hard = lum(40.0, 60.0, 0.0);
    let w = |m: &PreparedMask, y: f32| m.weight(0.5, 0.5, &range_inputs([y; 3], 1.0, &tone));
    assert_eq!(w(&hard, 0.0731), 1.0);
    assert_eq!(w(&hard, 0.5), 0.0);
    assert_eq!(w(&hard, 0.01), 0.0);
    // 端の 0・100 は含む。
    assert_eq!(w(&lum(0.0, 100.0, 0.0), 0.0), 1.0);
    assert_eq!(w(&lum(0.0, 100.0, 0.0), 1000.0), 1.0);
    // 滑らかさで範囲の外へなめらかに 0。
    let soft = lum(40.0, 60.0, 50.0);
    let near = w(&soft, 0.2);
    assert!(near > 0.0 && near < 1.0, "{near}");
    // 露光量（全体）を上げると、同じ画素が明るく扱われる（表示の明るさ）。
    assert!(range_inputs([0.0731; 3], 2.0, &tone).luma > e(0.0731));
}

#[test]
fn weights_do_not_depend_on_the_resolution() {
    // 同じ G 座標の画素は、プレビュー（縮小）でもフル解像度でも同じ重み（G 座標で決まる）。
    let full = geometry(400, 200, 1.0);
    let preview = geometry(400, 200, 0.25);
    let m = mask(radial(0.4, 0.2, 0.15, 60.0), exposure(1.0));
    let reference = |g: &GeometryPlan| {
        let (w, h) = g.output_size();
        ImageTile::from_image(
            Roi::full(w, h),
            RgbImage::filled(w, h, ColorContract::B2Working, [0.18; 3]).unwrap(),
        )
        .unwrap()
    };
    let a = mask_weights(
        &m,
        &full,
        &reference(&full),
        0.0,
        InputKind::SceneReferred,
        None,
    );
    let b = mask_weights(
        &m,
        &preview,
        &reference(&preview),
        0.0,
        InputKind::SceneReferred,
        None,
    );
    // プレビューの画素 (i, j) の中心はフル解像度の (4i + 2, 4j + 2) の左上の角（ここでは近い画素で比べる）。
    let (fw, _) = full.output_size();
    let (pw, ph) = preview.output_size();
    let mut max_diff = 0.0f32;
    for j in 0..ph {
        for i in 0..pw {
            let pv = b[(j * pw + i) as usize];
            let (gx, gy) = g_coords(
                g_mapping(&preview).0,
                g_mapping(&preview).1,
                i64::from(i),
                i64::from(j),
            );
            // フル解像度の画素で、中心の G 座標が同じ点（4i + 1.5 の位置）を挟む 2 × 2 の平均。
            let fx = (gx * 400.0 - 0.5).floor() as u32;
            let fy = (gy * 400.0 - 0.5).floor() as u32;
            let mut sum = 0.0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                sum += a[((fy + dy) * fw + fx + dx) as usize];
            }
            max_diff = max_diff.max((sum / 4.0 - pv).abs());
        }
    }
    assert!(max_diff < 0.02, "{max_diff}");
}

#[test]
fn the_field_sums_weights_times_deltas_and_is_zero_outside() {
    let mut s = v2_settings();
    s.masks
        .push(mask(radial(0.25, 0.25, 0.1, 0.0), exposure(1.0)));
    s.masks.push(mask(
        radial(0.25, 0.25, 0.1, 0.0),
        LocalDelta {
            exposure_ev: 0.5,
            saturation: 20.0,
            ..Default::default()
        },
    ));
    let frames = CoordinateFrames::new(80, 40, Orientation::Normal).unwrap();
    let ctx = StageContext::new(ProcessVersion::V2, frames);
    let g = GeometryPlan::from_settings(&s, &ctx).unwrap();
    let plan = LocalPlan::new(&s.normalized(), &ctx, &g).unwrap().unwrap();
    let tile = ImageTile::from_image(
        Roi::full(80, 40),
        RgbImage::filled(80, 40, ColorContract::B2Working, [0.18; 3]).unwrap(),
    )
    .unwrap();
    let f = plan.field(&tile).unwrap();
    assert_eq!(f.planes.len(), 2 * 80 * 40);
    // G 座標 (0.25, 0.25) は画素 (20, 20) の近く。
    let v = f.values(20, 20);
    assert_eq!(v[LocalParam::Exposure.index()], 1.5);
    assert_eq!(v[LocalParam::Saturation.index()], 20.0);
    assert_eq!(v[LocalParam::Contrast.index()], 0.0);
    assert_eq!(f.values(70, 5), [0.0; LOCAL_PARAM_COUNT]);
    // FinishPlan は局所の値の面を作る（露光量・彩度のステージが有効）。
    let fp = FinishPlan::new(&s, &ctx).unwrap();
    assert!(fp.local().is_some());
    assert!(fp.active_stage_ids().contains(&"finish.exposure"));
    assert!(fp.active_stage_ids().contains(&"finish.color"));
    // 版 1 では作らない。
    s.process_version = 1;
    let ctx1 = StageContext::new(ProcessVersion::V1, frames);
    let fp1 = FinishPlan::new(&s, &ctx1).unwrap();
    assert!(fp1.local().is_none());
    assert!(!fp1.active_stage_ids().contains(&"finish.exposure"));
    assert!(!fp1.active_stage_ids().contains(&"finish.color"));
}

#[test]
fn brush_masks_use_the_raster() {
    let g = geometry(200, 100, 1.0);
    let m = mask(
        MaskShape::Brush {
            strokes: vec![BrushStroke {
                size: 0.05,
                feather: 0.0,
                points: vec![[0.2, 0.2], [0.6, 0.2]],
                ..Default::default()
            }],
        },
        exposure(1.0),
    );
    let p = prepare_mask(&m, &g, None, None);
    assert_eq!(p.weight(0.4, 0.2, &none()), 1.0);
    assert_eq!(p.weight(0.4, 0.4, &none()), 0.0);
    assert!(matches!(
        p.components[0].shape,
        PreparedShape::Brush(Some(_))
    ));
}

#[test]
fn white_balance_gains_keep_neutral_luminance() {
    assert_eq!(white_balance_gains(0.0, 0.0), [1.0, 1.0, 1.0]);
    let warm = white_balance_gains(100.0, 0.0);
    assert!(warm[0] > 1.0 && warm[2] < 1.0, "{warm:?}");
    assert!((luminance(warm) - 1.0).abs() < 1e-6);
    let magenta = white_balance_gains(0.0, 100.0);
    assert!(magenta[1] < 1.0 && magenta[0] > magenta[1], "{magenta:?}");
    assert!((luminance(magenta) - 1.0).abs() < 1e-6);
    assert!((add_unit(0.5, 30.0, -2.0, 2.0) - 0.8).abs() < 1e-6);
    assert_eq!(add_unit(1.5, 100.0, -2.0, 2.0), 2.0);
    assert_eq!(step(0.5, 0.5, 0.5), 1.0);
    assert_eq!(step(0.5, 0.5, 0.49), 0.0);
}

#[test]
fn local_white_balance_sums_are_limited_to_100() {
    // 重なるマスクの色温度・色かぶりの和は ±100 に収める（決定事項ログ No.152）。
    assert_eq!(
        white_balance_gains(200.0, 0.0),
        white_balance_gains(100.0, 0.0)
    );
    assert_eq!(
        white_balance_gains(-10_000.0, 350.0),
        white_balance_gains(-100.0, 100.0)
    );
    // ステージ 10 の局所の値: 2 つのマスクの +100 が重なった画素（和 200）は +100 と同じ。
    let p = crate::finish::exposure::ExposureParams::new(0.5);
    let mut two = [0.0f32; LOCAL_PARAM_COUNT];
    two[LocalParam::Temperature.index()] = 200.0;
    two[LocalParam::Tint.index()] = -250.0;
    let mut one = [0.0f32; LOCAL_PARAM_COUNT];
    one[LocalParam::Temperature.index()] = 100.0;
    one[LocalParam::Tint.index()] = -100.0;
    let c = [0.2, 0.3, 0.4];
    assert_eq!(p.apply_local(c, &two), p.apply_local(c, &one));
}

#[test]
fn ai_masks_use_the_resolved_raster_and_are_zero_when_missing() {
    use crate::profile::{AiMaskRaster, MemoryProfiles, ProfileResolver, ProfileResources};
    use crate::sensor::SensorWarning;
    // 400 × 200 の画像（G の箱は 1 × 0.5）。ラスター 8 × 4 の右半分だけ 255。
    let g = geometry(400, 200, 1.0);
    let values: Vec<u8> = (0..4)
        .flat_map(|_| (0..8).map(|x| if x >= 4 { 255 } else { 0 }))
        .collect();
    let mut lib = MemoryProfiles::new();
    let sha = lib.insert_ai_mask(AiMaskRaster::new(8, 4, values).unwrap());
    let shape = |sha: &str| MaskShape::Ai {
        kind: AiMaskKind::Sky,
        model: "test".into(),
        mask: DataRef::new(AiMaskKind::Sky.ref_id(), sha),
        width: 8,
        height: 4,
        bounds: [0.0, 0.0, 1.0, 0.5],
    };
    let mut s = v2_settings();
    s.masks.push(mask(shape(&sha), exposure(1.0)));
    let res = ProfileResources::resolve(&s, Some(&lib as &dyn ProfileResolver));
    assert_eq!(res.ai_masks.len(), 1);
    assert!(res.ai_masks_missing.is_empty());
    assert!(res.stage_c_warnings().is_empty());
    let p = prepare_mask(&s.masks[0], &g, None, Some(&res));
    assert!(matches!(
        p.components[0].shape,
        PreparedShape::Brush(Some(_))
    ));
    assert_eq!(p.weight(0.1, 0.2, &none()), 0.0);
    assert_eq!(p.weight(0.9, 0.2, &none()), 1.0);
    // 境目（x = 0.5）は中間。範囲の外は 0。
    assert!((p.weight(0.5, 0.25, &none()) - 0.5).abs() < 1e-6);
    assert_eq!(p.weight(0.9, 0.7, &none()), 0.0);
    // 引けない参照: 重み 0 で、知らせる。資源なしでも 0。
    let mut missing = v2_settings();
    missing
        .masks
        .push(mask(shape(&"cd".repeat(32)), exposure(1.0)));
    let res = ProfileResources::resolve(&missing, Some(&lib as &dyn ProfileResolver));
    assert_eq!(res.ai_masks_missing, ["cd".repeat(32)]);
    assert_eq!(res.stage_c_warnings(), [SensorWarning::AiMaskMissing]);
    let p = prepare_mask(&missing.masks[0], &g, None, Some(&res));
    assert!(matches!(p.components[0].shape, PreparedShape::Brush(None)));
    assert_eq!(p.weight(0.9, 0.2, &none()), 0.0);
    let p = prepare_mask(&s.masks[0], &g, None, None);
    assert_eq!(p.weight(0.9, 0.2, &none()), 0.0);
}
