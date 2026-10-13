//! 現像のセッション・Undo / Redo・一括適用・仮想コピー・サムネイルの作り直し・GPU を使わない設定の
//! 結合テスト（01 のストーリー 4〜6、DEV-00・DEV-27・DEV-30・LIB-13・PRV-02・DATA-01・DATA-03、
//! 04 の 2.2 節・4.1 節・6.2 節）。

mod common;

use common::{Env, WAIT_LIMIT, all_items, by_name, drain, hashes, import, wait_until, write_jpeg};
use genzo_api::{
    ApiError, DevelopSettings, Event, GpuMode, HISTORY_LABEL_PASTE, JobResult, PasteReport,
    PasteVersion, RenderBackend, RenderQuality, SettingGroups, VariantId,
};
use genzo_model::{CropRect, WhiteBalance};

fn exposure(ev: f32) -> DevelopSettings {
    DevelopSettings {
        exposure_ev: ev,
        ..Default::default()
    }
}

fn labels(core: &genzo_api::Core, v: VariantId) -> Vec<String> {
    core.history(v)
        .unwrap()
        .into_iter()
        .map(|h| h.label)
        .collect()
}

/// 履歴の項目へ 1 回で移る（DEV-27。履歴のパネル）: 保存・変更の知らせは 1 回だけで、Undo / Redo を繰り返した
/// のと同じ設定になる。項目がない・すでにその位置なら何もしない。保存待ちの変更は先に保存し、そのときに
/// 消えた項目へは移らない。
#[test]
fn go_to_history_moves_in_one_step() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    for ev in [0.5, 1.0, 1.5] {
        core.set_settings(&exposure(ev), true).unwrap();
        core.end_drag().unwrap();
    }
    let entries = core.history(v).unwrap();
    assert_eq!(entries.len(), 4);
    assert!(entries[3].is_current);

    let events = core.subscribe();
    assert_eq!(
        core.go_to_history(v, entries[0].id).unwrap(),
        Some(DevelopSettings::default())
    );
    let state = core.develop_state().unwrap().unwrap();
    assert_eq!(state.settings, DevelopSettings::default());
    assert!(!state.can_undo && state.can_redo);
    let develop_changes = drain(&events)
        .into_iter()
        .filter(|e| matches!(e, Event::CatalogChanged { change, .. } if *change == genzo_api::CatalogChange::Develop))
        .count();
    assert_eq!(develop_changes, 1, "変更の知らせは 1 回だけ");
    let after = core.history(v).unwrap();
    assert_eq!(after.len(), 4, "移るだけでは履歴は消えない");
    assert!(after[0].is_current);

    // 先へ移る（Redo を 2 回したのと同じ）。
    assert_eq!(
        core.go_to_history(v, entries[2].id).unwrap(),
        Some(exposure(1.0))
    );
    // すでにその位置・ない項目は何もしない。
    assert_eq!(core.go_to_history(v, entries[2].id).unwrap(), None);
    assert_eq!(
        core.go_to_history(v, genzo_model::HistoryEntryId::new(i64::MAX))
            .unwrap(),
        None
    );
    assert_eq!(core.develop_settings(v).unwrap(), exposure(1.0));

    // 保存待ちの変更があると先に保存する（今の位置より後の項目は消える）ので、消えた項目へは移らない。
    core.set_settings(&exposure(2.0), false).unwrap();
    assert_eq!(core.go_to_history(v, entries[3].id).unwrap(), None);
    assert_eq!(core.develop_settings(v).unwrap(), exposure(2.0));
    assert_eq!(
        labels(&core, v),
        vec!["読み込み", "露光量 +0.50", "露光量 +1.00", "露光量 +2.00"]
    );
    // 残っている項目へは移れる。
    assert_eq!(
        core.go_to_history(v, entries[1].id).unwrap(),
        Some(exposure(0.5))
    );
    core.close().unwrap();
}

