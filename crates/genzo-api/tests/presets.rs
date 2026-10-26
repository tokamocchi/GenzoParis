//! スナップショット（DEV-28）・現像プリセット（DEV-29）・書き出しプリセットと書き出しのファイル名（EXP-02）の
//! 結合テスト（04 の 2.5 節・6.4 節、DATA-09）。

mod common;

use std::path::PathBuf;

use common::{Env, JST_MINUTES, all_items, by_name, exif, import, wait_ok, write_jpeg};
use genzo_api::{
    ApiError, Command, DevelopSettings, ExportFormat, ExportOutcome, ExportPresetSettings,
    ExportSettings, JobResult, PasteVersion, PresetKind, RenameRule, Response, SettingGroups,
    SnapshotSource, VariantId,
};
use genzo_model::{ConflictPolicy, CropRect};

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

fn tone_only() -> SettingGroups {
    SettingGroups {
        white_balance: false,
        basic_tone: true,
        presence: true,
        tone_curve: false,
        color: false,
        geometry: false,
        lens: false,
        detail: false,
        effects: false,
        profile: false,
        masks: false,
        retouch: false,
    }
}

/// スナップショット: 今の状態（保存待ちの変更を含む）・履歴の段階から作り、名前を変え、適用すると履歴に 1 件
/// 残って元に戻せる。処理バージョンも保存した版に戻す。
#[test]
fn snapshots_capture_pending_changes_and_apply_with_history() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    core.set_settings(&exposure(0.5), true).unwrap();
    core.end_drag().unwrap();
    // 保存待ち（自動保存の前）の変更も、先に保存してから写す。
    core.set_settings(&exposure(1.0), false).unwrap();
    let snap = core
        .create_snapshot(v, " 明るめ ", SnapshotSource::Current)
        .unwrap();
    assert_eq!(snap.name, "明るめ");
    assert_eq!(snap.variant_id, v);
    assert_eq!(snap.process_version, genzo_model::CURRENT_PROCESS_VERSION);
    assert_eq!(
        labels(&core, v),
        vec!["読み込み", "露光量 +0.50", "露光量 +1.00"]
    );
    // 履歴の段階から作る（今の設定・履歴の位置は変えない）。
    let first_edit = core.history(v).unwrap()[1].id;
    let from_history = core
        .create_snapshot(
            v,
            "途中",
            SnapshotSource::History {
                entry_id: first_edit,
            },
        )
        .unwrap();
    assert_eq!(core.develop_settings(v).unwrap(), exposure(1.0));
    // 名前の変更。
    let renamed = core.rename_snapshot(from_history.id, "+0.5").unwrap();
    assert_eq!(renamed.name, "+0.5");
    let list = core.snapshots(v).unwrap();
    assert_eq!(
        list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        vec!["明るめ", "+0.5"]
    );

    // 別の設定にしてから、スナップショットを適用する（履歴に残り、現像中の状態も変わる）。
    core.set_settings(&exposure(-1.0), true).unwrap();
    core.end_drag().unwrap();
    let applied = core.apply_snapshot(from_history.id).unwrap();
    assert_eq!(applied.exposure_ev, 0.5);
    let state = core.develop_state().unwrap().unwrap();
    assert_eq!(state.settings.exposure_ev, 0.5);
    assert_eq!(
        labels(&core, v).last().unwrap(),
        "スナップショット「+0.5」を適用"
    );
    // 元に戻せる。
    assert_eq!(core.undo(v).unwrap().unwrap().exposure_ev, -1.0);

    // 処理バージョン 1 の状態のスナップショットを適用すると、写真も版 1 に戻る（履歴の位置へ戻るのと同じ）。
    let v1 = DevelopSettings {
        process_version: 1,
        ..exposure(0.3)
    };
    core.paste_settings_with(&v1, &[v], SettingGroups::ALL, PasteVersion::Source, "版 1")
        .unwrap();
    let old = core
        .create_snapshot(v, "版 1", SnapshotSource::Current)
        .unwrap();
    assert_eq!(old.process_version, 1);
    core.upgrade_process_versions(&[v]).unwrap();
    assert_eq!(core.develop_settings(v).unwrap().process_version, 2);
    assert_eq!(core.apply_snapshot(old.id).unwrap().process_version, 1);

    // 削除。ない・他の写真の履歴の項目はエラー。
    core.delete_snapshot(snap.id).unwrap();
    assert!(matches!(
        core.delete_snapshot(snap.id),
        Err(ApiError::Catalog(_))
    ));
    assert_eq!(core.snapshots(v).unwrap().len(), 2);
    assert!(
        core.create_snapshot(v, "", SnapshotSource::Current)
            .is_err()
    );
    core.close().unwrap();
}

