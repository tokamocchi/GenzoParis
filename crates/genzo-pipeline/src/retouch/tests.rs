use genzo_model::{DevelopSettings, Orientation, RetouchMode, RetouchSpot};

use super::*;
use crate::contract::ColorContract;

fn image(w: u32, h: u32, f: impl FnMut(u32, u32) -> [f32; 3]) -> RgbImage {
    RgbImage::from_fn(w, h, ColorContract::B2Working, f).unwrap()
}

fn frames(w: u32, h: u32, o: Orientation) -> CoordinateFrames {
    CoordinateFrames::new(w, h, o).unwrap()
}

fn settings(spots: Vec<RetouchSpot>) -> DevelopSettings {
    DevelopSettings {
        retouch: spots,
        ..Default::default()
    }
}

/// O の画素の位置 (x, y) を、長辺 `long` の O 座標にする。
fn o(x: f32, y: f32, long: f32) -> [f32; 2] {
    [x / long, y / long]
}

fn apply(img: &RgbImage, f: CoordinateFrames, spots: Vec<RetouchSpot>) -> RgbImage {
    let plan = RetouchPlan::new(&settings(spots), f).unwrap().unwrap();
    plan.apply_to_image(img, SensorGrid::full_resolution(&f))
        .unwrap()
}

#[test]
fn no_plan_without_active_spots_or_in_version_1() {
    let f = frames(64, 48, Orientation::Normal);
    assert!(RetouchPlan::new(&settings(vec![]), f).unwrap().is_none());
    let mut off = RetouchSpot::new(vec![[0.5, 0.3]], [0.1, 0.0]);
    off.enabled = false;
    assert!(RetouchPlan::new(&settings(vec![off]), f).unwrap().is_none());
    let zero = RetouchSpot::new(vec![[0.5, 0.3]], [0.0, 0.0]);
    assert!(
        RetouchPlan::new(&settings(vec![zero]), f)
            .unwrap()
            .is_none()
    );
    let mut v1 = settings(vec![RetouchSpot::new(vec![[0.5, 0.3]], [0.1, 0.0])]);
    v1.process_version = 1;
    assert!(RetouchPlan::new(&v1, f).unwrap().is_none());
    let mut bad = v1.clone();
    bad.process_version = 9;
    assert!(RetouchPlan::new(&bad, f).is_err());
}

#[test]
fn weight_is_one_inside_and_zero_outside() {
    assert_eq!(spot_weight(0.0, 10.0, 5.0), 1.0);
    assert_eq!(spot_weight(5.0, 10.0, 5.0), 1.0);
    assert_eq!(spot_weight(10.0, 10.0, 5.0), 0.0);
    assert!((spot_weight(7.5, 10.0, 5.0) - 0.5).abs() < 1e-6);
    // ぼかし 0 なら縁まで 1。
    assert_eq!(spot_weight(9.99, 10.0, 10.0), 1.0);
    assert_eq!(circle_sample_count(1.0, 1.0), 16);
    assert_eq!(circle_sample_count(1.0e4, 1.0), 256);
}

#[test]
fn clone_copies_the_source_and_leaves_the_rest() {
    // 横に変わる模様。コピー元は右に 20 画素（整数のずれなので補間なしで同じ値）。
    let (w, h) = (96u32, 64u32);
    let img = image(w, h, |x, y| [x as f32 * 0.01, y as f32 * 0.01, 0.3]);
    let long = w as f32;
    let spot = RetouchSpot {
        mode: RetouchMode::Clone,
        size: 6.0 / long,
        feather: 0.0,
        ..RetouchSpot::new(vec![o(30.0, 32.0, long)], o(20.0, 0.0, long))
    };
    let out = apply(&img, frames(w, h, Orientation::Normal), vec![spot]);
    // 中心の画素（中心 (30, 32) を覆う画素 (29, 31) など）はコピー元の値。
    for (x, y) in [(29u32, 31u32), (30, 32), (27, 33)] {
        let v = out.pixel(x, y).unwrap();
        let e = img.pixel(x + 20, y).unwrap();
        for c in 0..3 {
            assert!((v[c] - e[c]).abs() < 1e-6, "({x}, {y}): {v:?} {e:?}");
        }
    }
    // 半径の外は元のまま（ビット単位）。
    for (x, y) in [(20u32, 32u32), (30, 20), (40, 40), (0, 0)] {
        assert_eq!(out.pixel(x, y), img.pixel(x, y), "({x}, {y})");
    }
}

