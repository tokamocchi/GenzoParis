//! スタック（LIB-12）・仮想コピー（LIB-13）と、版 9 への移行のテスト。

use std::time::Duration;

use genzo_model::{Rating, StackId, VariantId};

use super::{Fixture, rows};
use crate::{
    CATALOG_SCHEMA_VERSION, Catalog, CatalogError, Filter, STACKS_SCHEMA_VERSION, Sort,
    SortDirection, SortKey,
};

/// 撮影日時が 1 秒ずつ違う写真を `n` 枚（2024-05-01 12:00:00 から）。
fn burst(f: &mut Fixture, prefix: &str, start_n: u64, n: u64) -> Vec<VariantId> {
    (0..n)
        .map(|i| {
            let when = format!("2024:05:01 12:00:{:02}", start_n + i);
            f.photo(&format!("{prefix}{i}.ARW"), start_n + i, Some(&when))
                .master_variant_id
        })
        .collect()
}

fn members(cat: &Catalog, s: StackId) -> Vec<VariantId> {
    cat.stack(s).unwrap().members
}

fn exists(cat: &Catalog, s: StackId) -> bool {
    !matches!(cat.stack(s), Err(CatalogError::NotFound(_)))
}

#[test]
fn stacks_are_ordered_by_capture_time_with_the_chosen_top_first() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 4);
    // 渡した順によらず撮影日時の順。一番上は指定したもの。
    let s = f.cat.create_stack(&[p[3], p[1], p[2]], Some(p[2])).unwrap();
    let info = f.cat.stack(s).unwrap();
    assert_eq!(info.members, vec![p[2], p[1], p[3]]);
    assert_eq!(info.top_variant_id, p[2]);
    assert!(info.collapsed, "作ったスタックは折りたたむ");
    // 指定がなければ撮影日時の最も早いもの。
    f.cat.unstack(&[s]).unwrap();
    let s = f.cat.create_stack(&[p[3], p[1]], None).unwrap();
    assert_eq!(f.cat.stack(s).unwrap().top_variant_id, p[1]);
    // 写真は消えない。
    assert_eq!(f.rows("variant"), 4);
}

#[test]
fn grouping_photos_already_in_stacks_merges_the_stacks() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 5);
    let a = f.cat.create_stack(&[p[0], p[1]], None).unwrap();
    let b = f.cat.create_stack(&[p[3], p[4]], None).unwrap();
    // p[1]（a の写真）・p[2]・p[3]（b の写真）をまとめると、a・b の写真もすべて入る。
    let c = f.cat.create_stack(&[p[1], p[2], p[3]], None).unwrap();
    assert_eq!(members(&f.cat, c), p);
    assert!(!exists(&f.cat, a) && !exists(&f.cat, b));
    assert_eq!(f.cat.stack_count().unwrap(), 1);
    // 新しいスタックの ID は使い回さない（AUTOINCREMENT）。
    assert!(c.get() > b.get());
}

#[test]
fn invalid_stack_requests_change_nothing() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 3);
    assert!(matches!(
        f.cat.create_stack(&[p[0]], None),
        Err(CatalogError::InvalidInput(_))
    ));
    // 同じ写真を 2 回渡しても 1 枚。
    assert!(matches!(
        f.cat.create_stack(&[p[0], p[0]], None),
        Err(CatalogError::InvalidInput(_))
    ));
    // スタックに入っている写真 1 枚だけでは、まとめ直さない。
    let s = f.cat.create_stack(&[p[1], p[2]], Some(p[2])).unwrap();
    assert!(matches!(
        f.cat.create_stack(&[p[1]], None),
        Err(CatalogError::InvalidInput(_))
    ));
    assert_eq!(f.cat.stack(s).unwrap().top_variant_id, p[2]);
    f.cat.unstack(&[s]).unwrap();
    assert!(matches!(
        f.cat.create_stack(&[p[0], p[1]], Some(p[2])),
        Err(CatalogError::InvalidInput(_))
    ));
    assert!(matches!(
        f.cat.create_stack(&[p[0], VariantId::new(999)], None),
        Err(CatalogError::NotFound(_))
    ));
    assert!(matches!(
        f.cat.set_stack_top(p[0]),
        Err(CatalogError::InvalidInput(_))
    ));
    assert!(matches!(
        f.cat.add_to_stack(StackId::new(42), &[p[0]]),
        Err(CatalogError::NotFound(_))
    ));
    assert_eq!(f.rows("stack"), 0);
    assert_eq!(f.rows("stack_member"), 0);
}

