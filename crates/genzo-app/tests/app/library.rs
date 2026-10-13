//! (b) 取り込み → 検索 → `genzo://` のサムネイル・標準プレビュー（名前に空白と日本語を含む写真）。

use genzo_api::{CatalogChange, DevelopSettings, Event, JobKind, JobState, Response, VariantId};
use genzo_app::protocol::{self, Reply};
use genzo_app::state::AppState;
use serde_json::json;

use crate::support::{Env, Recorder, all_items, by_name, import, ok, write_image};

/// JPEG を読んで寸法を返す。
fn jpeg_size(r: &Reply) -> (u32, u32) {
    assert_eq!(r.content_type, "image/jpeg");
    assert!(r.body.starts_with(&[0xFF, 0xD8, 0xFF]), "JPEG の先頭");
    let img = image::load_from_memory_with_format(&r.body, image::ImageFormat::Jpeg)
        .expect("JPEG として読める");
    (img.width(), img.height())
}

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// 取り込み（ImportFolder → WaitJob）の後、検索した写真のサムネイルが `genzo://thumb` で 200 の JPEG。
/// 標準プレビューは、まだなければ 404 で作る処理を予約し、ThumbnailUpdated のイベントの後に 200。
/// サムネイルは `rev` が今の内容のものなら長くキャッシュさせ、古い `rev` では長くキャッシュさせない。
/// 標準プレビューは `rev` によらず長くキャッシュさせない（[`previews_are_not_cached_long_when_settings_go_back`]）。
pub fn import_thumbnails_and_previews_over_the_uri_scheme() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("旅行 2026");
    let sources = [
        ("夏の 海.jpg", photos.join("夏の 海.jpg"), (96, 64)),
        ("犬 と 猫.png", photos.join("犬 と 猫.png"), (80, 60)),
        (
            "金閣 寺.jpg",
            photos.join("京都 の 寺").join("金閣 寺.jpg"),
            (64, 96),
        ),
    ];
    for (i, (_, path, size)) in sources.iter().enumerate() {
        write_image(path, *size, i as u8);
    }
    state
        .open_catalog(&env.catalog("ライブラリ.db"), true)
        .unwrap();
    let mark = env.events.mark();
    let report = import(&state, &photos);
    assert_eq!(report.added, 3, "{report:?}");
    assert!(report.errors.is_empty(), "{report:?}");
    assert!(report.not_registered.is_empty(), "{report:?}");
    assert_eq!(report.thumbnails, 3, "{report:?}");
    // (f) 取り込みのイベントが送り先に届く。
    env.events.wait_for(mark, "取り込みの終わり", |e| {
        matches!(
            e,
            Event::JobFinished {
                kind: JobKind::Import,
                state: JobState::Succeeded,
                ..
            }
        )
    });
    env.events.wait_for(mark, "取り込みのカタログの変更", |e| {
        matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::Imported,
                ..
            }
        )
    });

    let items = all_items(&state);
    assert_eq!(items.len(), 3);
    for (name, path, size) in &sources {
        let item = by_name(&items, name);
        let id = item.variant_id.get();
        let rev = item.thumb_rev.clone().expect("取り込みで L0 を作る");
        let r = protocol::reply(&state, &format!("/thumb/{id}"), Some(&rev));
        assert_eq!(r.status, 200, "{name}");
        assert_eq!(
            r.cache_control, IMMUTABLE,
            "今の rev なら長くキャッシュさせる"
        );
        assert_eq!(
            jpeg_size(&r),
            *size,
            "{name}: 長辺 320 より小さいので縮小しない"
        );
        let r = protocol::reply(&state, &format!("/thumb/{id}"), None);
        assert_eq!((r.status, r.cache_control), (200, "no-cache"), "{name}");
        let r = protocol::reply(&state, &format!("/thumb/{id}"), Some("0123abcd"));
        assert_eq!(
            (r.status, r.cache_control),
            (200, "no-cache"),
            "{name}: 違う rev の URI に今の内容を長くキャッシュさせない"
        );
        // 元ファイルの場所（空白と日本語の名前のまま）。
        match ok(&state, json!({"type": "variant_details", "variant_id": id})) {
            Response::Details(d) => assert!(
                d.files
                    .iter()
                    .any(|f| f.path.as_deref() == Some(path.as_path())),
                "{d:?}"
            ),
            other => panic!("{other:?}"),
        }
    }
    match ok(&state, json!({"type": "folders"})) {
        Response::Folders { folders } => assert!(
            folders
                .iter()
                .any(|f| f.path.as_deref() == Some(photos.join("京都 の 寺").as_path())),
            "{folders:?}"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(protocol::reply(&state, "/thumb/999999", None).status, 404);
    assert_eq!(protocol::reply(&state, "/preview/999999", None).status, 404);
    assert_eq!(protocol::reply(&state, "/other/1", None).status, 400);
    assert_eq!(protocol::reply(&state, "/thumb/abc", None).status, 400);

    // 標準プレビュー: まだないので 404（作る処理を予約する）→ ThumbnailUpdated の後に 200。
    let item = by_name(&items, "夏の 海.jpg");
    let id = item.variant_id;
    let old_rev = item.thumb_rev.clone().unwrap();
    let mark = env.events.mark();
    let uri = format!("/preview/{}", id.get());
    let r = protocol::reply(&state, &uri, Some(&old_rev));
    assert_eq!((r.status, r.cache_control), (404, "no-store"));
    let new_rev = match env.events.wait_for(
        mark,
        "標準プレビューの作成",
        |e| matches!(e, Event::ThumbnailUpdated { variant_id, .. } if *variant_id == id),
    ) {
        Event::ThumbnailUpdated { rev, .. } => rev,
        _ => unreachable!(),
    };
    assert_ne!(new_rev, old_rev, "L0 も現像結果に置き換わる");
    let r = protocol::reply(&state, &uri, Some(&new_rev));
    assert_eq!(
        (r.status, r.cache_control),
        (200, "no-cache"),
        "標準プレビューは今の rev でも長くキャッシュさせない"
    );
    assert_eq!(
        jpeg_size(&r),
        (96, 64),
        "長辺 2560 より小さいので縮小しない"
    );
    let r = protocol::reply(&state, &uri, Some(&old_rev));
    assert_eq!(
        (r.status, r.cache_control),
        (200, "no-cache"),
        "古い rev の URI に新しい内容を長くキャッシュさせない"
    );
    let r = protocol::reply(&state, &format!("/thumb/{}", id.get()), Some(&new_rev));
    assert_eq!((r.status, r.cache_control), (200, IMMUTABLE));
    assert_eq!(jpeg_size(&r), (96, 64));
    let r = protocol::reply(&state, &format!("/thumb/{}", id.get()), Some(&old_rev));
    assert_eq!((r.status, r.cache_control), (200, "no-cache"));
    state.close_catalog(false).unwrap();
}

/// 現像設定を保存して、L0・L1 を作り直すのを待つ。作り直した L0 の rev を返す。
fn save_and_wait(
    state: &AppState,
    events: &Recorder,
    id: VariantId,
    settings: &DevelopSettings,
) -> String {
    let mark = events.mark();
    ok(
        state,
        json!({"type": "set_settings", "settings": settings, "dragging": false}),
    );
    match events.wait_for(
        mark,
        "L0・L1 の作り直し",
        |e| matches!(e, Event::ThumbnailUpdated { variant_id, .. } if *variant_id == id),
    ) {
        Event::ThumbnailUpdated { rev, .. } => rev,
        _ => unreachable!(),
    }
}

/// 標準プレビュー（L1）は、URI の `rev`（L0 のもの）によらず長くキャッシュさせない。
///
/// 現像設定を A → B → A と戻すと、A の L1 はキャッシュ（LRU）に残っていてすぐ返るが、L0 は作り直しの
/// ジョブが保存するまで B のまま。この間に `rev` = B の URI へ A の L1 を長くキャッシュさせると、後で設定を
/// B にしたとき、ルーペ（L1 の URI の `rev` は L0 のもの）が A の画像を表示し続ける。作り直しが速く終われば
/// この間に要求できないこともあるが、どの応答も長くキャッシュさせないことを確かめる。
pub fn previews_are_not_cached_long_when_settings_go_back() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("戻す 設定");
    // 作り直しに少し時間がかかるよう、ほかのテストより大きくする（食い違いの間に要求できるように。仮置き:
    // 600×400。この環境（Linux・debug ビルド）では、続けて 3 回とも間に要求できた）。
    write_image(&photos.join("大きな 写真.jpg"), (600, 400), 7);
    state.open_catalog(&env.catalog("戻す.db"), true).unwrap();
    import(&state, &photos);
    let id = all_items(&state)[0].variant_id;
    let uri = format!("/preview/{}", id.get());
    let core = state.core().unwrap();
    let mut a = match ok(&state, json!({"type": "open_develop", "variant_id": id})) {
        Response::Develop(d) => d.settings,
        other => panic!("{other:?}"),
    };
    a.exposure_ev = 0.5;
    let mut b = a.clone();
    b.exposure_ev = -1.5;

    // (rev, 応答) の記録。
    let mut replies: Vec<(String, Reply)> = Vec::new();
    let rev_a = save_and_wait(&state, &env.events, id, &a);
    let body_a = protocol::reply(&state, &uri, Some(&rev_a));
    assert_eq!(body_a.status, 200, "作り直しで L1 も作る");
    replies.push((rev_a.clone(), body_a.clone()));
    let rev_b = save_and_wait(&state, &env.events, id, &b);
    assert_ne!(rev_a, rev_b);
    let body_b = protocol::reply(&state, &uri, Some(&rev_b));
    assert_eq!(body_b.status, 200);
    assert_ne!(body_a.body, body_b.body, "露光量の違う L1");
    replies.push((rev_b.clone(), body_b.clone()));

    // A に戻す: 保存した直後（L0 を作り直す前）に、rev = B の URI で要求する。
    let mark = env.events.mark();
    ok(
        &state,
        json!({"type": "set_settings", "settings": &a, "dragging": false}),
    );
    env.events.wait_for(mark, "現像設定の保存", |e| {
        matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::Develop,
                ..
            }
        )
    });
    let r = protocol::reply(&state, &uri, Some(&rev_b));
    let l0_rev = core.thumbnail(id).unwrap().map(|t| t.rev);
    assert_eq!(r.status, 200, "A の L1 はキャッシュに残っている");
    assert_eq!(r.body, body_a.body, "今の設定（A）の L1 を返す");
    let mismatch = l0_rev.as_deref() == Some(rev_b.as_str());
    println!(
        "    L0 がまだ B のまま（rev = B の URI に A の L1 を返す間）に要求できたか: {mismatch}"
    );
    if mismatch {
        assert_ne!(
            r.cache_control, IMMUTABLE,
            "rev = B の URI に A の L1 を長くキャッシュさせない"
        );
    }
    replies.push((rev_b.clone(), r));
    // A の作り直しが終わると、L0 の rev も A に戻る（同じ設定なので同じキー）。
    env.events.wait_for(
        mark,
        "A の L0 の作り直し",
        |e| matches!(e, Event::ThumbnailUpdated { variant_id, rev } if *variant_id == id && *rev == rev_a),
    );
    let r = protocol::reply(&state, &uri, Some(&rev_a));
    assert_eq!(r.body, body_a.body);
    replies.push((rev_a.clone(), r));

    for (rev, r) in &replies {
        assert_eq!(
            (r.status, r.cache_control),
            (200, "no-cache"),
            "rev {rev}: 標準プレビューは長くキャッシュさせない"
        );
    }
    // サムネイルは、今の rev なら長くキャッシュさせる（L0 と rev は同時に読む）。
    let r = protocol::reply(&state, &format!("/thumb/{}", id.get()), Some(&rev_a));
    assert_eq!((r.status, r.cache_control), (200, IMMUTABLE));
    drop(core);
    ok(&state, json!({"type": "close_develop"}));
    state.close_catalog(false).unwrap();
}

