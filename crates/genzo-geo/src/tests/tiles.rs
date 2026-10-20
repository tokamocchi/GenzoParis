//! 地図のタイルのキャッシュ（[`TileService`]）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;

use super::server::{LoopbackServer, response};
use super::{FakeClient, FakeClock, http, png};
use crate::{
    EVICTION_CHECK_EVERY, GeoError, MAX_TILE_BYTES, TileProvider, TileService, UreqClient,
    user_agent,
};

const MINUTE: Duration = Duration::from_secs(60);

fn service(client: &Arc<FakeClient>, dir: Option<&Path>, clock: &Arc<FakeClock>) -> TileService {
    TileService::new(
        client.clone(),
        TileProvider::openstreetmap(),
        dir.map(Path::to_path_buf),
    )
    .with_clock(clock.clone())
}

/// `dir` の中の提供元のフォルダ（1 つだけのはず）。
fn provider_dir(dir: &Path) -> PathBuf {
    let dirs: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_type().unwrap().is_dir())
        .map(|e| e.path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.into_iter().next().unwrap()
}

#[test]
fn fresh_tiles_come_from_the_disk_cache_without_requests() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::scripted(vec![Ok(http(
        200,
        &[
            ("Content-Type", "image/png"),
            ("ETag", "\"v1\""),
            ("Last-Modified", "Sun, 18 Oct 2026 00:00:00 GMT"),
            ("Cache-Control", "max-age=3600"),
        ],
        &png("a"),
    ))]);
    let tiles = service(&client, Some(dir.path()), &clock);

    let t = tiles.tile(12, 3638, 1612).unwrap();
    assert_eq!(
        (t.bytes.clone(), t.content_type, t.stale),
        (png("a"), "image/png", false)
    );
    let requests = client.requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.url, "https://tile.openstreetmap.org/12/3638/1612.png");
    assert_eq!(req.header("accept"), Some("image/png"));
    assert_eq!(req.max_body_bytes, MAX_TILE_BYTES);
    for name in [
        "if-none-match",
        "if-modified-since",
        "cache-control",
        "pragma",
    ] {
        assert_eq!(req.header(name), None, "{name}");
    }

    // cache_dir/<提供元>/<z>/<x>/<y>.png と .json。
    let tile_dir = provider_dir(dir.path()).join("12").join("3638");
    assert_eq!(fs::read(tile_dir.join("1612.png")).unwrap(), png("a"));
    let meta: serde_json::Value =
        serde_json::from_slice(&fs::read(tile_dir.join("1612.json")).unwrap()).unwrap();
    assert_eq!(meta["etag"], "\"v1\"");
    assert_eq!(meta["last_modified"], "Sun, 18 Oct 2026 00:00:00 GMT");
    assert_eq!(meta["expires_at"], "2026-10-19T13:00:00Z");
    assert_eq!(meta["fetched_at"], "2026-10-19T12:00:00Z");

    // 期限内は通信しない（作り直しても、ディスクから読む）。
    clock.advance(59 * MINUTE);
    assert_eq!(tiles.tile(12, 3638, 1612).unwrap(), t);
    let again = service(&client, Some(dir.path()), &clock);
    assert_eq!(again.tile(12, 3638, 1612).unwrap(), t);
    assert_eq!(client.count(), 1);
}