#[test]
fn adding_moves_photos_from_other_stacks_and_removing_keeps_two_or_more() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 5);
    let a = f.cat.create_stack(&[p[0], p[1]], None).unwrap();
    let b = f.cat.create_stack(&[p[2], p[3]], None).unwrap();
    // b の p[3] と、どこにも入っていない p[4] を a に足す。b は 1 枚になるので消える。
    assert_eq!(f.cat.add_to_stack(a, &[p[4], p[3], p[0]]).unwrap(), 2);
    assert_eq!(members(&f.cat, a), vec![p[0], p[1], p[3], p[4]]);
    assert!(!exists(&f.cat, b));
    assert_eq!(f.rows("stack_member"), 4);
    // 一番上の写真を外すと、次の写真が一番上になる。
    assert_eq!(f.cat.remove_from_stack(&[p[0], p[2]]).unwrap(), 1);
    let info = f.cat.stack(a).unwrap();
    assert_eq!(info.top_variant_id, p[1]);
    assert_eq!(info.members, vec![p[1], p[3], p[4]]);
    // 1 枚になったら消える。
    f.cat.remove_from_stack(&[p[1], p[3]]).unwrap();
    assert!(!exists(&f.cat, a));
    assert_eq!(f.rows("stack_member"), 0);
}

#[test]
fn top_and_collapse_state_can_be_changed() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 4);
    let a = f.cat.create_stack(&p, None).unwrap();
    assert_eq!(members(&f.cat, a), p);
    // 一番上にする: ほかの写真の順は変えない。
    assert_eq!(f.cat.set_stack_top(p[2]).unwrap(), a);
    assert_eq!(members(&f.cat, a), vec![p[2], p[0], p[1], p[3]]);
    assert_eq!(f.cat.stack(a).unwrap().top_variant_id, p[2]);
    // 折りたたみ: 指定したもの・すべて。変わった数を返す。
    assert_eq!(f.cat.set_stacks_collapsed(Some(&[a]), false).unwrap(), 1);
    assert_eq!(f.cat.set_stacks_collapsed(Some(&[a]), false).unwrap(), 0);
    assert!(!f.cat.stack(a).unwrap().collapsed);
    assert_eq!(f.cat.set_stacks_collapsed(None, true).unwrap(), 1);
    assert!(f.cat.stack(a).unwrap().collapsed);
    // 解除しても写真は残る。存在しないスタックは数えない。
    assert_eq!(f.cat.unstack(&[a, StackId::new(999)]).unwrap(), 1);
    assert_eq!(f.rows("variant"), 4);
    assert_eq!(f.rows("stack_member"), 0);
}