/// JSON のコマンド（画面の経路）でも同じ（ビフォーにスナップショットを設定できる。DEV-31）。
#[test]
fn snapshot_commands_and_before_source() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    core.set_settings(&exposure(0.7), true).unwrap();
    core.end_drag().unwrap();
    let r = core
        .execute(
            serde_json::from_value(serde_json::json!({
                "type": "create_snapshot", "variant_id": v.get(), "name": "比べる"
            }))
            .unwrap(),
        )
        .unwrap();
    let Response::Snapshot { snapshot } = r else {
        panic!("{r:?}")
    };
    let r = core
        .execute(Command::SetBefore {
            source: Some(genzo_api::BeforeSource::Snapshot {
                snapshot_id: snapshot.id,
            }),
        })
        .unwrap();
    let Response::Before { before: Some(b) } = r else {
        panic!("{r:?}")
    };
    assert_eq!(b.label, "スナップショット: 比べる");
    let r = core.execute(Command::Snapshots { variant_id: v }).unwrap();
    assert!(matches!(r, Response::Snapshots { ref snapshots } if snapshots.len() == 1));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["type"], "snapshots");
    assert_eq!(json["snapshots"][0]["name"], "比べる");
    core.close().unwrap();
}

/// 現像プリセット: 作るときに選んだ項目だけを保存し、複数の写真に適用する。写真の処理バージョンは変えず
/// （選べば上げる）、版 1 の写真に版 2 の項目を当てたら知らせる。履歴に「プリセット「名前」を適用」が残る。
#[test]
fn develop_presets_apply_only_their_groups_and_keep_process_versions() {
    let env = Env::new();
    for (i, n) in ["A.jpg", "B.jpg", "C.jpg"].iter().enumerate() {
        write_jpeg(&env.photos.join(n), (96, 64), i as u8 + 1, None);
    }
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let (a, b, c) = (
        by_name(&items, "A.jpg").variant_id,
        by_name(&items, "B.jpg").variant_id,
        by_name(&items, "C.jpg").variant_id,
    );
    let mut source = exposure(0.8);
    source.presence.clarity = 30.0;
    source.geometry.crop = CropRect {
        left: 0.1,
        top: 0.1,
        right: 0.9,
        bottom: 0.9,
    };
    core.paste_settings(&source, &[a], SettingGroups::ALL)
        .unwrap();
    // C は処理バージョン 1 の写真。
    let c_v1 = DevelopSettings {
        process_version: 1,
        contrast: 10.0,
        ..Default::default()
    };
    core.paste_settings_with(
        &c_v1,
        &[c],
        SettingGroups::ALL,
        PasteVersion::Source,
        "版 1",
    )
    .unwrap();

    let preset = core
        .create_develop_preset("明るく", "基本", tone_only(), a)
        .unwrap();
    assert_eq!(preset.groups, Some(tone_only()));
    assert_eq!(core.presets(PresetKind::Develop).unwrap().len(), 1);
    // 保存した設定に切り抜きは入らない。
    let saved = core.develop_preset_settings(&preset.id).unwrap();
    assert_eq!(saved.geometry, DevelopSettings::default().geometry);
    assert_eq!(saved.presence.clarity, 30.0);

    let report = core
        .apply_develop_preset(&preset.id, &[b, c], PasteVersion::Keep)
        .unwrap();
    assert_eq!(report.count, 2);
    assert_eq!(report.upgraded, 0);
    assert_eq!(report.needs_upgrade, 1, "版 1 の写真には明瞭度が使われない");
    let sb = core.develop_settings(b).unwrap();
    assert_eq!(sb.exposure_ev, 0.8);
    assert_eq!(sb.presence.clarity, 30.0);
    assert_eq!(sb.geometry, DevelopSettings::default().geometry);
    let sc = core.develop_settings(c).unwrap();
    assert_eq!(sc.process_version, 1);
    assert_eq!(sc.contrast, 0.0, "基本補正の項目はプリセットの値になる");
    assert_eq!(
        labels(&core, b).last().unwrap(),
        "プリセット「明るく」を適用"
    );
    // 選べば、古い写真の処理バージョンを上げる。
    let report = core
        .apply_develop_preset(&preset.id, &[c], PasteVersion::Upgrade)
        .unwrap();
    assert_eq!(report.upgraded, 1);
    assert_eq!(core.develop_settings(c).unwrap().process_version, 2);
    assert_eq!(
        labels(&core, c).last().unwrap(),
        "プリセット「明るく」を適用・処理バージョンを 2 に更新"
    );

    // 現像中の写真に当てると、セッションの設定も変わる。
    core.open_develop(b).unwrap();
    core.set_settings(&exposure(-2.0), false).unwrap();
    core.apply_develop_preset(&preset.id, &[b], PasteVersion::Keep)
        .unwrap();
    let state = core.develop_state().unwrap().unwrap();
    assert_eq!(state.settings.exposure_ev, 0.8);
    let lb = labels(&core, b);
    // 保存待ちの変更を先に保存してから当てる。
    assert!(lb[lb.len() - 2].starts_with("露光量 -2.00"), "{lb:?}");

    // 今の設定で更新・名前の変更・フォルダの移動・削除（JSON のコマンド）。
    let updated = core
        .update_develop_preset(&preset.id, SettingGroups::ALL, a)
        .unwrap();
    assert_eq!(updated.groups, Some(SettingGroups::ALL));
    let r = core
        .execute(Command::RenamePreset {
            kind: PresetKind::Develop,
            preset_id: preset.id.clone(),
            name: "明るく 2".into(),
        })
        .unwrap();
    assert!(matches!(r, Response::Preset { ref preset } if preset.name == "明るく 2"));
    let dup = core.create_develop_preset("明るく 2", "基本", tone_only(), a);
    assert!(matches!(dup, Err(ApiError::NameConflict(_))));
    assert_eq!(dup.unwrap_err().info().kind, genzo_api::ErrorKind::Conflict);
    let r = core
        .execute(Command::Presets {
            kind: PresetKind::Develop,
        })
        .unwrap();
    let Response::Presets { presets, dir } = r else {
        panic!()
    };
    assert_eq!(presets.len(), 1);
    assert_eq!(dir, Some(env.dir.path().join("presets")));
    core.delete_preset(PresetKind::Develop, &preset.id).unwrap();
    assert!(core.presets(PresetKind::Develop).unwrap().is_empty());
    // 動画・ない写真は元にできない。
    assert!(
        core.create_develop_preset("x", "", tone_only(), VariantId::new(9999))
            .is_err()
    );
    core.close().unwrap();
}