/// (b) キーワード・コレクション・スマートコレクション（LIB-09〜11）の JSON のコマンドが、アプリの
/// `core_execute` の経路（`execute_command`）で通り、カタログの変更のイベントが送り先に届く。
pub fn keywords_and_collections_through_the_app() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("整理");
    write_image(&photos.join("A.jpg"), (48, 32), 1);
    write_image(&photos.join("B.jpg"), (48, 32), 2);
    state.open_catalog(&env.catalog("整理.db"), true).unwrap();
    import(&state, &photos);
    let items = all_items(&state);
    let a = by_name(&items, "A.jpg").variant_id.get();
    let b = by_name(&items, "B.jpg").variant_id.get();
    let mark = env.events.mark();
    let r = ok(
        &state,
        json!({"type": "add_keywords", "variant_ids": [a, b], "paths": [["場所", "京都"]]}),
    );
    assert!(matches!(r, Response::Updated { count: 2 }), "{r:?}");
    env.events.wait_for(mark, "キーワードの変更", |e| {
        matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::Keywords,
                ..
            }
        )
    });
    let Response::Keywords { keywords } = ok(&state, json!({"type": "keyword_tree"})) else {
        panic!("キーワードの一覧ではない");
    };
    let place = keywords
        .iter()
        .find(|k| k.name == "場所")
        .unwrap()
        .keyword_id;
    let Response::CollectionCreated { collection_id } = ok(
        &state,
        json!({"type": "create_collection", "name": "お気に入り", "kind": "manual", "variant_ids": [a]}),
    ) else {
        panic!("コレクションを作った応答ではない");
    };
    let rule = json!({"version": 1, "match": "all", "rules": [
        {"type": "keyword", "op": "has", "keyword_id": place},
        {"type": "collection", "op": "not_in", "collection_id": collection_id}
    ]});
    let Response::CollectionCreated {
        collection_id: smart,
    } = ok(
        &state,
        json!({"type": "create_collection", "name": "まだ選んでいない京都", "kind": "smart", "rule": rule}),
    )
    else {
        panic!("スマートコレクションを作った応答ではない");
    };
    // スマートコレクションで検索すると、B だけ（A はコレクションに入っている）。
    let Response::Search(s) = ok(
        &state,
        json!({"type": "search", "filter": {"collection_id": smart}}),
    ) else {
        panic!("検索の応答ではない");
    };
    assert_eq!(s.count, 1);
    let Response::Count { count } = ok(
        &state,
        json!({"type": "count", "filter": {"keywords": [place]}}),
    ) else {
        panic!("数の応答ではない");
    };
    assert_eq!(count, 2);
    let Response::Collections { collections } =
        ok(&state, json!({"type": "collections", "with_counts": true}))
    else {
        panic!("コレクションの一覧ではない");
    };
    assert_eq!(collections.len(), 2);
    assert!(
        collections.iter().all(|c| c.count == Some(1)),
        "{collections:?}"
    );
    // 名前の重なりは conflict。
    let err = crate::support::exec(
        &state,
        json!({"type": "create_collection", "name": "ＯＫＩＮＩＩＲＩ", "kind": "set"}),
    );
    assert!(err.is_ok(), "別の名前なら作れる");
    crate::support::assert_kind(
        crate::support::exec(
            &state,
            json!({"type": "create_collection", "name": "お気に入り", "kind": "set"}),
        ),
        genzo_api::ErrorKind::Conflict,
    );
    state.close_catalog(false).unwrap();
}