#[test]
fn heal_matches_the_surroundings_and_clone_does_not() {
    // 左半分 0.2・右半分 0.4 の背景。左の暗いゴミ（中心 (30, 32)・半径 4）を、右の明るい側から写す。
    let (w, h) = (128u32, 64u32);
    let img = image(w, h, |x, y| {
        let d = ((x as f32 + 0.5 - 30.0).powi(2) + (y as f32 + 0.5 - 32.0).powi(2)).sqrt();
        if d < 4.0 {
            [0.02; 3]
        } else if x < 64 {
            [0.2, 0.18, 0.15]
        } else {
            [0.4, 0.36, 0.3]
        }
    });
    let long = w as f32;
    let spot = |mode| RetouchSpot {
        mode,
        size: 8.0 / long,
        feather: 30.0,
        ..RetouchSpot::new(vec![o(30.0, 32.0, long)], o(60.0, 0.0, long))
    };
    let f = frames(w, h, Orientation::Normal);
    let healed = apply(&img, f, vec![spot(RetouchMode::Heal)]);
    let cloned = apply(&img, f, vec![spot(RetouchMode::Clone)]);
    for (x, y) in [(29u32, 31u32), (30, 32), (27, 30), (33, 34)] {
        let v = healed.pixel(x, y).unwrap();
        for (c, e) in [0.2f32, 0.18, 0.15].iter().enumerate() {
            assert!((v[c] - e).abs() < e * 0.01, "修復 ({x}, {y}): {v:?}");
        }
        let v = cloned.pixel(x, y).unwrap();
        assert!(
            (v[0] - 0.4).abs() < 1e-5,
            "コピースタンプ ({x}, {y}): {v:?}"
        );
    }
}

#[test]
fn heal_removes_a_thin_line_on_a_gradient() {
    // 縦に明るさが変わる空（上が明るい）に、斜めの暗い電線（太さ約 2 画素）。線に沿ってなぞり、上にずらして写す。
    let (w, h) = (160u32, 100u32);
    let sky = |y: f32| 0.6 - y * 0.004;
    let line_y = |x: f32| 40.0 + x * 0.2;
    let img = image(w, h, |x, y| {
        let (xf, yf) = (x as f32 + 0.5, y as f32 + 0.5);
        if (yf - line_y(xf)).abs() < 1.0 && (20.0..140.0).contains(&xf) {
            [0.01; 3]
        } else {
            [sky(yf); 3]
        }
    });
    let long = w as f32;
    let points: Vec<[f32; 2]> = (0..=12)
        .map(|k| {
            let x = 15.0 + k as f32 * 10.833_333;
            o(x, line_y(x), long)
        })
        .collect();
    let spot = RetouchSpot {
        size: 3.0 / long,
        feather: 30.0,
        ..RetouchSpot::new(points, o(0.0, -12.0, long))
    };
    let out = apply(&img, frames(w, h, Orientation::Normal), vec![spot]);
    let mut worst = 0.0f32;
    for x in 25..135u32 {
        let yc = line_y(x as f32 + 0.5);
        for dy in -1..=1 {
            let y = (yc.floor() as i32 + dy) as u32;
            let v = out.pixel(x, y).unwrap()[1];
            let e = sky(y as f32 + 0.5);
            worst = worst.max((v - e).abs() / e);
        }
    }
    assert!(worst < 0.02, "線の跡が残っている（相対の差 {worst}）");
}

