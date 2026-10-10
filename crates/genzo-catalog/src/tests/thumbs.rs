//! サムネイル DB（L0）と L1 プレビューのファイルキャッシュのテスト（4 章・4.1 節）。

use std::collections::HashSet;

use genzo_model::{
    CacheColorSpace, CacheFormat, CacheKey, CacheKind, DevelopSettings, FileId, RenderQuality,
    VariantId,
};

use super::Fixture;
use crate::{CatalogError, PreviewCache, ThumbStore};

/// JPEG の先頭の印を持つダミーのデータ。
fn jpeg(n: usize, fill: u8) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
    v.resize(n.max(4), fill);
    v
}

fn key(file: i64, kind: CacheKind) -> String {
    CacheKey::for_settings(
        FileId::new(file),
        1,
        &DevelopSettings::default(),
        (320, 213),
        RenderQuality::Final,
        CacheFormat {
            kind,
            color_space: CacheColorSpace::DisplayP3,
            format_version: 1,
        },
    )
    .hex()
}

#[test]
fn thumbs_db_uses_wal_and_normal_sync() {
    let dir = tempfile::tempdir().unwrap();
    let store = ThumbStore::open(dir.path().join("thumbs.db")).unwrap();
    let s = store.connection_settings().unwrap();
    assert_eq!(s.journal_mode, "wal");
    assert_eq!(s.synchronous, 1, "synchronous = NORMAL");
    assert!(s.foreign_keys);
    assert!(!s.fullfsync);
    store.close().unwrap();
    // カタログを thumbs.db として開かない。
    let f = Fixture::new();
    assert!(matches!(
        ThumbStore::open(f.catalog_path()),
        Err(CatalogError::NotOurDatabase(_))
    ));
}

#[test]
fn thumbnails_are_stored_replaced_and_checked_by_key() {
    let mut store = ThumbStore::open_in_memory().unwrap();
    let v = VariantId::new(1);
    let k1 = key(1, CacheKind::L0Thumb);
    let k2 = key(2, CacheKind::L0Thumb);
    assert_eq!(store.get(v).unwrap(), None);
    store.put(v, &k1, &jpeg(100, 1)).unwrap();
    let t = store.get(v).unwrap().unwrap();
    assert_eq!(t.cache_key, k1);
    assert_eq!(t.jpeg, jpeg(100, 1));
    assert_eq!(store.get_if_current(v, &k1).unwrap(), Some(jpeg(100, 1)));
    // キーが変わったら使わない。
    assert_eq!(store.get_if_current(v, &k2).unwrap(), None);
    // 置き換え。
    store.put(v, &k2, &jpeg(200, 2)).unwrap();
    assert_eq!(store.cache_key(v).unwrap(), Some(k2.clone()));
    assert_eq!(store.count().unwrap(), 1);
    assert_eq!(store.total_bytes().unwrap(), 200);
    // 不正な入力。
    assert!(store.put(v, "../x", &jpeg(10, 0)).is_err());
    assert!(store.put(v, &k1, b"not a jpeg").is_err());
    assert_eq!(store.cache_key(v).unwrap(), Some(k2));
    assert_eq!(store.remove(&[v, VariantId::new(9)]).unwrap(), 1);
    assert_eq!(store.count().unwrap(), 0);
}

#[test]
fn garbage_collection_removes_rows_of_deleted_variants() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    let vc = f
        .cat
        .create_virtual_copy(a.master_variant_id, None)
        .unwrap();
    let mut store = ThumbStore::open(f.dir.path().join("thumbs.db")).unwrap();
    for (i, v) in [a.master_variant_id, b.master_variant_id, vc]
        .iter()
        .enumerate()
    {
        store
            .put(*v, &key(i as i64, CacheKind::L0Thumb), &jpeg(50, i as u8))
            .unwrap();
    }
    f.cat.delete_virtual_copy(vc).unwrap();
    let removed = f.cat.remove_assets(&[b.asset_id]).unwrap();
    assert_eq!(removed.variants, vec![b.master_variant_id]);
    let alive: HashSet<VariantId> = f.cat.all_variant_ids().unwrap().into_iter().collect();
    assert_eq!(store.collect_garbage(&alive).unwrap(), 2);
    assert_eq!(store.count().unwrap(), 1);
    assert!(store.get(a.master_variant_id).unwrap().is_some());
    assert_eq!(store.collect_garbage(&alive).unwrap(), 0);
}

