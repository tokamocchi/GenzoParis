//! AI マスク（DEV-20）: 推論の仕組みの差し替え（genzo-ml の `FixedSegmenter`）で、計算 → カタログへの保存 → 設定への
//! 適用 → 描画（右半分だけに効く）→ 履歴・スナップショット・仮想コピーの参照 → 回収 → 使えないときのエラーを、
//! コア API を通して確かめる。ONNX Runtime とモデルは要らない。

mod common;

use std::sync::Arc;

use common::{Env, all_items, by_name, import, write_jpeg};
use genzo_api::{ApiError, Command, Core, ErrorKind, Response};
use genzo_ml::FixedSegmenter;
use genzo_model::{
    AiMaskKind, DevelopSettings, LocalAdjustment, LocalDelta, MaskComponent, MaskShape, VariantId,
};

fn labels(core: &Core, v: VariantId) -> Vec<String> {
    core.history(v)
        .unwrap()
        .into_iter()
        .map(|h| h.label)
        .collect()
}

fn open_photo(env: &Env, core: &Core) -> VariantId {
    import(core, &env.photos);
    let items = all_items(core);
    let a = by_name(&items, "A.jpg").variant_id;
    core.open_develop(a).unwrap();
    a
}

#[test]
fn ai_masks_are_computed_stored_applied_and_rendered() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 3, None);
    let core = env.open_with_hooks(|h| h.segmenter = Some(Arc::new(FixedSegmenter::right_half())));
    let a = open_photo(&env, &core);
    // 状態: 3 種類とも使える。
    let status = core.ai_mask_status().unwrap();
    assert!(status.available, "{status:?}");
    assert_eq!(status.kinds, AiMaskKind::ALL);
    assert_eq!(status.backend, "fixed");
    assert_eq!(status.rasters, 0);
    // 計算: 右半分のラスター。形は G の全体（64 × 48 → [0, 0, 1, 0.75]）。
    let r = core.compute_ai_mask(a, AiMaskKind::Sky).unwrap();
    assert_eq!(r.kind, AiMaskKind::Sky);
    assert_eq!(r.model, "fixed-right-half");
    assert_eq!((r.width, r.height), (64, 48));
    let MaskShape::Ai {
        kind,
        model,
        mask,
        width,
        height,
        bounds,
    } = &r.shape
    else {
        panic!("{:?}", r.shape)
    };
    assert_eq!(*kind, AiMaskKind::Sky);
    assert_eq!(model, "fixed-right-half");
    assert_eq!(mask.id, "ai-mask:sky");
    assert_eq!(mask.sha256, r.sha256);
    assert_eq!((*width, *height), (64, 48));
    assert!(
        bounds[0].abs() < 1e-6
            && bounds[1].abs() < 1e-6
            && (bounds[2] - 1.0).abs() < 1e-6
            && (bounds[3] - 0.75).abs() < 1e-6,
        "{bounds:?}"
    );
    let status = core.ai_mask_status().unwrap();
    assert_eq!(status.rasters, 1);
    assert!(status.raster_bytes > 0);
    // 同じ内容をもう一度計算しても 1 行。
    let again = core.compute_ai_mask(a, AiMaskKind::Subject).unwrap();
    assert_eq!(again.sha256, r.sha256);
    assert_eq!(core.ai_mask_status().unwrap().rasters, 1);
    // 設定に入れて描く: 左は変わらず、右は明るい。
    let base = core.render_develop().unwrap();
    let before = core.develop_frame().unwrap().b3.image.clone();
    let mut s = core.develop_settings(a).unwrap();
    s.masks.push(LocalAdjustment {
        name: "空".to_owned(),
        delta: LocalDelta {
            exposure_ev: 1.0,
            ..Default::default()
        },
        ..LocalAdjustment::new(MaskComponent::add(r.shape.clone()))
    });
    core.set_settings(&s, false).unwrap();
    let info = core.render_develop().unwrap();
    assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    assert_eq!((info.width, info.height), (base.width, base.height));
    let after = core.develop_frame().unwrap().b3.image.clone();
    let (w, h) = before.dimensions();
    let (pl0, pl1) = (
        before.pixel(4, h / 2).unwrap(),
        after.pixel(4, h / 2).unwrap(),
    );
    assert_eq!(pl0, pl1);
    let (pr0, pr1) = (
        before.pixel(w - 4, h / 2).unwrap(),
        after.pixel(w - 4, h / 2).unwrap(),
    );
    assert!(pr1[1] > pr0[1] * 1.5, "{pr0:?} {pr1:?}");
    // 重ねて表示もラスターを使う。
    let o = core.mask_overlay(a, 0, None).unwrap();
    assert_eq!(
        o.weights[(o.info.height / 2 * o.info.width + 2) as usize],
        0
    );
    assert_eq!(
        o.weights[(o.info.height / 2 * o.info.width + o.info.width - 3) as usize],
        255
    );
    core.flush_develop().unwrap();
    assert_eq!(labels(&core, a).last().unwrap(), "マスクを追加");
    // 保存した設定は参照だけを持つ（ラスターは JSON に入らない）。
    let saved = core.develop_settings(a).unwrap();
    let json = saved.to_json().unwrap();
    assert!(json.contains(&r.sha256));
    assert!(json.len() < 4000, "{}", json.len());
    // 仮想コピーは同じ参照を写し、同じように描ける。
    let copy = core.create_virtual_copy(a, Some("コピー")).unwrap();
    let cs = core.develop_settings(copy).unwrap();
    assert_eq!(cs.masks, saved.masks);
    // 参照のない行の回収: 今の設定が参照しているので消えない。
    let report = core.collect_ai_masks().unwrap();
    assert_eq!((report.removed, report.remaining), (0, 1));
    // マスクを外して履歴を消す（Undo の後の新しい編集）と、回収で消え、設定が参照していれば警告して描く。
    core.close_develop().unwrap();
    core.open_develop(a).unwrap();
    let mut t = core.develop_settings(a).unwrap();
    t.masks.clear();
    core.set_settings(&t, false).unwrap();
    core.flush_develop().unwrap();
    // コピーの設定はまだ参照している。
    assert_eq!(core.collect_ai_masks().unwrap().removed, 0);
    core.close_develop().unwrap();
    let plan = core
        .plan_delete(genzo_api::types::DeleteKind::VirtualCopies, &[copy])
        .unwrap();
    core.execute_delete(plan.plan_id).unwrap();
    let report = core.collect_ai_masks().unwrap();
    // 履歴（a の「マスクを追加」の項目）がまだ参照している。
    assert_eq!(report.removed, 0, "{report:?}");
    core.open_develop(a).unwrap();
    core.undo(a).unwrap();
    core.undo(a).unwrap();
    let mut u = core.develop_settings(a).unwrap();
    u.exposure_ev = 0.3;
    core.set_settings(&u, false).unwrap();
    core.flush_develop().unwrap();
    assert_eq!(core.collect_ai_masks().unwrap().removed, 1);
    // 消えたラスターを指す設定: 重み 0 で描いて知らせる。
    core.set_settings(&s, false).unwrap();
    let info = core.render_develop().unwrap();
    assert!(
        info.warnings.iter().any(|w| w.contains("AI マスク")),
        "{:?}",
        info.warnings
    );
    core.close_develop().unwrap();
    core.close().unwrap();
}

