//! 削除・ファイル操作の確定・バックアップと復元・設定の結合テスト（FILE-01・DATA-04・DATA-06・
//! DATA-07・SYS-04・SYS-05、04 の 6.4 節）。

mod common;

use common::{Env, JST_MINUTES, all_items, by_name, drain, exif, import, wait_ok, write_jpeg};
use genzo_api::{
    ApiError, DeleteKind, ErrorKind, Event, JobResult, OutputColorSpace, SettingsUpdate,
    WarningCode,
};
use genzo_model::FileRole;

/// テスト 6: ゴミ箱への移動の計画（影響するファイルと variant の一覧）と、確認後の実行。ゴミ箱の操作は
/// 偽物に差し替える（OS のゴミ箱を汚さない）。
#[test]
fn trash_plan_and_execute_with_fake_trash() {
    let env = Env::new();
    // RAW と JPEG のペアの代わりに、同じ名前の DNG（LibRaw がなくても登録できる。メタデータを読めなければ
    // status = error）と JPEG を置く。
    std::fs::write(env.photos.join("A.dng"), b"not really a dng").unwrap();
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (32, 24), 2, None);
    write_jpeg(&env.photos.join("C.jpg"), (32, 24), 3, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    assert_eq!(items.len(), 3, "A.dng と A.jpg は 1 つの写真にまとまる");
    let a = by_name(&items, "A.dng").clone();
    assert!(a.has_sidecar_jpeg);
    let b = by_name(&items, "B.jpg").variant_id;
    let c = by_name(&items, "C.jpg").variant_id;
    let vc = core
        .create_virtual_copy(a.variant_id, Some("白黒"))
        .unwrap();

    // 計画: asset のすべてのファイルと、仮想コピーを含む variant。何も変えない。
    let plan = core.plan_delete(DeleteKind::Trash, &[vc]).unwrap();
    assert_eq!(plan.kind, DeleteKind::Trash);
    assert_eq!(plan.assets, vec![a.asset_id]);
    let files: Vec<(String, FileRole)> = plan
        .files
        .iter()
        .map(|f| {
            (
                f.path
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                f.role,
            )
        })
        .collect();
    assert_eq!(
        files,
        vec![
            ("A.dng".to_owned(), FileRole::Primary),
            ("A.jpg".to_owned(), FileRole::SidecarJpeg)
        ]
    );
    let variants: Vec<_> = plan.variants.iter().map(|v| v.variant_id).collect();
    assert_eq!(variants, vec![a.variant_id, vc]);
    assert!(env.photos.join("A.dng").exists());
    assert_eq!(all_items(&core).len(), 4);
    assert!(env.trash.moved.lock().is_empty());

    // 実行: 偽物のゴミ箱へ移り、カタログから除かれる。
    let events = core.subscribe();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.removed_assets, vec![a.asset_id]);
    assert_eq!(report.trashed_files.len(), 2);
    assert!(report.failed.is_empty());
    assert!(!env.photos.join("A.dng").exists());
    assert!(!env.photos.join("A.jpg").exists());
    assert_eq!(env.trash.moved.lock().len(), 2);
    assert_eq!(all_items(&core).len(), 2);
    assert!(core.thumbnail(a.variant_id).unwrap().is_none());
    assert!(drain(&events).iter().any(|e| matches!(
        e,
        Event::CatalogChanged {
            change: genzo_api::CatalogChange::Removed,
            ..
        }
    )));
    // 同じ計画は 2 回実行できない。
    assert!(matches!(
        core.execute_delete(plan.plan_id),
        Err(ApiError::NotFound(_))
    ));

    // ゴミ箱が使えない場合は、直接削除せずに失敗にする（6.4 節）。写真はカタログに残る。
    env.trash.fail_names.lock().insert("B.jpg".to_owned());
    let plan = core.plan_delete(DeleteKind::Trash, &[b]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.removed_assets.is_empty());
    assert!(env.photos.join("B.jpg").exists());
    assert_eq!(all_items(&core).len(), 2);
    let err = genzo_api::ApiError::Trash {
        path: "B.jpg".into(),
        message: "x".into(),
    };
    assert_eq!(err.kind(), ErrorKind::TrashUnavailable);

    // カタログからの除去: ファイルは残る。
    let plan = core
        .plan_delete(DeleteKind::RemoveFromCatalog, &[c])
        .unwrap();
    core.execute_delete(plan.plan_id).unwrap();
    assert!(env.photos.join("C.jpg").exists());
    assert_eq!(all_items(&core).len(), 1);

    // 仮想コピーの削除にマスターは渡せない。
    assert!(matches!(
        core.plan_delete(DeleteKind::VirtualCopies, &[b]),
        Err(ApiError::InvalidArgument(_))
    ));
    // 計画の後にカタログが変わったら実行しない（もう一度確認する）。
    let plan = core
        .plan_delete(DeleteKind::RemoveFromCatalog, &[b])
        .unwrap();
    let vc2 = core.create_virtual_copy(b, None).unwrap();
    assert!(matches!(
        core.execute_delete(plan.plan_id),
        Err(ApiError::PlanOutdated(_))
    ));
    // 仮想コピーの削除（元ファイルは変えない）。
    let plan = core.plan_delete(DeleteKind::VirtualCopies, &[vc2]).unwrap();
    assert!(plan.files.is_empty());
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.removed_variants, vec![vc2]);
    assert!(env.photos.join("B.jpg").exists());
    core.close().unwrap();
}