/// スタックの所属は接続ごとに覚え、カタログが変わっていなければ読み直さない。この接続の書き込み（スタックの
/// 操作・写真の削除の連鎖・SQL）と、ほかの接続の書き込み（読み取り専用の接続から見た、本体の接続の書き込み）の
/// どちらでも読み直す。
#[test]
fn stack_layout_is_reused_until_the_catalog_changes() {
    use std::sync::Arc;

    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 4);
    let reader = Catalog::open_read_only(f.catalog_path()).unwrap();
    let empty = reader.stack_layout().unwrap();
    assert!(empty.is_empty());
    let a = f.cat.create_stack(&p[..3], None).unwrap();
    // ほかの接続の書き込み: 読み直す。
    let seen = reader.stack_layout().unwrap();
    assert_eq!(seen.member_count(), 3);
    assert!(
        Arc::ptr_eq(&seen, &reader.stack_layout().unwrap()),
        "変わっていなければ使い回す"
    );
    // この接続の書き込み。
    let first = f.cat.stack_layout().unwrap();
    assert!(Arc::ptr_eq(&first, &f.cat.stack_layout().unwrap()));
    f.cat.set_stacks_collapsed(Some(&[a]), false).unwrap();
    let expanded = f.cat.stack_layout().unwrap();
    assert!(!Arc::ptr_eq(&first, &expanded));
    let grouped = expanded.group(p.clone(), false);
    assert_eq!(
        (grouped.ids.len(), grouped.hidden),
        (4, 0),
        "展開した状態で読み直した"
    );
    f.cat.add_to_stack(a, &p[3..]).unwrap();
    assert_eq!(f.cat.stack_layout().unwrap().member_count(), 4);
    // SQL で直接変えても（外部キーの連鎖・トリガーを含む）読み直す。
    f.cat
        .conn
        .execute("DELETE FROM variant WHERE id = ?1", [p[3].get()])
        .unwrap();
    assert_eq!(f.cat.stack_layout().unwrap().member_count(), 3);
    f.cat
        .conn
        .execute("UPDATE stack SET collapsed = 1", [])
        .unwrap();
    assert_eq!(
        f.cat
            .stack_layout()
            .unwrap()
            .group(p[..3].to_vec(), false)
            .hidden,
        2
    );
    // 読み取り専用の接続も、本体の書き込みを見て読み直す。
    let after = reader.stack_layout().unwrap();
    assert!(!Arc::ptr_eq(&seen, &after));
    assert_eq!(after.member_count(), 3);
    assert_eq!(after.group(p[..3].to_vec(), false).hidden, 2);
    f.cat.unstack(&[a]).unwrap();
    assert!(reader.stack_layout().unwrap().is_empty());
    assert!(f.cat.stack_layout().unwrap().is_empty());
}

#[test]
fn deleting_photos_keeps_stacks_consistent() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 5);
    let a = f.cat.create_stack(&p[..3], None).unwrap();
    let b = f.cat.create_stack(&p[3..], None).unwrap();
    let asset_of = |cat: &Catalog, v: VariantId| cat.variant_summaries(&[v]).unwrap()[0].asset_id;
    // 一番上の写真の asset をカタログから除く（ON DELETE CASCADE とトリガー）。
    let top = asset_of(&f.cat, p[0]);
    f.cat.remove_assets(&[top]).unwrap();
    let info = f.cat.stack(a).unwrap();
    assert_eq!(info.top_variant_id, p[1]);
    assert_eq!(info.members, vec![p[1], p[2]]);
    // 2 枚のスタックの 1 枚を除くと、スタックは消える。
    let one = asset_of(&f.cat, p[4]);
    f.cat.remove_assets(&[one]).unwrap();
    assert!(!exists(&f.cat, b));
    assert_eq!(f.rows("stack_member"), 2);
    // 仮想コピーの削除でも同じ（仮想コピーは元の写真のスタックに入る）。
    let copy = f.cat.create_virtual_copy(p[3], None).unwrap();
    let c = f.cat.stacks_of_variants(&[copy]).unwrap()[0].id;
    assert_eq!(members(&f.cat, c), vec![p[3], copy]);
    f.cat.delete_virtual_copies(&[copy]).unwrap();
    assert!(!exists(&f.cat, c));
    let violations: i64 = f
        .cat
        .conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
}

#[test]
fn search_results_are_grouped_by_stack() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 6);
    let s = f.cat.create_stack(&[p[1], p[2], p[3]], Some(p[2])).unwrap();
    let all = Filter::default();
    let by_time = Sort::default();
    // 折りたたみ: 一番上の写真（p[2]）の位置に 1 枚だけ。
    let g = f.cat.search_stacked(&all, &by_time, false).unwrap();
    assert_eq!(g.ids, vec![p[0], p[2], p[4], p[5]]);
    assert_eq!(g.hidden, 2);
    // 展開（保存した状態・すべて展開の指定）: スタックの中の順で 1 か所に。
    let g = f.cat.search_stacked(&all, &by_time, true).unwrap();
    assert_eq!(g.ids, vec![p[0], p[2], p[1], p[3], p[4], p[5]]);
    f.cat.set_stacks_collapsed(Some(&[s]), false).unwrap();
    let desc = Sort::new(SortKey::CaptureTime, SortDirection::Descending);
    let g = f.cat.search_stacked(&all, &desc, false).unwrap();
    assert_eq!(g.ids, vec![p[5], p[4], p[2], p[1], p[3], p[0]]);
    assert_eq!(g.hidden, 0);
    // 一番上の写真が条件に合わないときは、一致した写真から代表を選ぶ（写真が消えない）。
    f.cat.set_stacks_collapsed(None, true).unwrap();
    f.cat
        .set_rating(&[p[1], p[3]], Rating::new(3).unwrap())
        .unwrap();
    let three = Filter {
        rating_min: Rating::new(3),
        ..Default::default()
    };
    let g = f.cat.search_stacked(&three, &by_time, false).unwrap();
    assert_eq!(g.ids, vec![p[1]]);
    assert_eq!(g.hidden, 1);
    // 平らな検索・件数はスタックに関係なく、一致したすべての写真。
    assert_eq!(f.cat.search(&three, &by_time).unwrap(), vec![p[1], p[3]]);
    assert_eq!(f.cat.count(&all).unwrap(), 6);
}

