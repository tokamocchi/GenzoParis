//! (f) コアのイベントが EventSink に届く。起動時の警告を 2 回送らない。閉じたカタログのイベントを、次の
//! カタログを開いた後に送らない。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use genzo_api::{CatalogChange, Event, JobKind, JobState, PreviousShutdown, WarningCode};
use genzo_app::state::EventSink;
use parking_lot::Mutex;
use serde_json::json;

use crate::support::{Env, all_items, import, ok, write_image};

fn is_marks(e: &Event) -> bool {
    matches!(
        e,
        Event::CatalogChanged {
            change: CatalogChange::Marks,
            ..
        }
    )
}

fn is_import_finished(e: &Event) -> bool {
    matches!(
        e,
        Event::JobFinished {
            kind: JobKind::Import,
            state: JobState::Succeeded,
            ..
        }
    )
}

/// 取り込み・評価・現像のイベントが送り先に届く（画面には `core-event` として JSON で送る）。閉じて別の
/// カタログを開いた後も届く。
pub fn core_events_reach_the_sink() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("イベント");
    write_image(&photos.join("花 火.jpg"), (64, 48), 7);
    state.open_catalog(&env.catalog("一つ目.db"), true).unwrap();

    let mark = env.events.mark();
    import(&state, &photos);
    let finished = env
        .events
        .wait_for(mark, "取り込みの終わり", is_import_finished);
    let json = serde_json::to_value(&finished).unwrap();
    assert_eq!(json["type"], "job_finished", "{json}");

    let id = all_items(&state)[0].variant_id;
    let mark = env.events.mark();
    ok(
        &state,
        json!({"type": "set_rating", "variant_ids": [id], "rating": 4}),
    );
    env.events.wait_for(mark, "評価の変更", is_marks);

    let mark = env.events.mark();
    ok(&state, json!({"type": "open_develop", "variant_id": id}));
    ok(&state, json!({"type": "render_develop"}));
    env.events.wait_for(
        mark,
        "現像のプレビュー",
        |e| matches!(e, Event::PreviewUpdated { variant_id, .. } if *variant_id == id),
    );

    // 閉じて別のカタログを開いても、新しいカタログのイベントが届く。
    state.close_catalog(false).unwrap();
    state.open_catalog(&env.catalog("二つ目.db"), true).unwrap();
    let mark = env.events.mark();
    import(&state, &photos);
    env.events.wait_for(
        mark,
        "二つ目のカタログの取り込みの終わり",
        is_import_finished,
    );
    state.close_catalog(false).unwrap();
}

/// 起動時の警告（前回の異常終了など）は CatalogStatus::warnings で返し、イベントとしては送らない（画面が
/// 両方を受けて 2 回知らせないため）。
pub fn startup_warnings_are_not_forwarded_twice() {
    let env = Env::new();
    let state = env.state();
    let path = env.catalog("警告.db");
    state.open_catalog(&path, true).unwrap();
    state.close_catalog(false).unwrap();
    // 異常終了を模す: カタログを開いて、閉じずに終わる（使用中の印が残る）。
    drop(genzo_catalog::Catalog::open(&path).unwrap());

    let mark = env.events.mark();
    let status = state.open_catalog(&path, false).unwrap();
    assert_eq!(status.startup.previous_shutdown, PreviousShutdown::Unclean);
    let unclean = |e: &Event| {
        matches!(
            e,
            Event::Warning {
                code: WarningCode::UncleanShutdown,
                ..
            }
        )
    };
    assert_eq!(status.warnings.iter().filter(|e| unclean(e)).count(), 1);
    // 後のイベント（空のフォルダの取り込みの終わり）が届くまで待つ（イベントは順に届くので、起動時の警告を
    // 送っていれば、それより前に届いている）。
    let empty = env.photos("空");
    std::fs::create_dir_all(&empty).unwrap();
    import(&state, &empty);
    env.events
        .wait_for(mark, "取り込みの終わり", is_import_finished);
    let forwarded = env.events.since(mark);
    assert!(
        !forwarded.iter().any(unclean),
        "起動時の警告をイベントでも送った: {forwarded:?}"
    );
    state.close_catalog(false).unwrap();
}

/// 閉じたカタログのイベントが送り先に溜まっていても（画面への送信が遅いなど）、閉じた後は送らない（次に
/// 開いたカタログの画面に、前のカタログの検索結果の世代・ジョブ・サムネイルの更新が届かないように）。
pub fn events_of_a_closed_catalog_are_not_forwarded_later() {
    let env = Env::new();
    let phase = Arc::new(AtomicU32::new(0));
    let log: Arc<Mutex<Vec<(u32, Event)>>> = Arc::default();
    let sink: EventSink = {
        let (phase, log) = (Arc::clone(&phase), Arc::clone(&log));
        Arc::new(move |e: &Event| {
            if is_marks(e) {
                // 画面への送信が遅く、イベントが溜まる場合を模す。
                std::thread::sleep(Duration::from_millis(50));
            }
            log.lock().push((phase.load(Ordering::SeqCst), e.clone()));
        })
    };
    let state = env.state_with(env.config_dir(), sink);
    let photos = env.photos("溜まる");
    write_image(&photos.join("一 枚.jpg"), (64, 48), 9);
    state.open_catalog(&env.catalog("前.db"), true).unwrap();
    import(&state, &photos);
    let id = all_items(&state)[0].variant_id;
    // 評価の変更のイベントを溜める（1 件 50 ms で送る）。
    for i in 0..60u8 {
        ok(
            &state,
            json!({"type": "set_rating", "variant_ids": [id], "rating": i % 5 + 1}),
        );
    }
    state.close_catalog(false).unwrap();
    state.open_catalog(&env.catalog("次.db"), true).unwrap();
    phase.store(1, Ordering::SeqCst);
    // 前のカタログのイベントを送り続けていれば、この間に届く。
    std::thread::sleep(Duration::from_millis(500));
    let late = log
        .lock()
        .iter()
        .filter(|(p, e)| *p == 1 && is_marks(e))
        .count();
    assert_eq!(
        late, 0,
        "閉じたカタログのイベントを、次のカタログを開いた後に送った"
    );
    // 次のカタログのイベントは届く。
    import(&state, &photos);
    crate::support::wait_until("次のカタログの取り込みの終わり", || {
        log.lock()
            .iter()
            .any(|(p, e)| *p == 1 && is_import_finished(e))
    });
    state.close_catalog(false).unwrap();
}