/// (b) スタック（LIB-12）・仮想コピー（LIB-13）の JSON のコマンドが、アプリの `core_execute` の経路
/// （`execute_command`）で通り、検索の応答・イベントに折りたたんだスタックの隠した写真の数が載る。
pub fn stacks_and_virtual_copies_through_the_app() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("連写");
    for (i, n) in ["A.jpg", "B.jpg", "C.jpg"].iter().enumerate() {
        write_image(&photos.join(n), (48, 32), i as u8);
    }
    state.open_catalog(&env.catalog("連写.db"), true).unwrap();
    import(&state, &photos);
    let items = all_items(&state);
    let id = |n: &str| by_name(&items, n).variant_id.get();
    let (a, b, c) = (id("A.jpg"), id("B.jpg"), id("C.jpg"));
    let mark = env.events.mark();
    let Response::Stack(stack) = ok(
        &state,
        json!({"type": "create_stack", "variant_ids": [a, b], "top": b}),
    ) else {
        panic!("スタックの応答ではない");
    };
    assert_eq!(stack.top_variant_id, VariantId::new(b));
    assert!(stack.collapsed);
    env.events.wait_for(mark, "スタックの変更", |e| {
        matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::Stacks,
                ..
            }
        )
    });
    // 折りたたんだスタックは 1 枚に数え、隠した写真の数を返す。
    let Response::Search(s) = ok(&state, json!({"type": "search"})) else {
        panic!("検索の応答ではない");
    };
    assert_eq!((s.count, s.hidden), (2, 1));
    let Response::Range(r) = ok(
        &state,
        json!({"type": "range", "generation": s.generation, "start": 0, "len": 10}),
    ) else {
        panic!("範囲の応答ではない");
    };
    let top = r
        .items
        .iter()
        .find(|i| i.variant_id == VariantId::new(b))
        .unwrap();
    let badge = top.stack.unwrap();
    assert_eq!((badge.position, badge.count, badge.collapsed), (1, 2, true));
    // 展開を保存すると、作り直しの世代でスタックの写真が並ぶ。
    let mark = env.events.mark();
    let r = ok(
        &state,
        json!({"type": "set_stacks_collapsed", "stack_ids": [stack.stack_id.get()], "collapsed": false}),
    );
    assert!(matches!(r, Response::Updated { count: 1 }), "{r:?}");
    env.events.wait_for(mark, "作り直した検索結果", |e| {
        matches!(
            e,
            Event::SearchUpdated {
                count: 3,
                hidden: 0,
                ..
            }
        )
    });
    // 仮想コピー: 作る・名前の変更・マスターにする。
    let Response::VariantCreated { variant_id: copy } = ok(
        &state,
        json!({"type": "create_virtual_copy", "variant_id": c, "name": null}),
    ) else {
        panic!("仮想コピーの応答ではない");
    };
    ok(
        &state,
        json!({"type": "rename_virtual_copy", "variant_id": copy.get(), "name": "白黒"}),
    );
    let r = ok(
        &state,
        json!({"type": "set_as_master", "variant_id": copy.get()}),
    );
    assert_eq!(
        r,
        Response::MasterChanged {
            previous_master: VariantId::new(c)
        }
    );
    let Response::Details(d) = ok(&state, json!({"type": "variant_details", "variant_id": c}))
    else {
        panic!("詳細の応答ではない");
    };
    assert!(!d.summary.is_master);
    assert_eq!(d.summary.variant_name.as_deref(), Some("白黒"));
    assert_eq!(d.variants.len(), 2);
    assert_eq!(d.stack.as_ref().map(|s| s.members.len()), Some(2));
    crate::support::assert_kind(
        crate::support::exec(&state, json!({"type": "create_stack", "variant_ids": [a]})),
        genzo_api::ErrorKind::InvalidInput,
    );
    state.close_catalog(false).unwrap();
}