#[test]
fn preview_cache_layout_publish_and_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let previews = dir.path().join("previews");
    let mut cache = PreviewCache::open(&previews, dir.path().join("thumbs.db"), 1_000_000).unwrap();
    let k = key(7, CacheKind::L1Preview);
    let path = cache.path_for(&k).unwrap();
    assert_eq!(
        path,
        previews
            .join(&k[0..2])
            .join(&k[2..4])
            .join(format!("{k}.jpg"))
    );
    assert_eq!(cache.get(&k).unwrap(), None);
    assert!(!cache.contains(&k).unwrap());
    let published = cache
        .put(&k, Some(VariantId::new(3)), &jpeg(1000, 7))
        .unwrap();
    assert_eq!(published, path);
    assert_eq!(std::fs::read(&path).unwrap(), jpeg(1000, 7));
    assert!(cache.contains(&k).unwrap());
    assert_eq!(cache.get(&k).unwrap(), Some(path.clone()));
    assert!(cache.last_used_at(&k).unwrap().is_some());
    assert_eq!(cache.total_bytes().unwrap(), 1000);
    assert_eq!(cache.count().unwrap(), 1);
    // 一時ファイルが残っていない。
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec![format!("{k}.jpg")]);
    // 置き換え。
    cache.put(&k, None, &jpeg(500, 8)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), jpeg(500, 8));
    assert_eq!(cache.total_bytes().unwrap(), 500);
    // 削除。
    assert!(cache.remove(&k).unwrap());
    assert!(!cache.remove(&k).unwrap());
    assert!(!path.exists());
    // パスの外に出るキー・JPEG でないデータは拒否する。
    assert!(cache.path_for("../../etc/passwd").is_err());
    assert!(cache.put(&k, None, b"GIF89a").is_err());
    assert_eq!(cache.capacity_bytes(), 1_000_000);
    assert_eq!(cache.root(), previews.as_path());
}

#[test]
fn preview_cache_evicts_least_recently_used_over_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let mut cache = PreviewCache::open(
        dir.path().join("previews"),
        dir.path().join("thumbs.db"),
        3000,
    )
    .unwrap();
    let keys: Vec<String> = (0..4).map(|i| key(i, CacheKind::L1Preview)).collect();
    cache.put(&keys[0], None, &jpeg(1000, 0)).unwrap();
    cache.put(&keys[1], None, &jpeg(1000, 1)).unwrap();
    cache.put(&keys[2], None, &jpeg(1000, 2)).unwrap();
    // keys[0] を使ったので、最も古いのは keys[1]。
    assert!(cache.get(&keys[0]).unwrap().is_some());
    cache.put(&keys[3], None, &jpeg(1000, 3)).unwrap();
    assert!(cache.contains(&keys[0]).unwrap());
    assert!(!cache.contains(&keys[1]).unwrap());
    assert!(cache.contains(&keys[2]).unwrap());
    assert!(cache.contains(&keys[3]).unwrap());
    assert_eq!(cache.total_bytes().unwrap(), 3000);
    // 上限を下げると、古いものから削除する。
    let report = cache.set_capacity_bytes(1500).unwrap();
    assert_eq!(report.removed, 2);
    assert_eq!(report.freed_bytes, 2000);
    assert!(!cache.contains(&keys[2]).unwrap());
    assert!(!cache.contains(&keys[0]).unwrap());
    assert!(cache.contains(&keys[3]).unwrap());
    // 上限より大きいものを公開しても、公開したものは消さない。
    let big = key(10, CacheKind::L1Preview);
    let p = cache.put(&big, None, &jpeg(5000, 9)).unwrap();
    assert!(p.exists());
    assert!(!cache.contains(&keys[3]).unwrap());
    assert_eq!(cache.count().unwrap(), 1);
}

