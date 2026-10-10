//! 現像設定・履歴・Undo / Redo・スナップショット・仮想コピー・一括適用のテスト
//! （DEV-00、DEV-27、DEV-28、DEV-30、LIB-13）。

use genzo_model::{CURRENT_PROCESS_VERSION, DevelopSettings, HistoryEntryId, VariantId};

use super::Fixture;
use crate::{Catalog, CatalogError, HISTORY_LABEL_IMPORT, HISTORY_LABEL_VIRTUAL_COPY};

fn exposure(ev: f32) -> DevelopSettings {
    DevelopSettings {
        exposure_ev: ev,
        ..Default::default()
    }
}

fn labels(cat: &Catalog, v: VariantId) -> Vec<String> {
    cat.history(v)
        .unwrap()
        .into_iter()
        .map(|h| h.label)
        .collect()
}

#[test]
fn new_variant_has_default_settings() {
    let mut f = Fixture::new();
    let v = f.photo("A.ARW", 1, None).master_variant_id;
    let state = f.cat.develop_state(v).unwrap();
    assert_eq!(state.settings, DevelopSettings::default());
    assert_eq!(state.process_version, CURRENT_PROCESS_VERSION);
    assert_eq!(
        state.develop_hash,
        DevelopSettings::default().develop_hash_hex()
    );
    assert!(!state.can_undo);
    assert!(!state.can_redo);
    // 未調整の設定は JSON を持たない（容量を抑える）。
    let json: Option<String> = f
        .cat
        .conn
        .query_row(
            "SELECT develop_json FROM variant WHERE id = ?1",
            [v.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(json, None);
}

#[test]
fn saving_updates_variant_and_adds_one_history_entry() {
    let mut f = Fixture::new();
    let v = f.photo("A.ARW", 1, None).master_variant_id;
    let before = f.cat.develop_state(v).unwrap().updated_at;
    let s = exposure(0.5);
    let entry = f.cat.save_develop(v, &s, "露光量 +0.50").unwrap();
    let state = f.cat.develop_state(v).unwrap();
    assert_eq!(state.settings, s);
    assert_eq!(state.develop_hash, s.develop_hash_hex());
    assert!(state.updated_at >= before);
    assert!(state.can_undo);
    assert!(!state.can_redo);
    let history = f.cat.history(v).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].label, HISTORY_LABEL_IMPORT);
    assert_eq!(history[1].id, entry);
    assert_eq!(history[1].label, "露光量 +0.50");
    assert!(history[1].is_current);
    assert_eq!(f.cat.history_settings(entry).unwrap(), s);
    assert_eq!(f.cat.develop_settings(v).unwrap(), s);
}

#[test]
fn undo_redo_and_branch_truncation() {
    let mut f = Fixture::new();
    let v = f.photo("A.ARW", 1, None).master_variant_id;
    f.cat.save_develop(v, &exposure(0.1), "1").unwrap();
    f.cat.save_develop(v, &exposure(0.2), "2").unwrap();
    f.cat.save_develop(v, &exposure(0.3), "3").unwrap();

    assert_eq!(f.cat.undo_develop(v).unwrap(), Some(exposure(0.2)));
    assert_eq!(f.cat.undo_develop(v).unwrap(), Some(exposure(0.1)));
    assert_eq!(
        f.cat.undo_develop(v).unwrap(),
        Some(DevelopSettings::default())
    );
    // 最初の状態より前には戻れない。
    assert_eq!(f.cat.undo_develop(v).unwrap(), None);
    assert_eq!(
        f.cat.develop_settings(v).unwrap(),
        DevelopSettings::default()
    );
    let state = f.cat.develop_state(v).unwrap();
    assert!(!state.can_undo && state.can_redo);
    // 既定の設定に戻したときのハッシュも正しい。
    assert_eq!(
        state.develop_hash,
        DevelopSettings::default().develop_hash_hex()
    );

    assert_eq!(f.cat.redo_develop(v).unwrap(), Some(exposure(0.1)));
    assert_eq!(f.cat.redo_develop(v).unwrap(), Some(exposure(0.2)));
    assert_eq!(
        f.cat.develop_state(v).unwrap().develop_hash,
        exposure(0.2).develop_hash_hex()
    );
    // Undo の後に新しい編集をすると、先の履歴（3）は削除される。
    f.cat.undo_develop(v).unwrap();
    f.cat
        .save_develop(v, &exposure(-1.0), "新しい編集")
        .unwrap();
    assert_eq!(
        labels(&f.cat, v),
        vec![HISTORY_LABEL_IMPORT, "1", "新しい編集"]
    );
    assert_eq!(f.cat.redo_develop(v).unwrap(), None);
    assert_eq!(f.cat.develop_settings(v).unwrap(), exposure(-1.0));
}