/// テスト 3: 露光量を変えて保存 → 元ファイルのハッシュが変わらない → Undo / Redo → Core を開き直しても
/// 設定が残り、Undo できる（DEV-27「再起動後も可能」）。
#[test]
fn develop_save_undo_redo_and_reopen() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 1, None);
    let before = hashes(&env.photos);
    let v;
    {
        let core = env.open();
        let events = core.subscribe();
        import(&core, &env.photos);
        v = all_items(&core)[0].variant_id;
        let state = core.open_develop(v).unwrap();
        assert_eq!(state.settings, DevelopSettings::default());
        assert!(!state.can_undo && !state.dirty);
        assert!(!state.source.is_raw);
        assert_eq!((state.source.width, state.source.height), (96, 64));

        // スライダーのドラッグ（応答を待たずに次々と送る。古い要求は捨てる）。
        let mut last = 0;
        for i in 1..=5 {
            last = core.set_settings(&exposure(0.2 * i as f32), true).unwrap();
        }
        let frame = core
            .wait_for_frame(last, WAIT_LIMIT)
            .unwrap()
            .expect("描けている");
        assert_eq!(frame.generation, last, "最新の要求は必ず描く");
        assert_eq!(
            frame.quality,
            RenderQuality::Final,
            "RAW 以外は常に最終品質"
        );
        assert_eq!(
            frame.histogram.pixel_count,
            u64::from(frame.width * frame.height)
        );
        assert_eq!(frame.histogram.luma.len(), 256);
        let px = core.develop_frame().unwrap();
        assert_eq!(px.display_rgb8.len() as u32, frame.width * frame.height * 3);
        assert_eq!(px.b3.image.dimensions(), (frame.width, frame.height));
        // ドラッグ中は保存しない。
        assert_eq!(labels(&core, v), vec!["読み込み"]);
        let state = core.end_drag().unwrap();
        assert!(!state.dirty && state.can_undo);
        assert_eq!(
            labels(&core, v),
            vec!["読み込み", "露光量 +1.00"],
            "履歴は 1 件だけ"
        );
        assert!(
            drain(&events).iter().any(
                |e| matches!(e, Event::PreviewUpdated { generation, .. } if *generation == last)
            )
        );

        // Undo / Redo。
        assert_eq!(core.undo(v).unwrap(), Some(DevelopSettings::default()));
        assert_eq!(
            core.develop_state().unwrap().unwrap().settings,
            DevelopSettings::default()
        );
        assert_eq!(core.undo(v).unwrap(), None, "最初の履歴より前には戻れない");
        assert_eq!(core.redo(v).unwrap(), Some(exposure(1.0)));
        assert_eq!(core.redo(v).unwrap(), None);

        // ドラッグでない変更は、最後の操作から 1 秒後に保存する（DATA-03）。保存の時刻は時間に依存する
        // ので、ここでは最後の値が保存されることだけを確かめる（まとめて 1 件の履歴になることは
        // `pending_changes_are_saved_as_one_entry` で、待ち時間を長くして確かめる）。
        let mut s = exposure(1.0);
        s.contrast = 10.0;
        core.set_settings(&s, false).unwrap();
        s.contrast = 20.0;
        core.set_settings(&s, false).unwrap();
        wait_until("自動保存", || {
            !core.develop_state().unwrap().unwrap().dirty
        });
        assert_eq!(labels(&core, v).last().unwrap(), "コントラスト +20");
        assert_eq!(core.develop_settings(v).unwrap(), s);

        // 保存待ちの変更は close で保存する。
        s.tone.shadows = 30.0;
        core.set_settings(&s, false).unwrap();
        core.close().unwrap();
    }
    assert_eq!(
        hashes(&env.photos),
        before,
        "元ファイルは変わらない（DATA-01）"
    );

    // 開き直しても設定が残り、Undo できる。
    let core = env.open();
    let mut expected = exposure(1.0);
    expected.contrast = 20.0;
    expected.tone.shadows = 30.0;
    assert_eq!(core.develop_settings(v).unwrap(), expected);
    let undone = core.undo(v).unwrap().unwrap();
    assert_eq!(undone.tone.shadows, 0.0);
    assert_eq!(undone.contrast, 20.0);
    core.close().unwrap();
    assert_eq!(hashes(&env.photos), before);
}

/// 現像設定を変えたら、バックグラウンドで現像結果からサムネイル（L0）と標準プレビュー（L1）を作り直す
/// （PRV-02。4.1 節）。取り込みの後の作り直し（4 章「最初の表示」）も。
#[test]
fn previews_are_regenerated_from_develop_results() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (120, 80), 1, None);
    let core = env.open_with(|c| c.render_previews_after_import = true);
    import(&core, &env.photos);
    core.wait_idle();
    let v = all_items(&core)[0].variant_id;
    let t0 = core.thumbnail(v).unwrap().unwrap();
    let l1 = core
        .preview_path(v)
        .unwrap()
        .expect("取り込みの後に L1 を作った");
    let decoded = genzo_media::decode_image_file(&l1).unwrap();
    assert_eq!((decoded.pixels.width(), decoded.pixels.height()), (120, 80));
    assert!(decoded.profile.is_embedded(), "B5 は ICC 付き");

    core.open_develop(v).unwrap();
    core.set_settings(&exposure(-1.0), true).unwrap();
    core.end_drag().unwrap();
    core.wait_idle();
    let t1 = core.thumbnail(v).unwrap().unwrap();
    assert_ne!(t1.rev, t0.rev, "設定が変わるとキャッシュキーが変わる");
    assert_ne!(t1.jpeg, t0.jpeg);
    assert!(core.preview_path(v).unwrap().is_some());
    // 古いキーの L1 は使われない（今の設定の L1 だけを返す）。
    assert_ne!(core.preview_path(v).unwrap().unwrap(), l1);

    // 明示的な作り直しのジョブ。
    let job = core.regenerate_previews(&[v]).unwrap();
    match common::wait_ok(&core, job) {
        JobResult::RegeneratePreviews(r) => assert_eq!(r.rendered, 1, "{r:?}"),
        other => panic!("{other:?}"),
    }
    core.close().unwrap();
}