#[test]
fn tiles_give_the_same_result_as_the_whole_image() {
    // タイル（画像の外にはみ出すものを含む）ごとに当てても、画像全体に当てたものとビット単位で同じ。
    let (w, h) = (90u32, 70u32);
    let img = image(w, h, |x, y| {
        let v = ((x * 7 + y * 13) % 17) as f32 * 0.02 + 0.05;
        [v, v * 0.9, v * 1.1]
    });
    let long = w as f32;
    let spots = vec![
        RetouchSpot {
            size: 9.0 / long,
            ..RetouchSpot::new(vec![o(4.0, 6.0, long)], o(25.0, 18.0, long))
        },
        RetouchSpot {
            mode: RetouchMode::Clone,
            size: 5.0 / long,
            ..RetouchSpot::new(
                vec![o(50.0, 60.0, long), o(85.0, 40.0, long)],
                o(-30.0, -15.0, long),
            )
        },
        RetouchSpot {
            size: 3.0 / long,
            opacity: 60.0,
            ..RetouchSpot::new(
                vec![
                    o(10.0, 40.0, long),
                    o(40.0, 30.0, long),
                    o(60.0, 50.0, long),
                ],
                o(0.0, -9.0, long),
            )
        },
    ];
    let f = frames(w, h, Orientation::Rotate90Cw);
    let grid = SensorGrid::full_resolution(&f);
    let plan = RetouchPlan::new(&settings(spots), f).unwrap().unwrap();
    let whole = plan.apply_to_image(&img, grid).unwrap();
    assert_ne!(whole, img);
    let fetch = |roi: Roi| crate::engine::gather_mirror_from(&img, roi);
    let prepared = plan.prepare(grid, &fetch).unwrap();
    for t in [
        Roi::new(-5, -5, 40, 30),
        Roi::new(30, 20, 32, 32),
        Roi::new(60, 50, 40, 30),
        Roi::new(0, 0, 90, 70),
    ] {
        let mut tile = crate::engine::gather_mirror_from(&img, t).unwrap();
        prepared.apply(&mut tile, &fetch).unwrap();
        let expected = crate::engine::gather_mirror_from(&whole, t).unwrap();
        assert_eq!(tile, expected, "{t:?}");
    }
}

#[test]
fn spots_follow_the_orientation() {
    // 向きが 90 度回転（S の幅 60・高さ 40 → O の幅 40・高さ 60）。O の (10, 45) のスポットは、S の
    // oriented_to_sensor(10, 45) の画素に当たる。
    let (w, h) = (60u32, 40u32);
    let img = image(w, h, |x, _| [x as f32 * 0.01 + 0.1; 3]);
    let f = frames(w, h, Orientation::Rotate90Cw);
    let long = 60.0;
    let spot = RetouchSpot {
        mode: RetouchMode::Clone,
        size: 3.0 / long,
        feather: 0.0,
        ..RetouchSpot::new(vec![o(10.0, 45.0, long)], o(0.0, -20.0, long))
    };
    let out = apply(&img, f, vec![spot]);
    let (sx, sy) = f.oriented_to_sensor(10.0, 45.0);
    let (px, py) = (sx.floor() as u32, sy.floor() as u32);
    assert_ne!(out.pixel(px, py), img.pixel(px, py));
    // O の上へのずれは、S では別の軸の向き。コピー元 O (10, 25) の S の画素の値になる。
    let (cx, cy) = f.oriented_to_sensor(10.0, 25.0);
    let e = img.pixel(cx.floor() as u32, cy.floor() as u32).unwrap();
    let v = out.pixel(px, py).unwrap();
    assert!((v[0] - e[0]).abs() < 0.011, "{v:?} {e:?}");
    // 離れた画素は元のまま。
    assert_eq!(out.pixel(5, 5), img.pixel(5, 5));
}