/// (b) ファイルの内容による同一性（ORG-03・IMP-04）と RAW と JPEG の扱い（IMP-06）: アプリはカタログを開いて
/// いる間、内容のハッシュをバックグラウンドで計算する（取り込みの後）。取り込みの設定（重複と思われるもの・
/// RAW と JPEG）の JSON のコマンドが `core_execute` の経路で通り、状況・重複の組を読める。
pub fn content_identity_through_the_app() {
    let env = Env::new();
    let state = env.state();
    let first = env.photos("一 回目");
    let second = env.photos("二 回目");
    write_image(&first.join("海.jpg"), (48, 32), 1);
    write_image(&first.join("山.jpg"), (48, 32), 2);
    std::fs::create_dir_all(&second).unwrap();
    std::fs::copy(first.join("海.jpg"), second.join("海 の コピー.jpg")).unwrap();
    state.open_catalog(&env.catalog("同一性.db"), true).unwrap();
    let mark = env.events.mark();
    let report = import(&state, &first);
    assert_eq!(report.added, 2, "{report:?}");
    // 取り込みの後に、内容のハッシュの計算がバックグラウンドで動いて終わる。
    env.events
        .wait_for(mark, "内容のハッシュの計算の終わり", |e| {
            matches!(
                e,
                Event::JobFinished {
                    kind: JobKind::HashFiles,
                    state: JobState::Succeeded,
                    ..
                }
            )
        });
    let Response::ContentHashStatus(st) = ok(&state, json!({"type": "content_hash_status"})) else {
        panic!("状況の応答ではない");
    };
    assert_eq!((st.files, st.hashed, st.pending), (2, 2, 0), "{st:?}");
    // 既定では、同じ内容のファイルを取り込まない。
    let r = import(&state, &second);
    assert_eq!((r.added, r.duplicates.len()), (0, 1), "{r:?}");
    assert!(r.duplicates[0].skipped);
    // 取り込む設定を指定すると、別の写真として登録する。
    let Response::JobStarted { job_id } = ok(
        &state,
        json!({"type": "import_folder", "path": second, "recursive": true,
               "options": {"skip_duplicates": false, "separate_raw_jpeg": true}}),
    ) else {
        panic!("ジョブの応答ではない");
    };
    let info = crate::support::wait_job(&state, job_id);
    let Some(genzo_api::JobResult::Import(r)) = info.result else {
        panic!("{info:?}");
    };
    assert_eq!(r.added, 1, "{r:?}");
    assert!(!r.skip_duplicates && r.separate_raw_jpeg);
    let Response::DuplicateGroups(g) = ok(&state, json!({"type": "duplicate_groups", "limit": 10}))
    else {
        panic!("重複の組の応答ではない");
    };
    assert_eq!((g.total, g.groups[0].files.len()), (1, 2));
    // 設定（取り込みの既定・バックグラウンドの計算）は get_settings・update_settings で読み書きできる。
    let Response::CoreSettings(s) = ok(
        &state,
        json!({"type": "update_settings", "update": {"import_skip_duplicates": false, "hash_network_volumes": true}}),
    ) else {
        panic!("設定の応答ではない");
    };
    assert!(!s.import_skip_duplicates && s.hash_network_volumes && s.hash_in_background);
    state.close_catalog(false).unwrap();
}