#[test]
fn expired_tiles_are_revalidated_with_conditional_requests() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::scripted(vec![
        Ok(http(
            200,
            &[
                ("Content-Type", "image/png"),
                ("ETag", "\"v1\""),
                ("Last-Modified", "Sun, 18 Oct 2026 00:00:00 GMT"),
                ("Cache-Control", "max-age=60"),
            ],
            &png("a"),
        )),
        Ok(http(304, &[("Cache-Control", "max-age=120")], b"")),
        Ok(http(
            200,
            &[
                ("Content-Type", "image/png"),
                ("ETag", "\"v2\""),
                ("Cache-Control", "max-age=60"),
            ],
            &png("b"),
        )),
    ]);
    let tiles = service(&client, Some(dir.path()), &clock);
    tiles.tile(3, 1, 2).unwrap();

    // 期限切れ → 条件付きの要求 → 304 ならキャッシュを返し、期限を延ばす。
    clock.advance(Duration::from_secs(61));
    let t = tiles.tile(3, 1, 2).unwrap();
    assert_eq!((t.bytes, t.stale), (png("a"), false));
    let req = &client.requests()[1];
    assert_eq!(req.header("if-none-match"), Some("\"v1\""));
    assert_eq!(
        req.header("if-modified-since"),
        Some("Sun, 18 Oct 2026 00:00:00 GMT")
    );
    assert_eq!(req.header("cache-control"), None);

    // 304 の max-age=120 の間は通信しない。
    clock.advance(Duration::from_secs(100));
    tiles.tile(3, 1, 2).unwrap();
    assert_eq!(client.count(), 2);

    // また切れたら条件付きの要求。新しいタイルなら置き換える（ETag も）。
    clock.advance(Duration::from_secs(30));
    let t = tiles.tile(3, 1, 2).unwrap();
    assert_eq!(t.bytes, png("b"));
    assert_eq!(client.requests()[2].header("if-none-match"), Some("\"v1\""));
    let meta: serde_json::Value =
        serde_json::from_slice(&fs::read(provider_dir(dir.path()).join("3/1/2.json")).unwrap())
            .unwrap();
    assert_eq!(meta["etag"], "\"v2\"");
    assert_eq!(meta["last_modified"], serde_json::Value::Null);
    assert_eq!(
        fs::read(provider_dir(dir.path()).join("3/1/2.png")).unwrap(),
        png("b")
    );
}

#[test]
fn not_modified_responses_with_only_expires_update_the_lifetime() {
    // Cache-Control を返さず、Expires を絶対の時刻で返す提供元。304 のヘッダーで保存したヘッダーを更新して
    // から期限を決め直す（RFC 9111 の 4.3.4 節）。サーバーが期限を短くしたら従う。
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::scripted(vec![
        Ok(http(
            200,
            &[
                ("Content-Type", "image/png"),
                ("ETag", "\"v1\""),
                ("Date", "Mon, 19 Oct 2026 12:00:00 GMT"),
                ("Expires", "Mon, 26 Oct 2026 12:00:00 GMT"),
            ],
            &png("a"),
        )),
        Ok(http(
            304,
            &[
                ("Date", "Mon, 26 Oct 2026 12:00:01 GMT"),
                ("Expires", "Mon, 26 Oct 2026 13:00:01 GMT"),
            ],
            b"",
        )),
        Ok(http(
            304,
            &[("Date", "Mon, 26 Oct 2026 13:01:01 GMT"), ("Expires", "0")],
            b"",
        )),
        Ok(http(304, &[("Date", "Mon, 26 Oct 2026 13:01:01 GMT")], b"")),
    ]);
    let tiles = service(&client, Some(dir.path()), &clock);
    tiles.tile(5, 1, 1).unwrap();
    // 7 日（200 の Expires − Date）の後に切れて、304 が 1 時間の期限を返す。
    clock.advance(Duration::from_secs(7 * 24 * 3600 + 1));
    assert_eq!(tiles.tile(5, 1, 1).unwrap().bytes, png("a"));
    assert_eq!(client.count(), 2);
    clock.advance(59 * MINUTE);
    tiles.tile(5, 1, 1).unwrap();
    assert_eq!(client.count(), 2);
    // 1 時間で切れる（前の 7 日を使い続けない）。
    clock.advance(Duration::from_secs(60));
    tiles.tile(5, 1, 1).unwrap();
    assert_eq!(client.count(), 3);
    // `Expires: 0`（すでに切れている）の 304 の後は、使うたびに確かめる。304 に Expires がなければ、
    // 保存した Expires（`0`）が残る。
    assert_eq!(tiles.tile(5, 1, 1).unwrap().bytes, png("a"));
    assert_eq!(client.count(), 4);
}

