//! カタログを開く・閉じる（作る・データのフォルダ・同じものを返す・エラーの分類・設定に残る・パスの
//! 書き方・同時に呼ぶ）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use genzo_api::{ErrorKind, PreviousShutdown, Response};
use genzo_app::catalog_file::{self, CatalogFileState};
use genzo_app::{commands, protocol, settings};
use serde_json::json;

use crate::support::{Env, ExitSwitch, all_items, assert_kind, exec, import, ok, write_image};

/// (a) ないカタログを作る・データのフォルダができる・同じパスでもう一度開くと同じものを返す・カタログで
/// ないファイルは invalid_input・ないパスで create = false は not_found・settings.json に last_catalog が
/// 残る。(e) 閉じた後は closed のエラー。
pub fn open_creates_catalog_data_dir_and_remembers_it() {
    let env = Env::new();
    let state = env.state();
    let path = env.catalog("マイ カタログ.db");
    assert_eq!(
        catalog_file::inspect(&path).unwrap(),
        CatalogFileState::Missing
    );

    // ないカタログは、create = false では開かない（何も作らない）。
    let e = assert_kind(state.open_catalog(&path, false), ErrorKind::NotFound);
    assert!(e.hint.is_some(), "{e:?}");
    assert!(!path.parent().unwrap().exists(), "フォルダも作らない");

    // 作る（フォルダもなければ作る）。
    let status = state.open_catalog(&path, true).unwrap();
    assert_eq!(status.catalog_path, path);
    let data_dir = path.with_file_name("マイ カタログ-data");
    assert_eq!(status.data_dir, data_dir, "<名前>-data（CLI と同じ）");
    assert!(data_dir.is_dir());
    assert_eq!(
        catalog_file::inspect(&path).unwrap(),
        CatalogFileState::Catalog
    );
    assert_eq!(
        status.startup.previous_shutdown,
        PreviousShutdown::FirstOpen
    );
    assert_eq!(status.stats.assets, 0);
    assert!(status.worker_error.is_none(), "{:?}", status.worker_error);
    assert!(status.capabilities.is_some(), "ワーカーを起動できる");
    assert!(status.warnings.is_empty(), "{:?}", status.warnings);
    let core = state.core().unwrap();

    // settings.json に残る。
    let saved = settings::load(&env.config_dir()).settings;
    assert_eq!(saved.last_catalog.as_deref(), Some(path.as_path()));
    assert_eq!(saved.recent_catalogs.first(), Some(&path));
    assert_eq!(
        state.settings().last_catalog.as_deref(),
        Some(path.as_path())
    );

    // 同じパスでもう一度開くと、同じものを返す（閉じて開き直さない）。
    for create in [true, false] {
        let again = state.open_catalog(&path, create).unwrap();
        assert_eq!(again.catalog_path, status.catalog_path);
        assert_eq!(again.data_dir, status.data_dir);
        assert!(Arc::ptr_eq(&state.core().unwrap(), &core));
    }

    // カタログでないファイル・フォルダ・空のパスは invalid_input。何も変えず、今のカタログは開いたまま。
    let memo = env.root().join("メモ 帳.db");
    std::fs::write(&memo, "カタログではない").unwrap();
    for create in [true, false] {
        let e = assert_kind(state.open_catalog(&memo, create), ErrorKind::InvalidInput);
        assert!(e.message.contains("カタログではありません"), "{e:?}");
        assert!(e.hint.is_some(), "{e:?}");
    }
    assert_eq!(std::fs::read_to_string(&memo).unwrap(), "カタログではない");
    assert!(
        !memo.with_file_name("メモ 帳-data").exists(),
        "データのフォルダを作らない"
    );
    assert_kind(
        state.open_catalog(env.root(), true),
        ErrorKind::InvalidInput,
    );
    assert_kind(
        state.open_catalog(Path::new(""), true),
        ErrorKind::InvalidInput,
    );
    assert!(
        Arc::ptr_eq(&state.core().unwrap(), &core),
        "開けないものを選んでも、今のカタログは閉じない"
    );
    // 不正なコマンドの JSON は invalid_input。
    assert_kind(
        exec(&state, json!({"type": "no_such_command"})),
        ErrorKind::InvalidInput,
    );

    // (e) 閉じた後は、コア・コマンド・画素・URI スキームのどれも「閉じている」。
    drop(core);
    state.close_catalog(false).unwrap();
    assert!(state.session().is_none());
    assert!(state.status().unwrap().is_none());
    let e = assert_kind(state.core(), ErrorKind::Closed);
    assert!(e.hint.is_some(), "{e:?}");
    assert_kind(
        exec(&state, json!({"type": "catalog_stats"})),
        ErrorKind::Closed,
    );
    assert_kind(commands::develop_frame_bytes(&state), ErrorKind::Closed);
    assert_kind(commands::region_frame_bytes(&state), ErrorKind::Closed);
    assert_kind(commands::before_frame_bytes(&state), ErrorKind::Closed);
    assert_kind(
        commands::before_region_frame_bytes(&state),
        ErrorKind::Closed,
    );
    assert_eq!(protocol::reply(&state, "/thumb/1", None).status, 503);
    assert_eq!(protocol::reply(&state, "/preview/1", None).status, 503);
    state.close_catalog(false).unwrap();
    state.close_catalog(true).unwrap();

    // アプリを起動し直すと、前回のカタログを覚えている。閉じたカタログは開き直せる（ロックが外れている。
    // 前回は正常に終了した）。
    let restarted = env.state();
    assert_eq!(
        restarted.settings().last_catalog.as_deref(),
        Some(path.as_path())
    );
    let reopened = restarted.open_catalog(&path, false).unwrap();
    assert_eq!(reopened.startup.previous_shutdown, PreviousShutdown::Clean);
    restarted.close_catalog(false).unwrap();
}

