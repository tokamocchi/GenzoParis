//! AI マスクのラスター（DEV-20。`ai_mask`）: 保存と読み出し、同じ内容の重複、asset の削除（行は残る）、参照のない行の回収、
//! 版 14 への移行。

use genzo_model::{
    AiMaskKind, AssetId, DataRef, DevelopSettings, LocalAdjustment, MaskComponent, MaskShape,
};

use super::{Fixture, photo_req};
use crate::{AI_MASKS_SCHEMA_VERSION, AiMaskInsert, CATALOG_SCHEMA_VERSION, Catalog, CatalogError};

fn raster(asset_id: AssetId, seed: u8, kind: &str) -> AiMaskInsert {
    let data: Vec<u8> = (0..24u8)
        .map(|i| i.wrapping_mul(7).wrapping_add(seed))
        .collect();
    AiMaskInsert {
        sha256: genzo_model::ai_mask_sha256(6, 4, &data),
        asset_id,
        kind: kind.to_owned(),
        model: "test-model".to_owned(),
        width: 6,
        height: 4,
        encoding: "raw".to_owned(),
        data,
    }
}

fn settings_with(sha256: &str) -> DevelopSettings {
    let mut s = DevelopSettings {
        process_version: 2,
        ..Default::default()
    };
    s.masks
        .push(LocalAdjustment::new(MaskComponent::add(MaskShape::Ai {
            kind: AiMaskKind::Sky,
            model: "test-model".into(),
            mask: DataRef::new(AiMaskKind::Sky.ref_id(), sha256),
            width: 6,
            height: 4,
            bounds: [0.0, 0.0, 1.0, 0.667],
        })));
    s
}

#[test]
fn ai_masks_are_stored_once_per_content_and_read_back() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let m = raster(a.asset_id, 1, "sky");
    assert!(f.cat.put_ai_mask(&m).unwrap());
    // 同じ内容は 1 行（何もしない）。
    assert!(!f.cat.put_ai_mask(&m).unwrap());
    assert_eq!(f.rows("ai_mask"), 1);
    let r = f.cat.ai_mask(&m.sha256).unwrap().unwrap();
    assert_eq!(r.sha256, m.sha256);
    assert_eq!(r.asset_id, Some(a.asset_id));
    assert_eq!((r.kind.as_str(), r.model.as_str()), ("sky", "test-model"));
    assert_eq!((r.width, r.height, r.encoding.as_str()), (6, 4, "raw"));
    assert_eq!(r.data, m.data);
    assert!(f.cat.ai_mask(&"00".repeat(32)).unwrap().is_none());
    let other = raster(a.asset_id, 2, "person");
    assert!(f.cat.put_ai_mask(&other).unwrap());
    let infos = f.cat.ai_masks_of_asset(a.asset_id).unwrap();
    assert_eq!(infos.len(), 2);
    let mut shas: Vec<&str> = infos.iter().map(|i| i.sha256.as_str()).collect();
    shas.sort_unstable();
    let mut expected = [m.sha256.as_str(), other.sha256.as_str()];
    expected.sort_unstable();
    assert_eq!(shas, expected);
    assert!(infos.iter().any(|i| i.kind == "person"));
    assert!(infos.iter().all(|i| i.bytes == 24));
    assert_eq!(f.cat.ai_mask_totals().unwrap(), (2, 48));
}

#[test]
fn invalid_ai_masks_are_rejected() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let good = raster(a.asset_id, 1, "sky");
    let bad_sha = AiMaskInsert {
        sha256: "ABC".into(),
        ..good.clone()
    };
    assert!(matches!(
        f.cat.put_ai_mask(&bad_sha),
        Err(CatalogError::InvalidInput(_))
    ));
    let bad_encoding = AiMaskInsert {
        encoding: "jpeg".into(),
        ..good.clone()
    };
    assert!(matches!(
        f.cat.put_ai_mask(&bad_encoding),
        Err(CatalogError::InvalidInput(_))
    ));
    let bad_len = AiMaskInsert {
        width: 7,
        ..good.clone()
    };
    assert!(matches!(
        f.cat.put_ai_mask(&bad_len),
        Err(CatalogError::InvalidInput(_))
    ));
    let bad_kind = AiMaskInsert {
        kind: "cat".into(),
        ..good.clone()
    };
    assert!(f.cat.put_ai_mask(&bad_kind).is_err());
    let no_asset = AiMaskInsert {
        asset_id: AssetId::new(9999),
        ..good.clone()
    };
    assert!(matches!(
        f.cat.put_ai_mask(&no_asset),
        Err(CatalogError::NotFound(_))
    ));
    assert_eq!(f.rows("ai_mask"), 0);
}