/// テスト 4: 設定のコピーと複数の variant への一括適用（DEV-30）、一部の項目だけの貼り付け、仮想コピー。
#[test]
fn paste_settings_to_many_and_virtual_copies() {
    let env = Env::new();
    for i in 0..4 {
        write_jpeg(&env.photos.join(format!("P{i}.jpg")), (48, 32), i, None);
    }
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let ids: Vec<VariantId> = ["P0.jpg", "P1.jpg", "P2.jpg", "P3.jpg"]
        .iter()
        .map(|n| by_name(&items, n).variant_id)
        .collect();
    // P0 を現像して、設定をコピーする。
    core.open_develop(ids[0]).unwrap();
    let mut s = exposure(0.7);
    s.color.saturation = 15.0;
    s.white_balance = WhiteBalance::Custom {
        temperature_k: 5000.0,
        tint: 5.0,
    };
    core.set_settings(&s, false).unwrap();
    let copied = core.develop_settings(ids[0]).unwrap();
    assert_eq!(copied, s, "保存待ちの変更を含めてコピーする");
    // P1・P2 へすべての項目を貼り付ける。
    assert_eq!(
        core.paste_settings(&copied, &ids[1..3], SettingGroups::default())
            .unwrap(),
        2
    );
    for &v in &ids[1..3] {
        assert_eq!(core.develop_settings(v).unwrap(), s);
        assert_eq!(labels(&core, v), vec!["読み込み", HISTORY_LABEL_PASTE]);
    }
    // P3 は切り抜き済み。基本のトーンだけを貼り付けると、切り抜きと WB は残る。
    let mut own = DevelopSettings::default();
    own.geometry.crop = CropRect {
        left: 0.1,
        top: 0.1,
        right: 0.9,
        bottom: 0.9,
    };
    core.paste_settings(&own, &ids[3..], SettingGroups::default())
        .unwrap();
    let groups = SettingGroups {
        white_balance: false,
        geometry: false,
        color: false,
        ..SettingGroups::default()
    };
    core.paste_settings(&copied, &ids[3..], groups).unwrap();
    let p3 = core.develop_settings(ids[3]).unwrap();
    assert_eq!(p3.exposure_ev, 0.7);
    assert_eq!(p3.geometry.crop, own.geometry.crop);
    assert_eq!(p3.white_balance, WhiteBalance::default());
    assert_eq!(p3.color.saturation, 0.0);
    // 一括適用は Undo できる。
    assert_eq!(core.undo(ids[1]).unwrap(), Some(DevelopSettings::default()));

    // 仮想コピー（LIB-13）: 今の設定をコピーし、一覧に増える。
    let before = all_items(&core).len();
    let vc = core.create_virtual_copy(ids[0], Some("白黒")).unwrap();
    assert_eq!(core.develop_settings(vc).unwrap(), s);
    let after = all_items(&core);
    assert_eq!(after.len(), before + 1);
    let copy = after.iter().find(|i| i.variant_id == vc).unwrap();
    assert!(!copy.is_master);
    assert_eq!(copy.variant_name.as_deref(), Some("白黒"));
    // 不正な設定は貼り付けない。
    assert!(matches!(
        core.paste_settings(&exposure(f32::NAN), &ids, SettingGroups::default()),
        Err(ApiError::Develop(_))
    ));
    core.close().unwrap();
}

/// テスト 8: GPU を使わない設定（Off）でも、現像・書き出しができる（CPU 版。2.4 節）。
#[test]
fn develop_and_export_without_gpu() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    let core = env.open_with(|c| c.gpu = GpuMode::Off);
    assert!(core.gpu_adapter().is_none());
    assert!(core.capabilities().unwrap().gpu.is_none());
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    core.set_settings(&exposure(0.5), true).unwrap();
    core.end_drag().unwrap();
    let frame = core.render_develop().unwrap();
    assert_eq!(frame.backend, RenderBackend::Cpu);
    assert!(
        frame.display_assumed_srgb,
        "モニターのプロファイルがなければ sRGB とみなす"
    );
    let out = env.dir.path().join("out");
    let job = core
        .export(&[v], &genzo_api::ExportSettings::default(), &out)
        .unwrap();
    match common::wait_ok(&core, job) {
        JobResult::Export(r) => {
            assert_eq!(r.written, 1, "{r:?}");
            assert!(matches!(
                r.items[0].outcome,
                genzo_api::ExportOutcome::Written {
                    backend: RenderBackend::Cpu,
                    ..
                }
            ));
        }
        other => panic!("{other:?}"),
    }
    core.close().unwrap();
}

/// ソフトウェアの GPU（llvmpipe・WARP など）は、許可しなければ使わない（決定 No.20）。この開発環境
/// （llvmpipe だけ）では、既定の設定では CPU 版で描く。
#[test]
fn software_gpu_is_not_used_unless_allowed() {
    if std::env::var_os("GENZO_GPU_ALLOW_SOFTWARE").is_some() {
        eprintln!("環境変数 GENZO_GPU_ALLOW_SOFTWARE があるのでスキップ");
        return;
    }
    let env = Env::new();
    let core = env.open_with(|c| {
        c.gpu = GpuMode::Auto;
        c.gpu_allow_software = false;
    });
    if let Some(adapter) = core.gpu_adapter() {
        assert!(
            !adapter.contains("ソフトウェアの実装"),
            "既定ではソフトウェアの GPU を使わない: {adapter}"
        );
    }
    core.close().unwrap();
}