/// 起動時に、planned / executing のままのファイル操作を実際のファイルの場所を見て確定させる（DATA-07）。
#[test]
fn unfinished_file_operations_are_settled_at_startup() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (32, 24), 2, None);
    {
        let core = env.open();
        import(&core, &env.photos);
        core.close().unwrap();
    }
    // ゴミ箱への移動の途中で終了した状態を作る（A は移した後、B は移す前）。
    {
        let mut cat = genzo_catalog::Catalog::open(env.catalog_path()).unwrap();
        let ids = cat.all_variant_ids().unwrap();
        let assets = cat.assets_of_variants(&ids).unwrap();
        for &asset in &assets {
            let (op, plan) = cat.plan_trash(&[asset]).unwrap();
            cat.start_file_op(op).unwrap();
            if plan.files[0].location.name == "A.jpg" {
                std::fs::remove_file(env.photos.join("A.jpg")).unwrap();
            }
        }
        cat.close().unwrap();
    }
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 2);
    let warnings = core.startup_warnings();
    assert_eq!(
        warnings
            .iter()
            .filter(|e| matches!(
                e,
                Event::Warning {
                    code: WarningCode::FileOperationRecovered,
                    ..
                }
            ))
            .count(),
        2
    );
    // A は移し終えていたので done（カタログから除く）、B は元の場所にあるので failed（残す）。
    let items = all_items(&core);
    assert_eq!(common::names(&items), vec!["B.jpg"]);
    // 除いた写真のサムネイルは、回収のジョブで消える（4.1 節「回収」）。
    let job = core.collect_cache_garbage().unwrap();
    match wait_ok(&core, job) {
        JobResult::CollectGarbage {
            thumbnails_removed, ..
        } => assert_eq!(thumbnails_removed, 1),
        other => panic!("{other:?}"),
    }
    assert!(core.thumbnail(items[0].variant_id).unwrap().is_some());
    core.close().unwrap();
    // 確定した記録は、次の起動では対象にならない。
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 0);
    core.close().unwrap();
}

/// 自動バックアップ（DATA-04）・手動のバックアップ・詳細チェック（DATA-05）・バックアップからの復元。
#[test]
fn backup_integrity_check_and_restore() {
    let env = Env::new();
    write_jpeg(
        &env.photos.join("A.jpg"),
        (32, 24),
        1,
        Some(&exif(Some("2024-05-01 10:00:00"), Some(JST_MINUTES), None)),
    );
    let core = env.open();
    // 初めて開いたときに自動バックアップを作る（前回のバックアップがない）。
    let first = core.startup_report().backup.expect("自動バックアップ");
    assert!(first.path.exists());
    import(&core, &env.photos);
    let backup = core.backup_now().unwrap();
    let list = core.list_backups().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].path, backup.path, "新しい順");
    // 詳細チェック。
    let job = core.check_integrity().unwrap();
    match wait_ok(&core, job) {
        JobResult::IntegrityCheck(r) => assert!(r.ok, "{r:?}"),
        other => panic!("{other:?}"),
    }
    // バックアップの後に評価を付ける。
    let v = all_items(&core)[0].variant_id;
    core.set_rating(&[v], genzo_api::Rating::new(5).unwrap())
        .unwrap();
    core.close().unwrap();

    // 復元: 別のファイルに作ってから差し替える。
    let plan = genzo_api::prepare_restore(&backup.path, &env.catalog_path()).unwrap();
    assert!(plan.restored_path.exists());
    assert!(
        env.catalog_path().exists(),
        "準備の段階では今のカタログを変えない"
    );
    assert!(!plan.steps.is_empty());
    genzo_api::apply_restore(&plan).unwrap();
    assert!(plan.displaced_path.exists(), "今のカタログは退避して残す");
    let core = env.open();
    let items = all_items(&core);
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].rating,
        genzo_api::Rating::new(0).unwrap(),
        "バックアップの時点の状態"
    );
    assert_eq!(
        core.startup_report().previous_shutdown,
        genzo_api::PreviousShutdown::Clean
    );
    core.close().unwrap();
}