#[test]
fn undo_redo_survive_restart() {
    let mut f = Fixture::new();
    let path = f.catalog_path();
    let v = f.photo("A.ARW", 1, None).master_variant_id;
    f.cat.save_develop(v, &exposure(0.1), "1").unwrap();
    f.cat.save_develop(v, &exposure(0.2), "2").unwrap();
    f.cat.undo_develop(v).unwrap();
    f.cat.close().unwrap();

    let mut cat = Catalog::open(&path).unwrap();
    assert_eq!(cat.develop_settings(v).unwrap(), exposure(0.1));
    let state = cat.develop_state(v).unwrap();
    assert!(state.can_undo && state.can_redo);
    assert_eq!(cat.redo_develop(v).unwrap(), Some(exposure(0.2)));
    assert_eq!(cat.undo_develop(v).unwrap(), Some(exposure(0.1)));
    assert_eq!(
        cat.undo_develop(v).unwrap(),
        Some(DevelopSettings::default())
    );
    cat.close().unwrap();
}

#[test]
fn go_to_history_entry() {
    let mut f = Fixture::new();
    let v = f.photo("A.ARW", 1, None).master_variant_id;
    let first = f.cat.save_develop(v, &exposure(0.1), "1").unwrap();
    f.cat.save_develop(v, &exposure(0.2), "2").unwrap();
    assert_eq!(f.cat.go_to_history(v, first).unwrap(), exposure(0.1));
    let h = f.cat.history(v).unwrap();
    assert!(h[1].is_current);
    // 他の variant の履歴へは移動できない。
    let other = f.photo("B.ARW", 2, None).master_variant_id;
    assert!(matches!(
        f.cat.go_to_history(other, first),
        Err(CatalogError::NotFound(_))
    ));
    assert!(f.cat.history_settings(HistoryEntryId::new(9999)).is_err());
}

#[test]
fn invalid_settings_are_not_saved() {
    let mut f = Fixture::new();
    let v = f.photo("A.ARW", 1, None).master_variant_id;
    let bad = exposure(f32::NAN);
    assert!(matches!(
        f.cat.save_develop(v, &bad, "x"),
        Err(CatalogError::Develop(_))
    ));
    let out_of_range = exposure(99.0);
    assert!(f.cat.save_develop(v, &out_of_range, "x").is_err());
    // 丸めれば保存できる。
    f.cat
        .save_develop(v, &out_of_range.normalized(), "丸めた")
        .unwrap();
    assert_eq!(f.cat.develop_settings(v).unwrap().exposure_ev, 5.0);
    assert_eq!(f.cat.history(v).unwrap().len(), 2);
    assert!(matches!(
        f.cat
            .save_develop(VariantId::new(9999), &exposure(0.1), "x"),
        Err(CatalogError::NotFound(_))
    ));
}

#[test]
fn snapshots_can_be_created_applied_and_deleted() {
    let mut f = Fixture::new();
    let v = f.photo("A.ARW", 1, None).master_variant_id;
    f.cat.save_develop(v, &exposure(0.7), "露光量").unwrap();
    let snap = f.cat.create_snapshot(v, " 明るめ ").unwrap();
    f.cat.save_develop(v, &exposure(-0.3), "露光量").unwrap();
    let list = f.cat.snapshots(v).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "明るめ");
    assert_eq!(f.cat.snapshot_settings(snap).unwrap(), exposure(0.7));
    f.cat.apply_snapshot(v, snap).unwrap();
    assert_eq!(f.cat.develop_settings(v).unwrap(), exposure(0.7));
    assert_eq!(
        labels(&f.cat, v).last().unwrap(),
        "スナップショット「明るめ」を適用"
    );
    // 別の variant にも適用できる。
    let other = f.photo("B.ARW", 2, None).master_variant_id;
    f.cat.apply_snapshot(other, snap).unwrap();
    assert_eq!(f.cat.develop_settings(other).unwrap(), exposure(0.7));
    f.cat.delete_snapshot(snap).unwrap();
    assert!(f.cat.snapshots(v).unwrap().is_empty());
    assert!(matches!(
        f.cat.delete_snapshot(snap),
        Err(CatalogError::NotFound(_))
    ));
    assert!(f.cat.apply_snapshot(v, snap).is_err());
    assert!(f.cat.create_snapshot(v, "  ").is_err());
    assert!(f.cat.create_snapshot(VariantId::new(9999), "x").is_err());
}