/// GPU を使える環境では（CI 以外）、既定の設定で GPU 版か CPU 版のどちらかで描ける（GPU の失敗は CPU 版に
/// 切り替える）。
#[test]
fn develop_with_default_gpu_setting() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    let core = env.open();
    let events = core.subscribe();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    let g = core.set_settings(&exposure(0.3), false).unwrap();
    let frame = core.wait_for_frame(g, WAIT_LIMIT).unwrap().unwrap();
    let fallbacks = drain(&events)
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Event::Warning {
                    code: genzo_api::WarningCode::GpuFallback | genzo_api::WarningCode::GpuDisabled,
                    ..
                }
            )
        })
        .collect::<Vec<_>>();
    match core.gpu_adapter() {
        // GPU があれば GPU 版で描ける（CPU 版への切り替えは起きない）。
        Some(adapter) => {
            eprintln!("GPU: {adapter}");
            assert!(fallbacks.is_empty(), "{fallbacks:?}");
            assert_eq!(frame.backend, RenderBackend::Gpu);
        }
        None => assert_eq!(frame.backend, RenderBackend::Cpu),
    }
    // 画面のプロファイル（IQ-05）: 読めないプロファイルは sRGB とみなして警告する。
    let info = core
        .set_display_profile(Some(b"not an icc profile"))
        .unwrap();
    assert!(info.assumed_srgb && info.reason.is_some());
    assert!(drain(&events).iter().any(|e| matches!(
        e,
        Event::Warning {
            code: genzo_api::WarningCode::DisplayProfileAssumedSrgb,
            ..
        }
    )));
    // Display P3 のプロファイルは使える。
    let p3 = genzo_color::IccProfile::standard(genzo_color::StandardProfile::DisplayP3).unwrap();
    let info = core.set_display_profile(Some(p3.as_bytes())).unwrap();
    assert!(!info.assumed_srgb);
    let frame = core.render_develop().unwrap();
    assert!(!frame.display_assumed_srgb);
    // 現像の操作は開いた写真がないとできない。
    core.close_develop().unwrap();
    assert!(matches!(
        core.set_settings(&exposure(0.1), false),
        Err(ApiError::NoDevelopSession)
    ));
    core.close().unwrap();
}

/// 埋め込みのサムネイルがない RAW（プレビューを埋め込まない DNG）は、取り込みの後に現像結果から L0 を作る
/// （取り込みの後の作り直しを止める既定（No.24）でも。L0 がないとグリッドに何も出ないため）。サムネイルの
/// 失敗には数えない。
#[test]
fn raw_without_embedded_thumbnail_gets_a_rendered_thumbnail() {
    let env = Env::new();
    if !common::make_dng_with(&env.photos.join("R.dng"), "2024:05:01 08:00:00", false) {
        return;
    }
    let core = env.open();
    let report = import(&core, &env.photos);
    assert_eq!(report.added, 1, "{report:?}");
    assert!(report.thumbnail_failures.is_empty(), "{report:?}");
    core.wait_idle();
    let v = all_items(&core)[0].variant_id;
    let t = core.thumbnail(v).unwrap().expect("現像結果から作った L0");
    assert!(!t.jpeg.is_empty());
    assert!(
        all_items(&core)[0].thumb_rev.is_some(),
        "検索結果にもサムネイルの rev が出る"
    );
    core.close().unwrap();
}

/// LibRaw があれば、RAW の WB のドラッグ中は簡易処理（Draft）、離したら最終品質で描き直す（2.2 節）。
#[test]
fn raw_white_balance_drag_uses_draft_quality() {
    let env = Env::new();
    if !common::make_dng(&env.photos.join("R.dng"), "2024:05:01 08:00:00") {
        return;
    }
    let core = env.open();
    let report = import(&core, &env.photos);
    assert_eq!(report.added, 1, "{report:?}");
    let v = all_items(&core)[0].variant_id;
    let state = core.open_develop(v).unwrap();
    assert!(state.source.is_raw);
    let s = DevelopSettings {
        white_balance: WhiteBalance::Custom {
            temperature_k: 4500.0,
            tint: 0.0,
        },
        ..Default::default()
    };
    let g = core.set_settings(&s, true).unwrap();
    let f = core.wait_for_frame(g, WAIT_LIMIT).unwrap().unwrap();
    assert_eq!(f.quality, RenderQuality::Draft);
    let st = core.end_drag().unwrap();
    let f = core
        .wait_for_frame(st.generation, WAIT_LIMIT)
        .unwrap()
        .unwrap();
    assert_eq!(f.quality, RenderQuality::Final);
    // 露光量だけのドラッグは最終品質のまま（A1 はキャッシュから使う）。
    let mut s2 = s.clone();
    s2.exposure_ev = 0.5;
    let g = core.set_settings(&s2, true).unwrap();
    let f = core.wait_for_frame(g, WAIT_LIMIT).unwrap().unwrap();
    assert_eq!(f.quality, RenderQuality::Final);
    core.end_drag().unwrap();
    core.close().unwrap();
}

