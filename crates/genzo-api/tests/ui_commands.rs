//! M4（UI）のために足したコマンドの結合テスト: 絞り込みの候補（[`genzo_api::Core::facets`]）、
//! 切り抜きの編集中の表示（[`genzo_api::Core::set_develop_view`]）とフル解像度の出力の寸法
//! （[`genzo_api::FrameInfo::output_width`]）、描画が実際に使う切り抜き（[`genzo_api::Core::constrain_crop`]）、
//! WB のスポイト（DEV-03。[`genzo_api::Core::white_balance_at`]）。

mod common;

use std::path::Path;

use common::{Env, WAIT_LIMIT, all_items, by_name, exif, hashes, import, write_jpeg};
use genzo_api::{
    ApiError, Core, DevelopSettings, FrameInfo, RegionRequest, RegionSpace, RenderBackend,
    SearchFilter, SearchSort, VariantId,
};
use genzo_model::{CropRect, WhiteBalance};

/// 設定を変えて、その世代のプレビューを待つ。
fn render(core: &Core, s: &DevelopSettings) -> FrameInfo {
    let g = core.set_settings(s, false).unwrap();
    wait_frame(core, g)
}

fn wait_frame(core: &Core, generation: u64) -> FrameInfo {
    let f = core
        .wait_for_frame(generation, WAIT_LIMIT)
        .unwrap()
        .expect("描けている");
    assert!(f.generation >= generation);
    f
}

/// 表示範囲の描画（出力座標の左上の小さな範囲）が報告する、出力の画像全体の寸法。
fn region_full_size(core: &Core, v: VariantId) -> (u32, u32) {
    let r = core
        .render_region(&RegionRequest {
            variant_id: v,
            space: RegionSpace::Output,
            x: 0.0,
            y: 0.0,
            width: 16.0,
            height: 16.0,
            scale: 1.0,
        })
        .unwrap();
    (r.info.full_width, r.info.full_height)
}

fn rotated_crop(angle: f32, crop: CropRect) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.geometry.angle_deg = angle;
    s.geometry.crop = crop;
    s
}