/// 現像プリセットを写真と動画をまとめて選んで適用すると、動画を除いて写真にだけ適用し、除いた数を返す（統合
/// タイムラインで全選択しても使える。検証のラウンド 1 の指摘。直す前は動画が 1 本でもあると全体が失敗した）。
#[test]
fn develop_presets_skip_videos_in_the_selection() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (96, 64), 2, None);
    if !common::make_video(&env.photos.join("V.mp4"), "2024-05-01T09:00:00Z") {
        return;
    }
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let (a, b, v) = (
        by_name(&items, "A.jpg").variant_id,
        by_name(&items, "B.jpg").variant_id,
        by_name(&items, "V.mp4").variant_id,
    );
    core.paste_settings(&exposure(0.8), &[a], SettingGroups::ALL)
        .unwrap();
    let preset = core
        .create_develop_preset("明るく", "", tone_only(), a)
        .unwrap();
    let before = labels(&core, v);
    let report = core
        .apply_develop_preset(&preset.id, &[v, b, v], PasteVersion::Keep)
        .unwrap();
    assert_eq!((report.count, report.skipped_videos), (1, 1), "{report:?}");
    assert_eq!(core.develop_settings(b).unwrap().exposure_ev, 0.8);
    assert_eq!(labels(&core, v), before, "動画には何も保存しない");
    // 動画だけなら何もしない（エラーにしない）。
    let report = core
        .apply_develop_preset(&preset.id, &[v], PasteVersion::Keep)
        .unwrap();
    assert_eq!((report.count, report.skipped_videos), (0, 1));
    // JSON のコマンドの応答にも数が入る。
    let r = core
        .execute(Command::ApplyDevelopPreset {
            preset_id: preset.id.clone(),
            targets: vec![b, v],
            process_version: PasteVersion::Keep,
        })
        .unwrap();
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["type"], "pasted");
    assert_eq!(json["count"], 1);
    assert_eq!(json["skipped_videos"], 1);
    // 設定の貼り付けは、これまでどおり動画を含めば何も保存しない。
    assert!(matches!(
        core.paste_settings(&exposure(0.1), &[b, v], SettingGroups::ALL),
        Err(ApiError::InvalidArgument(_))
    ));
    core.close().unwrap();
}