/// 同じ写真を開き直しても、設定の世代は前のセッションの続きから増える（前のセッションの描画の結果を、
/// 新しいセッションの結果と取り違えない。6.2 節「最新の 1 件だけ」、4.1 節「古い世代の結果で上書き
/// しない」）。世代が開き直すたびに 1 から始まると、前のセッションで遅れて終わった描画（大きい世代）が
/// 新しいセッションのプレビューになり、その後の新しい描画が「古い」として捨てられる。
#[test]
fn reopening_a_photo_keeps_generations_increasing() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (64, 48), 2, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;
    core.open_develop(a).unwrap();
    let mut last = 0;
    for i in 1..=3 {
        last = core.set_settings(&exposure(0.1 * i as f32), true).unwrap();
    }
    core.end_drag().unwrap();
    core.wait_for_frame(last, WAIT_LIMIT).unwrap().unwrap();

    // 別の写真を開いてから戻る・閉じてから開き直す。
    for reopen_via_other in [true, false] {
        if reopen_via_other {
            core.open_develop(b).unwrap();
        } else {
            core.close_develop().unwrap();
        }
        let state = core.open_develop(a).unwrap();
        assert!(
            state.generation > last,
            "開き直した後の世代 {} は前の世代 {last} より大きい",
            state.generation
        );
        let f = core
            .wait_for_frame(state.generation, WAIT_LIMIT)
            .unwrap()
            .unwrap();
        assert!(f.generation >= state.generation);
        last = core.set_settings(&exposure(0.9), false).unwrap();
        assert!(last > state.generation);
        let f = core.wait_for_frame(last, WAIT_LIMIT).unwrap().unwrap();
        assert_eq!(f.generation, last, "最新の要求は必ず描く");
    }
    core.close().unwrap();
}

/// ドラッグでない変更は、保存されるまでの変更をまとめて 1 件の履歴にする（DATA-03・DEV-27）。自動保存の
/// 待ち時間を長くして、時間によらずに確かめる。
#[test]
fn pending_changes_are_saved_as_one_entry() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    let core = env.open_with(|c| c.develop_save_delay = Some(std::time::Duration::from_secs(3600)));
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    let mut s = DevelopSettings::default();
    for contrast in [10.0, 20.0] {
        s.contrast = contrast;
        core.set_settings(&s, false).unwrap();
    }
    s.tone.shadows = 5.0;
    core.set_settings(&s, false).unwrap();
    assert!(core.develop_state().unwrap().unwrap().dirty);
    assert_eq!(
        labels(&core, v),
        vec!["読み込み"],
        "待ち時間の前は保存しない"
    );
    assert!(core.flush_develop().unwrap());
    assert_eq!(
        labels(&core, v),
        vec!["読み込み", "コントラスト +20・シャドウ +5"],
        "まとめて 1 件"
    );
    assert!(!core.develop_state().unwrap().unwrap().dirty);
    assert!(
        !core.flush_develop().unwrap(),
        "保存待ちがなければ何もしない"
    );

    // ドラッグ中のまま、close を呼ばずに drop しても、変更を保存して正常に閉じる（DATA-03・DATA-05）。
    s.exposure_ev = 0.4;
    core.set_settings(&s, true).unwrap();
    drop(core);
    let core = env.open();
    assert_eq!(
        core.startup_report().previous_shutdown,
        genzo_api::PreviousShutdown::Clean
    );
    assert_eq!(core.develop_settings(v).unwrap(), s);
    assert_eq!(labels(&core, v).len(), 3);
    core.close().unwrap();
}