#[test]
fn preview_index_is_rebuilt_from_files() {
    let dir = tempfile::tempdir().unwrap();
    let previews = dir.path().join("previews");
    let thumbs = dir.path().join("thumbs.db");
    let keys: Vec<String> = (0..3).map(|i| key(i, CacheKind::L1Preview)).collect();
    {
        let mut cache = PreviewCache::open(&previews, &thumbs, 1_000_000).unwrap();
        for k in &keys {
            cache.put(k, None, &jpeg(100, 1)).unwrap();
        }
        // 生成の途中で終了した一時ファイル（最後の更新から十分に時間が経ったもの）。
        let p = cache.path_for(&keys[0]).unwrap();
        let tmp = p.with_file_name(format!(".{}.1.2.tmp", keys[0]));
        std::fs::write(&tmp, b"partial").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&tmp)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - crate::PREVIEW_TEMP_FILE_MIN_AGE * 2)
            .unwrap();
        // ファイルだけが消えた。
        std::fs::remove_file(cache.path_for(&keys[1]).unwrap()).unwrap();
    }
    // 索引（thumbs.db）を失った。
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", thumbs.display()));
    }
    let mut cache = PreviewCache::open(&previews, &thumbs, 1_000_000).unwrap();
    assert_eq!(cache.count().unwrap(), 0);
    let report = cache.reconcile().unwrap();
    assert_eq!(report.added, 2);
    assert_eq!(report.dropped, 0);
    assert_eq!(report.temp_files_removed, 1);
    assert_eq!(cache.count().unwrap(), 2);
    assert_eq!(cache.total_bytes().unwrap(), 200);
    // 索引にあってファイルがないもの。
    std::fs::remove_file(cache.path_for(&keys[2]).unwrap()).unwrap();
    let report = cache.reconcile().unwrap();
    assert_eq!(report.dropped, 1);
    // get でも、ファイルのない索引の行は消える。
    std::fs::remove_file(cache.path_for(&keys[0]).unwrap()).unwrap();
    assert_eq!(cache.get(&keys[0]).unwrap(), None);
    assert_eq!(cache.count().unwrap(), 0);
}

/// 削除できないプレビュー（Windows で他のプログラムが開いているものなど）があっても、
/// 公開（put）は失敗せず、回収はそれを飛ばして次の古いものを削除する。
///
/// 修正前は、最も古いファイルを削除できないと、以後のすべての put がエラーになっていた
/// （レビューで再現。ここではファイルの代わりに空でないフォルダを置いて削除を失敗させる）。
#[test]
fn eviction_skips_previews_that_cannot_be_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let mut cache = PreviewCache::open(
        dir.path().join("previews"),
        dir.path().join("thumbs.db"),
        250,
    )
    .unwrap();
    let keys: Vec<String> = (0..5).map(|i| key(i, CacheKind::L1Preview)).collect();
    cache.put(&keys[0], None, &jpeg(100, 0)).unwrap();
    cache.put(&keys[1], None, &jpeg(100, 1)).unwrap();
    // 最も古い keys[0] を削除できない状態にする。
    let stuck = cache.path_for(&keys[0]).unwrap();
    std::fs::remove_file(&stuck).unwrap();
    std::fs::create_dir_all(stuck.join("busy")).unwrap();

    // 上限 250 を超えるので回収が動く。keys[0] は飛ばし、次に古い keys[1] を削除する。
    let p = cache.put(&keys[2], None, &jpeg(100, 2)).unwrap();
    assert!(p.exists());
    assert!(!cache.contains(&keys[1]).unwrap());
    assert!(cache.contains(&keys[2]).unwrap());
    // 削除できなかったものは索引に残り、次の回収で再び試す。
    assert_eq!(cache.count().unwrap(), 2);
    let report = cache.evict_to(100, Some(&keys[2])).unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.removed, 0);
    assert_eq!(cache.count().unwrap(), 2);
    // 削除できるようになれば削除される。
    std::fs::remove_dir_all(&stuck).unwrap();
    std::fs::write(&stuck, jpeg(100, 0)).unwrap();
    let report = cache.evict_to(100, Some(&keys[2])).unwrap();
    assert_eq!(
        (report.removed, report.failed, report.freed_bytes),
        (1, 0, 100)
    );
    assert!(!stuck.exists());
    assert_eq!(cache.total_bytes().unwrap(), 100);
    // 続けて公開しても失敗しない。
    cache.put(&keys[3], None, &jpeg(100, 3)).unwrap();
    cache.put(&keys[4], None, &jpeg(100, 4)).unwrap();
    assert!(cache.total_bytes().unwrap() <= 250);
}