#[test]
fn summaries_carry_the_stack_badge() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 4);
    let s = f.cat.create_stack(&p[..3], Some(p[1])).unwrap();
    let sums = f.cat.variant_summaries(&p).unwrap();
    let badge = |i: usize| sums[i].stack.unwrap();
    assert_eq!(badge(1).stack_id, s);
    assert_eq!((badge(1).position, badge(1).count), (1, 3));
    assert_eq!((badge(0).position, badge(2).position), (2, 3));
    assert!(badge(0).collapsed);
    assert!(sums[3].stack.is_none());
}

#[test]
fn auto_stack_groups_runs_by_capture_time_gap() {
    let mut f = Fixture::new();
    // 12:00:00・01・02（1 秒おき）、12:00:10・11、12:00:30（ひとり）、撮影日時なし 2 枚。
    let a = burst(&mut f, "A", 0, 3);
    let b = burst(&mut f, "B", 10, 2);
    let c = burst(&mut f, "C", 30, 1);
    let n1 = f.photo("N1.ARW", 100, None).master_variant_id;
    let n2 = f.photo("N2.ARW", 101, None).master_variant_id;
    let all = Filter::default();
    // 見込み（何も変えない）。
    let r = f
        .cat
        .auto_stack(&all, Duration::from_secs(2), true)
        .unwrap();
    assert_eq!((r.stacks, r.photos, r.considered), (2, 5, 6));
    assert_eq!(r.skipped_without_time, 2);
    assert_eq!(f.rows("stack"), 0);
    // 間隔 0 秒では、同じ時刻の写真だけ（ここではない）。
    let r = f.cat.auto_stack(&all, Duration::ZERO, true).unwrap();
    assert_eq!(r.stacks, 0);
    // 作る: 一番上は撮影日時の最も早いもの、折りたたむ。
    let r = f
        .cat
        .auto_stack(&all, Duration::from_secs(2), false)
        .unwrap();
    assert_eq!((r.stacks, r.photos), (2, 5));
    let s = f
        .cat
        .stacks_of_variants(&[a[1], b[0], c[0], n1, n2])
        .unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].members, a);
    assert_eq!(s[1].members, b);
    assert!(s.iter().all(|x| x.collapsed));
    // もう一度: スタックに入っている写真は対象にしない。間隔を広げても、残りは 1 枚ずつ。
    let r = f
        .cat
        .auto_stack(&all, Duration::from_secs(60), false)
        .unwrap();
    assert_eq!(r.stacks, 0);
    assert_eq!(r.skipped_in_stacks, 5);
    // 上限を超える間隔は断る。
    assert!(matches!(
        f.cat.auto_stack(
            &all,
            crate::MAX_AUTO_STACK_GAP + Duration::from_millis(1),
            true
        ),
        Err(CatalogError::InvalidInput(_))
    ));
}

#[test]
fn auto_stack_respects_the_filter() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 4);
    f.cat.set_rating(&p[..2], Rating::new(4).unwrap()).unwrap();
    let r = f
        .cat
        .auto_stack(
            &Filter {
                rating_min: Rating::new(4),
                ..Default::default()
            },
            Duration::from_secs(5),
            false,
        )
        .unwrap();
    assert_eq!((r.stacks, r.photos, r.considered), (1, 2, 2));
    assert_eq!(f.cat.stacks_of_variants(&p).unwrap()[0].members, p[..2]);
}

