//! v1 のパッケージ「compare」の結合テスト: 比較表示（LIB-03）の表示範囲の描画（面ごとに任意の写真を描く・
//! 展開した入力の上限と解放・現像中の写真の設定・保存した設定・面ごとの「最新の 1 件だけ」・見つからない
//! ファイル・JSON のコマンド）。

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Env, all_items, by_name, import, write_jpeg};
use genzo_api::{
    ApiError, Core, MAX_VIEW_PANES, MAX_VIEW_SOURCES, RegionRequest, RegionSpace, SettingGroups,
    VariantId,
};

fn region(v: VariantId, x: f64, y: f64, w: f64, h: f64) -> RegionRequest {
    RegionRequest {
        variant_id: v,
        space: RegionSpace::Output,
        x,
        y,
        width: w,
        height: h,
        scale: 1.0,
    }
}

/// 3 枚の写真（寸法と絵柄が違う）を取り込む。
fn three_photos(env: &Env) -> (Core, VariantId, VariantId, VariantId) {
    write_jpeg(&env.photos.join("A.jpg"), (300, 200), 3, None);
    write_jpeg(&env.photos.join("B.jpg"), (240, 160), 7, None);
    write_jpeg(&env.photos.join("C.jpg"), (120, 90), 11, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;
    let c = by_name(&items, "C.jpg").variant_id;
    (core, a, b, c)
}

/// 現像中でない写真も面ごとに描け、結果は現像のセッションで描いたものと同じ。展開した入力は上限まで持ち、
/// 比較表示をやめると捨てる。
#[test]
fn view_regions_render_any_photo_per_pane() {
    let env = Env::new();
    let (core, a, b, c) = three_photos(&env);
    assert_eq!(MAX_VIEW_PANES, 2);

    // 出力の大きさ（等倍表示の準備）。
    let size = core.view_output_size(0, a).unwrap();
    assert_eq!((size.variant_id, size.width, size.height), (a, 300, 200));
    assert_eq!(
        (
            core.view_output_size(1, b).unwrap().width,
            core.view_output_size(1, b).unwrap().height
        ),
        (240, 160)
    );

    // 2 つの面に別の写真を描く（現像のセッションは開かない）。
    let fa = core
        .render_view_region(0, &region(a, 0.0, 0.0, 300.0, 200.0))
        .unwrap();
    let fb = core
        .render_view_region(1, &region(b, 20.0, 10.0, 64.0, 48.0))
        .unwrap();
    assert_eq!(fa.info.pane, Some(0));
    assert_eq!(fb.info.pane, Some(1));
    assert_eq!((fa.info.variant_id, fb.info.variant_id), (a, b));
    assert_eq!(fa.info.generation, 0, "保存した設定で描いた");
    assert_eq!(
        (fb.info.x, fb.info.y, fb.info.width, fb.info.height),
        (20, 10, 64, 48)
    );
    assert_eq!((fb.info.full_width, fb.info.full_height), (240, 160));
    assert_eq!(core.view_sources(), 2);
    assert!(
        core.develop_state().unwrap().is_none(),
        "現像のセッションは開かない"
    );
    // 面ごとに最後の結果を取れる（要求の番号は面ごと）。
    assert_eq!(core.view_region_frame(0).unwrap().info.variant_id, a);
    assert_eq!(core.view_region_frame(1).unwrap().info.variant_id, b);
    assert_eq!(core.view_region_frame(1).unwrap().info.request, 1);
    assert!(core.view_region_frame(MAX_VIEW_PANES).is_none());

    // 3 枚目を描くと、最も古く使った入力を捨てる（上限 MAX_VIEW_SOURCES）。
    core.render_view_region(0, &region(c, 0.0, 0.0, 16.0, 16.0))
        .unwrap();
    assert_eq!(core.view_sources(), MAX_VIEW_SOURCES);

    // 現像のセッションで描いた結果と同じ画素。
    core.open_develop(a).unwrap();
    let dev = core
        .render_region(&region(a, 0.0, 0.0, 300.0, 200.0))
        .unwrap();
    assert_eq!(*dev.display_rgb8, *fa.display_rgb8);
    assert_eq!(dev.info.pane, None);
    core.close_develop().unwrap();

    // 不正な面・範囲。
    assert!(matches!(
        core.render_view_region(MAX_VIEW_PANES, &region(a, 0.0, 0.0, 8.0, 8.0)),
        Err(ApiError::InvalidArgument(_))
    ));
    assert!(matches!(
        core.render_view_region(0, &region(a, 0.0, 0.0, 0.0, 8.0)),
        Err(ApiError::InvalidArgument(_))
    ));
    assert!(matches!(
        core.view_output_size(0, VariantId::new(9999)),
        Err(ApiError::NotFound(_))
    ));

    // 比較表示をやめると、入力と結果を捨てる。
    assert_eq!(core.release_view().unwrap(), 2);
    assert_eq!(core.view_sources(), 0);
    assert!(core.view_region_frame(0).is_none());
    assert!(!core.cancel_view_regions().unwrap());
    core.close().unwrap();
}

/// 現像中の写真は今の設定（保存待ちを含む）とその世代で、それ以外は保存した設定で描く。仮想コピーは同じ入力を
/// 使う（展開し直さない）。
#[test]
fn view_regions_use_the_session_or_the_saved_settings() {
    let env = Env::new();
    let (core, a, b, _) = three_photos(&env);
    let before_b = core
        .render_view_region(1, &region(b, 0.0, 0.0, 240.0, 160.0))
        .unwrap();

    // 保存した設定を変えると、次の描画に反映する（画面は検索結果の develop_hash の変化で描き直す）。
    let mut s = core.develop_settings(b).unwrap();
    s.exposure_ev = 1.0;
    core.paste_settings(&s, &[b], SettingGroups::default())
        .unwrap();
    let after_b = core
        .render_view_region(1, &region(b, 0.0, 0.0, 240.0, 160.0))
        .unwrap();
    assert_ne!(*after_b.display_rgb8, *before_b.display_rgb8);
    assert!(after_b.info.request > before_b.info.request);

    // 現像中の写真は、セッションの今の設定（保存の前の変更を含む）と世代で描く。
    core.open_develop(a).unwrap();
    let mut s = core.develop_settings(a).unwrap();
    s.exposure_ev = -0.7;
    let generation = core.set_settings(&s, true).unwrap();
    let view = core
        .render_view_region(0, &region(a, 10.0, 10.0, 50.0, 40.0))
        .unwrap();
    let dev = core
        .render_region(&region(a, 10.0, 10.0, 50.0, 40.0))
        .unwrap();
    assert_eq!(view.info.generation, generation);
    assert_eq!(*view.display_rgb8, *dev.display_rgb8);
    core.end_drag().unwrap();

    // 現像中の写真の仮想コピーは、セッションの入力を使う（展開し直さない）。
    core.release_view().unwrap();
    let copy = core.create_virtual_copy(a, Some("比較")).unwrap();
    core.render_view_region(1, &region(copy, 0.0, 0.0, 32.0, 32.0))
        .unwrap();
    assert_eq!(core.view_sources(), 0, "セッションの入力を使った");
    core.close_develop().unwrap();
    // セッションを閉じた後は展開するが、マスターと仮想コピーは 1 つの入力を使う。
    core.render_view_region(0, &region(a, 0.0, 0.0, 32.0, 32.0))
        .unwrap();
    core.render_view_region(1, &region(copy, 0.0, 0.0, 32.0, 32.0))
        .unwrap();
    assert_eq!(core.view_sources(), 1);
    core.close().unwrap();
}

/// 面ごとの「最新の 1 件だけ」: 同じ面の古い要求は取り消され（または先に終わり）、別の面の要求は取り消さない。
#[test]
fn only_the_latest_request_of_each_pane_is_rendered() {
    let env = Env::new();
    write_jpeg(&env.photos.join("L.jpg"), (1600, 1200), 5, None);
    write_jpeg(&env.photos.join("M.jpg"), (800, 600), 9, None);
    let core = Arc::new(env.open());
    import(&core, &env.photos);
    let items = all_items(&core);
    let l = by_name(&items, "L.jpg").variant_id;
    let m = by_name(&items, "M.jpg").variant_id;
    // 入力を先に展開しておく（描画の重なりを確かめる）。
    core.view_output_size(0, l).unwrap();
    core.view_output_size(1, m).unwrap();

    let mut handles = Vec::new();
    for i in 0..3 {
        let core = Arc::clone(&core);
        handles.push(std::thread::spawn(move || {
            core.render_view_region(0, &region(l, 0.0, 0.0, 1600.0, 1200.0 - f64::from(i)))
        }));
        std::thread::sleep(Duration::from_millis(5));
    }
    let other = {
        let core = Arc::clone(&core);
        std::thread::spawn(move || core.render_view_region(1, &region(m, 0.0, 0.0, 800.0, 600.0)))
    };
    let last = core
        .render_view_region(0, &region(l, 8.0, 8.0, 16.0, 16.0))
        .expect("最新の要求は描ける");
    for h in handles {
        match h.join().unwrap() {
            Ok(f) => assert!(f.info.request < last.info.request),
            Err(e) => assert!(e.is_cancelled(), "{e:?}"),
        }
    }
    let other = other.join().unwrap().expect("別の面の要求は取り消さない");
    assert_eq!((other.info.width, other.info.height), (800, 600));
    assert_eq!(
        core.view_region_frame(0).unwrap().info.request,
        last.info.request
    );
    assert_eq!(core.view_region_frame(1).unwrap().info.variant_id, m);
    // 取り消しの後も、次の要求は描ける。
    core.cancel_view_regions().unwrap();
    assert!(
        core.render_view_region(0, &region(l, 0.0, 0.0, 4.0, 4.0))
            .is_ok()
    );
    core.close().unwrap();
}

/// 見つからないファイルは描けない（FileAccess。見つからない印が付く）。動画は描けない。JSON のコマンドでも使える。
#[test]
fn missing_files_videos_and_json_commands() {
    let env = Env::new();
    let (core, a, b, _) = three_photos(&env);
    std::fs::remove_file(env.photos.join("B.jpg")).unwrap();
    assert!(matches!(
        core.render_view_region(1, &region(b, 0.0, 0.0, 8.0, 8.0)),
        Err(ApiError::FileAccess { .. })
    ));
    assert_eq!(
        core.variant_details(b).unwrap().summary.file_status,
        genzo_api::FileStatus::Missing
    );

    if common::make_video(&env.photos.join("V.mp4"), "2024-05-01T03:04:05Z") {
        import(&core, &env.photos);
        let v = by_name(&all_items(&core), "V.mp4").variant_id;
        assert!(matches!(
            core.view_output_size(0, v),
            Err(ApiError::InvalidArgument(_))
        ));
    }

    let out = core.execute_json(&format!(
        r#"{{"type":"view_output_size","pane":1,"variant_id":{}}}"#,
        a.get()
    ));
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["ok"]["type"], "view_size", "{out}");
    assert_eq!(json["ok"]["width"], 300, "{out}");
    let out = core.execute_json(&format!(
        r#"{{"type":"render_view_region","pane":1,"request":{{"variant_id":{},"x":0,"y":0,"width":8,"height":4}}}}"#,
        a.get()
    ));
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["ok"]["type"], "region", "{out}");
    assert_eq!(json["ok"]["pane"], 1, "{out}");
    assert_eq!(json["ok"]["width"], 8, "{out}");
    // 現像の表示範囲の応答には面を入れない。
    core.open_develop(a).unwrap();
    let out = core.execute_json(&format!(
        r#"{{"type":"render_region","request":{{"variant_id":{},"x":0,"y":0,"width":8,"height":4}}}}"#,
        a.get()
    ));
    assert!(!out.contains("\"pane\""), "{out}");
    core.close_develop().unwrap();
    for t in ["cancel_view_regions", "release_view"] {
        let out = core.execute_json(&format!(r#"{{"type":"{t}"}}"#));
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(json["ok"]["type"], "ok", "{out}");
    }
    assert_eq!(core.view_sources(), 0);
    core.close().unwrap();
}

/// RAW（合成 DNG。LibRaw が有効なときだけ）も比較表示の面に描ける。
#[test]
fn raw_photos_are_rendered_in_view_panes() {
    let env = Env::new();
    if !common::make_dng(&env.photos.join("R.dng"), "2024:05:01 12:00:00") {
        return;
    }
    let core = env.open();
    import(&core, &env.photos);
    let r = by_name(&all_items(&core), "R.dng").variant_id;
    let size = core.view_output_size(0, r).unwrap();
    assert!(size.width > 0 && size.height > 0);
    let f = core
        .render_view_region(
            0,
            &region(
                r,
                0.0,
                0.0,
                f64::from(size.width.min(64)),
                f64::from(size.height.min(64)),
            ),
        )
        .unwrap();
    assert_eq!(
        (f.info.full_width, f.info.full_height),
        (size.width, size.height)
    );
    assert_eq!(core.view_sources(), 1);
    core.close().unwrap();
}
