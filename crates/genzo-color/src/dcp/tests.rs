use super::*;
use crate::camera::camera_to_working_matrix;
use crate::space::D65;

/// テスト用の「典型的な形の」カメラ行列（XYZ → カメラ RGB。D65 用。実在の機種の値ではない）。
fn cm_d65() -> Mat3 {
    Mat3::from_rows([
        [0.70, -0.20, -0.06],
        [-0.50, 1.30, 0.23],
        [-0.07, 0.13, 0.65],
    ])
}

/// テスト用の光源 A の行列（D65 の行列を少し変えたもの。実在の機種の値ではない）。
fn cm_a() -> Mat3 {
    Mat3::from_rows([
        [0.80, -0.25, -0.02],
        [-0.42, 1.22, 0.30],
        [-0.03, 0.20, 0.52],
    ])
}

fn single_d65() -> DcpProfile {
    DcpProfile {
        name: Some("テスト標準".into()),
        unique_camera_model: Some("Sony ILCE-7M4".into()),
        calibration_illuminants: [Some(21), None],
        color_matrices: [Some(cm_d65()), None],
        ..Default::default()
    }
}

fn dual() -> DcpProfile {
    DcpProfile {
        calibration_illuminants: [Some(17), Some(21)],
        color_matrices: [Some(cm_a()), Some(cm_d65())],
        ..single_d65()
    }
}

/// 光源の白 `white` の下でのカメラの中立色（行列 `cm` で XYZ をカメラ RGB にしたもの）。
fn neutral_of(cm: &Mat3, white: Xy) -> [f64; 3] {
    cm.apply(white.to_xyz(1.0).unwrap())
}

fn identity_table(dims: [u32; 3], srgb: bool) -> HsvTable {
    let n = (dims[0] * dims[1] * dims[2]) as usize;
    HsvTable::new(dims, srgb, vec![[0.0, 1.0, 1.0]; n]).unwrap()
}

fn assert_close3(a: [f64; 3], b: [f64; 3], tol: f64) {
    for k in 0..3 {
        assert!((a[k] - b[k]).abs() <= tol, "{a:?} vs {b:?}");
    }
}

fn assert_close3_f32(a: [f32; 3], b: [f32; 3], tol: f32) {
    for k in 0..3 {
        assert!(
            (a[k] - b[k]).abs() <= tol * b[k].abs().max(1.0),
            "{a:?} vs {b:?}"
        );
    }
}

#[test]
fn round_trip_through_the_file_format() {
    let mut p = dual();
    p.copyright = Some("© テスト".into());
    p.calibration_signature = Some("com.example".into());
    p.embed_policy = Some(1);
    p.forward_matrices = [
        Some(Mat3::from_rows([
            [0.6, 0.3, 0.06],
            [0.25, 0.8, -0.05],
            [0.02, -0.1, 0.9],
        ])),
        Some(Mat3::from_rows([
            [0.62, 0.28, 0.06],
            [0.27, 0.78, -0.05],
            [0.03, -0.12, 0.92],
        ])),
    ];
    let mut entries = Vec::new();
    for v in 0..2 {
        for h in 0..6 {
            for s in 0..3 {
                entries.push([
                    h as f32 * 2.0 - 5.0,
                    1.0 + s as f32 * 0.1,
                    1.0 - v as f32 * 0.05,
                ]);
            }
        }
    }
    let map = HsvTable::new([6, 3, 2], true, entries).unwrap();
    p.hue_sat_maps = [Some(map.clone()), Some(map.clone())];
    p.look_table = Some(HsvTable::new([6, 3, 1], false, map.entries()[..18].to_vec()).unwrap());
    p.tone_curve = Some(vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]);
    p.baseline_exposure_offset = Some(-0.25);
    p.default_black_render = Some(1);
    for bytes in [p.to_bytes(), p.to_bytes_big_endian()] {
        assert!(bytes.starts_with(b"IIRC") || bytes.starts_with(b"MMCR"));
        let back = DcpProfile::parse(&bytes).unwrap();
        // 行列は 1e−6 の有理数で書くので、その精度で一致する。
        for k in 0..2 {
            let d = back.color_matrices[k]
                .unwrap()
                .max_abs_diff(&p.color_matrices[k].unwrap());
            assert!(d < 1e-6, "{d}");
            let d = back.forward_matrices[k]
                .unwrap()
                .max_abs_diff(&p.forward_matrices[k].unwrap());
            assert!(d < 1e-6, "{d}");
        }
        assert_eq!(back.name, p.name);
        assert_eq!(back.unique_camera_model, p.unique_camera_model);
        assert_eq!(back.copyright, p.copyright);
        assert_eq!(back.calibration_signature, p.calibration_signature);
        assert_eq!(back.embed_policy, Some(1));
        assert_eq!(back.calibration_illuminants, [Some(17), Some(21)]);
        assert_eq!(back.hue_sat_maps, p.hue_sat_maps);
        assert_eq!(back.look_table, p.look_table);
        assert_eq!(back.tone_curve, p.tone_curve);
        assert_eq!(back.baseline_exposure_offset, Some(-0.25));
        assert_eq!(back.default_black_render, Some(1));
        assert!(back.ignored.is_empty());
        assert!(back.is_dual_illuminant());
    }
}