#[test]
fn tiles_without_cache_headers_are_kept_for_seven_days() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let ok = || Ok(http(200, &[("Content-Type", "image/png")], &png("a")));
    let client = FakeClient::scripted(vec![ok(), ok()]);
    let tiles = service(&client, Some(dir.path()), &clock);
    tiles.tile(1, 0, 0).unwrap();
    clock.advance(Duration::from_secs(7 * 24 * 3600 - 1));
    tiles.tile(1, 0, 0).unwrap();
    assert_eq!(client.count(), 1);
    clock.advance(Duration::from_secs(2));
    tiles.tile(1, 0, 0).unwrap();
    assert_eq!(client.count(), 2);
    // ETag も Last-Modified もないので、条件なしで取り直す。
    let req = &client.requests()[1];
    assert_eq!(
        (req.header("if-none-match"), req.header("if-modified-since")),
        (None, None)
    );
}

#[test]
fn no_store_tiles_are_not_saved_and_no_cache_tiles_are_always_revalidated() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::scripted(vec![
        Ok(http(
            200,
            &[("Content-Type", "image/png"), ("Cache-Control", "no-store")],
            &png("a"),
        )),
        Ok(http(
            200,
            &[
                ("Content-Type", "image/png"),
                ("Cache-Control", "no-cache"),
                ("ETag", "\"n1\""),
            ],
            &png("b"),
        )),
        Ok(http(304, &[], b"")),
    ]);
    let tiles = service(&client, Some(dir.path()), &clock);
    assert_eq!(tiles.tile(2, 1, 1).unwrap().bytes, png("a"));
    assert_eq!(tiles.cache_info().files, 0);

    assert_eq!(tiles.tile(2, 1, 1).unwrap().bytes, png("b"));
    assert_eq!(tiles.cache_info().files, 2);
    // 時間がたっていなくても、使うたびに確かめる。
    assert_eq!(tiles.tile(2, 1, 1).unwrap().bytes, png("b"));
    assert_eq!(client.count(), 3);
    assert_eq!(client.requests()[2].header("if-none-match"), Some("\"n1\""));
}

#[test]
fn stale_tiles_are_used_when_the_network_fails() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::scripted(vec![
        Ok(http(
            200,
            &[
                ("Content-Type", "image/png"),
                ("Cache-Control", "max-age=60"),
            ],
            &png("a"),
        )),
        Err(GeoError::Network("つながりません".into())),
        Ok(http(503, &[], b"")),
        Ok(http(404, &[], b"")),
    ]);
    let tiles = service(&client, Some(dir.path()), &clock);
    tiles.tile(4, 2, 3).unwrap();
    clock.advance(Duration::from_secs(61));
    let t = tiles.tile(4, 2, 3).unwrap();
    assert_eq!((t.bytes, t.stale), (png("a"), true));
    let t = tiles.tile(4, 2, 3).unwrap();
    assert_eq!((t.bytes, t.stale), (png("a"), true));
    // 404（タイルがない）は古いものを使わない。
    assert!(matches!(tiles.tile(4, 2, 3), Err(GeoError::Status(404))));

    // キャッシュがなければ誤り。
    let client = FakeClient::scripted(vec![Err(GeoError::Network("つながりません".into()))]);
    let tiles = service(&client, Some(dir.path()), &clock);
    assert!(matches!(tiles.tile(4, 0, 0), Err(GeoError::Network(_))));
}