/// 設定（SYS-05）: カタログに保存し、開き直しても残る。既定のタイムゾーンを変えると、既定のオフセットで
/// 推定していた撮影日時を推定し直す。
#[test]
fn settings_are_stored_in_the_catalog() {
    let env = Env::new();
    // オフセットなしの撮影日時（既定のタイムゾーンで推定する）。
    write_jpeg(
        &env.photos.join("A.jpg"),
        (32, 24),
        1,
        Some(&exif(Some("2024-05-01 10:00:00"), None, None)),
    );
    let core = env.open();
    import(&core, &env.photos);
    let s = core.settings();
    assert_eq!(s.default_utc_offset_minutes, JST_MINUTES);
    assert_eq!(s.default_export_color_space, OutputColorSpace::Srgb);
    assert_eq!(
        s.preview_cache_bytes,
        genzo_catalog::DEFAULT_PREVIEW_CAPACITY_BYTES
    );
    assert_eq!(
        all_items(&core)[0].capture.utc.unwrap().to_rfc3339(),
        "2024-05-01T01:00:00+00:00"
    );
    // 不正な値は弾く。
    assert!(
        core.update_settings(&SettingsUpdate {
            preview_cache_bytes: Some(1),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        core.update_settings(&SettingsUpdate {
            default_utc_offset_minutes: Some(15 * 60),
            ..Default::default()
        })
        .is_err()
    );
    let updated = core
        .update_settings(&SettingsUpdate {
            default_export_color_space: Some(OutputColorSpace::DisplayP3),
            preview_cache_bytes: Some(500_000_000),
            default_utc_offset_minutes: Some(0),
            preview_cache_dir: None,
        })
        .unwrap();
    assert_eq!(
        updated.default_export_color_space,
        OutputColorSpace::DisplayP3
    );
    core.wait_idle();
    assert_eq!(
        all_items(&core)[0].capture.utc.unwrap().to_rfc3339(),
        "2024-05-01T10:00:00+00:00",
        "既定のオフセットで推定していた撮影日時を推定し直す"
    );
    core.close().unwrap();
    let core = env.open();
    let s = core.settings();
    assert_eq!(s.default_export_color_space, OutputColorSpace::DisplayP3);
    assert_eq!(s.preview_cache_bytes, 500_000_000);
    assert_eq!(s.default_utc_offset_minutes, 0, "カタログの値を優先する");
    core.close().unwrap();
}

/// 同じ写真の 2 つのファイル（RAW と JPEG の代わりに DNG と JPEG）のうち、2 つ目をゴミ箱へ移せなかった
/// 場合: 直接は削除せず、写真はカタログに残し、移したファイルは見つからないもの（missing）にする。
/// ファイル操作の記録は failed で確定し、次に開いたときに確定し直すものは残らない（6.4 節。DATA-06・07）。
#[test]
fn partially_failed_trash_keeps_the_photo_and_marks_moved_files_missing() {
    let env = Env::new();
    std::fs::write(env.photos.join("A.dng"), b"not really a dng").unwrap();
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    let jpeg_hash = genzo_catalog::full_hash(&env.photos.join("A.jpg")).unwrap();
    let core = env.open();
    import(&core, &env.photos);
    let a = by_name(&all_items(&core), "A.dng").variant_id;
    env.trash.fail_names.lock().insert("A.jpg".to_owned());
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.removed_assets.is_empty() && report.removed_variants.is_empty());
    assert!(!env.photos.join("A.dng").exists(), "1 つ目はゴミ箱へ移った");
    assert_eq!(env.trash.moved.lock().len(), 1);
    assert_eq!(
        genzo_catalog::full_hash(&env.photos.join("A.jpg")).unwrap(),
        jpeg_hash,
        "移せなかったファイルは直接削除しない"
    );
    // 写真はカタログに残り、移した DNG は missing。
    let details = core.variant_details(a).unwrap();
    let status: Vec<(String, genzo_model::FileStatus)> = details
        .files
        .iter()
        .map(|f| {
            (
                f.path
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                f.status,
            )
        })
        .collect();
    assert_eq!(
        status,
        vec![
            ("A.dng".to_owned(), genzo_model::FileStatus::Missing),
            ("A.jpg".to_owned(), genzo_model::FileStatus::Ok)
        ]
    );
    core.close().unwrap();
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 0);
    assert_eq!(all_items(&core).len(), 1);
    core.close().unwrap();
}