/// 処理バージョン 1 と 2 の写真が混ざったカタログでの設定の貼り付け（DEV-30・DATA-09。検証の指摘: 一部の
/// 項目の貼り付けは版が違うと必ず失敗し、すべての項目では貼り付け先の版が黙って貼り付け元の版になっていた）。
///
/// - 画面の既定の選択（切り抜きと角度を除く）でも、版の違う写真へ貼り付けられ、貼り付け先の版は変わらない
///   （すべての項目でも）。版 1 の写真に写した版 2 の項目は保存し、使われないことを数で知らせる。
/// - 利用者が選んだときだけ、古い写真を貼り付け元の版に上げる（履歴の名前に残り、Undo で戻せる）。新しい版の
///   写真を下げることはない。
#[test]
fn paste_between_process_versions_keeps_the_target_version() {
    let env = Env::new();
    for (i, name) in ["A.jpg", "B.jpg", "C.jpg"].into_iter().enumerate() {
        write_jpeg(&env.photos.join(name), (48, 32), i as u8 + 1, None);
    }
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let (a, b, c) = (
        by_name(&items, "A.jpg").variant_id,
        by_name(&items, "B.jpg").variant_id,
        by_name(&items, "C.jpg").variant_id,
    );
    // A を版 1 にする（MVP で現像した写真）。B・C は新しく取り込んだ版 2。
    let a_v1 = DevelopSettings {
        process_version: 1,
        ..core.develop_settings(a).unwrap()
    };
    core.paste_settings_with(
        &a_v1,
        &[a],
        SettingGroups::ALL,
        PasteVersion::Source,
        "版 1",
    )
    .unwrap();
    assert_eq!(core.develop_settings(a).unwrap().process_version, 1);
    let mut src = core.develop_settings(b).unwrap();
    assert_eq!(src.process_version, 2);
    src.exposure_ev = 0.5;
    src.lens.distortion = -20.0;
    let ui_default = SettingGroups {
        geometry: false,
        ..SettingGroups::ALL
    };
    let report = core
        .paste_settings_with(
            &src,
            &[a, c],
            ui_default,
            PasteVersion::Keep,
            HISTORY_LABEL_PASTE,
        )
        .unwrap();
    assert_eq!(
        report,
        PasteReport {
            count: 2,
            upgraded: 0,
            needs_upgrade: 1,
        }
    );
    let got = core.develop_settings(a).unwrap();
    assert_eq!(got.process_version, 1, "版 1 のまま");
    assert_eq!(got.exposure_ev, 0.5);
    assert_eq!(
        got.lens.distortion, -20.0,
        "値は保存する（版を更新すると使われる）"
    );
    assert_eq!(labels(&core, a).last().unwrap(), HISTORY_LABEL_PASTE);
    // すべての項目でも、貼り付け先の版のまま（paste_settings は版を変えない）。
    core.paste_settings(&src, &[a], SettingGroups::ALL).unwrap();
    assert_eq!(core.develop_settings(a).unwrap().process_version, 1);
    // 逆向き（版 1 の設定を版 2 の写真へ）も、版 2 のまま（下げない）。
    let from_v1 = core.develop_settings(a).unwrap();
    for version in [PasteVersion::Keep, PasteVersion::Upgrade] {
        let r = core
            .paste_settings_with(
                &from_v1,
                &[c],
                SettingGroups::ALL,
                version,
                HISTORY_LABEL_PASTE,
            )
            .unwrap();
        assert_eq!(r.upgraded, 0, "{version:?}");
        assert_eq!(core.develop_settings(c).unwrap().process_version, 2);
    }
    // 選んだときだけ上げる。上げた写真の履歴の名前に残る。
    let report = core
        .paste_settings_with(
            &src,
            &[a, c],
            ui_default,
            PasteVersion::Upgrade,
            HISTORY_LABEL_PASTE,
        )
        .unwrap();
    assert_eq!(
        report,
        PasteReport {
            count: 2,
            upgraded: 1,
            needs_upgrade: 0,
        }
    );
    assert_eq!(core.develop_settings(a).unwrap().process_version, 2);
    assert_eq!(
        labels(&core, a).last().unwrap(),
        "設定の貼り付け・処理バージョンを 2 に更新"
    );
    assert_eq!(labels(&core, c).last().unwrap(), HISTORY_LABEL_PASTE);
    // Undo で版 1 に戻る。
    assert_eq!(core.undo(a).unwrap().unwrap().process_version, 1);
    // コマンドの形: 省略すると版を変えず、結果は pasted。
    let json = core.execute_json(
        &serde_json::json!({
            "type": "paste_settings",
            "settings": src,
            "targets": [a],
            "groups": ui_default,
        })
        .to_string(),
    );
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["ok"]["type"], "pasted", "{value}");
    assert_eq!(value["ok"]["count"], 1);
    assert_eq!(value["ok"]["upgraded"], 0);
    assert_eq!(value["ok"]["needs_upgrade"], 1);
    assert_eq!(core.develop_settings(a).unwrap().process_version, 1);
    core.close().unwrap();
}

/// 選んだ写真の処理バージョンをまとめて更新する（DATA-09。ライブラリの右クリックのメニュー・CLI の
/// `develop upgrade`）: 最新でない写真だけを、ほかの値を変えずに更新し、履歴に残す（Undo で戻せる）。現像中の
/// 写真は保存待ちの変更を先に保存してから更新し、セッションの設定も更新する。動画は飛ばす。
#[test]
fn upgrade_process_versions_updates_only_older_photos() {
    let env = Env::new();
    for (i, name) in ["A.jpg", "B.jpg", "C.jpg"].into_iter().enumerate() {
        write_jpeg(&env.photos.join(name), (48, 32), i as u8 + 1, None);
    }
    let has_video = common::make_video(&env.photos.join("V.mp4"), "2024-05-01T09:00:00Z");
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let id = |n: &str| by_name(&items, n).variant_id;
    let (a, b, c) = (id("A.jpg"), id("B.jpg"), id("C.jpg"));
    for v in [a, b] {
        let s = DevelopSettings {
            process_version: 1,
            exposure_ev: 0.25,
            ..core.develop_settings(v).unwrap()
        };
        core.paste_settings_with(&s, &[v], SettingGroups::ALL, PasteVersion::Source, "版 1")
            .unwrap();
    }
    // A を現像中にして、保存待ちの変更を作る。
    let state = core.open_develop(a).unwrap();
    assert_eq!(state.settings.process_version, 1);
    let mut pending = state.settings.clone();
    pending.contrast = 15.0;
    core.set_settings(&pending, false).unwrap();
    let mut targets = vec![a, b, c, a];
    if has_video {
        targets.push(id("V.mp4"));
    }
    assert_eq!(core.upgrade_process_versions(&targets).unwrap(), 2);
    for v in [a, b, c] {
        let s = core.develop_settings(v).unwrap();
        assert_eq!(s.process_version, 2, "{v}");
    }
    let a_saved = core.develop_settings(a).unwrap();
    assert_eq!(a_saved.contrast, 15.0, "保存待ちの変更は先に保存した");
    assert_eq!(a_saved.exposure_ev, 0.25, "ほかの値は変えない");
    assert_eq!(
        labels(&core, a).last().unwrap(),
        "処理バージョンを 2 に更新"
    );
    assert_eq!(
        labels(&core, b).last().unwrap(),
        "処理バージョンを 2 に更新"
    );
    assert_ne!(
        labels(&core, c).last().unwrap(),
        "処理バージョンを 2 に更新",
        "最新の写真は記録しない"
    );
    // 現像のセッションの設定も更新した。
    assert_eq!(
        core.develop_state()
            .unwrap()
            .unwrap()
            .settings
            .process_version,
        2
    );
    // もう一度は何もしない。Undo で戻せる。
    assert_eq!(core.upgrade_process_versions(&[a, b, c]).unwrap(), 0);
    assert_eq!(core.undo(b).unwrap().unwrap().process_version, 1);
    // コマンドの形。
    let json = core.execute_json(
        &serde_json::json!({"type": "upgrade_process_versions", "variant_ids": [b]}).to_string(),
    );
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value["ok"],
        serde_json::json!({"type": "updated", "count": 1})
    );
    core.close().unwrap();
}