#[test]
fn must_revalidate_tiles_are_not_used_after_they_expire() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::scripted(vec![
        Ok(http(
            200,
            &[
                ("Content-Type", "image/png"),
                ("Cache-Control", "max-age=60, must-revalidate"),
            ],
            &png("a"),
        )),
        Err(GeoError::Network("つながりません".into())),
    ]);
    let tiles = service(&client, Some(dir.path()), &clock);
    tiles.tile(4, 2, 3).unwrap();
    clock.advance(Duration::from_secs(61));
    assert!(matches!(tiles.tile(4, 2, 3), Err(GeoError::Network(_))));
}

#[test]
fn out_of_range_tiles_are_rejected_without_requests() {
    let clock = FakeClock::new();
    let client = FakeClient::scripted(Vec::new());
    let tiles = service(&client, None, &clock);
    for (z, x, y) in [(20, 0, 0), (1, 2, 0), (1, 0, 2), (0, 1, 0)] {
        assert!(
            matches!(tiles.tile(z, x, y), Err(GeoError::InvalidInput(_))),
            "{z}/{x}/{y}"
        );
    }
    assert_eq!(client.count(), 0);
}

#[test]
fn responses_that_are_not_png_are_not_saved() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::scripted(vec![
        Ok(http(200, &[("Content-Type", "text/html")], b"<html>")),
        Ok(http(200, &[("Content-Type", "image/png")], b"GIF89a")),
        Ok(http(204, &[("Content-Type", "image/png")], b"")),
    ]);
    let tiles = service(&client, Some(dir.path()), &clock);
    for _ in 0..3 {
        assert!(matches!(tiles.tile(5, 1, 1), Err(GeoError::BadResponse(_))));
    }
    assert_eq!(tiles.cache_info().files, 0);
}

#[test]
fn the_cache_limit_removes_the_oldest_tiles() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::new(|_| Ok(http(200, &[("Content-Type", "image/png")], &png("t"))));
    let tiles = service(&client, Some(dir.path()), &clock);
    for x in 0..3 {
        tiles.tile(2, x, 0).unwrap();
    }
    fs::write(dir.path().join("keep.txt"), b"not ours").unwrap();
    let info = tiles.cache_info();
    assert_eq!(info.files, 6);

    // 更新日時を古い順に x = 0, 1, 2 にする。
    let base = provider_dir(dir.path()).join("2");
    let now = SystemTime::now();
    for x in 0..3u64 {
        for ext in ["png", "json"] {
            let file = fs::File::options()
                .write(true)
                .open(base.join(x.to_string()).join(format!("0.{ext}")))
                .unwrap();
            file.set_modified(now - Duration::from_secs(3600 * (3 - x)))
                .unwrap();
        }
    }
    // 1 枚分だけ上限を超える。
    let limited = service(&client, Some(dir.path()), &clock).with_cache_limit(info.bytes - 1);
    limited.enforce_cache_limit();
    assert!(!base.join("0/0.png").exists());
    assert!(!base.join("0/0.json").exists());
    assert!(base.join("1/0.png").exists() && base.join("2/0.png").exists());
    assert_eq!(limited.cache_info().files, 4);
    assert!(limited.cache_info().bytes < info.bytes);
    // cache_dir のほかのファイルは数えず、消さない。
    assert!(dir.path().join("keep.txt").exists());
}

#[test]
fn the_cache_limit_is_checked_every_few_writes() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::new(|_| Ok(http(200, &[("Content-Type", "image/png")], &png("t"))));
    // 上限 1 バイト: 確かめるたびにすべて消える。
    let tiles = service(&client, Some(dir.path()), &clock).with_cache_limit(1);
    // 最初の書き込みで確かめる。
    tiles.tile(10, 0, 0).unwrap();
    assert_eq!(tiles.cache_info().files, 0);
    for x in 1..EVICTION_CHECK_EVERY as u32 {
        tiles.tile(10, x, 0).unwrap();
    }
    assert_eq!(tiles.cache_info().files, 2 * (EVICTION_CHECK_EVERY - 1));
    tiles.tile(10, EVICTION_CHECK_EVERY as u32, 0).unwrap();
    assert_eq!(tiles.cache_info().files, 0);
}

