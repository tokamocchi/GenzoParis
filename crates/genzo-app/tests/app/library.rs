//! (b) 取り込み → 検索 → `genzo://` のサムネイル・標準プレビュー（名前に空白と日本語を含む写真）。

use genzo_api::{CatalogChange, Event, JobKind, JobState, Response};
use genzo_app::protocol::{self, Reply};
use serde_json::json;

use crate::support::{Env, all_items, by_name, import, ok, write_image};

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
/// `rev` が今の内容のものなら長くキャッシュさせ、古い `rev` では長くキャッシュさせない。
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
    assert_eq!((r.status, r.cache_control), (200, IMMUTABLE));
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