#[test]
fn a_d65_matrix_with_a_d65_white_matches_the_standard_stage_8() {
    // ColorMatrix が D65 の 1 つだけで、白が D65 なら、今のステージ 8（行ごとに正規化した逆行列）と同じ行列。
    let p = single_d65();
    let n = neutral_of(&cm_d65(), D65);
    let r = p.render(n).unwrap();
    assert!(!r.uses_forward_matrix);
    assert!(r.hue_sat_map.is_none());
    assert_eq!(r.weight1, 1.0);
    assert!((r.white.x - D65.x).abs() < 1e-9 && (r.white.y - D65.y).abs() < 1e-9);
    assert!(
        (r.temperature_k - 6504.0).abs() < 10.0,
        "{}",
        r.temperature_k
    );
    let standard = camera_to_working_matrix(&cm_d65()).unwrap();
    let d = r.camera_to_working().max_abs_diff(&standard);
    assert!(d < 1e-9, "差 {d:e}");
}

#[test]
fn neutral_stays_neutral() {
    for p in [single_d65(), dual()] {
        for white in [D65, D50, Xy::new(0.4476, 0.4074), Xy::new(0.38, 0.38)] {
            let r = p.render(neutral_of(&cm_d65(), white)).unwrap();
            assert_close3(r.camera_to_prophoto.apply([1.0; 3]), [1.0; 3], 1e-12);
            assert_close3(r.prophoto_to_working.apply([1.0; 3]), [1.0; 3], 1e-12);
            assert_close3(r.camera_to_working().apply([0.18; 3]), [0.18; 3], 1e-12);
        }
    }
    assert_close3(
        working_to_prophoto().unwrap().apply([1.0; 3]),
        [1.0; 3],
        1e-12,
    );
    let round = prophoto_to_working()
        .unwrap()
        .mul_mat(&working_to_prophoto().unwrap());
    assert!(round.max_abs_diff(&Mat3::IDENTITY) < 1e-9);
}