#[test]
fn clear_cache_removes_only_the_provider_folders() {
    let dir = TempDir::new().unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::new(|_| Ok(http(200, &[("Content-Type", "image/png")], &png("t"))));
    let osm = service(&client, Some(dir.path()), &clock);
    let other = TileService::new(
        client.clone(),
        TileProvider::new("https://tiles.example.org/{z}/{x}/{y}.png", 19, "© x").unwrap(),
        Some(dir.path().to_path_buf()),
    )
    .with_clock(clock.clone());
    osm.tile(1, 1, 1).unwrap();
    other.tile(1, 1, 1).unwrap();
    assert_eq!(
        client.requests()[1].url,
        "https://tiles.example.org/1/1/1.png"
    );
    fs::write(dir.path().join("keep.txt"), b"x").unwrap();
    fs::create_dir_all(dir.path().join("other")).unwrap();
    fs::write(dir.path().join("other/file.png"), b"x").unwrap();
    fs::create_dir_all(dir.path().join("0123456789ABCDEF")).unwrap();

    // 提供元ごとのフォルダに分かれ、大きさは両方を数える。
    assert_eq!(osm.cache_info().files, 4);
    osm.clear_cache().unwrap();
    assert_eq!(osm.cache_info(), crate::CacheInfo::default());
    assert_eq!(other.cache_info().files, 0);
    for kept in ["keep.txt", "other/file.png", "0123456789ABCDEF"] {
        assert!(dir.path().join(kept).exists(), "{kept}");
    }
    // 消した後は取り直す。
    osm.tile(1, 1, 1).unwrap();
    assert_eq!(client.count(), 3);
    // キャッシュがなければ何もしない。
    let none = service(&client, None, &clock);
    none.clear_cache().unwrap();
    assert_eq!(none.cache_info(), crate::CacheInfo::default());
}