/// 書き出しのファイル名の規則（EXP-02）と書き出しプリセット: 撮影日・連番で名前を作り、仮想コピーは名前を
/// 足す。プリセットの中身はそのまま書き出しに使える。
#[test]
fn export_presets_and_file_naming() {
    let env = Env::new();
    write_jpeg(
        &env.photos.join("IMG_0001.jpg"),
        (64, 48),
        1,
        Some(&exif(Some("2024-05-01 10:00:00"), Some(JST_MINUTES), None)),
    );
    write_jpeg(
        &env.photos.join("IMG_0002.jpg"),
        (64, 48),
        2,
        Some(&exif(Some("2024-05-02 11:00:00"), Some(JST_MINUTES), None)),
    );
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let v1 = by_name(&items, "IMG_0001.jpg").variant_id;
    let v2 = by_name(&items, "IMG_0002.jpg").variant_id;
    let copy = core.create_virtual_copy(v2, Some("白黒")).unwrap();
    let out = env.dir.path().join("out");
    let settings = ExportPresetSettings {
        export: ExportSettings {
            format: ExportFormat::Png8,
            on_conflict: ConflictPolicy::Sequence,
            ..ExportSettings::default()
        },
        destination: Some(out.clone()),
        file_naming: Some(RenameRule {
            template: "{date}_{text}_{seq}".to_owned(),
            custom_text: "旅行".to_owned(),
            sequence_start: 7,
            sequence_digits: 2,
        }),
    };
    let preset = core.create_export_preset("Web", "", &settings).unwrap();
    // 一覧から読んだ中身で書き出す（画面・CLI と同じ）。
    let listed = core.presets(PresetKind::Export).unwrap();
    let p = listed.iter().find(|p| p.id == preset.id).unwrap();
    let e = p.export.clone().unwrap();
    assert_eq!(e, settings);
    let job = core
        .export_with(
            &[v1, v2, copy],
            &e.export,
            e.destination.as_deref().unwrap(),
            e.file_naming.as_ref(),
        )
        .unwrap();
    let JobResult::Export(r) = wait_ok(&core, job) else {
        panic!()
    };
    let names: Vec<String> = r
        .items
        .iter()
        .map(|i| match &i.outcome {
            ExportOutcome::Written { path, .. } => {
                path.file_name().unwrap().to_str().unwrap().to_owned()
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "20240501_旅行_07.png",
            "20240502_旅行_08.png",
            "20240502_旅行_09-白黒.png"
        ]
    );
    // 正しくない規則はジョブを始めない。
    let bad = RenameRule {
        template: "{date".to_owned(),
        ..RenameRule::default()
    };
    assert!(matches!(
        core.export_with(&[v1], &e.export, &out, Some(&bad)),
        Err(ApiError::InvalidArgument(_))
    ));
    // 名前の例（JSON のコマンド）。
    let r = core
        .execute(Command::ExportNamePreview {
            file_naming: e.file_naming.clone(),
            format: ExportFormat::Tiff16,
        })
        .unwrap();
    let Response::NamePreview(preview) = r else {
        panic!()
    };
    assert_eq!(preview.examples[0].destination, "20261013_旅行_07.tif");

    // ファイルへの書き出しと読み込み（ほかの PC へ渡す）。
    let files_dir: PathBuf = env.dir.path().join("files");
    std::fs::create_dir_all(&files_dir).unwrap();
    let written = core
        .export_preset_files(
            PresetKind::Export,
            std::slice::from_ref(&preset.id),
            &files_dir,
        )
        .unwrap();
    assert_eq!(written, vec![files_dir.join("Web.json")]);
    let report = core
        .import_presets(&written, Some(PresetKind::Export))
        .unwrap();
    assert_eq!(report.imported[0].name, "Web (2)");
    assert_eq!(report.imported[0].export.as_ref(), Some(&settings));
    core.close().unwrap();
}