#[test]
fn weights_follow_the_inverse_temperature() {
    let p = dual();
    // 光源 A（2850 K）より低い 2500 K の白: 光源 1（A）の行列だけ。
    let warm = crate::white_balance::planckian_locus_uv(2500.0).to_xy();
    let r = p.render(neutral_of(&cm_a(), warm)).unwrap();
    assert!(
        (r.temperature_k - 2500.0).abs() < 1.0,
        "{}",
        r.temperature_k
    );
    assert_eq!(r.weight1, 1.0);
    assert!((r.white.x - warm.x).abs() + (r.white.y - warm.y).abs() < 1e-6);
    // D65 の白: 光源 2（D65）の行列だけ。
    let r = p.render(neutral_of(&cm_d65(), D65)).unwrap();
    assert_eq!(r.weight1, 0.0);
    // 間の白: 逆色温度の重み。
    let r = p
        .render(neutral_of(&cm_d65(), Xy::new(0.38, 0.38)))
        .unwrap();
    let t = r.temperature_k;
    assert!(t > 2850.0 && t < 6500.0, "{t}");
    let expected = (1.0 / t - 1.0 / 6500.0) / (1.0 / 2850.0 - 1.0 / 6500.0);
    assert!(
        (r.weight1 - expected).abs() < 1e-12,
        "{} {expected}",
        r.weight1
    );
    // 白は補間した行列で求めた値（行列の補間と白の色度がつり合っている）。
    let cm = lerp_mat(
        &normalize_color_matrix(&cm_a()).unwrap(),
        &normalize_color_matrix(&cm_d65()).unwrap(),
        r.weight1,
    );
    let xy = Xy::from_xyz(
        cm.inverse()
            .unwrap()
            .apply(neutral_of(&cm_d65(), Xy::new(0.38, 0.38))),
    )
    .unwrap();
    assert!((xy.x - r.white.x).abs() + (xy.y - r.white.y).abs() < 1e-6);
    // 光源の順が逆でも同じ行列になる（重みは光源 1 のもの）。
    let swapped = DcpProfile {
        calibration_illuminants: [Some(21), Some(17)],
        color_matrices: [Some(cm_d65()), Some(cm_a())],
        ..dual()
    };
    let r2 = swapped
        .render(neutral_of(&cm_d65(), Xy::new(0.38, 0.38)))
        .unwrap();
    assert!((r2.weight1 - (1.0 - r.weight1)).abs() < 1e-12);
    assert!(r2.camera_to_working().max_abs_diff(&r.camera_to_working()) < 1e-12);
    // 光源の色温度がわからなければ行列 1 だけ。
    let unknown = DcpProfile {
        calibration_illuminants: [Some(17), Some(255)],
        ..dual()
    };
    assert!(!unknown.is_dual_illuminant());
    assert_eq!(
        unknown.render(neutral_of(&cm_d65(), D65)).unwrap().weight1,
        1.0
    );
}

#[test]
fn forward_matrix_maps_white_balanced_camera_to_d50() {
    let fm = Mat3::from_rows([[0.6, 0.3, 0.06], [0.25, 0.8, -0.05], [0.02, -0.1, 0.9]]);
    let p = DcpProfile {
        forward_matrices: [Some(fm), None],
        ..single_d65()
    };
    let r = p.render(neutral_of(&cm_d65(), Xy::new(0.4, 0.39))).unwrap();
    assert!(r.uses_forward_matrix);
    // カメラ → ProPhoto は、正規化した ForwardMatrix と同じ（白によらない）。
    let expected = XYZ_D50_TO_PROPHOTO.mul_mat(&normalize_forward_matrix(&fm).unwrap());
    assert!(r.camera_to_prophoto.max_abs_diff(&expected) < 1e-6);
    let r2 = p.render(neutral_of(&cm_d65(), D65)).unwrap();
    assert!(r2.camera_to_prophoto.max_abs_diff(&r.camera_to_prophoto) < 1e-12);
    // 光源が 2 つで ForwardMatrix が片方だけなら使わない。
    let half = DcpProfile {
        forward_matrices: [Some(fm), None],
        ..dual()
    };
    assert!(
        !half
            .render(neutral_of(&cm_d65(), D65))
            .unwrap()
            .uses_forward_matrix
    );
}

#[test]
fn identity_table_keeps_colors() {
    for srgb in [false, true] {
        for t in [
            identity_table([90, 30, 1], srgb),
            identity_table([36, 8, 16], srgb),
        ] {
            for c in [
                [0.5f32, 0.2, 0.1],
                [0.1, 0.6, 0.3],
                [0.2, 0.25, 0.9],
                [2.5, 1.0, 0.4],
                [0.3, 0.3, 0.3],
                [0.8, -0.05, 0.1],
                [1e-6, 2e-6, 3e-6],
            ] {
                assert_close3_f32(t.apply(c), c, 2e-6);
            }
            // 0 以下・有限でない値は変えない。
            for c in [[0.0f32; 3], [-0.1, -0.2, -0.3], [f32::NAN, 0.1, 0.1]] {
                let out = t.apply(c);
                assert_eq!(out.map(f32::to_bits), c.map(f32::to_bits));
            }
        }
    }
}