#[test]
fn virtual_copies_join_the_stack_of_their_source() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 3);
    // スタックに入っていない写真の仮想コピー: 元の写真を一番上にした、展開したスタック。
    let c1 = f.cat.create_virtual_copy(p[0], None).unwrap();
    let s = f.cat.stacks_of_variants(&[p[0]]).unwrap()[0].clone();
    assert_eq!(s.members, vec![p[0], c1]);
    assert_eq!(s.top_variant_id, p[0]);
    assert!(!s.collapsed);
    // スタックに入っている写真の仮想コピーは、元の写真のすぐ後ろ。折りたたんでいれば展開する。
    let t = f.cat.create_stack(&[p[1], p[2]], None).unwrap();
    let c2 = f.cat.create_virtual_copy(p[1], Some("  白黒  ")).unwrap();
    let info = f.cat.stack(t).unwrap();
    assert_eq!(info.members, vec![p[1], c2, p[2]]);
    assert!(!info.collapsed);
    // 仮想コピーの仮想コピーも、元（c1）のすぐ後ろ。
    let c3 = f.cat.create_virtual_copy(c1, None).unwrap();
    assert_eq!(members(&f.cat, s.id), vec![p[0], c1, c3]);
    // 既定の名前は重ならない「コピー n」、指定した名前は前後の空白を除く。
    let names = |cat: &Catalog, ids: &[VariantId]| -> Vec<Option<String>> {
        cat.variant_summaries(ids)
            .unwrap()
            .into_iter()
            .map(|s| s.variant_name)
            .collect()
    };
    assert_eq!(
        names(&f.cat, &[c1, c2, c3]),
        vec![
            Some("コピー 1".to_owned()),
            Some("白黒".to_owned()),
            Some("コピー 2".to_owned())
        ]
    );
    f.cat.delete_virtual_copies(&[c1]).unwrap();
    let c4 = f.cat.create_virtual_copy(p[0], None).unwrap();
    assert_eq!(names(&f.cat, &[c4]), vec![Some("コピー 1".to_owned())]);
}

#[test]
fn virtual_copies_can_be_renamed_and_made_master() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 1);
    let master = p[0];
    let copy = f.cat.create_virtual_copy(master, Some("夕景")).unwrap();
    f.cat.set_rating(&[copy], Rating::new(5).unwrap()).unwrap();
    // 名前の変更: マスター・空・長すぎる・制御文字は断る。
    f.cat.rename_virtual_copy(copy, "夕景 2").unwrap();
    for bad in ["", "  ", "a\nb"] {
        assert!(matches!(
            f.cat.rename_virtual_copy(copy, bad),
            Err(CatalogError::InvalidInput(_))
        ));
    }
    let long = "あ".repeat(crate::MAX_VARIANT_NAME_CHARS + 1);
    assert!(matches!(
        f.cat.rename_virtual_copy(copy, &long),
        Err(CatalogError::InvalidInput(_))
    ));
    assert!(matches!(
        f.cat.create_virtual_copy(master, Some(&long)),
        Err(CatalogError::InvalidInput(_))
    ));
    assert!(matches!(
        f.cat.rename_virtual_copy(master, "x"),
        Err(CatalogError::InvalidInput(_))
    ));
    // マスターにする: 前のマスターは仮想コピーになり、名前を受け継ぐ。評価などは variant のまま。
    assert_eq!(f.cat.set_as_master(copy).unwrap(), master);
    let sums = f.cat.variant_summaries(&[master, copy]).unwrap();
    assert!(!sums[0].is_master && sums[1].is_master);
    assert_eq!(sums[0].variant_name.as_deref(), Some("夕景 2"));
    assert_eq!(sums[1].variant_name, None);
    assert_eq!(sums[1].rating, Rating::new(5).unwrap());
    // マスターだけの検索・仮想コピーの削除はマスターに従う。
    let masters = f
        .cat
        .search(
            &Filter {
                masters_only: true,
                ..Default::default()
            },
            &Sort::default(),
        )
        .unwrap();
    assert_eq!(masters, vec![copy]);
    assert!(matches!(
        f.cat.delete_virtual_copies(&[copy]),
        Err(CatalogError::CannotDeleteMaster(_))
    ));
    // もう一度入れ替えると元に戻る。すでにマスターなら何もしない。
    assert_eq!(f.cat.set_as_master(master).unwrap(), copy);
    assert_eq!(f.cat.set_as_master(master).unwrap(), master);
    let sums = f.cat.variant_summaries(&[master, copy]).unwrap();
    assert!(sums[0].is_master && !sums[1].is_master);
    assert_eq!(sums[1].variant_name.as_deref(), Some("夕景 2"));
    assert!(matches!(
        f.cat.set_as_master(VariantId::new(999)),
        Err(CatalogError::NotFound(_))
    ));
}