#[test]
fn ai_masks_report_when_no_model_is_available() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 3, None);
    let core = env.open_with_hooks(|h| h.segmenter = Some(Arc::new(FixedSegmenter::unavailable())));
    let a = open_photo(&env, &core);
    let status = core.ai_mask_status().unwrap();
    assert!(!status.available);
    assert!(status.kinds.is_empty());
    assert_eq!(status.problems.len(), 1);
    let err = core.compute_ai_mask(a, AiMaskKind::Person).unwrap_err();
    assert!(matches!(err, ApiError::AiMask(_)), "{err:?}");
    let info = err.info();
    assert_eq!(info.kind, ErrorKind::InvalidInput);
    assert!(info.hint.as_deref().unwrap().contains("ONNX Runtime"));
    // 写真を開いていなければ NoDevelopSession（計算の前に確かめる）。
    core.close_develop().unwrap();
    assert!(matches!(
        core.compute_ai_mask(a, AiMaskKind::Person),
        Err(ApiError::NoDevelopSession | ApiError::AiMask(_))
    ));
    core.close().unwrap();
}

#[test]
fn ai_masks_without_a_runtime_explain_the_missing_pieces() {
    // 差し替えなし・モデルのフォルダなし: 使えない理由が入り、計算はエラー。
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 3, None);
    let models = env.dir.path().join("models-empty");
    let core = env.open_with(|c| {
        c.models_dir = Some(models.clone());
        c.onnxruntime = Some(env.dir.path().join("no-such-onnxruntime.so"));
    });
    let a = open_photo(&env, &core);
    let status = core.ai_mask_status().unwrap();
    assert!(!status.available, "{status:?}");
    assert!(!status.problems.is_empty());
    assert_eq!(status.models_dir.as_deref(), Some(models.as_path()));
    assert!(matches!(
        core.compute_ai_mask(a, AiMaskKind::Sky),
        Err(ApiError::AiMask(_))
    ));
    core.close_develop().unwrap();
    core.close().unwrap();
}