#[test]
fn hue_shift_saturation_and_value_scales() {
    // 色相のずれ 60 度: 赤（色相 0）→ 黄（色相 1）。
    let n = 6 * 2;
    let t = HsvTable::new([6, 2, 1], false, vec![[60.0, 1.0, 1.0]; n]).unwrap();
    assert_close3_f32(t.apply([1.0, 0.0, 0.0]), [1.0, 1.0, 0.0], 1e-6);
    // 彩度 0.5 倍: (1, 0, 0) → s = 0.5 → (1, 0.5, 0.5)。
    let t = HsvTable::new([6, 2, 1], false, vec![[0.0, 0.5, 1.0]; n]).unwrap();
    assert_close3_f32(t.apply([1.0, 0.0, 0.0]), [1.0, 0.5, 0.5], 1e-6);
    // 彩度を上げても 1 を超えない。
    let t = HsvTable::new([6, 2, 1], false, vec![[0.0, 3.0, 1.0]; n]).unwrap();
    assert_close3_f32(t.apply([1.0, 0.5, 0.5]), [1.0, 0.0, 0.0], 1e-6);
    // 色域の外（負の値）は、彩度を上げない（そのまま）。
    let out = t.apply([1.0, -0.1, 0.2]);
    assert_close3_f32(out, [1.0, -0.1, 0.2], 1e-6);
    // 明度の倍率（リニア）。1 を超えても切りそろえない。
    let t = HsvTable::new([6, 2, 1], false, vec![[0.0, 1.0, 1.5]; n]).unwrap();
    assert_close3_f32(t.apply([2.0, 1.0, 1.0]), [3.0, 1.5, 1.5], 1e-6);
    // 明度の倍率（sRGB で符号化した値に掛ける）。
    let t = HsvTable::new([6, 2, 1], true, vec![[0.0, 1.0, 0.5]; n]).unwrap();
    let v = srgb_decode_f32(srgb_encode_f32(0.6) * 0.5);
    assert_close3_f32(t.apply([0.6, 0.6, 0.6]), [v, v, v], 1e-6);
    // 中立色は色相・彩度の値に左右されない。
    let t = HsvTable::new([6, 2, 1], false, vec![[45.0, 2.0, 1.0]; n]).unwrap();
    assert_eq!(t.apply([0.4; 3]), [0.4; 3]);
}

#[test]
fn table_lookup_interpolates_and_wraps_the_hue() {
    // 色相の分割 4（0・1.5・3・4.5）で、最後の分割の値だけ 40 度。色相 5.9 は最後（4.5）と最初（6 ≡ 0）の間。
    let mut e = vec![[0.0f32, 1.0, 1.0]; 4 * 2];
    e[3 * 2] = [40.0, 1.0, 1.0];
    e[3 * 2 + 1] = [40.0, 1.0, 1.0];
    let t = HsvTable::new([4, 2, 1], false, e).unwrap();
    let [dh, ..] = t.lookup(5.25, 0.5, 1.0);
    assert!((dh - 20.0).abs() < 1e-4, "{dh}");
    let [dh, ..] = t.lookup(4.5, 0.5, 1.0);
    assert!((dh - 40.0).abs() < 1e-4, "{dh}");
    let [dh, ..] = t.lookup(0.0, 0.5, 1.0);
    assert!(dh.abs() < 1e-4, "{dh}");
    // 色相 6 の手前と 0 の直後で連続。
    let a = t.lookup(5.9999, 0.5, 1.0)[0];
    let b = t.lookup(0.0001, 0.5, 1.0)[0];
    assert!((a - b).abs() < 0.01, "{a} {b}");
    // 3 次元: 明度の分割 2 の面ごとに違う倍率。符号化がリニアなら、明度 0.25 で 1/4 の重み。
    let mut e = vec![[0.0f32, 1.0, 1.0]; 2 * 2 * 2];
    for x in &mut e[4..] {
        *x = [0.0, 1.0, 2.0];
    }
    let t = HsvTable::new([2, 2, 2], false, e.clone()).unwrap();
    let [.., kv] = t.lookup(1.0, 0.5, 0.25);
    assert!((kv - 1.25).abs() < 1e-6, "{kv}");
    // 1 を超える明度は最後の面の値。
    let [.., kv] = t.lookup(1.0, 0.5, 4.0);
    assert!((kv - 2.0).abs() < 1e-6, "{kv}");
    // sRGB の符号化なら、符号化した明度の位置。
    let t = HsvTable::new([2, 2, 2], true, e).unwrap();
    let [.., kv] = t.lookup(1.0, 0.5, 0.2);
    let w = srgb_encode_f32(0.2);
    assert!((kv - (1.0 + w)).abs() < 1e-6, "{kv}");
}