#[test]
fn source_search_prefers_similar_surroundings() {
    // 一様な背景に、右側だけ明るい帯。円のスポットのコピー元は明るい帯を避ける。
    let (w, h) = (200u32, 120u32);
    let img = image(w, h, |x, _| if x >= 120 { [0.8; 3] } else { [0.2; 3] });
    let f = frames(w, h, Orientation::Normal);
    let long = w as f32;
    let spot = RetouchSpot {
        size: 8.0 / long,
        ..RetouchSpot::new(vec![o(100.0, 60.0, long)], [0.0, 0.0])
    };
    let d = find_source_offset(&img, SensorGrid::full_resolution(&f), f, &spot)
        .unwrap()
        .expect("候補");
    let sx = 100.0 + d[0] * long;
    assert!(sx + 8.0 * 1.25 < 120.0, "明るい帯から写している: {d:?}");
    // 線のスポットのコピー元は線と重ならない（横の線なら縦にずらす）。
    let line = RetouchSpot {
        size: 3.0 / long,
        ..RetouchSpot::new(vec![o(20.0, 60.0, long), o(110.0, 60.0, long)], [0.0, 0.0])
    };
    let d = find_source_offset(&img, SensorGrid::full_resolution(&f), f, &line)
        .unwrap()
        .expect("候補");
    assert!((d[1] * long).abs() >= 6.0, "線と重なる: {d:?}");
    // 枠に収まる候補がなければ None。
    let huge = RetouchSpot {
        size: 0.25,
        ..RetouchSpot::new(vec![[0.5, 0.3]], [0.0, 0.0])
    };
    assert_eq!(
        find_source_offset(&img, SensorGrid::full_resolution(&f), f, &huge).unwrap(),
        None
    );
}

#[test]
fn boundary_samples_of_a_very_long_stroke_are_capped() {
    // 長さの上限ちょうど（長辺の 4 倍）の線を、最小の大きさで（Codex の指摘）。
    let long = 6000.0;
    let min = genzo_model::develop::ranges::RETOUCH_SIZE.min;
    let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]];
    let plan = SpotPlan::new(
        &RetouchSpot {
            size: min,
            ..RetouchSpot::new(square, [0.01, 0.0])
        },
        long,
    );
    assert!(plan.stroke_step(1.0) > plan.band(1.0));
    let n = plan.boundary_points(1.0).len();
    assert!(n <= 2 * (STROKE_SAMPLES_MAX + 2), "{n}");
    // 長辺を斜めに横切る線は、間隔を広げない（窓の幅のまま）。
    let line = SpotPlan::new(
        &RetouchSpot {
            size: min,
            ..RetouchSpot::new(vec![[0.0, 0.0], [1.0, 2.0 / 3.0]], [0.01, 0.0])
        },
        long,
    );
    assert_eq!(line.stroke_step(1.0), line.band(1.0));
}

/// 画像の外の端どうしを往復する、検証を通らない極端な線（Codex の指摘）。
fn zigzag(n: usize, a: [f32; 2], b: [f32; 2]) -> Vec<[f32; 2]> {
    (0..n).map(|i| if i % 2 == 0 { a } else { b }).collect()
}

#[test]
fn extreme_strokes_are_shortened_and_thinned() {
    let long = 6000.0;
    let max_length = f64::from(genzo_model::MAX_RETOUCH_STROKE_LENGTH) * long;
    // 長すぎる線は、長さの上限で切る（検証を通らない設定が来ても、計算の量を抑える）。
    let plan = SpotPlan::new(
        &RetouchSpot::new(zigzag(5000, [-1.0, -1.0], [2.0, 2.0]), [0.01, 0.0]),
        long,
    );
    let length = |plan: &SpotPlan| match &plan.path {
        Path::Stroke { arc, .. } => arc[arc.len() - 1],
        Path::Circle(_) => 0.0,
    };
    assert!(length(&plan) <= max_length);
    // 大きなスポットで短い線分がたくさん重なった線は、半径の 1/8 より近い点を間引く。
    let big = RetouchSpot {
        size: 0.25,
        ..RetouchSpot::new(zigzag(5000, [0.5, 0.5], [0.501, 0.5]), [0.3, 0.0])
    };
    let Path::Stroke { points, .. } = SpotPlan::new(&big, long).path else {
        panic!("線")
    };
    assert!(points.len() <= 3, "{}", points.len());
    // 画面が大きさの 1/4 で間引いた線（座標は小数 4 桁に丸める）の点は除かない。
    let size = 0.002f32;
    let ui: Vec<[f32; 2]> = (0..200)
        .map(|k| {
            let x = 0.1 + k as f32 * size / 4.0;
            [(x * 1e4).round() / 1e4, 0.3]
        })
        .collect();
    let spot = RetouchSpot {
        size,
        ..RetouchSpot::new(ui.clone(), [0.0, 0.01])
    };
    let Path::Stroke { points, .. } = SpotPlan::new(&spot, long).path else {
        panic!("線")
    };
    assert_eq!(points.len(), ui.len());
}

