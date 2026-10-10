//! 削除（6.4 節）とファイル操作の記録（file_op）のテスト。

use genzo_model::{AssetId, FileOpKind, FileOpState, FileRole};

use super::{Fixture, photo_req};
use crate::{Catalog, CatalogError, Filter, Sort};

#[test]
fn trash_plan_lists_all_files_and_variants_without_changes() {
    let mut f = Fixture::new();
    let raw = f.photo("A.ARW", 1, None);
    f.photo("A.JPG", 2, None);
    let vc = f
        .cat
        .create_virtual_copy(raw.master_variant_id, Some("白黒"))
        .unwrap();
    let other = f.photo("B.ARW", 3, None);

    let plan = f
        .cat
        .trash_plan(&[raw.asset_id, AssetId::new(9999)])
        .unwrap();
    assert_eq!(plan.assets, vec![raw.asset_id]);
    assert_eq!(plan.files.len(), 2);
    assert_eq!(plan.files[0].role, FileRole::Primary);
    assert_eq!(plan.files[0].location.name, "A.ARW");
    assert_eq!(plan.files[1].role, FileRole::SidecarJpeg);
    assert_eq!(
        plan.files[1].location.absolute_path().unwrap(),
        std::path::Path::new("/mnt/photos/2024/京都/A.JPG")
    );
    let variant_ids: Vec<_> = plan.variants.iter().map(|v| v.id).collect();
    assert_eq!(variant_ids, vec![raw.master_variant_id, vc]);
    // 一覧を作っただけでは何も変わらない。
    assert_eq!(f.rows("asset"), 2);
    assert_eq!(f.rows("file_op"), 0);
    // 選んだ variant から asset を求められる（仮想コピーを選んでも asset ごと）。
    assert_eq!(
        f.cat
            .assets_of_variants(&[vc, other.master_variant_id, raw.master_variant_id])
            .unwrap(),
        vec![raw.asset_id, other.asset_id]
    );
    assert_eq!(f.cat.asset_of_variant(vc).unwrap(), raw.asset_id);
}

#[test]
fn trash_operation_state_machine_and_catalog_update() {
    let mut f = Fixture::new();
    let raw = f.photo("A.ARW", 1, None);
    f.photo("A.JPG", 2, None);
    let keep = f.photo("B.ARW", 3, None);

    let (op, plan) = f.cat.plan_trash(&[raw.asset_id]).unwrap();
    assert_eq!(plan.files.len(), 2);
    let rec = f.cat.file_op(op).unwrap();
    assert_eq!(rec.kind, FileOpKind::Trash);
    assert_eq!(rec.state, FileOpState::Planned);
    assert_eq!(rec.payload.entries.len(), 2);
    assert!(rec.payload.entries.iter().all(|e| e.to.is_none()));
    assert_eq!(
        rec.payload.entries[0].from.absolute_path.as_deref(),
        Some(native_path(&["/mnt/photos", "2024", "京都", "A.ARW"]).as_str())
    );

    // planned から直接 done にはできるが、done の後は変えられない（遷移の確認）。
    f.cat.start_file_op(op).unwrap();
    assert_eq!(f.cat.file_op(op).unwrap().state, FileOpState::Executing);
    assert!(matches!(
        f.cat.start_file_op(op),
        Err(CatalogError::InvalidFileOpTransition {
            from: FileOpState::Executing,
            to: FileOpState::Executing,
            ..
        })
    ));
    // OS の操作が成功したので done。カタログから除かれる（同じトランザクション）。
    f.cat.complete_file_op(op).unwrap();
    assert_eq!(f.cat.file_op(op).unwrap().state, FileOpState::Done);
    assert_eq!(
        f.cat.search(&Filter::default(), &Sort::default()).unwrap(),
        vec![keep.master_variant_id]
    );
    assert_eq!(f.rows("file"), 1);
    for next in [
        FileOpState::Failed,
        FileOpState::Executing,
        FileOpState::Done,
    ] {
        assert!(matches!(
            f.cat.transition_file_op(op, next, None),
            Err(CatalogError::InvalidFileOpTransition { .. })
        ));
    }
    assert!(f.cat.unfinished_file_ops().unwrap().is_empty());
    assert!(f.cat.plan_trash(&[AssetId::new(9999)]).is_err());
}