#[test]
fn interpolated_maps_and_rendering_with_a_map() {
    let n = 6 * 2;
    let a = HsvTable::new([6, 2, 1], false, vec![[10.0, 1.2, 1.0]; n]).unwrap();
    let b = HsvTable::new([6, 2, 1], false, vec![[-10.0, 0.8, 1.0]; n]).unwrap();
    let m = HsvTable::interpolate(&a, &b, 0.25).unwrap();
    assert!((m.entries()[0][0] - -5.0).abs() < 1e-6);
    assert!((m.entries()[0][1] - 0.9).abs() < 1e-6);
    // 分割の数が違えば補間できない。
    let c = HsvTable::new([3, 2, 1], false, vec![[0.0, 1.0, 1.0]; 6]).unwrap();
    assert!(HsvTable::interpolate(&a, &c, 0.5).is_err());
    // プロファイルに 2 つのマップがあれば、行列と同じ重みで補間する。
    let p = DcpProfile {
        hue_sat_maps: [Some(a.clone()), Some(b.clone())],
        ..dual()
    };
    let r = p
        .render(neutral_of(&cm_d65(), Xy::new(0.38, 0.38)))
        .unwrap();
    let expected = HsvTable::interpolate(&a, &b, r.weight1).unwrap();
    assert_eq!(r.hue_sat_map.as_ref(), Some(&expected));
    // 何もしない表は使わない。
    let p = DcpProfile {
        hue_sat_maps: [Some(identity_table([6, 2, 1], false)), None],
        ..single_d65()
    };
    assert!(
        p.render(neutral_of(&cm_d65(), D65))
            .unwrap()
            .hue_sat_map
            .is_none()
    );
}

#[test]
fn table_values_are_validated() {
    assert!(HsvTable::new([0, 2, 1], false, vec![]).is_err());
    assert!(HsvTable::new([1, 1, 1], false, vec![[0.0, 1.0, 1.0]]).is_err());
    assert!(HsvTable::new([2, 2, 1], false, vec![[0.0, 1.0, 1.0]; 3]).is_err());
    for bad in [
        [f32::NAN, 1.0, 1.0],
        [400.0, 1.0, 1.0],
        [0.0, -1.0, 1.0],
        [0.0, 1.0, f32::INFINITY],
    ] {
        let mut e = vec![[0.0, 1.0, 1.0]; 4];
        e[2] = bad;
        assert!(HsvTable::new([2, 2, 1], false, e).is_err(), "{bad:?}");
    }
    assert!(HsvTable::new([2048, 1024, 2], false, vec![]).is_err());
}