#[test]
fn unreferenced_ai_masks_are_collected_and_outlive_their_asset() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    let used = raster(a.asset_id, 1, "sky");
    let in_history = raster(a.asset_id, 2, "subject");
    let in_snapshot = raster(a.asset_id, 3, "person");
    let unused = raster(a.asset_id, 4, "sky");
    let of_b = raster(b.asset_id, 5, "sky");
    for m in [&used, &in_history, &in_snapshot, &unused, &of_b] {
        f.cat.put_ai_mask(m).unwrap();
    }
    let v = a.master_variant_id;
    // 履歴にだけ残る参照、スナップショットにだけ残る参照、今の設定の参照。
    f.cat
        .save_develop(v, &settings_with(&in_history.sha256), "AI 1")
        .unwrap();
    f.cat
        .save_develop(v, &settings_with(&in_snapshot.sha256), "AI 2")
        .unwrap();
    f.cat.create_snapshot(v, "s").unwrap();
    f.cat
        .save_develop(v, &settings_with(&used.sha256), "AI 3")
        .unwrap();
    // 履歴を消す（Undo で「読み込み」まで戻った後の新しい編集）: in_history は履歴にだけあった。
    f.cat.undo_develop(v).unwrap();
    f.cat.undo_develop(v).unwrap();
    f.cat.undo_develop(v).unwrap();
    f.cat
        .save_develop(v, &settings_with(&used.sha256), "AI 4")
        .unwrap();
    assert_eq!(f.cat.delete_unreferenced_ai_masks().unwrap(), 3);
    assert!(f.cat.ai_mask(&used.sha256).unwrap().is_some());
    assert!(f.cat.ai_mask(&in_snapshot.sha256).unwrap().is_some());
    assert!(f.cat.ai_mask(&in_history.sha256).unwrap().is_none());
    assert!(f.cat.ai_mask(&unused.sha256).unwrap().is_none());
    assert!(f.cat.ai_mask(&of_b.sha256).unwrap().is_none());
    // asset を消しても、ラスターは残る（推論した写真の記録は NULL）。参照がなくなれば回収で消える。
    f.cat.remove_assets(&[a.asset_id]).unwrap();
    assert_eq!(f.rows("ai_mask"), 2);
    assert_eq!(f.cat.ai_mask(&used.sha256).unwrap().unwrap().asset_id, None);
    assert_eq!(f.cat.delete_unreferenced_ai_masks().unwrap(), 2);
    assert_eq!(f.rows("ai_mask"), 0);
}

/// 別の asset に貼り付けた設定が参照するラスターは、推論した asset を消しても消えない（Codex のレビューの指摘）。
#[test]
fn ai_masks_pasted_to_another_asset_survive_deleting_the_source() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    let m = raster(a.asset_id, 1, "sky");
    f.cat.put_ai_mask(&m).unwrap();
    // a で推論したラスターを、b の設定だけが参照する（貼り付け）。
    f.cat
        .save_develop(b.master_variant_id, &settings_with(&m.sha256), "貼り付け")
        .unwrap();
    f.cat.remove_assets(&[a.asset_id]).unwrap();
    let r = f.cat.ai_mask(&m.sha256).unwrap().expect("b が参照している");
    assert_eq!(r.asset_id, None);
    assert_eq!(r.data, m.data);
    assert_eq!(f.cat.delete_unreferenced_ai_masks().unwrap(), 0);
    assert!(f.cat.ai_mask(&m.sha256).unwrap().is_some());
}

/// 版 13 のカタログを開くと版 14 に移行し、`ai_mask` の表（空）ができる。
#[test]
fn version_13_catalog_migrates_to_ai_masks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let asset = {
        let mut cat = crate::testing::open_at_version(&path, AI_MASKS_SCHEMA_VERSION - 1).unwrap();
        let vol = cat.ensure_volume("v", None, None).unwrap();
        let folder = cat.ensure_folder(vol, "2024").unwrap();
        let a = cat
            .register_file(&photo_req(folder, "A.ARW", 1, Some("2024:05:01 12:00:00")))
            .unwrap();
        assert!(cat.conn.prepare("SELECT sha256 FROM ai_mask").is_err());
        cat.close().unwrap();
        a.asset_id
    };
    let mut cat = Catalog::open(&path).unwrap();
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    assert_eq!(cat.ai_mask_totals().unwrap(), (0, 0));
    assert!(cat.put_ai_mask(&raster(asset, 1, "sky")).unwrap());
    assert_eq!(cat.ai_mask_totals().unwrap(), (1, 24));
}