#[test]
fn boundary_fetches_stay_small_for_large_and_folded_strokes() {
    let long = 6000.0;
    for spot in [
        // 写真を斜めに横切る大きな線。
        RetouchSpot {
            size: 0.25,
            ..RetouchSpot::new(vec![[0.0, 0.0], [1.0, 2.0 / 3.0]], [0.0, 0.3])
        },
        // 長さの上限まで折り返す線。
        RetouchSpot {
            size: 0.01,
            ..RetouchSpot::new(zigzag(9, [0.0, 0.0], [1.0, 0.0]), [0.0, 0.1])
        },
    ] {
        let plan = SpotPlan::new(&spot, long);
        let px = 1.0;
        let h = plan.band(px) / 2.0;
        let points = plan.boundary_points(px);
        let groups = fetch_groups(&points, h, px);
        let mut next = 0;
        for g in &groups {
            assert_eq!(g.start, next);
            next = g.end;
            assert!(g.len() <= SAMPLES_PER_FETCH);
            let chunk = &points[g.clone()];
            let b = chunk.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |b, q| {
                    [
                        b[0].min(q[0] - h),
                        b[1].min(q[1] - h),
                        b[2].max(q[0] + h),
                        b[3].max(q[1] + h),
                    ]
                },
            );
            let area = ((b[2] - b[0]) / px + 3.0) * ((b[3] - b[1]) / px + 3.0);
            assert!(g.len() == 1 || area <= FETCH_PIXELS_MAX, "{area}");
        }
        assert_eq!(next, points.len());
    }
}

/// ずれ `d`（O 座標）のコピー元の線の上の点（弧長で細かく）から、消したい線までの最も近い距離（O の画素）。
fn closest_source_distance(spot: &RetouchSpot, d: [f32; 2], long: f64) -> (f64, f64) {
    let plan = SpotPlan::new(
        &RetouchSpot {
            offset: [0.0, 0.0],
            ..spot.clone()
        },
        long,
    );
    let Path::Stroke { points, arc } = &plan.path else {
        panic!("線")
    };
    let (dx, dy) = (f64::from(d[0]) * long, f64::from(d[1]) * long);
    let total = arc[arc.len() - 1];
    let mut min = f64::INFINITY;
    for j in 0..=2000 {
        let p = Path::point_at(points, arc, total * j as f64 / 2000.0).0;
        let q = [p[0] + dx, p[1] + dy];
        for i in 0..points.len() - 1 {
            min = min.min(Path::nearest_on_segment(points, arc, i, q).d2.sqrt());
        }
    }
    (min, plan.radius)
}

#[test]
fn source_search_checks_the_whole_line_not_just_its_points() {
    // 点の少ない折れ線では、コピー元の線分が点の間で消したい線と交わることがある（Codex の指摘の例）。
    let (w, h) = (400u32, 400u32);
    let img = image(w, h, |_, _| [0.3; 3]);
    let f = frames(w, h, Orientation::Normal);
    let grid = SensorGrid::full_resolution(&f);
    // V 字（の逆）: 半径の 5 倍までのずれは、どれも点の間で交わるので候補はない（前は右へ 2.5 倍のずれを選んでいた）。
    let v = RetouchSpot {
        size: 0.01,
        ..RetouchSpot::new(vec![[0.2, 0.2], [0.5, 0.8], [0.8, 0.2]], [0.0, 0.0])
    };
    let (crossing, _) = closest_source_distance(&v, [0.025, 0.0], 400.0);
    assert!(crossing < 1.0, "例のずれは交わる: {crossing}");
    assert_eq!(find_source_offset(&img, grid, f, &v).unwrap(), None);
    // 緩く曲がった線: 候補があり、コピー元の線のどの点も消したい線から直径より離れている。
    let bent = RetouchSpot {
        size: 0.01,
        ..RetouchSpot::new(vec![[0.2, 0.5], [0.5, 0.56], [0.8, 0.5]], [0.0, 0.0])
    };
    let d = find_source_offset(&img, grid, f, &bent)
        .unwrap()
        .expect("候補");
    let (closest, r) = closest_source_distance(&bent, d, 400.0);
    assert!(closest >= 2.0 * r, "{d:?}: {closest}");
}