/// 絞り込みの候補の値は検索の条件にそのまま渡して一致し、件数は asset の数（仮想コピーは数えない）。
/// JSON のコマンドでも同じ形で返す。
#[test]
fn facets_list_values_usable_in_filters() {
    let env = Env::new();
    let with_lens = |lens: &str| {
        let mut e = exif(Some("2024-05-01 10:00:00"), None, None);
        e.lens_model = Some(lens.to_owned());
        e
    };
    write_jpeg(
        &env.photos.join("A.jpg"),
        (32, 24),
        1,
        Some(&with_lens("Lens A")),
    );
    write_jpeg(
        &env.photos.join("B.jpg"),
        (32, 24),
        2,
        Some(&with_lens("Lens B")),
    );
    // カメラ・レンズの分からない写真（候補にしない）。
    write_jpeg(&env.photos.join("C.jpg"), (32, 24), 3, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    core.create_virtual_copy(by_name(&items, "A.jpg").variant_id, None)
        .unwrap();

    let facets = core.facets().unwrap();
    let pairs = |v: &[genzo_api::FacetValue]| -> Vec<(String, u64)> {
        v.iter().map(|f| (f.value.clone(), f.count)).collect()
    };
    assert_eq!(
        pairs(&facets.cameras),
        vec![("GenzoTest Synthetic".to_owned(), 2)]
    );
    assert_eq!(
        pairs(&facets.lenses),
        vec![("Lens A".to_owned(), 1), ("Lens B".to_owned(), 1)]
    );
    assert!(facets.codecs.is_empty());

    let count = |filter: SearchFilter| core.search(&filter, SearchSort::default()).unwrap().count;
    let camera = facets.cameras[0].value.clone();
    assert_eq!(
        count(SearchFilter {
            cameras: Some(vec![camera.clone()]),
            masters_only: true,
            ..Default::default()
        }),
        facets.cameras[0].count
    );
    assert_eq!(
        count(SearchFilter {
            cameras: Some(vec![camera]),
            ..Default::default()
        }),
        3,
        "仮想コピーを含めた variant の数は、asset の数より多い"
    );
    assert_eq!(
        count(SearchFilter {
            lenses: Some(vec!["Lens A".to_owned()]),
            ..Default::default()
        }),
        2
    );

    let json: serde_json::Value =
        serde_json::from_str(&core.execute_json(r#"{"type":"facets"}"#)).unwrap();
    assert_eq!(json["ok"]["type"], "facets");
    assert_eq!(
        json["ok"]["cameras"],
        serde_json::json!([{"value": "GenzoTest Synthetic", "count": 2}])
    );
    assert_eq!(json["ok"]["codecs"], serde_json::json!([]));
    core.close().unwrap();
}

/// 切り抜きの編集中の表示: 切り抜く前の全体（G の外接矩形）を描き、設定・履歴は変えない。開き直すと
/// 元に戻る。プレビューのフル解像度の出力の寸法は、表示範囲の描画の座標の格子と同じ。
#[test]
fn develop_view_and_output_size_follow_region_rendering() {
    let env = Env::new();
    // プレビュー（長辺 2560）より大きい写真（フル解像度とプレビューの格子が違う。デバッグビルドでも
    // 時間がかからないよう、細長くする）。
    write_jpeg(&env.photos.join("BIG.jpg"), (2600, 400), 4, None);
    write_jpeg(&env.photos.join("OTHER.jpg"), (64, 48), 5, None);
    let before = hashes(&env.photos);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let v = by_name(&items, "BIG.jpg").variant_id;
    assert!(matches!(
        core.set_develop_view(true),
        Err(ApiError::NoDevelopSession)
    ));
    core.open_develop(v).unwrap();

    // 切り抜き・回転なし: 出力はフル解像度の画像そのもの、プレビューは縮小。
    let f = render(&core, &DevelopSettings::default());
    assert!(!f.uncropped);
    assert_eq!((f.output_width, f.output_height), (2600, 400));
    assert_eq!(f.width, 2560);
    assert_eq!(region_full_size(&core, v), (2600, 400));

    // 回転と切り抜き。
    let s = rotated_crop(
        6.0,
        CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.6,
            bottom: 0.9,
        },
    );
    let cropped = render(&core, &s);
    assert!(!cropped.uncropped);
    let out = (cropped.output_width, cropped.output_height);
    assert!(
        out.0 > 0 && out.0 < 2600 && out.1 > 0 && out.1 < 400,
        "{out:?}"
    );
    assert_eq!(region_full_size(&core, v), out);
    core.flush_develop().unwrap();
    let history = core.history(v).unwrap().len();

    // 切り抜く前の全体の表示。
    let g = core.set_develop_view(true).unwrap();
    assert!(g > cropped.generation, "描き直す");
    let whole = wait_frame(&core, g);
    assert!(whole.uncropped);
    assert_eq!(
        whole.backend,
        RenderBackend::Cpu,
        "GPU 版はまだ対応していない"
    );
    assert!(whole.width > cropped.width && whole.height > cropped.height);
    let full = (whole.output_width, whole.output_height);
    assert!(full.0 > 2600 && full.1 > 400, "回転した外接矩形: {full:?}");
    assert_eq!(region_full_size(&core, v), full);
    // 回転で画像の外になった四隅は黒で塗り、ヒストグラムに数えない。
    let px = core.develop_frame().unwrap();
    assert!(px.info.uncropped);
    assert_eq!(&px.display_rgb8[..3], &[0, 0, 0]);
    assert!(whole.histogram.pixel_count < u64::from(whole.width) * u64::from(whole.height));
    // 同じ値なら描き直さない。
    assert_eq!(core.set_develop_view(true).unwrap(), whole.generation);
    // 設定・履歴は変わらない。
    let state = core.develop_state().unwrap().unwrap();
    assert_eq!(state.settings.geometry, s.geometry);
    assert!(!state.dirty);
    assert_eq!(core.history(v).unwrap().len(), history);
    // JSON のコマンド。
    let json: serde_json::Value = serde_json::from_str(
        &core.execute_json(r#"{"type":"set_develop_view","uncropped":false}"#),
    )
    .unwrap();
    assert_eq!(json["ok"]["type"], "ok");
    let back = wait_frame(&core, core.develop_state().unwrap().unwrap().generation);
    assert!(!back.uncropped);
    assert_eq!((back.output_width, back.output_height), out);

    // 開き直すと（同じ写真でも）元の表示に戻る。
    core.set_develop_view(true).unwrap();
    let st = core.open_develop(v).unwrap();
    let reopened = wait_frame(&core, st.generation);
    assert!(!reopened.uncropped);
    assert_eq!(region_full_size(&core, v), out);
    // 別の写真を開いても。
    core.set_develop_view(true).unwrap();
    let other = by_name(&items, "OTHER.jpg").variant_id;
    let st = core.open_develop(other).unwrap();
    assert!(!wait_frame(&core, st.generation).uncropped);
    core.close().unwrap();
    assert_eq!(hashes(&env.photos), before);
}

/// 描画が実際に使う切り抜き（回転で画像の外に出ないよう制限したもの）。返した切り抜きで描くと、全体の
/// 切り抜きで描いたのと同じ出力になる。
#[test]
fn constrain_crop_returns_the_crop_used_for_rendering() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (300, 200), 6, None);
    write_jpeg(&env.photos.join("B.jpg"), (300, 200), 7, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let v = by_name(&items, "A.jpg").variant_id;
    let other = by_name(&items, "B.jpg").variant_id;
    assert!(matches!(
        core.constrain_crop(v, &CropRect::FULL, 5.0),
        Err(ApiError::NoDevelopSession)
    ));
    core.open_develop(v).unwrap();
    // 回転なしなら、そのまま。
    assert_eq!(
        core.constrain_crop(v, &CropRect::FULL, 0.0).unwrap(),
        CropRect::FULL
    );
    // 回転すると、中心と縦横比を保って画像の内側に縮める。
    let c = core.constrain_crop(v, &CropRect::FULL, 8.0).unwrap();
    assert!(
        c.left > 0.0 && c.right < 1.0 && c.top > 0.0 && c.bottom < 1.0,
        "{c:?}"
    );
    assert!(((c.left + c.right) * 0.5 - 0.5).abs() < 1e-4);
    assert!(((c.top + c.bottom) * 0.5 - 0.5).abs() < 1e-4);
    // 返した切り抜きは制限し直しても変わらない。
    let again = core.constrain_crop(v, &c, 8.0).unwrap();
    for (a, b) in [
        (again.left, c.left),
        (again.top, c.top),
        (again.right, c.right),
        (again.bottom, c.bottom),
    ] {
        assert!((a - b).abs() < 1e-6, "{again:?} / {c:?}");
    }
    let full = render(&core, &rotated_crop(8.0, CropRect::FULL));
    let constrained = render(&core, &rotated_crop(8.0, c));
    assert_eq!(
        (full.output_width, full.output_height),
        (constrained.output_width, constrained.output_height)
    );
    assert_eq!(
        (full.width, full.height),
        (constrained.width, constrained.height)
    );
    // 角度は ±45 度に、切り抜きは各軸 0〜1 に丸めて使う。
    assert_eq!(
        core.constrain_crop(v, &CropRect::FULL, 90.0).unwrap(),
        core.constrain_crop(v, &CropRect::FULL, 45.0).unwrap()
    );
    // 数でない値・別の写真。
    let nan = CropRect {
        left: f32::NAN,
        ..CropRect::FULL
    };
    assert!(matches!(
        core.constrain_crop(v, &nan, 5.0),
        Err(ApiError::InvalidArgument(_))
    ));
    assert!(matches!(
        core.constrain_crop(v, &CropRect::FULL, f32::INFINITY),
        Err(ApiError::InvalidArgument(_))
    ));
    assert!(matches!(
        core.constrain_crop(other, &CropRect::FULL, 5.0),
        Err(ApiError::NoDevelopSession)
    ));
    // JSON のコマンド。
    let json: serde_json::Value = serde_json::from_str(&core.execute_json(&format!(
        r#"{{"type":"constrain_crop","variant_id":{},"crop":{{"left":0,"top":0,"right":1,"bottom":1}},"angle_deg":8.0}}"#,
        v.get()
    )))
    .unwrap();
    assert_eq!(json["ok"]["type"], "crop");
    assert!((json["ok"]["crop"]["left"].as_f64().unwrap() - f64::from(c.left)).abs() < 1e-6);
    core.close().unwrap();
}

/// WB のスポイトの合成 DNG（LibRaw が有効なときだけ書く）: 左半分は撮影時の WB で中立の灰色、右半分は
/// 青みのある灰色。撮影時の WB は genzo-testkit の既定（R 2・B 1.5）。
fn make_wb_dng(path: &Path) -> bool {
    if !genzo_raw::LIBRAW_ENABLED {
        eprintln!("LibRaw が無効な build のため、DNG の部分を飛ばす");
        return false;
    }
    use genzo_testkit::bayer::{BayerParams, mosaic, working_to_camera};
    use genzo_testkit::image::FloatImage;
    const CAM_XYZ: [[f32; 3]; 3] = [
        [0.7424, -0.2329, -0.0466],
        [-0.4598, 1.2471, 0.2347],
        [-0.0715, 0.1505, 0.6066],
    ];
    let (w, h) = (128u32, 96u32);
    let scene = FloatImage::from_rgb_fn(w, h, |x, _| {
        if x < w / 2 {
            [0.18, 0.18, 0.18]
        } else {
            [0.14, 0.18, 0.26]
        }
    })
    .unwrap();
    let camera = working_to_camera(&scene, &CAM_XYZ).unwrap();
    let params = BayerParams {
        cam_xyz: Some(CAM_XYZ),
        ..BayerParams::default()
    };
    let raw = mosaic(&camera, &params).unwrap();
    genzo_raw::dng::write_file(path, &raw, &genzo_raw::dng::DngOptions::default()).unwrap();
    true
}

/// WB のスポイト: 灰色の面の上の点を選ぶと、その面を中立にする WB（撮影時の WB と同じ光源なので撮影時の
/// 値）が返る。今の WB によらず、切り抜き・回転があっても同じ被写体の点を指す。適用はしない。
#[test]
fn white_balance_picker_neutralizes_the_picked_point() {
    let env = Env::new();
    write_jpeg(&env.photos.join("J.jpg"), (64, 48), 8, None);
    let has_raw = make_wb_dng(&env.photos.join("R.dng"));
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let jpeg = by_name(&items, "J.jpg").variant_id;
    assert!(matches!(
        core.white_balance_at(jpeg, 0.5, 0.5, None),
        Err(ApiError::NoDevelopSession)
    ));
    // RAW 以外は WB の設定を使わないので、エラー（日本語の説明）。
    core.open_develop(jpeg).unwrap();
    match core.white_balance_at(jpeg, 0.5, 0.5, None) {
        Err(ApiError::InvalidArgument(m)) => assert!(m.contains("RAW"), "{m}"),
        other => panic!("{other:?}"),
    }
    if !has_raw {
        core.close().unwrap();
        return;
    }
    let v = by_name(&items, "R.dng").variant_id;
    let state = core.open_develop(v).unwrap();
    assert!(state.source.is_raw);
    let as_shot = state.source.as_shot_white_balance.expect("撮影時の WB");
    assert!(matches!(
        core.white_balance_at(jpeg, 0.5, 0.5, None),
        Err(ApiError::NoDevelopSession)
    ));
    let near_as_shot = |p: genzo_api::PickedWhiteBalance, what: &str| {
        assert!(
            (p.temperature_k - as_shot.temperature_k).abs() < 20.0
                && (p.tint - as_shot.tint).abs() < 0.5,
            "{what}: {p:?} / 撮影時 {as_shot:?}"
        );
    };
    // 灰色の面（左半分）。
    near_as_shot(
        core.white_balance_at(v, 0.25, 0.5, None).unwrap(),
        "撮影時の WB",
    );
    // 青みのある面（右半分）は、それを中立にする値（光源が青寄りとみなすので色温度が高い）。
    let blue = core.white_balance_at(v, 0.75, 0.5, None).unwrap();
    eprintln!("撮影時の WB {as_shot:?}、青みの面を選んだ WB {blue:?}");
    assert!(
        blue.temperature_k > as_shot.temperature_k + 300.0,
        "{blue:?} / {as_shot:?}"
    );
    // 今の WB を変えても、同じ点なら同じ値（適用はしない: 設定は変わらない）。
    let custom = DevelopSettings {
        white_balance: WhiteBalance::Custom {
            temperature_k: 3200.0,
            tint: 25.0,
        },
        ..Default::default()
    };
    render(&core, &custom);
    near_as_shot(
        core.white_balance_at(v, 0.25, 0.5, None).unwrap(),
        "Custom の WB",
    );
    assert_eq!(
        core.develop_state()
            .unwrap()
            .unwrap()
            .settings
            .white_balance,
        custom.white_balance
    );
    // 切り抜き・回転: 左半分だけを切り抜くと、出力の中心は灰色の面、右半分だけなら青みの面。
    let left = rotated_crop(
        4.0,
        CropRect {
            left: 0.05,
            top: 0.2,
            right: 0.45,
            bottom: 0.8,
        },
    );
    render(&core, &left);
    near_as_shot(
        core.white_balance_at(v, 0.5, 0.5, None).unwrap(),
        "左を切り抜き",
    );
    let right = rotated_crop(
        -4.0,
        CropRect {
            left: 0.55,
            top: 0.2,
            right: 0.95,
            bottom: 0.8,
        },
    );
    render(&core, &right);
    let p = core.white_balance_at(v, 0.5, 0.5, None).unwrap();
    assert!(
        (p.temperature_k - blue.temperature_k).abs() < 20.0,
        "{p:?} / {blue:?}"
    );
    let near_blue = |p: genzo_api::PickedWhiteBalance, what: &str| {
        assert!(
            (p.temperature_k - blue.temperature_k).abs() < 20.0,
            "{what}: {p:?} / {blue:?}"
        );
    };
    // 切り抜いた後の画像（角度補正あり）の四隅・端（0・1 ちょうど）も選べる（画像の内側に収めて求める）。
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 1.0)] {
        near_blue(
            core.white_balance_at(v, x, y, None).unwrap(),
            &format!("切り抜いた後の端 ({x}, {y})"),
        );
    }
    // 切り抜く前の全体の表示では、全体の画像に対する割合（左の 4 分の 1 は灰色の面）。回転で画像の外に
    // なった角はエラー。
    let g = core.set_develop_view(true).unwrap();
    assert!(wait_frame(&core, g).uncropped);
    near_as_shot(
        core.white_balance_at(v, 0.25, 0.5, None).unwrap(),
        "全体の表示",
    );
    assert!(matches!(
        core.white_balance_at(v, 0.0, 0.0, None),
        Err(ApiError::InvalidArgument(_))
    ));
    // 表示を切り替えた直後: UI がクリックした画像（まだ見せている全体の表示のプレビュー）の表示を送れば、
    // その画像の上の点として求める（切り抜いた表示のプレビューが届く前でも後でも）。
    let g = core.set_develop_view(false).unwrap();
    near_as_shot(
        core.white_balance_at(v, 0.25, 0.5, Some(true)).unwrap(),
        "切り替えの直後に全体の表示の上で選んだ点",
    );
    assert!(!wait_frame(&core, g).uncropped);
    near_as_shot(
        core.white_balance_at(v, 0.25, 0.5, Some(true)).unwrap(),
        "切り抜いた表示が届いた後に、前の全体の表示の上で選んだ点",
    );
    // 省略すると最新のプレビュー（切り抜いた表示。右半分だけ）の上の点（同じ割合でも青みの面）。
    near_blue(
        core.white_balance_at(v, 0.25, 0.5, None).unwrap(),
        "切り抜いた表示",
    );
    near_blue(
        core.white_balance_at(v, 0.25, 0.5, Some(false)).unwrap(),
        "切り抜いた表示（明示）",
    );
    // 範囲の外の点。
    assert!(matches!(
        core.white_balance_at(v, 1.5, 0.5, None),
        Err(ApiError::InvalidArgument(_))
    ));
    // JSON のコマンド（UI の形）。
    let json: serde_json::Value = serde_json::from_str(&core.execute_json(&format!(
        r#"{{"type":"white_balance_at","variant_id":{},"x":0.5,"y":0.5}}"#,
        v.get()
    )))
    .unwrap();
    assert_eq!(json["ok"]["type"], "white_balance", "{json}");
    assert!(json["ok"]["temperature_k"].as_f64().unwrap() > 0.0);
    assert!(json["ok"]["tint"].is_number());
    // クリックした画像の表示を送る形（省略可）。
    let json: serde_json::Value = serde_json::from_str(&core.execute_json(&format!(
        r#"{{"type":"white_balance_at","variant_id":{},"x":0.25,"y":0.5,"uncropped":true}}"#,
        v.get()
    )))
    .unwrap();
    assert_eq!(json["ok"]["type"], "white_balance", "{json}");
    assert!(
        (json["ok"]["temperature_k"].as_f64().unwrap() - f64::from(as_shot.temperature_k)).abs()
            < 20.0,
        "{json} / {as_shot:?}"
    );
    core.close().unwrap();
}