/// 写真を開かずに設定を保存する（CLI の `develop set`・`develop reset`）: 履歴の名前を指定でき、
/// カタログの件数（[`genzo_api::Core::catalog_stats`]）に履歴が数えられる。
#[test]
fn settings_saved_with_label_and_catalog_stats() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 3, None);
    let before = hashes(&env.photos);
    let core = env.open();
    let empty = core.catalog_stats().unwrap();
    assert_eq!(
        (
            empty.assets,
            empty.variants,
            empty.files,
            empty.history_entries
        ),
        (0, 0, 0, 0)
    );
    assert!(empty.schema_version >= 1);
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    let old = core.develop_settings(v).unwrap();
    let new = exposure(0.4);
    let label = genzo_api::describe_change(&old, &new);
    assert_eq!(label, "露光量 +0.40");
    assert_eq!(
        core.paste_settings_with(
            &new,
            &[v],
            SettingGroups::default(),
            PasteVersion::Source,
            &label
        )
        .unwrap()
        .count,
        1
    );
    assert_eq!(core.develop_settings(v).unwrap(), new);
    assert_eq!(labels(&core, v), vec!["読み込み", "露光量 +0.40"]);
    let stats = core.catalog_stats().unwrap();
    assert_eq!((stats.assets, stats.variants, stats.files), (1, 1, 1));
    assert_eq!(stats.history_entries, 2);
    assert_eq!(stats.schema_version, empty.schema_version);
    // コマンドの形でも同じ値が返る。
    let json = core.execute_json(r#"{"type":"catalog_stats"}"#);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["ok"]["type"], "catalog_stats");
    assert_eq!(value["ok"]["history_entries"], 2);
    core.close().unwrap();
    assert_eq!(
        hashes(&env.photos),
        before,
        "元ファイルは変わらない（DATA-01）"
    );
}

/// レンズ補正（DEV-14）: 埋め込みの補正の情報を画面に知らせ、使った情報を render_deps に記録し、プロファイル
/// の補正で描く。版 1 の写真では適用せずに知らせ、「処理バージョンを更新」で版 2 にできる（DATA-09。履歴に
/// 残り、Undo で戻せる）。
#[test]
fn lens_correction_and_process_version_upgrade() {
    use genzo_raw::{DngLensCorrection, DngVignetteRadial, DngWarpPlane, DngWarpRectilinear};
    let env = Env::new();
    let lens = genzo_raw::LensCorrectionData::DngOpcode(DngLensCorrection {
        warp: Some(DngWarpRectilinear {
            planes: vec![DngWarpPlane {
                kr: [1.0, -0.04, 0.0, 0.0],
                kt: [0.0, 0.0],
            }],
            center: [0.5, 0.5],
        }),
        vignette: Some(DngVignetteRadial {
            k: [0.5, 0.0, 0.0, 0.0, 0.0],
            center: [0.5, 0.5],
        }),
    });
    if !common::make_dng_with_lens(&env.photos.join("L.dng"), lens) {
        return;
    }
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    let state = core.open_develop(v).unwrap();
    let info = state.source.lens_correction.clone().expect("補正の情報");
    assert_eq!(info.source, "dng_opcode");
    assert!(info.distortion && info.vignetting && !info.chromatic_aberration);
    assert_eq!(state.latest_process_version, 2);
    assert_eq!(state.settings.process_version, 2);
    let deps = state
        .settings
        .render_deps
        .lens_profile
        .clone()
        .expect("記録");
    assert!(deps.id.starts_with("dng_opcode:"), "{deps:?}");

    // プロファイルの補正で描ける（警告なし）。保存すると使った情報も記録される。
    let mut s = state.settings.clone();
    s.lens.profile_enabled = true;
    core.set_settings(&s, false).unwrap();
    let frame = core.render_develop().unwrap();
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
    core.flush_develop().unwrap();
    let saved = core.develop_settings(v).unwrap();
    assert_eq!(saved.render_deps.lens_profile, Some(deps));
    assert!(labels(&core, v).last().unwrap().contains("レンズ補正"));

    // 版 1 の写真（MVP で現像した写真）では適用せず、警告で知らせる。
    let v1 = DevelopSettings {
        process_version: 1,
        ..s.clone()
    };
    core.set_settings(&v1, false).unwrap();
    core.flush_develop().unwrap();
    let frame = core.render_develop().unwrap();
    assert!(
        frame
            .warnings
            .iter()
            .any(|w| w.contains("処理バージョンを更新")),
        "{:?}",
        frame.warnings
    );
    // 処理バージョンの更新: ほかの値は変えず、履歴に残してすぐに保存する。
    let up = core.upgrade_process_version().unwrap();
    assert_eq!(up.settings.process_version, 2);
    assert!(up.settings.lens.profile_enabled);
    assert!(!up.dirty);
    assert_eq!(core.develop_settings(v).unwrap().process_version, 2);
    assert_eq!(
        labels(&core, v).last().unwrap(),
        "処理バージョンを 2 に更新"
    );
    assert!(core.render_develop().unwrap().warnings.is_empty());
    // もう最新なので何もしない（履歴を増やさない）。
    let n = labels(&core, v).len();
    core.upgrade_process_version().unwrap();
    assert_eq!(labels(&core, v).len(), n);
    // Undo で版 1 に戻る。
    let back = core.undo(v).unwrap().unwrap();
    assert_eq!(back.process_version, 1);
}