#[test]
fn virtual_copies_copy_settings_and_cannot_delete_master() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, None);
    let master = o.master_variant_id;
    f.cat
        .save_develop(master, &exposure(1.0), "露光量")
        .unwrap();
    f.cat
        .set_rating(&[master], genzo_model::Rating::MAX)
        .unwrap();
    let vc = f.cat.create_virtual_copy(master, None).unwrap();
    let vc2 = f.cat.create_virtual_copy(master, Some("モノクロ")).unwrap();
    assert_eq!(f.cat.develop_settings(vc).unwrap(), exposure(1.0));
    assert_eq!(labels(&f.cat, vc), vec![HISTORY_LABEL_VIRTUAL_COPY]);
    // 評価はコピーしない。
    assert_eq!(f.cat.variant_marks(vc).unwrap().0, genzo_model::Rating::MIN);
    let variants = f.cat.variants_of_asset(o.asset_id).unwrap();
    assert_eq!(variants.len(), 3);
    assert!(variants[0].is_master);
    assert_eq!(variants[1].name.as_deref(), Some("コピー 1"));
    assert_eq!(variants[2].name.as_deref(), Some("モノクロ"));
    // 仮想コピーの編集はマスターに影響しない。
    f.cat.save_develop(vc, &exposure(-2.0), "露光量").unwrap();
    assert_eq!(f.cat.develop_settings(master).unwrap(), exposure(1.0));
    // 仮想コピーの Undo は、作ったときの状態まで。
    assert_eq!(f.cat.undo_develop(vc).unwrap(), Some(exposure(1.0)));
    assert_eq!(f.cat.undo_develop(vc).unwrap(), None);

    assert!(matches!(
        f.cat.delete_virtual_copy(master),
        Err(CatalogError::CannotDeleteMaster(v)) if v == master
    ));
    f.cat.create_snapshot(vc, "s").unwrap();
    f.cat.delete_virtual_copy(vc).unwrap();
    assert_eq!(f.cat.variants_of_asset(o.asset_id).unwrap().len(), 2);
    assert!(f.cat.history(vc).is_err());
    assert_eq!(f.rows("snapshot"), 0);
    assert!(matches!(
        f.cat.delete_virtual_copy(vc),
        Err(CatalogError::NotFound(_))
    ));
    assert!(
        f.cat
            .create_virtual_copy(VariantId::new(9999), None)
            .is_err()
    );
    assert!(f.cat.create_virtual_copy(master, Some(" ")).is_err());
    let _ = vc2;
}

#[test]
fn bulk_apply_saves_to_each_variant_with_one_history_entry() {
    let mut f = Fixture::new();
    let ids: Vec<VariantId> = (0..5)
        .map(|i| f.photo(&format!("IMG{i}.ARW"), i, None).master_variant_id)
        .collect();
    let s = DevelopSettings {
        exposure_ev: 0.3,
        contrast: 20.0,
        ..Default::default()
    };
    let entries = f
        .cat
        .apply_develop_to_many(&ids, &s, "設定の貼り付け")
        .unwrap();
    assert_eq!(entries.len(), 5);
    for &v in &ids {
        assert_eq!(f.cat.develop_settings(v).unwrap(), s);
        assert_eq!(
            labels(&f.cat, v),
            vec![HISTORY_LABEL_IMPORT, "設定の貼り付け"]
        );
    }
    // 重複した id でも履歴は 1 件。
    let dup = f
        .cat
        .apply_develop_to_many(&[ids[0], ids[0]], &exposure(0.4), "重複")
        .unwrap();
    assert_eq!(dup[0], dup[1]);
    assert_eq!(f.cat.history(ids[0]).unwrap().len(), 3);
    f.cat.undo_develop(ids[0]).unwrap();
    // 1 件でも存在しなければ、すべて取り消す。
    let mut with_missing = ids.clone();
    with_missing.push(VariantId::new(9999));
    assert!(
        f.cat
            .apply_develop_to_many(&with_missing, &exposure(-1.0), "x")
            .is_err()
    );
    for &v in &ids {
        assert_eq!(f.cat.develop_settings(v).unwrap(), s);
        let expected = if v == ids[0] { 3 } else { 2 };
        assert_eq!(f.cat.history(v).unwrap().len(), expected);
    }
    // 不正な設定は、どれにも保存しない。
    assert!(
        f.cat
            .apply_develop_to_many(&ids, &exposure(f32::INFINITY), "x")
            .is_err()
    );
}

#[test]
fn reverting_to_default_settings_is_stored_compactly() {
    let mut f = Fixture::new();
    let v = f.photo("A.ARW", 1, None).master_variant_id;
    f.cat.save_develop(v, &exposure(0.5), "露光量").unwrap();
    f.cat
        .save_develop(v, &DevelopSettings::default(), "初期化")
        .unwrap();
    let (json, hash): (Option<String>, String) = f
        .cat
        .conn
        .query_row(
            "SELECT develop_json, develop_hash FROM variant WHERE id = ?1",
            [v.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(json, None);
    assert_eq!(hash, DevelopSettings::default().develop_hash_hex());
}