#[test]
fn photo_stacks_view_lists_members_in_order() {
    let mut f = Fixture::new();
    let p = burst(&mut f, "A", 0, 3);
    let s = f.cat.create_stack(&p, Some(p[2])).unwrap();
    let mut stmt = f
        .cat
        .conn
        .prepare(
            "SELECT variant_id, stack_id, position, is_top, stack_size FROM photo_stacks_v1
             ORDER BY position",
        )
        .unwrap();
    let got: Vec<(i64, i64, i64, bool, i64)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        got,
        vec![
            (p[2].get(), s.get(), 1, true, 3),
            (p[0].get(), s.get(), 2, false, 3),
            (p[1].get(), s.get(), 3, false, 3),
        ]
    );
}

#[test]
fn version_8_catalog_migrates_stacks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (p, good, single) = {
        let mut cat = crate::testing::open_at_version(&path, STACKS_SCHEMA_VERSION - 1).unwrap();
        let vol = cat.ensure_volume("v", None, Some("/media")).unwrap();
        let folder = cat.ensure_folder(vol, "2024").unwrap();
        let p: Vec<VariantId> = (0..4)
            .map(|i| {
                cat.register_file(&super::photo_req(
                    folder,
                    &format!("A{i}.ARW"),
                    i,
                    Some(&format!("2024:05:01 12:00:0{i}")),
                ))
                .unwrap()
                .master_variant_id
            })
            .collect();
        // 版 8 の表に、外部のツールで作られた行を入れる: 一番上の写真がメンバーでなく、位置が飛んでいる
        // スタックと、1 枚だけのスタック。
        cat.conn
            .execute_batch(&format!(
                "INSERT INTO stack(id, top_variant_id) VALUES (5, NULL), (7, {top});
                 INSERT INTO stack_member(stack_id, variant_id, position)
                 VALUES (5, {b}, 10), (5, {a}, 20), (5, {c}, 20), (7, {d}, 0);",
                top = p[3].get(),
                a = p[0].get(),
                b = p[1].get(),
                c = p[2].get(),
                d = p[3].get(),
            ))
            .unwrap();
        cat.close().unwrap();
        (p, StackId::new(5), StackId::new(7))
    };
    let mut cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(STACKS_SCHEMA_VERSION - 1)
    );
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    let sql: String = cat
        .conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'stack'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        sql.contains("AUTOINCREMENT") && sql.contains("collapsed"),
        "{sql}"
    );
    // 位置は (position, variant_id) の順で 0 から付け直し、一番上は最初の写真。既定は折りたたむ。
    let info = cat.stack(good).unwrap();
    assert_eq!(info.members, vec![p[1], p[0], p[2]]);
    assert_eq!(info.top_variant_id, p[1]);
    assert!(info.collapsed);
    let positions: Vec<i64> = {
        let mut stmt = cat
            .conn
            .prepare("SELECT position FROM stack_member WHERE stack_id = 5 ORDER BY position")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert_eq!(positions, vec![0, 1, 2]);
    // 1 枚だけのスタックは消える。
    assert!(matches!(cat.stack(single), Err(CatalogError::NotFound(_))));
    // 移行の後もトリガーが働き、ID は使い回さない。
    cat.remove_from_stack(&[p[1]]).unwrap();
    assert_eq!(cat.stack(good).unwrap().top_variant_id, p[0]);
    let s = cat.create_stack(&[p[2], p[3]], None).unwrap();
    assert!(s.get() > single.get(), "{s}");
    assert_eq!(rows(&cat, "stack"), 1);
    cat.close().unwrap();
}