#[test]
fn failed_operation_leaves_catalog_unchanged() {
    let mut f = Fixture::new();
    let raw = f.photo("A.ARW", 1, None);
    let (op, _) = f.cat.plan_trash(&[raw.asset_id]).unwrap();
    f.cat.start_file_op(op).unwrap();
    f.cat
        .fail_file_op(op, "ゴミ箱が使えないドライブです")
        .unwrap();
    let rec = f.cat.file_op(op).unwrap();
    assert_eq!(rec.state, FileOpState::Failed);
    assert_eq!(rec.error.as_deref(), Some("ゴミ箱が使えないドライブです"));
    assert_eq!(f.rows("asset"), 1);
    assert!(f.cat.complete_file_op(op).is_err());
    assert!(f.cat.file_op(genzo_model::FileOpId::new(999)).is_err());
}

#[test]
fn unfinished_operations_are_listed_after_restart() {
    let mut f = Fixture::new();
    let path = f.catalog_path();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    let c = f.photo("C.ARW", 3, None);
    let (planned, _) = f.cat.plan_trash(&[a.asset_id]).unwrap();
    let (executing, _) = f.cat.plan_trash(&[b.asset_id]).unwrap();
    f.cat.start_file_op(executing).unwrap();
    let (finished, _) = f.cat.plan_trash(&[c.asset_id]).unwrap();
    f.cat.fail_file_op(finished, "x").unwrap();
    // 異常終了（close を呼ばない）。
    drop(f.cat);

    let mut cat = Catalog::open(&path).unwrap();
    let pending = cat.unfinished_file_ops().unwrap();
    assert_eq!(
        pending.iter().map(|r| (r.id, r.state)).collect::<Vec<_>>(),
        vec![
            (planned, FileOpState::Planned),
            (executing, FileOpState::Executing)
        ]
    );
    // 呼び出し側が実際のファイルを確認して確定させる:
    // planned のもの → ファイルは元の場所にある → failed、executing のもの → ゴミ箱にある → done。
    cat.fail_file_op(planned, "起動時の確認で元の場所にありました")
        .unwrap();
    cat.complete_file_op(executing).unwrap();
    assert!(cat.unfinished_file_ops().unwrap().is_empty());
    assert_eq!(super::rows(&cat, "asset"), 2);
    // 終わった記録の整理。
    assert_eq!(cat.purge_finished_file_ops(1).unwrap(), 2);
    assert_eq!(super::rows(&cat, "file_op"), 1);
    cat.close().unwrap();
}

#[test]
fn move_and_rename_update_catalog_on_done() {
    let mut f = Fixture::new();
    let raw = f.photo("A.ARW", 1, None);
    let jpg = f.photo("A.JPG", 2, None);
    let dest = f.cat.ensure_folder(f.volume, "2024/選別済み").unwrap();
    let op = f.cat.plan_move(&[raw.file_id, jpg.file_id], dest).unwrap();
    let rec = f.cat.file_op(op).unwrap();
    assert_eq!(rec.kind, FileOpKind::Move);
    let to = rec.payload.entries[0].to.as_ref().unwrap();
    assert_eq!(to.folder_id, dest);
    assert_eq!(
        to.absolute_path.as_deref(),
        Some(native_path(&["/mnt/photos", "2024", "選別済み", "A.ARW"]).as_str())
    );
    // done の前はカタログは元のまま。
    assert_eq!(f.cat.file(raw.file_id).unwrap().folder_id, f.folder);
    f.cat.start_file_op(op).unwrap();
    f.cat.complete_file_op(op).unwrap();
    assert_eq!(f.cat.file(raw.file_id).unwrap().folder_id, dest);
    assert_eq!(f.cat.file(jpg.file_id).unwrap().folder_id, dest);
    assert_eq!(
        f.cat.file_location(raw.file_id).unwrap().rel_path(),
        "2024/選別済み/A.ARW"
    );
    // 移動先で登録し直しても件数は増えない（冪等）。
    let again = f
        .cat
        .register_file(&photo_req(dest, "A.ARW", 1, None))
        .unwrap();
    assert_eq!(again.file_id, raw.file_id);
    assert_eq!(f.rows("file"), 2);

    // リネーム（名前の入れ替えも同じ操作の中ならできる）。
    let b = f
        .cat
        .register_file(&photo_req(dest, "B.ARW", 3, None))
        .unwrap();
    let op = f
        .cat
        .plan_rename(&[
            (raw.file_id, "B.ARW".to_owned()),
            (b.file_id, "A.ARW".to_owned()),
        ])
        .unwrap();
    f.cat.complete_file_op(op).unwrap();
    assert_eq!(f.cat.file(raw.file_id).unwrap().name, "B.ARW");
    assert_eq!(f.cat.file(b.file_id).unwrap().name, "A.ARW");
    assert!(f.cat.check_integrity().unwrap().is_ok());
}