#[test]
fn tiles_fetched_while_the_cache_is_cleared_are_not_saved() {
    // 取っている途中に消したら、そのタイルはディスクにもメモリーにも書かない（消した後に戻らない）。
    for disk in [true, false] {
        let dir = TempDir::new().unwrap();
        let clock = FakeClock::new();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let client = {
            let gate = gate.clone();
            FakeClient::new(move |_| {
                let (open, cv) = &*gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                Ok(http(200, &[("Content-Type", "image/png")], &png("late")))
            })
        };
        let tiles = Arc::new(service(&client, disk.then_some(dir.path()), &clock));
        let fetching = {
            let tiles = tiles.clone();
            std::thread::spawn(move || tiles.tile(1, 1, 1))
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while client.count() == 0 {
            assert!(std::time::Instant::now() < deadline, "要求が届かない");
            std::thread::sleep(Duration::from_millis(5));
        }
        tiles.clear_cache().unwrap();
        {
            let (open, cv) = &*gate;
            *open.lock().unwrap() = true;
            cv.notify_all();
        }
        // 取ったタイルは返すが、キャッシュには残さない。
        assert_eq!(fetching.join().unwrap().unwrap().bytes, png("late"));
        assert_eq!(
            tiles.cache_info(),
            crate::CacheInfo::default(),
            "disk={disk}"
        );
        tiles.tile(1, 1, 1).unwrap();
        assert_eq!(client.count(), 2, "disk={disk}");
        // 消した後に取り始めたものは、今までどおり保存する。
        tiles.tile(1, 1, 1).unwrap();
        assert_eq!(client.count(), 2, "disk={disk}");
    }
}

#[test]
fn without_a_disk_cache_tiles_are_kept_in_memory() {
    // キャッシュのフォルダがない（app_cache_dir() が取れない）ときも、再訪で上流から取り直さない（タイルの
    // ポリシーの「再訪で取り直さない手元のキャッシュ」）。期限と条件付きの要求はディスクと同じ規則。
    let clock = FakeClock::new();
    let client = FakeClient::scripted(vec![
        Ok(http(
            200,
            &[
                ("Content-Type", "image/png"),
                ("ETag", "\"m1\""),
                ("Cache-Control", "max-age=60"),
            ],
            &png("a"),
        )),
        Ok(http(304, &[("Cache-Control", "max-age=60")], b"")),
        Ok(http(200, &[("Content-Type", "image/png")], &png("b"))),
    ]);
    let tiles = service(&client, None, &clock);
    tiles.tile(6, 1, 2).unwrap();
    clock.advance(Duration::from_secs(30));
    assert_eq!(tiles.tile(6, 1, 2).unwrap().bytes, png("a"));
    assert_eq!(client.count(), 1);
    // 期限が切れたら条件付きの要求。304 ならメモリーのものを返し、期限を延ばす。
    clock.advance(Duration::from_secs(31));
    assert_eq!(tiles.tile(6, 1, 2).unwrap().bytes, png("a"));
    assert_eq!(client.requests()[1].header("if-none-match"), Some("\"m1\""));
    clock.advance(Duration::from_secs(30));
    tiles.tile(6, 1, 2).unwrap();
    assert_eq!(client.count(), 2);
    // ディスクのキャッシュの大きさには数えない。消すとメモリーの分も消える。
    assert_eq!(tiles.cache_info(), crate::CacheInfo::default());
    tiles.clear_cache().unwrap();
    assert_eq!(tiles.tile(6, 1, 2).unwrap().bytes, png("b"));
    assert_eq!(client.count(), 3);
    assert_eq!(client.requests()[2].header("if-none-match"), None);
}

#[test]
fn tiles_are_kept_in_memory_when_the_disk_cache_cannot_be_written() {
    let dir = TempDir::new().unwrap();
    // キャッシュのフォルダの位置に普通のファイルがある（フォルダを作れない。root で動いても書けない）。
    let root = dir.path().join("map-tiles");
    fs::write(&root, b"not a folder").unwrap();
    let clock = FakeClock::new();
    let client = FakeClient::new(|_| Ok(http(200, &[("Content-Type", "image/png")], &png("a"))));
    let tiles = service(&client, Some(&root), &clock);
    assert_eq!(tiles.tile(3, 1, 1).unwrap().bytes, png("a"));
    assert_eq!(tiles.tile(3, 1, 1).unwrap().bytes, png("a"));
    assert_eq!(client.count(), 1);
    // 作り直す（提供元を変える・設定を保存し直す）とメモリーの分はなくなる。
    let again = service(&client, Some(&root), &clock);
    again.tile(3, 1, 1).unwrap();
    assert_eq!(client.count(), 2);
}

#[test]
fn the_memory_cache_keeps_the_most_recent_tiles_within_its_limit() {
    let clock = FakeClock::new();
    let client = FakeClient::new(|_| Ok(http(200, &[("Content-Type", "image/png")], &png("t"))));
    let one = png("t").len() as u64;
    let tiles = service(&client, None, &clock).with_memory_cache_limit(2 * one);
    tiles.tile(4, 0, 0).unwrap();
    tiles.tile(4, 1, 0).unwrap();
    // (4, 0, 0) を使い直すと新しくなり、次に追い出されるのは (4, 1, 0)。
    tiles.tile(4, 0, 0).unwrap();
    assert_eq!(client.count(), 2);
    tiles.tile(4, 2, 0).unwrap();
    assert_eq!(client.count(), 3);
    tiles.tile(4, 0, 0).unwrap();
    tiles.tile(4, 2, 0).unwrap();
    assert_eq!(client.count(), 3);
    tiles.tile(4, 1, 0).unwrap();
    assert_eq!(client.count(), 4);
    // no-store のタイルはメモリーにも残さない。
    let client = FakeClient::new(|_| {
        Ok(http(
            200,
            &[("Content-Type", "image/png"), ("Cache-Control", "no-store")],
            &png("n"),
        ))
    });
    let tiles = service(&client, None, &clock);
    tiles.tile(4, 0, 0).unwrap();
    tiles.tile(4, 0, 0).unwrap();
    assert_eq!(client.count(), 2);
}

#[test]
fn concurrent_requests_for_one_tile_are_coalesced() {
    let clock = FakeClock::new();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let client = {
        let gate = gate.clone();
        FakeClient::new(move |_| {
            let (open, cv) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
            // no-store（メモリーのキャッシュにも残さない）。
            Ok(http(
                200,
                &[("Content-Type", "image/png"), ("Cache-Control", "no-store")],
                &png("shared"),
            ))
        })
    };
    // ディスクのキャッシュなし・保存しない応答（まとめているのがキャッシュでないことを確かめる）。
    let tiles = Arc::new(service(&client, None, &clock));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let tiles = tiles.clone();
            std::thread::spawn(move || tiles.tile(5, 3, 4))
        })
        .collect();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !(client.count() == 1 && tiles.waiting_followers() == 7) {
        assert!(
            std::time::Instant::now() < deadline,
            "待ち合わせがそろわない"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    {
        let (open, cv) = &*gate;
        *open.lock().unwrap() = true;
        cv.notify_all();
    }
    for t in threads {
        assert_eq!(t.join().unwrap().unwrap().bytes, png("shared"));
    }
    assert_eq!(client.count(), 1);
    // 終わった後の要求は、新しく取る。
    tiles.tile(5, 3, 4).unwrap();
    assert_eq!(client.count(), 2);
}

#[test]
fn a_panicking_fetch_does_not_leave_the_tile_blocked() {
    let clock = FakeClock::new();
    let calls = Arc::new(Mutex::new(0));
    let client = {
        let calls = calls.clone();
        FakeClient::new(move |_| {
            let mut calls = calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                drop(calls);
                panic!("テストの panic");
            }
            Ok(http(200, &[("Content-Type", "image/png")], &png("ok")))
        })
    };
    let tiles = service(&client, None, &clock);
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tiles.tile(1, 0, 0)));
    assert!(first.is_err());
    assert_eq!(tiles.tile(1, 0, 0).unwrap().bytes, png("ok"));
}