/// 埋め込みのレンズ補正の情報が壊れている RAW（DEV-14）: 写真は開け、補正の情報は使わず、理由を
/// `SourceInfo::lens_correction_note` で知らせる（検証の指摘: 理由がワーカーで捨てられ、画面では「情報が
/// ない」と区別できなかった）。LibRaw が有効な build だけ。
#[test]
fn broken_lens_correction_is_reported_in_source_info() {
    use genzo_raw::{DngLensCorrection, DngWarpPlane, DngWarpRectilinear};
    let env = Env::new();
    let plane = |k0: f64| DngWarpPlane {
        kr: [k0, 0.0, 0.0, 0.0],
        kt: [0.0, 0.0],
    };
    let lens = genzo_raw::LensCorrectionData::DngOpcode(DngLensCorrection {
        warp: Some(DngWarpRectilinear {
            planes: vec![plane(1.0), plane(1e-5), plane(1.0)],
            center: [0.5, 0.5],
        }),
        vignette: None,
    });
    if !common::make_dng_with_lens(&env.photos.join("B.dng"), lens) {
        return;
    }
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    let state = core.open_develop(v).unwrap();
    assert!(state.source.is_raw);
    assert_eq!(state.source.lens_correction, None);
    let note = state.source.lens_correction_note.clone().expect("理由");
    assert!(note.contains("WarpRectilinear"), "{note}");
    assert_eq!(state.settings.render_deps.lens_profile, None);
    core.close().unwrap();
}

/// ハイライト復元（DEV-06）・ノイズ軽減（DEV-13）: RAW の版 2 の写真では警告なしに描け、履歴に項目の名前が残る。
/// 版 1 の写真では適用せずに知らせる。RAW 以外の写真では使われないことを知らせる。
#[test]
fn highlight_recovery_and_noise_reduction_are_applied_or_reported() {
    let env = Env::new();
    write_jpeg(&env.photos.join("J.jpg"), (64, 48), 3, None);
    let has_raw = common::make_dng(&env.photos.join("R.dng"), "2024:05:01 08:00:00");
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let id_of = |name: &str| {
        items
            .iter()
            .find(|i| i.file_name == name)
            .map(|i| i.variant_id)
    };
    let with_sensor = |base: &DevelopSettings| {
        let mut s = base.clone();
        s.highlight_recovery.method = genzo_model::HighlightMethod::Reconstruct;
        s.detail.noise_reduction.luminance = 30.0;
        s.detail.noise_reduction.color = 40.0;
        s
    };
    // RAW 以外: 使われないことを知らせる。
    let j = id_of("J.jpg").unwrap();
    let state = core.open_develop(j).unwrap();
    core.set_settings(&with_sensor(&state.settings), false)
        .unwrap();
    let frame = core.render_develop().unwrap();
    for w in [
        "RAW 以外の写真では、ハイライト復元は使われません",
        "RAW 以外の写真では、ノイズ軽減は使われません",
    ] {
        assert!(
            frame.warnings.iter().any(|x| x == w),
            "{:?}",
            frame.warnings
        );
    }
    core.flush_develop().unwrap();
    assert_eq!(
        labels(&core, j).last().unwrap(),
        "ハイライト復元・輝度ノイズの軽減 30・カラーノイズの軽減 40"
    );
    if !has_raw {
        core.close().unwrap();
        return;
    }
    // RAW の版 2: 警告なしで描ける。
    let r = id_of("R.dng").unwrap();
    let state = core.open_develop(r).unwrap();
    assert_eq!(state.settings.process_version, 2);
    let s = with_sensor(&state.settings);
    core.set_settings(&s, false).unwrap();
    let frame = core.render_develop().unwrap();
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
    // 版 1 の写真では適用せず、更新を促す。
    let v1 = DevelopSettings {
        process_version: 1,
        ..s
    };
    core.set_settings(&v1, false).unwrap();
    let frame = core.render_develop().unwrap();
    for w in ["ハイライト復元", "ノイズ軽減"] {
        assert!(
            frame
                .warnings
                .iter()
                .any(|x| x.contains(w) && x.contains("処理バージョンを更新")),
            "{w}: {:?}",
            frame.warnings
        );
    }
    core.close().unwrap();
}