#[test]
fn conflicting_destinations_are_rejected_when_planning() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    // 既にある名前へのリネーム。
    assert!(matches!(
        f.cat.plan_rename(&[(a.file_id, "b.arw".to_owned())]),
        Err(CatalogError::Conflict(_))
    ));
    // 同じ操作の中で同じ名前になる。
    assert!(matches!(
        f.cat.plan_rename(&[
            (a.file_id, "X.ARW".to_owned()),
            (b.file_id, "x.arw".to_owned())
        ]),
        Err(CatalogError::Conflict(_))
    ));
    // 移動先に同じ名前がある。
    let dest = f.cat.ensure_folder(f.volume, "dest").unwrap();
    f.cat
        .register_file(&photo_req(dest, "A.ARW", 9, None))
        .unwrap();
    assert!(matches!(
        f.cat.plan_move(&[a.file_id], dest),
        Err(CatalogError::Conflict(_))
    ));
    // 不正な名前、存在しないファイル。
    assert!(f.cat.plan_rename(&[(a.file_id, "a/b".to_owned())]).is_err());
    assert!(f.cat.plan_rename(&[(a.file_id, String::new())]).is_err());
    assert!(
        f.cat
            .plan_rename(&[(genzo_model::FileId::new(999), "x".to_owned())])
            .is_err()
    );
    assert!(
        f.cat
            .plan_move(&[genzo_model::FileId::new(999)], dest)
            .is_err()
    );
    assert_eq!(f.rows("file_op"), 0);
}

#[test]
fn remove_assets_ignores_unknown_ids_and_keeps_files_on_disk() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("A.ARW");
    std::fs::write(&path, b"raw data").unwrap();
    let a = f.photo("A.ARW", 1, None);
    let report = f
        .cat
        .remove_assets(&[a.asset_id, AssetId::new(12345)])
        .unwrap();
    assert_eq!(report.assets, vec![a.asset_id]);
    assert_eq!(report.files, vec![a.file_id]);
    assert_eq!(f.rows("asset"), 0);
    // 元ファイルは触らない。
    assert_eq!(std::fs::read(&path).unwrap(), b"raw data");
    assert!(f.cat.remove_assets(&[]).unwrap().assets.is_empty());
    // 除いた後に同じファイルを登録し直すと、新しい asset になる（ID は再利用しない）。
    let again = f.photo("A.ARW", 1, None);
    assert!(again.asset_id > a.asset_id);
    assert!(again.master_variant_id > a.master_variant_id);
    assert!(again.file_id > a.file_id);
}

/// 移動の予定の後に asset をカタログから除いても、OS の移動が終わった記録を done にできる。
///
/// 修正前は、除かれたファイルの更新が NotFound になり、記録が executing のまま残っていた
/// （起動のたびに未完了として列挙され、done にできない。レビューで再現）。
#[test]
fn move_can_be_completed_after_an_asset_was_removed() {
    let mut f = Fixture::new();
    let removed = f.photo("A.ARW", 1, None);
    let kept = f.photo("B.ARW", 2, None);
    let dest = f.cat.ensure_folder(f.volume, "2024/選別済み").unwrap();
    let op = f
        .cat
        .plan_move(&[removed.file_id, kept.file_id], dest)
        .unwrap();
    f.cat.start_file_op(op).unwrap();
    f.cat.remove_assets(&[removed.asset_id]).unwrap();
    f.cat.complete_file_op(op).unwrap();
    assert_eq!(f.cat.file_op(op).unwrap().state, FileOpState::Done);
    assert!(f.cat.unfinished_file_ops().unwrap().is_empty());
    // 残っているファイルは移動先に移り、除いた asset は戻らない。
    assert_eq!(f.cat.file(kept.file_id).unwrap().folder_id, dest);
    assert!(matches!(
        f.cat.file(removed.file_id),
        Err(CatalogError::NotFound(_))
    ));
    assert_eq!(f.rows("asset"), 1);
    assert!(f.cat.check_integrity().unwrap().is_ok());

    // リネームでも同じ（すべて除かれていても done にできる）。
    let op = f
        .cat
        .plan_rename(&[(kept.file_id, "C.ARW".to_owned())])
        .unwrap();
    f.cat.remove_assets(&[kept.asset_id]).unwrap();
    f.cat.complete_file_op(op).unwrap();
    assert_eq!(f.cat.file_op(op).unwrap().state, FileOpState::Done);
    assert_eq!(f.rows("asset"), 0);
    assert!(f.cat.check_integrity().unwrap().is_ok());
}

/// OS の区切り文字で組み立てたパスの文字列（Windows では '\\' 区切りになるため）。
fn native_path(parts: &[&str]) -> String {
    let mut p = std::path::PathBuf::new();
    for part in parts {
        p.push(part);
    }
    p.to_string_lossy().into_owned()
}