/// 一辺 `n` 点の表を双線形補間した O 座標と、正しい O 座標（細かい表の点）との差の最大（長辺 `long` の O の画素）。
fn grid_error(g: &crate::finish::GeometryPlan, n: usize, long: f64) -> f64 {
    const M: usize = 257;
    let exact = oriented_grid_of(g, M);
    let grid = oriented_grid_of(g, n);
    let k = (n - 1) as f64;
    let at = |i: usize, j: usize| grid[j * n + i].map(f64::from);
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let mut worst = 0.0f64;
    for (idx, e) in exact.iter().enumerate() {
        let (fx, fy) = (
            (idx % M) as f64 / (M - 1) as f64,
            (idx / M) as f64 / (M - 1) as f64,
        );
        let (ci, cj) = (
            ((fx * k) as usize).min(n - 2),
            ((fy * k) as usize).min(n - 2),
        );
        let (tx, ty) = (fx * k - ci as f64, fy * k - cj as f64);
        let (a, b, c, d) = (
            at(ci, cj),
            at(ci + 1, cj),
            at(ci, cj + 1),
            at(ci + 1, cj + 1),
        );
        let p = [0, 1].map(|q| lerp(lerp(a[q], b[q], tx), lerp(c[q], d[q], tx), ty));
        worst = worst.max((p[0] - f64::from(e[0])).hypot(p[1] - f64::from(e[1])) * long);
    }
    worst
}

#[test]
fn oriented_grid_is_fine_enough_for_the_smallest_spot_with_strong_distortion() {
    use crate::lens::RadialDistortion;
    use crate::version::ProcessVersion;
    use genzo_model::{Geometry, LensCorrection};
    let f0 = frames(6000, 4000, Orientation::Normal);
    let min_radius = f64::from(genzo_model::develop::ranges::RETOUCH_SIZE.min) * 6000.0;
    for amount in [100.0f32, -100.0] {
        let lens = LensCorrection {
            distortion: amount,
            ..Default::default()
        };
        let f = f0.with_distortion(RadialDistortion::for_settings(
            &f0,
            ProcessVersion::V2,
            &lens,
        ));
        let g = crate::finish::GeometryPlan::new(
            &Geometry::default(),
            f,
            SensorGrid::full_resolution(&f),
        )
        .unwrap();
        // 一辺 9 点では最小のスポットの半径（3 画素）より大きくずれる（Codex のレビューの指摘）。
        assert!(grid_error(&g, 9, 6000.0) > min_radius, "{amount}");
        let e = grid_error(&g, ORIENTED_GRID_SIZE, 6000.0);
        assert!(e < min_radius / 4.0, "{amount}: {e}");
    }
}

#[test]
fn stroke_correction_reaches_the_last_sample_at_the_end_of_the_line() {
    // 長さ 2.5・間隔 1 の線: 標本は弧長 0・1・2・2.5（最後は線の終わり。Codex の指摘）。両側とも同じ値にする。
    let values: Vec<[f32; 3]> = [0.0f32, 1.0, 2.0, 3.0]
        .iter()
        .flat_map(|&v| [[v; 3], [v; 3]])
        .collect();
    let b = Boundary {
        points: vec![[0.0, 0.0]; values.len()],
        values,
    };
    let at = |s: f64| {
        across(
            &b,
            1.0,
            2.5,
            1.0,
            Nearest {
                d2: 0.0,
                s,
                sigma: 0.0,
            },
        )[0]
    };
    assert_eq!(at(0.0), 0.0);
    assert_eq!(at(1.5), 1.5);
    // 最後の区間（2〜2.5）は、その長さで補間して、線の終わりで最後の標本の値になる。
    assert_eq!(at(2.25), 2.5);
    assert_eq!(at(2.5), 3.0);
    assert_eq!(at(9.0), 3.0);
}
