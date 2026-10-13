//! 説明のメタデータ（LIB-15。スキーマの版 6 の `asset_iptc`）のテスト: 読み書き・一括編集・まとめ・
//! テキスト検索・公開ビュー・版 5 からの移行。

use genzo_model::{IptcMetadata, MAX_METADATA_CHARS, MetadataField as F, VariantId};

use super::{Fixture, rows};
use crate::{
    CATALOG_SCHEMA_VERSION, Catalog, CatalogError, Filter, IPTC_SCHEMA_VERSION, Sort,
    SortDirection, SortKey,
};

fn search(cat: &Catalog, text: &str) -> Vec<VariantId> {
    cat.search(
        &Filter {
            text: Some(text.to_owned()),
            ..Default::default()
        },
        &Sort::new(SortKey::ImportOrder, SortDirection::Ascending),
    )
    .unwrap()
}

#[test]
fn metadata_is_stored_per_asset_and_rows_exist_only_with_values() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    assert_eq!(
        f.cat.asset_metadata(a.asset_id).unwrap(),
        IptcMetadata::default()
    );
    assert_eq!(f.rows("asset_iptc"), 0);

    let n = f
        .cat
        .set_asset_metadata(
            &[a.asset_id, b.asset_id],
            &[
                (F::Title, Some(" 清水寺 ")),
                (F::Creator, Some("山田 太郎")),
                (F::Copyright, Some("© 2024 山田 太郎")),
                (F::CopyrightStatus, Some("copyrighted")),
                (F::Caption, Some("朝の\r\n清水の舞台")),
            ],
        )
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(f.rows("asset_iptc"), 2);
    let m = f.cat.asset_metadata(a.asset_id).unwrap();
    assert_eq!(m.title.as_deref(), Some("清水寺"), "前後の空白は除く");
    assert_eq!(m.creator.as_deref(), Some("山田 太郎"));
    assert_eq!(m.copyright_status.as_deref(), Some("copyrighted"));
    assert_eq!(m.caption.as_deref(), Some("朝の\n清水の舞台"), "改行は \\n");
    // キャプションは asset.caption（MVP と同じ列）。
    assert_eq!(
        f.cat.asset(a.asset_id).unwrap().caption.as_deref(),
        Some("朝の\n清水の舞台")
    );

    // 並べていない項目は変えない。空・None は削除。
    f.cat
        .set_asset_metadata(
            &[b.asset_id],
            &[(F::Title, Some("  ")), (F::City, Some("京都市"))],
        )
        .unwrap();
    let mb = f.cat.asset_metadata(b.asset_id).unwrap();
    assert_eq!(mb.title, None);
    assert_eq!(mb.city.as_deref(), Some("京都市"));
    assert_eq!(mb.creator.as_deref(), Some("山田 太郎"));

    // すべての値を消したら行も消す（キャプションは asset の列なので関係しない）。
    f.cat
        .set_asset_metadata(
            &[a.asset_id],
            &[
                (F::Title, None),
                (F::Creator, None),
                (F::Copyright, None),
                (F::CopyrightStatus, Some("unknown")),
            ],
        )
        .unwrap();
    assert_eq!(f.rows("asset_iptc"), 1);
    assert_eq!(
        f.cat.asset_metadata(a.asset_id).unwrap().caption.as_deref(),
        Some("朝の\n清水の舞台")
    );

    // 仮想コピーも同じ asset の値。asset を消せば行も消える（ON DELETE CASCADE）。
    let copy = f
        .cat
        .create_virtual_copy(b.master_variant_id, Some("白黒"))
        .unwrap();
    assert_eq!(f.cat.asset_of_variant(copy).unwrap(), b.asset_id);
    f.cat.remove_assets(&[b.asset_id]).unwrap();
    assert_eq!(f.rows("asset_iptc"), 0);
}