#[test]
fn malformed_files_are_rejected() {
    let good = single_d65().to_bytes();
    DcpProfile::parse(&good).unwrap();
    // 印が違う（TIFF の 42）。
    let mut tiff = good.clone();
    tiff[2..4].copy_from_slice(&42u16.to_le_bytes());
    let e = DcpProfile::parse(&tiff).unwrap_err().to_string();
    assert!(e.contains("TIFF"), "{e}");
    assert!(DcpProfile::parse(b"XXRC\0\0\0\0").is_err());
    assert!(DcpProfile::parse(b"II").is_err());
    // 途中で切れている。
    for len in [8, 12, good.len() / 2, good.len() - 1] {
        assert!(DcpProfile::parse(&good[..len]).is_err(), "{len}");
    }
    // ColorMatrix1 がない。
    let no_cm = DcpProfile {
        color_matrices: [None, Some(cm_d65())],
        ..single_d65()
    };
    let e = DcpProfile::parse(&no_cm.to_bytes())
        .unwrap_err()
        .to_string();
    assert!(e.contains("ColorMatrix1"), "{e}");
    // 特異な行列。
    let singular = DcpProfile {
        color_matrices: [
            Some(Mat3::from_rows([
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
            ])),
            None,
        ],
        ..single_d65()
    };
    assert!(DcpProfile::parse(&singular.to_bytes()).is_err());
    // マップの次元がない。
    let mut w = TiffWriter::new(false);
    w.srationals(tags::COLOR_MATRIX_1, &cm_d65().rows().concat());
    w.floats(tags::PROFILE_HUE_SAT_MAP_DATA_1, &[0.0, 1.0, 1.0]);
    assert!(DcpProfile::parse(&w.finish()).is_err());
    // 分母が 0。
    let mut bytes = good.clone();
    let pos = find_tag_value_offset(&bytes, tags::COLOR_MATRIX_1);
    bytes[pos + 4..pos + 8].copy_from_slice(&0u32.to_le_bytes());
    let e = DcpProfile::parse(&bytes).unwrap_err().to_string();
    assert!(e.contains("分母"), "{e}");
    // 大きすぎるファイル。
    assert!(DcpProfile::parse(&vec![0u8; MAX_DCP_BYTES + 1]).is_err());
    // 3 つ目の光源は使わないことを残す。
    let mut w = TiffWriter::new(false);
    w.srationals(tags::COLOR_MATRIX_1, &cm_d65().rows().concat());
    w.short(tags::CALIBRATION_ILLUMINANT_3, 23);
    let p = DcpProfile::parse(&w.finish()).unwrap();
    assert_eq!(p.ignored.len(), 1);
}

/// ファイルの中のタグ `tag` の値の位置（リトルエンディアン・値が 4 バイトを超えるタグ）。
fn find_tag_value_offset(bytes: &[u8], tag: u16) -> usize {
    let ifd = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let n = u16::from_le_bytes(bytes[ifd..ifd + 2].try_into().unwrap()) as usize;
    for i in 0..n {
        let at = ifd + 2 + i * 12;
        if u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap()) == tag {
            return u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize;
        }
    }
    panic!("タグ {tag} がない");
}

#[test]
fn camera_names_are_matched_loosely() {
    let p = single_d65();
    assert!(p.matches_camera("SONY ILCE-7M4"));
    assert!(p.matches_camera("sony  ilce-7m4"));
    assert!(p.matches_camera("ILCE-7M4"));
    assert!(!p.matches_camera("SONY ILCE-7C"));
    assert!(!p.matches_camera("ILCE-7M"));
    assert!(!p.matches_camera(""));
    assert!(camera_names_match("Nikon Z 6", "NIKON Z 6"));
    assert!(!camera_names_match("Canon EOS R", "Canon EOS R5"));
    assert!(!camera_names_match("Canon EOS R5", "5"));
    let none = DcpProfile {
        unique_camera_model: None,
        ..single_d65()
    };
    assert!(!none.matches_camera("SONY ILCE-7M4"));
}

#[test]
fn illuminant_temperatures_and_names() {
    assert_eq!(illuminant_temperature(17), Some(2850.0));
    assert_eq!(illuminant_temperature(21), Some(6500.0));
    assert_eq!(illuminant_temperature(23), Some(5000.0));
    assert_eq!(illuminant_temperature(12), Some(6400.0));
    assert_eq!(illuminant_temperature(0), None);
    assert_eq!(illuminant_temperature(255), None);
    assert!(illuminant_name(21).contains("D65"));
    assert!(illuminant_name(99).contains("不明"));
}

#[test]
fn invalid_neutrals_are_errors() {
    let p = single_d65();
    for n in [[0.0, 1.0, 1.0], [f64::NAN, 1.0, 1.0], [-1.0, 1.0, 1.0]] {
        assert!(p.render(n).is_err(), "{n:?}");
    }
}