/// 開いているカタログを別の書き方（`..` を含む・シンボリックリンク）で開いても、閉じて開き直さずに
/// 同じものを使う（閉じると、取り込みなどのジョブを取り消してしまう）。バックアップの一覧も、開いている
/// カタログ（コア）のものを返す。
pub fn other_spellings_of_the_open_catalog_reuse_it() {
    let env = Env::new();
    let state = env.state();
    let path = env.catalog("書き方.db");
    state.open_catalog(&path, true).unwrap();
    let core = state.core().unwrap();
    let sub = path.with_file_name("下の フォルダ");
    std::fs::create_dir_all(&sub).unwrap();
    let dotdot = sub.join("..").join("書き方.db");

    let status = state.open_catalog(&dotdot, false).unwrap();
    assert!(
        Arc::ptr_eq(&state.core().unwrap(), &core),
        "`..` を含む書き方でも、開いているカタログをそのまま使う"
    );
    assert_eq!(status.catalog_path, path);
    let backup = match ok(&state, json!({"type": "backup_now"})) {
        Response::Backup(b) => b,
        other => panic!("{other:?}"),
    };
    let listed = state.list_backups(&dotdot).unwrap();
    assert_eq!(
        listed,
        match ok(&state, json!({"type": "list_backups"})) {
            Response::Backups { backups } => backups,
            other => panic!("{other:?}"),
        },
        "開いているカタログのコアの一覧"
    );
    assert!(listed.iter().any(|b| b.path == backup.path));

    #[cfg(unix)]
    {
        let alias = env.root().join("別名.db");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        state.open_catalog(&alias, false).unwrap();
        assert!(
            Arc::ptr_eq(&state.core().unwrap(), &core),
            "シンボリックリンクの別名でも、開いているカタログをそのまま使う"
        );
    }
    drop(core);
    state.close_catalog(false).unwrap();
}

/// 今のフォルダを変えて、テストの終わりに戻す。
struct CurrentDir(PathBuf);

impl CurrentDir {
    fn change_to(dir: &Path) -> Self {
        let before = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        Self(before)
    }
}