#[test]
fn invalid_values_change_nothing() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let long = "あ".repeat(MAX_METADATA_CHARS + 1);
    for bad in [
        vec![(F::Title, Some("ok")), (F::Caption, Some(long.as_str()))],
        vec![(F::Title, Some("ok")), (F::CopyrightStatus, Some("yes"))],
    ] {
        let err = f.cat.set_asset_metadata(&[a.asset_id], &bad).unwrap_err();
        assert!(matches!(err, CatalogError::InvalidInput(_)), "{err}");
    }
    assert_eq!(
        f.cat.asset_metadata(a.asset_id).unwrap(),
        IptcMetadata::default()
    );
    // キャプションだけの変更（MVP の set_caption）も同じ上限で確かめる。
    assert!(matches!(
        f.cat.set_caption(a.asset_id, Some(&long)),
        Err(CatalogError::InvalidInput(_))
    ));
    // 存在しない asset は飛ばし、数えない。
    let n = f
        .cat
        .set_asset_metadata(
            &[a.asset_id, genzo_model::AssetId::new(9_999)],
            &[(F::Title, Some("x"))],
        )
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(f.rows("asset_iptc"), 1);
}

#[test]
fn summary_reports_common_values_and_mixed_fields() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    let c = f.photo("C.ARW", 3, None);
    f.cat
        .set_asset_metadata(
            &[a.asset_id, b.asset_id, c.asset_id],
            &[(F::Creator, Some("山田")), (F::Copyright, Some("©"))],
        )
        .unwrap();
    f.cat
        .set_asset_metadata(
            &[a.asset_id],
            &[(F::Title, Some("A")), (F::Copyright, Some("© A"))],
        )
        .unwrap();
    f.cat
        .set_asset_metadata(&[b.asset_id], &[(F::Title, Some("B"))])
        .unwrap();
    let s = f
        .cat
        .metadata_summary(&[a.asset_id, b.asset_id, c.asset_id])
        .unwrap();
    assert_eq!(s.assets, 3);
    assert_eq!(s.fields.len(), F::ALL.len());
    let get = |field: F| s.fields.iter().find(|x| x.field == field).unwrap().clone();
    assert_eq!(
        (get(F::Creator).value.as_deref(), get(F::Creator).mixed),
        (Some("山田"), false)
    );
    // 値が異なる・値のあるものとないものが混ざる → mixed。
    assert!(get(F::Copyright).mixed);
    assert_eq!(get(F::Copyright).value, None);
    assert!(get(F::Title).mixed, "C だけタイトルなし");
    // どれも値なし → 値なし・異ならない。
    assert_eq!((get(F::City).value, get(F::City).mixed), (None, false));
    // 1 件だけなら、その値。
    let one = f.cat.metadata_summary(&[b.asset_id]).unwrap();
    let t = one.fields.iter().find(|x| x.field == F::Title).unwrap();
    assert_eq!((t.value.as_deref(), t.mixed), (Some("B"), false));
    // 空なら 0 件。
    assert_eq!(f.cat.metadata_summary(&[]).unwrap().assets, 0);
}

