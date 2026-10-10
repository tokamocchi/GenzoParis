//! 現像のセッション・Undo / Redo・一括適用・仮想コピー・サムネイルの作り直し・GPU を使わない設定の
//! 結合テスト（01 のストーリー 4〜6、DEV-00・DEV-27・DEV-30・LIB-13・PRV-02・DATA-01・DATA-03、
//! 04 の 2.2 節・4.1 節・6.2 節）。

mod common;

use common::{Env, WAIT_LIMIT, all_items, by_name, drain, hashes, import, wait_until, write_jpeg};
use genzo_api::{
    ApiError, DevelopSettings, Event, GpuMode, HISTORY_LABEL_PASTE, JobResult, RenderBackend,
    RenderQuality, SettingGroups, VariantId,
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
        core.paste_settings_with_label(&new, &[v], SettingGroups::default(), &label)
            .unwrap(),
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