#[test]
fn ai_mask_commands_round_trip_through_json() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 3, None);
    let core = env.open_with_hooks(|h| h.segmenter = Some(Arc::new(FixedSegmenter::right_half())));
    let a = open_photo(&env, &core);
    let cmd: Command = serde_json::from_value(serde_json::json!({
        "type": "compute_ai_mask", "variant_id": a, "kind": "person"
    }))
    .unwrap();
    let Response::AiMask(r) = core.execute(cmd).unwrap() else {
        panic!()
    };
    assert_eq!(r.kind, AiMaskKind::Person);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["shape"]["ai"]["kind"], "person");
    assert_eq!(v["shape"]["ai"]["mask"]["id"], "ai-mask:person");
    let Response::AiMaskStatus(st) = core.execute(Command::AiMaskStatus).unwrap() else {
        panic!()
    };
    assert!(st.available);
    // 設定の JSON にも入る（検証を通る）。
    let mut s = core.develop_settings(a).unwrap();
    s.masks
        .push(LocalAdjustment::new(MaskComponent::add(r.shape.clone())));
    let json = s.to_json().unwrap();
    let back = DevelopSettings::from_json(&json).unwrap();
    assert_eq!(back.masks, s.masks);
    // 設定に入れていないラスターは、回収で消える。
    let Response::AiMasksCollected(c) = core.execute(Command::CollectAiMasks).unwrap() else {
        panic!()
    };
    assert_eq!((c.removed, c.remaining), (1, 0));
    core.close_develop().unwrap();
    core.close().unwrap();
}

/// 設定の貼り付け（`paste_settings`。カタログのロックの中で参照を引き直す経路）に AI マスクが含まれても止まらず、
/// 貼り付け先でも同じラスターで描ける（ラスターは内容のアドレスなので、別の variant から指せる）。
#[test]
fn ai_masks_can_be_pasted_to_another_variant() {
    use genzo_api::types::SettingGroups;
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 3, None);
    write_jpeg(&env.photos.join("B.jpg"), (64, 48), 5, None);
    let core = env.open_with_hooks(|h| h.segmenter = Some(Arc::new(FixedSegmenter::right_half())));
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;
    core.open_develop(a).unwrap();
    let r = core.compute_ai_mask(a, AiMaskKind::Subject).unwrap();
    let mut s = core.develop_settings(a).unwrap();
    s.masks.push(LocalAdjustment {
        delta: LocalDelta {
            exposure_ev: 1.0,
            ..Default::default()
        },
        ..LocalAdjustment::new(MaskComponent::add(r.shape.clone()))
    });
    core.set_settings(&s, false).unwrap();
    core.flush_develop().unwrap();
    // 現像中の写真（a）を開いたまま、b と a 自身に貼り付ける（ロックの順の確認）。
    let groups = SettingGroups {
        masks: true,
        ..SettingGroups::ALL
    };
    assert_eq!(core.paste_settings(&s, &[b, a], groups).unwrap(), 2);
    core.close_develop().unwrap();
    assert_eq!(core.develop_settings(b).unwrap().masks, s.masks);
    core.open_develop(b).unwrap();
    let info = core.render_develop().unwrap();
    assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    let o = core.mask_overlay(b, 0, None).unwrap();
    assert_eq!(
        o.weights[(o.info.height / 2 * o.info.width + 2) as usize],
        0
    );
    assert_eq!(
        o.weights[(o.info.height / 2 * o.info.width + o.info.width - 3) as usize],
        255
    );
    core.close_develop().unwrap();
    core.close().unwrap();
}