#[test]
fn searchable_fields_are_in_the_text_index() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    f.cat
        .set_asset_metadata(
            &[a.asset_id],
            &[
                (F::Title, Some("清水寺の朝焼け")),
                (F::Creator, Some("ＹＡＭＡＤＡ Taro")),
                (F::Sublocation, Some("清水の舞台")),
                (F::Country, Some("日本")),
                // 連絡先・URL は検索の対象にしない。
                (F::CreatorEmail, Some("taro@example.com")),
            ],
        )
        .unwrap();
    assert_eq!(search(&f.cat, "朝焼け"), vec![a.master_variant_id]);
    assert_eq!(
        search(&f.cat, "yamada"),
        vec![a.master_variant_id],
        "全角・大文字を区別しない"
    );
    assert_eq!(search(&f.cat, "舞台"), vec![a.master_variant_id]);
    assert_eq!(
        search(&f.cat, "日本"),
        vec![a.master_variant_id],
        "2 文字は LIKE"
    );
    assert!(search(&f.cat, "example").is_empty());
    // ファイル名・キャプションと組み合わせ（AND）。
    f.cat.set_caption(b.asset_id, Some("清水寺の夕方")).unwrap();
    assert_eq!(
        search(&f.cat, "清水寺"),
        vec![a.master_variant_id, b.master_variant_id]
    );
    assert_eq!(search(&f.cat, "清水寺 A.ARW"), vec![a.master_variant_id]);
    // 値を消すと検索から外れる（索引の作り直しでも同じ）。
    f.cat
        .set_asset_metadata(&[a.asset_id], &[(F::Title, None)])
        .unwrap();
    assert!(search(&f.cat, "朝焼け").is_empty());
    f.cat.rebuild_text_index().unwrap();
    assert!(search(&f.cat, "朝焼け").is_empty());
    assert_eq!(search(&f.cat, "yamada"), vec![a.master_variant_id]);
}

#[test]
fn public_view_has_title_creator_and_copyright() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    f.cat
        .set_asset_metadata(
            &[a.asset_id],
            &[
                (F::Title, Some("清水寺")),
                (F::Creator, Some("山田")),
                (F::Copyright, Some("© 山田")),
            ],
        )
        .unwrap();
    let read = |v: VariantId| -> (Option<String>, Option<String>, Option<String>, String) {
        f.cat
            .conn
            .query_row(
                "SELECT title, creator, copyright, file_name FROM photos_v1 WHERE variant_id = ?1",
                [v.get()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
    };
    assert_eq!(
        read(a.master_variant_id),
        (
            Some("清水寺".to_owned()),
            Some("山田".to_owned()),
            Some("© 山田".to_owned()),
            "A.ARW".to_owned()
        )
    );
    assert_eq!(
        read(b.master_variant_id),
        (None, None, None, "B.ARW".to_owned())
    );
    assert_eq!(
        rows(&f.cat, "photos_v1"),
        2,
        "行は variant ごと（asset_iptc で増えない）"
    );
}

#[test]
fn version_5_catalog_migrates_to_iptc() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (asset, variant) = {
        let mut cat = crate::testing::open_at_version(&path, IPTC_SCHEMA_VERSION - 1).unwrap();
        let vol = cat.ensure_volume("v", None, Some("/media")).unwrap();
        let folder = cat.ensure_folder(vol, "2024").unwrap();
        let o = cat
            .register_file(&super::photo_req(folder, "A.ARW", 1, None))
            .unwrap();
        cat.set_caption(o.asset_id, Some("京都の夕焼け")).unwrap();
        // 版 5 の photos_v1 には title の列がない。
        assert!(cat.conn.prepare("SELECT title FROM photos_v1").is_err());
        cat.close().unwrap();
        (o.asset_id, o.master_variant_id)
    };
    let mut cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(IPTC_SCHEMA_VERSION - 1)
    );
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    // 既存の値はそのまま（キャプションは asset の列のまま、ほかは値なし）。
    let m = cat.asset_metadata(asset).unwrap();
    assert_eq!(m.caption.as_deref(), Some("京都の夕焼け"));
    assert_eq!(m.title, None);
    assert_eq!(search(&cat, "夕焼け"), vec![variant]);
    // 移行の後は書ける。公開ビューは新しい列を持ち、版 5 の列も同じ値。
    cat.set_asset_metadata(&[asset], &[(F::Title, Some("金閣"))])
        .unwrap();
    let (title, caption, folder): (String, String, String) = cat
        .conn
        .query_row(
            "SELECT title, caption, folder_path FROM photos_v1 WHERE variant_id = ?1",
            [variant.get()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (title.as_str(), caption.as_str(), folder.as_str()),
        ("金閣", "京都の夕焼け", "2024")
    );
    assert_eq!(search(&cat, "金閣"), vec![variant]);
    cat.close().unwrap();
}