impl Drop for CurrentDir {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

/// 相対パスは、今のフォルダから絶対パスにして扱う（カタログ・データのフォルダ・設定・バックアップ・
/// 復元の結果）。テストは 1 つずつ実行するので、今のフォルダを変えてよい。
pub fn relative_paths_are_made_absolute() {
    let env = Env::new();
    let state = env.state();
    let _cwd = CurrentDir::change_to(env.root());
    // macOS の一時フォルダはシンボリックリンク（/var → /private/var）なので、変えた後の今のフォルダで比べる。
    let cwd = std::env::current_dir().unwrap();
    let rel = Path::new("相対 カタログ.db");

    let status = state.open_catalog(rel, true).unwrap();
    let expected = cwd.join("相対 カタログ.db");
    assert_eq!(status.catalog_path, expected);
    assert_eq!(status.data_dir, cwd.join("相対 カタログ-data"));
    assert_eq!(
        state.settings().last_catalog.as_deref(),
        Some(expected.as_path())
    );
    let core = state.core().unwrap();
    state
        .open_catalog(Path::new(".").join(rel).as_path(), false)
        .unwrap();
    assert!(Arc::ptr_eq(&state.core().unwrap(), &core));
    drop(core);

    // 相対パスのバックアップで復元しても、結果は絶対パス。
    let backup = match ok(&state, json!({"type": "backup_now"})) {
        Response::Backup(b) => b,
        other => panic!("{other:?}"),
    };
    let rel_backup = backup.path.strip_prefix(&cwd).unwrap().to_path_buf();
    assert!(rel_backup.is_relative());
    let outcome = state.restore(rel, &rel_backup).unwrap();
    assert!(outcome.open_error.is_none(), "{:?}", outcome.open_error);
    assert_eq!(outcome.backup, backup.path);
    assert!(outcome.displaced.iter().all(|p| p.is_absolute()));
    assert_eq!(outcome.status.unwrap().catalog_path, expected);
    state.close_catalog(false).unwrap();
}

/// 同じカタログを別のアプリ（別の AppState）が開いていれば conflict（案内付き）。開いている側は使える。
pub fn catalog_open_in_another_app_is_a_conflict() {
    let env = Env::new();
    let a = env.state();
    let b = env.state_with(env.root().join("config-b"), env.events.sink());
    let path = env.catalog("共有.db");
    a.open_catalog(&path, true).unwrap();

    let e = assert_kind(b.open_catalog(&path, false), ErrorKind::Conflict);
    assert!(e.hint.is_some(), "{e:?}");
    assert!(b.session().is_none());
    assert!(
        b.settings().last_catalog.is_none(),
        "開けなかったカタログは記録しない"
    );
    ok(&a, json!({"type": "catalog_stats"}));

    a.close_catalog(false).unwrap();
    b.open_catalog(&path, false).unwrap();
    b.close_catalog(false).unwrap();
}

/// 開く・閉じるを同時に呼んでも、1 つずつ行う（同じカタログを同時に開いても 1 つ、ロックの競合で失敗
/// しない）。その間のコマンドの失敗は「閉じている」だけ。
pub fn open_and_close_from_many_threads() {
    let env = Env::new();
    let state = env.state();
    let path = env.catalog("並行.db");

    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| s.spawn(|| state.open_catalog(&path, true)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for r in results {
        assert_eq!(r.unwrap().catalog_path, path);
    }
    let core = state.core().unwrap();
    assert!(Arc::ptr_eq(&state.core().unwrap(), &core));
    drop(core);

    let stop = AtomicBool::new(false);
    let kinds: HashSet<ErrorKind> = std::thread::scope(|s| {
        let reader = s.spawn(|| {
            let mut kinds = HashSet::new();
            while !stop.load(Ordering::SeqCst) {
                if let Err(e) = exec(&state, json!({"type": "catalog_stats"})) {
                    kinds.insert(e.kind);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            kinds
        });
        for round in 0..2 {
            let handles: Vec<_> = (0..6)
                .map(|i| {
                    let (state, path) = (&state, &path);
                    s.spawn(move || {
                        if (i + round) % 2 == 0 {
                            state.open_catalog(path, false).map(|_| ())
                        } else {
                            state.close_catalog(false).map_err(|e| e.info())
                        }
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap().unwrap();
            }
        }
        stop.store(true, Ordering::SeqCst);
        reader.join().unwrap()
    });
    assert!(
        kinds.iter().all(|k| *k == ErrorKind::Closed),
        "閉じている以外の失敗: {kinds:?}"
    );
    state.open_catalog(&path, false).unwrap();
    ok(&state, json!({"type": "catalog_stats"}));
    state.close_catalog(false).unwrap();
}

/// 閉じる途中に届いた要求（コマンド・サムネイル・画素）は、成功するか「閉じている」（URI スキームは 503）
/// だけで、内部のエラーにならない。
pub fn requests_while_closing_get_closed_errors() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("閉じる 途中");
    for i in 0..3u8 {
        write_image(&photos.join(format!("写真 {i}.jpg")), (64, 48), i);
    }
    state.open_catalog(&env.catalog("閉じる.db"), true).unwrap();
    import(&state, &photos);
    let ids: Vec<i64> = all_items(&state)
        .iter()
        .map(|s| s.variant_id.get())
        .collect();

    let stop = AtomicBool::new(false);
    let (statuses, kinds) = std::thread::scope(|s| {
        let thumbs = s.spawn(|| {
            let mut statuses = HashSet::new();
            while !stop.load(Ordering::SeqCst) {
                for id in &ids {
                    statuses.insert(protocol::reply(&state, &format!("/thumb/{id}"), None).status);
                }
            }
            statuses
        });
        let commands = s.spawn(|| {
            let mut kinds = HashSet::new();
            while !stop.load(Ordering::SeqCst) {
                for id in &ids {
                    let r = exec(&state, json!({"type": "variant_details", "variant_id": id}));
                    if let Err(e) = r {
                        kinds.insert(e.kind);
                    }
                }
                if let Err(e) = commands::develop_frame_bytes(&state) {
                    kinds.insert(e.kind);
                }
            }
            kinds
        });
        std::thread::sleep(Duration::from_millis(100));
        state.close_catalog(false).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::SeqCst);
        (thumbs.join().unwrap(), commands.join().unwrap())
    });
    assert!(statuses.contains(&200), "{statuses:?}");
    assert!(
        statuses.iter().all(|s| matches!(s, 200 | 503)),
        "閉じる途中のサムネイルは 503: {statuses:?}"
    );
    assert!(
        kinds.iter().all(|k| *k == ErrorKind::Closed),
        "閉じている以外の失敗: {kinds:?}"
    );
}

/// 現像設定の露光量だけを変えた設定。
fn exposure(state: &genzo_app::state::AppState, ev: f32) -> genzo_api::DevelopSettings {
    let mut s = match ok(state, json!({"type": "develop_state"})) {
        Response::DevelopOrNone { state: Some(s) } => s.settings,
        other => panic!("{other:?}"),
    };
    s.exposure_ev = ev;
    s
}

/// 写真を 1 枚取り込んで現像を開き、現像設定の保存だけを失敗させてから、露光量 +0.5 を保存待ちにする。
fn open_with_failing_saves(env: &Env, path: &Path) -> (genzo_app::state::AppState, i64) {
    let state = env.state();
    let photos = env.photos("保存 できない");
    write_image(&photos.join("一 枚.jpg"), (64, 48), 4);
    state.open_catalog(path, true).unwrap();
    import(&state, &photos);
    let id = all_items(&state)[0].variant_id.get();
    ok(&state, json!({"type": "open_develop", "variant_id": id}));
    // 取り込み（最初の履歴を作る）の後で、現像設定の書き込みだけを失敗させる（時間によらず、自動保存も
    // 終了のときの保存も失敗する）。
    genzo_catalog::testing::fail_develop_writes(path).unwrap();
    let settings = exposure(&state, 0.5);
    ok(
        &state,
        json!({"type": "set_settings", "settings": settings, "dragging": false}),
    );
    (state, id)
}

fn saved_exposure(state: &genzo_app::state::AppState, id: i64) -> f32 {
    match ok(state, json!({"type": "copy_settings", "variant_id": id})) {
        Response::Settings { settings } => settings.exposure_ev,
        other => panic!("{other:?}"),
    }
}

/// 決定事項ログ No.61: 閉じるときに保存待ちの現像設定を保存できなければ、閉じずに（still_open）エラーを返す。
/// その間もカタログは使え、イベントも届く。原因を取り除いて再試行すれば保存して閉じ、捨てて閉じることも
/// できる（次に開いたとき、前回の異常終了として扱う）。
pub fn failed_save_keeps_the_catalog_open_until_retry_or_discard() {
    let env = Env::new();
    let path = env.catalog("保存.db");

    // 再試行: 原因がなくなれば、保存して閉じる。
    let (state, id) = open_with_failing_saves(&env, &path);
    let core = state.core().unwrap();
    for _ in 0..2 {
        let e = state
            .close_catalog(false)
            .expect_err("保存できないのに閉じた");
        assert!(e.still_open(), "{e:?}");
        let info = e.info();
        assert!(
            info.hint.as_deref().is_some_and(|h| h.contains("再試行")),
            "{info:?}"
        );
        assert!(Arc::ptr_eq(&state.core().unwrap(), &core), "開いたまま");
        assert!(!core.is_closed());
    }
    let mark = env.events.mark();
    ok(
        &state,
        json!({"type": "set_rating", "variant_ids": [id], "rating": 2}),
    );
    env.events
        .wait_for(mark, "閉じられなかった後のイベント", |e| {
            matches!(e, genzo_api::Event::CatalogChanged { .. })
        });
    drop(core);
    genzo_catalog::testing::allow_develop_writes(&path).unwrap();
    state.close_catalog(false).unwrap();
    let reopened = state.open_catalog(&path, false).unwrap();
    assert_eq!(reopened.startup.previous_shutdown, PreviousShutdown::Clean);
    assert_eq!(saved_exposure(&state, id), 0.5, "保存してから閉じた");
    state.close_catalog(false).unwrap();
    drop(state);

    // 捨てて閉じる: 保存できないまま閉じ、ロックを外す（すぐに開き直せる）。
    let env2 = Env::new();
    let path2 = env2.catalog("捨てる.db");
    let (state, id) = open_with_failing_saves(&env2, &path2);
    assert!(state.close_catalog(false).unwrap_err().still_open());
    // ほかのスレッド（genzo:// の処理など）がコアを持っていても、ここで閉じる（参照が外れるのを待たない）。
    let held = state.core().unwrap();
    state.close_catalog(true).unwrap();
    assert!(held.is_closed(), "ほかの参照が残っていても閉じた");
    assert!(state.session().is_none());
    assert_kind(state.core(), ErrorKind::Closed);
    genzo_catalog::testing::allow_develop_writes(&path2).unwrap();
    let reopened = state.open_catalog(&path2, false).unwrap();
    drop(held);
    assert_eq!(
        reopened.startup.previous_shutdown,
        PreviousShutdown::Unclean,
        "保存できなかった編集があるので、正常に終了した印を付けない"
    );
    assert_eq!(
        saved_exposure(&state, id),
        0.0,
        "保存しなかった設定は捨てた"
    );
    state.close_catalog(false).unwrap();
}

/// 閉じる処理が「閉じた後の段階のエラー」（ここではワーカーがいなくなっていて、終了を頼めない）を返したら、
/// カタログは閉じている（still_open ではない。ロックも外れていて、開き直せる）。別のカタログを開くときに
/// 前のカタログがこのエラーで閉じても、新しいカタログを開く（止めると、閉じたまま何も開かずに返す）。
pub fn errors_after_the_catalog_closed_do_not_stop_opening_another() {
    let env = Env::new();
    let switch = ExitSwitch::new(&env.root().join("合図"));
    let state = env.state_with_worker(env.config_dir(), env.events.sink(), switch.launch());
    let photos = env.photos("閉じる エラー");
    write_image(&photos.join("一 枚.jpg"), (64, 48), 6);
    let first = env.catalog("一つ目.db");
    let second = env.catalog("二つ目.db");

    // 閉じるだけ: エラーを返すが、閉じている。
    state.open_catalog(&first, true).unwrap();
    import(&state, &photos);
    let core = state.core().unwrap();
    switch.exit_all();
    let e = state
        .close_catalog(false)
        .expect_err("ワーカーに終了を頼めないのに、エラーにならない");
    assert!(!e.still_open(), "{e:?}");
    assert!(core.is_closed());
    assert!(state.session().is_none());
    assert_kind(state.core(), ErrorKind::Closed);
    drop(core);

    // 別のカタログを開く: 前のカタログが同じエラーで閉じても、新しいカタログを開く。
    state.open_catalog(&first, false).unwrap();
    import(&state, &photos);
    let core = state.core().unwrap();
    switch.exit_all();
    let status = state.open_catalog(&second, true).unwrap();
    assert_eq!(status.catalog_path, second);
    assert!(core.is_closed(), "前のカタログは閉じた");
    assert!(!Arc::ptr_eq(&state.core().unwrap(), &core));
    assert_eq!(
        state.settings().last_catalog.as_deref(),
        Some(second.as_path())
    );
    drop(core);
    state.close_catalog(false).unwrap();
    // 前のカタログのロックは外れている。
    state.open_catalog(&first, false).unwrap();
    assert_eq!(all_items(&state).len(), 1);
    state.close_catalog(false).unwrap();
}

/// アプリが終わる間際（lib.rs の `RunEvent::Exit`。macOS の Dock の「終了」・ログアウト、Windows の
/// シャットダウン・サインアウトなど、終了の要求を経ずに終わる場合。[`AppState::close_at_exit`]）に、閉じて
/// いないカタログを閉じる: 保存待ちの現像設定を保存し、終了時のバックアップを作り、正常に終了した印を付けて、
/// ロックを外す。保存できなければ、開いたままにせず捨てて閉じる（次に開いたとき、前回の異常終了として扱う）。
///
/// [`AppState::close_at_exit`]: genzo_app::state::AppState::close_at_exit
pub fn exit_closes_the_catalog_left_open() {
    let env = Env::new();
    let path = env.catalog("終了.db");
    let state = env.state();
    // 開いていなければ何もしない。
    state.close_at_exit().unwrap();

    state.update_settings(|s| s.backup_on_close = true).unwrap();
    let photos = env.photos("終了");
    write_image(&photos.join("一 枚.jpg"), (64, 48), 5);
    state.open_catalog(&path, true).unwrap();
    import(&state, &photos);
    let id = all_items(&state)[0].variant_id.get();
    ok(&state, json!({"type": "open_develop", "variant_id": id}));
    let settings = exposure(&state, 0.5);
    let backups = state.list_backups(&path).unwrap().len();
    let core = state.core().unwrap();
    ok(
        &state,
        json!({"type": "set_settings", "settings": settings, "dragging": false}),
    );
    // 自動保存（1 秒後）を待たずに終わる。
    state.close_at_exit().unwrap();
    assert!(core.is_closed());
    assert!(state.session().is_none());
    assert_kind(state.core(), ErrorKind::Closed);
    drop(core);
    assert_eq!(
        state.list_backups(&path).unwrap().len(),
        backups + 1,
        "終了時のバックアップを作った"
    );
    // 別のアプリとして開き直せる（ロックが外れている）。
    let next = env.state();
    let reopened = next.open_catalog(&path, false).unwrap();
    assert_eq!(reopened.startup.previous_shutdown, PreviousShutdown::Clean);
    assert_eq!(
        saved_exposure(&next, id),
        0.5,
        "保存待ちの現像設定を保存してから閉じた"
    );
    next.close_catalog(false).unwrap();
    drop((state, next));

    // 保存できない: 開いたままにせず、捨てて閉じる。
    let env = Env::new();
    let path = env.catalog("終了 保存できない.db");
    let (state, id) = open_with_failing_saves(&env, &path);
    let core = state.core().unwrap();
    state.close_at_exit().unwrap();
    assert!(core.is_closed(), "保存できなくても閉じた");
    assert!(state.session().is_none());
    drop(core);
    genzo_catalog::testing::allow_develop_writes(&path).unwrap();
    let next = env.state();
    let reopened = next.open_catalog(&path, false).unwrap();
    assert_eq!(
        reopened.startup.previous_shutdown,
        PreviousShutdown::Unclean,
        "保存できなかった編集があるので、正常に終了した印を付けない"
    );
    assert_eq!(
        saved_exposure(&next, id),
        0.0,
        "保存できなかった設定は捨てた"
    );
    next.close_catalog(false).unwrap();
}