#[test]
fn the_real_client_sends_conditional_requests_without_cache_busting_headers() {
    let server = LoopbackServer::start(|req| {
        if req.header("if-none-match") == Some("\"w1\"") {
            response("304 Not Modified", &[("Cache-Control", "max-age=0")], b"")
        } else {
            response(
                "200 OK",
                &[
                    ("Content-Type", "image/png"),
                    ("ETag", "\"w1\""),
                    ("Cache-Control", "max-age=0"),
                ],
                &png("wire"),
            )
        }
    });
    let dir = TempDir::new().unwrap();
    let tiles = TileService::new(
        Arc::new(UreqClient::for_test(true)),
        TileProvider::for_loopback_test(&server.url("/tiles/{z}/{x}/{y}.png"), 19),
        Some(dir.path().to_path_buf()),
    );
    assert_eq!(tiles.tile(7, 113, 50).unwrap().bytes, png("wire"));
    assert_eq!(tiles.tile(7, 113, 50).unwrap().bytes, png("wire"));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for (i, req) in requests.iter().enumerate() {
        assert_eq!(req.target(), "/tiles/7/113/50.png");
        assert_eq!(req.header("user-agent"), Some(user_agent()));
        assert_eq!(req.header("accept"), Some("image/png"));
        assert_eq!(req.header("cache-control"), None);
        assert_eq!(req.header("pragma"), None);
        let expected = if i == 0 { None } else { Some("\"w1\"") };
        assert_eq!(req.header("if-none-match"), expected);
    }
}
