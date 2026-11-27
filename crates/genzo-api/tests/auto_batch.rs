//! 複数の写真への一括の自動補正（DEV-32 の拡張。[`genzo_api::Core::auto_tone_variants`]）の結合テスト。

mod common;

use common::{Env, all_items, by_name, import, make_video, wait_ok, write_jpeg};
use genzo_api::{
    ApiError, AutoToneBatchReport, DevelopSettings, HISTORY_LABEL_AUTO_TONE, JobResult,
    MAX_AUTO_TONE_VARIANTS, VariantId,
};

fn run(core: &genzo_api::Core, ids: &[VariantId]) -> AutoToneBatchReport {
    match wait_ok(core, core.auto_tone_variants(ids).unwrap()) {
        JobResult::AutoTone(r) => r,
        other => panic!("{other:?}"),
    }
}

/// 選んだ写真のそれぞれに、1 枚のときと同じ値を入れて保存する（履歴に「自動補正」を 1 件。Undo で戻せる）。
/// 6 つの値のほかは変えない。同じ写真にもう一度当てても、値が同じなので保存しない。
#[test]
fn batch_applies_the_same_values_as_the_single_photo_command() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 8, None);
    write_jpeg(&env.photos.join("B.jpg"), (96, 64), 200, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;

    // 1 枚のときの値（適用はしない）。
    let mut expected = Vec::new();
    for v in [a, b] {
        core.open_develop(v).unwrap();
        expected.push(core.auto_tone(v).unwrap());
        core.close_develop().unwrap();
    }
    // B にはあらかじめ別の項目を入れておく（自動補正で変わらないことを確かめる）。
    let mut tinted: DevelopSettings = core.develop_settings(b).unwrap();
    tinted.color.saturation = 20.0;
    core.paste_settings(&tinted, &[b], genzo_api::SettingGroups::default())
        .unwrap();

    // 重なりは 1 つにする。
    let report = run(&core, &[a, b, a]);
    assert_eq!(
        (report.applied + report.unchanged, report.skipped),
        (2, 0),
        "{report:?}"
    );
    assert!(report.failed.is_empty() && !report.cancelled, "{report:?}");
    assert!(report.applied >= 1, "合成の画像でも値は変わる: {report:?}");
    for (v, auto) in [a, b].into_iter().zip(&expected) {
        let s = core.develop_settings(v).unwrap();
        assert_eq!(
            (s.exposure_ev, s.contrast, s.tone.highlights),
            (auto.exposure_ev, auto.contrast, auto.highlights),
            "variant {v}"
        );
        assert_eq!(
            (s.tone.shadows, s.tone.whites, s.tone.blacks),
            (auto.shadows, auto.whites, auto.blacks),
            "variant {v}"
        );
    }
    assert_eq!(core.develop_settings(b).unwrap().color.saturation, 20.0);
    // 履歴の最新は「自動補正」で、Undo で戻せる。
    let changed: Vec<VariantId> = [a, b]
        .into_iter()
        .filter(|v| {
            core.history(*v)
                .unwrap()
                .iter()
                .any(|h| h.is_current && h.label == HISTORY_LABEL_AUTO_TONE)
        })
        .collect();
    assert_eq!(changed.len() as u64, report.applied);
    // もう一度当てても、値が同じなので保存しない（履歴を増やさない）。
    let before: Vec<usize> = [a, b]
        .iter()
        .map(|v| core.history(*v).unwrap().len())
        .collect();
    let again = run(&core, &[a, b]);
    assert_eq!((again.applied, again.unchanged), (0, 2), "{again:?}");
    let after: Vec<usize> = [a, b]
        .iter()
        .map(|v| core.history(*v).unwrap().len())
        .collect();
    assert_eq!(before, after);
    // Undo で、当てる前の値に戻る。
    let v = changed[0];
    let undone = core.undo(v).unwrap().expect("戻せる");
    assert_ne!(
        undone,
        {
            let mut s = undone.clone();
            expected[usize::from(v == b)].apply(&mut s);
            s
        },
        "自動補正の前の値に戻っている"
    );
    core.close().unwrap();
}

/// 動画は飛ばし、見つからない写真は失敗に数えて、ほかの写真は続ける。JSON のコマンドでも同じ形。
#[test]
fn batch_skips_videos_and_reports_missing_photos() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 8, None);
    write_jpeg(&env.photos.join("Gone.jpg"), (96, 64), 90, None);
    let has_video = make_video(&env.photos.join("V.mp4"), "2026-01-02T03:04:05Z");
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let gone = by_name(&items, "Gone.jpg").variant_id;
    std::fs::remove_file(env.photos.join("Gone.jpg")).unwrap();
    let mut ids = vec![gone, a];
    if has_video {
        ids.push(by_name(&items, "V.mp4").variant_id);
    }

    let json: serde_json::Value = serde_json::from_str(&core.execute_json(
        &serde_json::json!({"type": "auto_tone_variants", "variant_ids": ids}).to_string(),
    ))
    .unwrap();
    assert_eq!(json["ok"]["type"], "job_started", "{json}");
    let report = match wait_ok(&core, json["ok"]["job_id"].as_u64().unwrap()) {
        JobResult::AutoTone(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(report.applied + report.unchanged, 1, "{report:?}");
    assert_eq!(report.skipped, u64::from(has_video), "{report:?}");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].variant_id, gone);
    assert!(!report.cancelled);
    // 結果の JSON の形。
    let v = serde_json::to_value(JobResult::AutoTone(report)).unwrap();
    assert_eq!(v["type"], "auto_tone");
    assert!(v["applied"].is_u64() && v["failed"].is_array());

    // 空・多すぎる指定は断る（ジョブを始めない）。
    assert!(matches!(
        core.auto_tone_variants(&[]),
        Err(ApiError::InvalidArgument(_))
    ));
    let too_many: Vec<VariantId> = (1..=MAX_AUTO_TONE_VARIANTS as i64 + 1)
        .map(VariantId::new)
        .collect();
    assert!(matches!(
        core.auto_tone_variants(&too_many),
        Err(ApiError::InvalidArgument(_))
    ));
    core.close().unwrap();
}

/// 現像中の写真が含まれていれば、保存待ちの変更を残したまま値を入れ、現像のセッションの設定も新しくなる。
#[test]
fn batch_updates_the_photo_open_in_develop() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 8, None);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    let state = core.open_develop(a).unwrap();
    let auto = core.auto_tone(a).unwrap();
    // 保存待ちの変更（彩度）。
    let mut pending = state.settings.clone();
    pending.color.saturation = 15.0;
    core.set_settings(&pending, false).unwrap();

    let report = run(&core, &[a]);
    assert_eq!(report.applied + report.unchanged, 1, "{report:?}");
    let now = core.develop_state().unwrap().unwrap().settings;
    assert_eq!(now.color.saturation, 15.0, "保存待ちの変更は残る");
    assert_eq!(
        (now.exposure_ev, now.tone.blacks),
        (auto.exposure_ev, auto.blacks)
    );
    assert_eq!(core.develop_settings(a).unwrap(), now);
    core.close_develop().unwrap();
    core.close().unwrap();
}