/// コピー取り込み（IMP-02・IMP-03・IMP-05）を画面と同じ JSON のコマンドで行う: 名前の例、日付別のフォルダへの
/// コピーとリネーム、まとめて付けるキーワード・作成者。取り込みのダイアログの設定はアプリの設定に保存され、
/// 開き直しても残る（カタログによらない）。
pub fn copy_import_through_the_app() {
    let env = Env::new();
    let state = env.state();
    let card = env.photos("SD カード");
    write_image(
        &card.join("DCIM").join("100MSDCF").join("DSC 1.jpg"),
        (48, 32),
        1,
    );
    write_image(
        &card.join("DCIM").join("100MSDCF").join("DSC 2.jpg"),
        (48, 32),
        2,
    );
    let dest = env.photos("写真 ライブラリ");
    std::fs::create_dir_all(&dest).unwrap();
    state.open_catalog(&env.catalog("コピー.db"), true).unwrap();
    // 名前の例（カタログ・ファイルは使わない）。
    let Response::NamePreview(p) = ok(
        &state,
        json!({"type": "import_name_preview", "copy": {"destination": dest,
               "folder_rule": "year_date", "rename": {"template": "{date}-{seq}"}}}),
    ) else {
        panic!("名前の例の応答ではない");
    };
    assert_eq!(
        p.examples[0].destination,
        "2026/2026-10-13/20261013-0001.ARW"
    );
    // コピー取り込み（撮影日時のない画像は、ファイルの更新日時で日付を決める）。
    let Response::JobStarted { job_id } = ok(
        &state,
        json!({"type": "import_folder", "path": card, "recursive": true,
               "options": {"copy": {"destination": dest, "folder_rule": "single",
                                    "rename": {"template": "旅行-{seq}", "sequence_digits": 2}},
                           "metadata": {"keywords": [["旅行", "京都"]], "fields": {"creator": "岡本"}}}}),
    ) else {
        panic!("ジョブの応答ではない");
    };
    let info = crate::support::wait_job(&state, job_id);
    let Some(genzo_api::JobResult::Import(r)) = info.result else {
        panic!("{info:?}");
    };
    assert_eq!((r.added, r.tagged), (2, 2), "{r:?}");
    let copy = r.copy.as_deref().unwrap();
    assert_eq!(copy.copied.len(), 2);
    assert_eq!(copy.dated_by_file_time, 2);
    let mut names: Vec<String> = std::fs::read_dir(&dest)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["旅行-01.jpg", "旅行-02.jpg"]);
    let items = all_items(&state);
    let Response::Details(d) = ok(
        &state,
        json!({"type": "variant_details", "variant_id": by_name(&items, "旅行-01.jpg").variant_id}),
    ) else {
        panic!("詳細の応答ではない");
    };
    assert_eq!(d.metadata.creator.as_deref(), Some("岡本"));
    // 取り込みのダイアログの設定をアプリに保存する。
    let mut prefs = genzo_app::settings::ImportPreferences {
        copy: true,
        destination: Some(dest.clone()),
        folder_rule: genzo_api::FolderRule::Single,
        rename: true,
        ..Default::default()
    };
    prefs.rename_rule.template = "旅行-{seq}".to_owned();
    let saved = state.update_settings(|s| s.import = prefs.clone()).unwrap();
    assert_eq!(saved.import, prefs);
    state.close_catalog(false).unwrap();
    let again = env.state();
    assert_eq!(again.settings().import, prefs);
}