/// 突き合わせ（reconcile）は、書き込み中かもしれない新しい一時ファイルを削除しない。
#[test]
fn reconcile_keeps_recent_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut cache = PreviewCache::open(
        dir.path().join("previews"),
        dir.path().join("thumbs.db"),
        1_000_000,
    )
    .unwrap();
    let k = key(1, CacheKind::L1Preview);
    let p = cache.put(&k, None, &jpeg(100, 1)).unwrap();
    let fresh = p.with_file_name(format!(".{k}.7.8.tmp"));
    std::fs::write(&fresh, b"writing").unwrap();
    let report = cache.reconcile().unwrap();
    assert_eq!(report.temp_files_removed, 0);
    assert!(fresh.exists());
    // 最後の更新から PREVIEW_TEMP_FILE_MIN_AGE 以上経ったものは、途中で終了したものとして削除する。
    let old = std::time::SystemTime::now()
        - crate::PREVIEW_TEMP_FILE_MIN_AGE
        - std::time::Duration::from_secs(1);
    std::fs::File::options()
        .write(true)
        .open(&fresh)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let report = cache.reconcile().unwrap();
    assert_eq!(report.temp_files_removed, 1);
    assert!(!fresh.exists());
    assert!(p.exists());
}

/// キャッシュの付随情報（持ち主のカタログの世代）と、作り直し待ちの印。
#[test]
fn thumbs_meta_and_regeneration_marks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("thumbs.db");
    let mut store = ThumbStore::open(&path).unwrap();
    assert_eq!(store.meta("catalog_generation").unwrap(), None);
    store.set_meta("catalog_generation", "abc").unwrap();
    store.set_meta("catalog_generation", "def").unwrap();
    assert_eq!(
        store.meta("catalog_generation").unwrap().as_deref(),
        Some("def")
    );
    let (a, b) = (VariantId::new(1), VariantId::new(2));
    store
        .put(a, &key(1, CacheKind::L0Thumb), &jpeg(10, 1))
        .unwrap();
    store.mark_regen_pending(&[a, b]).unwrap();
    store.mark_regen_pending(&[a]).unwrap();
    let mut pending = store.regen_pending().unwrap();
    pending.sort();
    assert_eq!(pending, vec![a, b]);
    store.clear_regen_pending(a).unwrap();
    assert_eq!(store.regen_pending().unwrap(), vec![b]);
    // 存在しない variant の印は回収で外す。
    store.collect_garbage(&HashSet::from([a])).unwrap();
    assert!(store.regen_pending().unwrap().is_empty());
    store.mark_regen_pending(&[a]).unwrap();
    assert_eq!(store.clear().unwrap(), 1);
    assert_eq!(store.count().unwrap(), 0);
    assert!(store.regen_pending().unwrap().is_empty());
    store.close().unwrap();
    // 開き直しても残る。
    let store = ThumbStore::open(&path).unwrap();
    assert_eq!(
        store.meta("catalog_generation").unwrap().as_deref(),
        Some("def")
    );
}
