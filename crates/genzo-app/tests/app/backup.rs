//! (d) バックアップと復元: BackupNow → 閉じる → カタログのファイルを壊す → 開けない（catalog_corrupt）→
//! バックアップの一覧（ファイル名で探す）→ 復元して開き直す・退避したファイルが残る。復元の最中のほかの
//! 要求・別のアプリが開いているカタログの復元・閉じる処理の途中のエラー。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use genzo_api::{BackupEntry, CatalogStats, ErrorKind, Response};
use genzo_app::catalog_file::{self, CatalogFileState};
use genzo_app::protocol;
use genzo_app::state::{AppState, DisplayTarget};
use serde_json::json;

use crate::support::{Env, ExitSwitch, all_items, assert_kind, exec, import, ok, write_image};

fn backup_now(state: &AppState) -> BackupEntry {
    match ok(state, json!({"type": "backup_now"})) {
        Response::Backup(b) => b,
        other => panic!("{other:?}"),
    }
}

fn stats(state: &AppState) -> CatalogStats {
    match ok(state, json!({"type": "catalog_stats"})) {
        Response::CatalogStats(s) => s,
        other => panic!("{other:?}"),
    }
}

/// 退避したカタログの本体（`-wal` などの付随するファイルを除く）。
fn displaced_main(displaced: &[PathBuf]) -> &Path {
    displaced
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "db"))
        .unwrap_or_else(|| panic!("退避したカタログがない: {displaced:?}"))
}

/// カタログのフォルダに、復元の途中のファイル（`*-restored-*.db`）が残っていないこと。
fn assert_no_restored_copy(path: &Path) {
    let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n.contains("-restored-")),
        "復元の途中のファイルを残さない: {names:?}"
    );
}

/// 写真 2 枚を取り込んでバックアップを作り、さらに 1 枚取り込んだカタログ。
fn catalog_with_backup(env: &Env, state: &AppState, path: &Path) -> BackupEntry {
    let first = env.photos("一回目");
    write_image(&first.join("朝 1.jpg"), (64, 48), 1);
    write_image(&first.join("朝 2.png"), (48, 64), 2);
    let second = env.photos("二回目");
    write_image(&second.join("夜 1.jpg"), (64, 48), 3);
    state.open_catalog(path, true).unwrap();
    import(state, &first);
    let backup = backup_now(state);
    assert!(backup.path.is_file());
    let data_dir = catalog_file::data_dir_for(path);
    assert_eq!(
        backup.path.parent(),
        Some(data_dir.join("backups").as_path())
    );
    import(state, &second);
    assert_eq!(stats(state).assets, 3);
    backup
}

/// 壊れて開けないカタログを、バックアップから復元して開き直す。壊れたファイルは消さずに退避する。
pub fn corrupt_catalog_is_restored_from_a_backup() {
    let env = Env::new();
    let state = env.state();
    let path = env.catalog("仕事 用.db");
    let backup = catalog_with_backup(&env, &state, &path);
    state.close_catalog(false).unwrap();

    // 壊す（SQLite の先頭を書き換える）。
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[..16].copy_from_slice(b"GARBAGE-GARBAGE!");
    std::fs::write(&path, &bytes).unwrap();

    // 開けない: 壊れたカタログとして、バックアップからの復元を案内する（作り直さない）。
    for create in [false, true] {
        let e = assert_kind(state.open_catalog(&path, create), ErrorKind::CatalogCorrupt);
        assert!(
            e.hint
                .as_deref()
                .is_some_and(|h| h.contains("バックアップ")),
            "{e:?}"
        );
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "壊れたファイルを変えない"
    );
    assert!(state.session().is_none());

    // 画面の事前の確認（inspect_catalog_path）も、開く処理と同じくカタログとして返す（画面は開こうとして、
    // catalog_corrupt で復元を案内する。「カタログではありません」で断ると、復元の案内に届かない）。
    // ファイルの先頭だけの判定はカタログではないとする。
    assert_eq!(
        catalog_file::inspect(&path).unwrap(),
        CatalogFileState::NotCatalog
    );
    assert_eq!(
        state.inspect_catalog(&path).unwrap(),
        CatalogFileState::Catalog
    );
    // データのフォルダのない、カタログでないファイルは、カタログではない（開く処理も invalid_input）。
    let memo = env.root().join("メモ.db");
    std::fs::write(&memo, &bytes).unwrap();
    assert_eq!(
        state.inspect_catalog(&memo).unwrap(),
        CatalogFileState::NotCatalog
    );
    assert_kind(state.open_catalog(&memo, false), ErrorKind::InvalidInput);
    assert_eq!(
        state.inspect_catalog(&env.root().join("ない.db")).unwrap(),
        CatalogFileState::Missing
    );

    // バックアップの一覧（カタログを読めないので、ファイル名で探す。新しい順）。
    let list = state.list_backups(&path).unwrap();
    assert!(list.iter().any(|b| b.path == backup.path), "{list:?}");
    assert!(list.windows(2).all(|w| w[0].created_at >= w[1].created_at));

    // 復元して開き直す。
    let outcome = state.restore(&path, &backup.path).unwrap();
    assert!(outcome.open_error.is_none(), "{:?}", outcome.open_error);
    let status = outcome.status.expect("開き直す");
    assert_eq!(status.catalog_path, path);
    assert_eq!(status.stats.assets, 2, "バックアップの時点の内容");
    assert_eq!(all_items(&state).len(), 2);
    assert_eq!(outcome.backup, backup.path);
    assert!(backup.path.is_file(), "バックアップは消さない");
    // 退避したファイルが残る（壊れたままの内容）。
    let displaced = displaced_main(&outcome.displaced);
    assert!(
        displaced
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("before-restore"),
        "{displaced:?}"
    );
    assert_eq!(std::fs::read(displaced).unwrap(), bytes);
    assert_eq!(
        state.settings().last_catalog.as_deref(),
        Some(path.as_path())
    );
    state.close_catalog(false).unwrap();
    assert!(displaced.is_file(), "閉じても退避したファイルは残る");
}

/// 開いているカタログを復元すると、閉じてから差し替えて開き直す（今のカタログは退避して残す）。
pub fn restoring_the_open_catalog_closes_and_reopens_it() {
    let env = Env::new();
    let state = env.state();
    let path = env.catalog("開いた まま.db");
    let backup = catalog_with_backup(&env, &state, &path);
    let before = state.core().unwrap();

    let outcome = state.restore(&path, &backup.path).unwrap();
    assert!(outcome.open_error.is_none(), "{:?}", outcome.open_error);
    assert_eq!(outcome.status.unwrap().stats.assets, 2);
    assert!(before.is_closed(), "前のコアは閉じた");
    assert!(!Arc::ptr_eq(&state.core().unwrap(), &before));
    let displaced = displaced_main(&outcome.displaced);
    assert_eq!(
        catalog_file::inspect(displaced).unwrap(),
        CatalogFileState::Catalog,
        "退避した（復元の前の）カタログはそのまま残る"
    );
    state.close_catalog(false).unwrap();
}

/// 復元しないもの: カタログでないファイル（データのフォルダもない）・ないバックアップ・バックアップでない
/// ファイル。どれも何も変えず、開いているカタログは開いたまま。
pub fn restore_refuses_files_that_are_not_catalogs() {
    let env = Env::new();
    let state = env.state();
    let path = env.catalog("守る.db");
    let backup = catalog_with_backup(&env, &state, &path);
    let core = state.core().unwrap();

    let other = env.root().join("写真 一覧.db");
    std::fs::write(&other, "カタログではない").unwrap();
    let e = assert_kind(state.restore(&other, &backup.path), ErrorKind::InvalidInput);
    assert!(e.message.contains("カタログではありません"), "{e:?}");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "カタログではない");

    assert_kind(
        state.restore(&path, &env.root().join("ない.db")),
        ErrorKind::NotFound,
    );

    // バックアップでないファイル（写真）を選んだ: 復元できず、開いているカタログは閉じない。
    let photo = env.photos("一回目").join("朝 1.jpg");
    let e = state.restore(&path, &photo).unwrap_err();
    assert_eq!(e.kind, ErrorKind::CatalogCorrupt, "{e:?}");
    assert!(
        e.hint
            .as_deref()
            .is_some_and(|h| h.contains("別のバックアップ")),
        "{e:?}"
    );
    assert!(
        Arc::ptr_eq(&state.core().unwrap(), &core),
        "選んだバックアップが使えなければ、開いているカタログを閉じない"
    );
    assert_eq!(stats(&state).assets, 3, "カタログは変えない");
    assert_no_restored_copy(&path);
    drop(core);
    state.close_catalog(false).unwrap();
}

/// 別のアプリ（別の AppState。CLI も同じロック）が開いているカタログは差し替えない（conflict）。カタログは
/// 変えず、復元した写しも残さない（やり直すたびに写しが増えないように）。別のアプリが閉じれば復元できる。
pub fn restore_waits_for_another_app_to_close_the_catalog() {
    let env = Env::new();
    let other = env.state();
    let path = env.catalog("別の アプリ.db");
    let backup = catalog_with_backup(&env, &other, &path);
    let state = env.state_with(env.root().join("config-b"), env.events.sink());

    for _ in 0..2 {
        let e = assert_kind(state.restore(&path, &backup.path), ErrorKind::Conflict);
        assert!(
            e.hint
                .as_deref()
                .is_some_and(|h| h.contains("終了してから") && h.contains("変更していません")),
            "{e:?}"
        );
        assert_no_restored_copy(&path);
    }
    assert!(state.session().is_none());
    assert_eq!(
        stats(&other).assets,
        3,
        "開いている側のカタログは変わらない"
    );

    other.close_catalog(false).unwrap();
    let outcome = state.restore(&path, &backup.path).unwrap();
    assert!(outcome.open_error.is_none(), "{:?}", outcome.open_error);
    assert_eq!(outcome.status.unwrap().stats.assets, 2);
    assert_no_restored_copy(&path);
    // 開いた側（state）が持っているので、もう一方は開けない。
    assert_kind(other.open_catalog(&path, false), ErrorKind::Conflict);
    state.close_catalog(false).unwrap();
}

/// 復元の最中に届いた要求: コマンド・`genzo://`・画面の色空間の指定・状態の取得は、成功するか「閉じている」
/// （URI スキームは 503）だけ。開く・バックアップの一覧・事前の確認は、復元が終わるまで待って成功する。
/// 終わった後は、復元したカタログが開いていて、画面の色空間の指定も効く。
pub fn requests_during_a_restore_wait_or_get_closed_errors() {
    let env = Env::new();
    let state = env.state();
    let path = env.catalog("復元 中.db");
    let backup = catalog_with_backup(&env, &state, &path);
    let ids: Vec<i64> = all_items(&state)
        .iter()
        .map(|s| s.variant_id.get())
        .collect();

    let stop = AtomicBool::new(false);
    let (statuses, kinds, waited) = std::thread::scope(|s| {
        let thumbs = s.spawn(|| {
            let mut statuses = HashSet::new();
            while !stop.load(Ordering::SeqCst) {
                for id in &ids {
                    statuses.insert(protocol::reply(&state, &format!("/thumb/{id}"), None).status);
                }
            }
            statuses
        });
        let requests = s.spawn(|| {
            let mut kinds = HashSet::new();
            let mut p3 = false;
            while !stop.load(Ordering::SeqCst) {
                let mut errors = vec![
                    exec(&state, json!({"type": "catalog_stats"})).err(),
                    exec(&state, json!({"type": "search"})).err(),
                    state.status().err(),
                ];
                p3 = !p3;
                let target = if p3 {
                    DisplayTarget::DisplayP3
                } else {
                    DisplayTarget::Srgb
                };
                errors.push(state.set_display_target(target).err());
                kinds.extend(errors.into_iter().flatten().map(|e| e.kind));
                std::thread::sleep(Duration::from_millis(1));
            }
            kinds
        });
        // 開く・一覧・事前の確認は、復元と同じロックで待つ（どれも成功する）。
        let waiting = s.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            let opened = state.open_catalog(&path, false).map(|s| s.catalog_path);
            let listed = state.list_backups(&path);
            let inspected = state.inspect_catalog(&path);
            (opened, listed, inspected)
        });
        std::thread::sleep(Duration::from_millis(10));
        let outcome = state.restore(&path, &backup.path).unwrap();
        assert!(outcome.open_error.is_none(), "{:?}", outcome.open_error);
        assert_eq!(outcome.status.unwrap().stats.assets, 2);
        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::SeqCst);
        (
            thumbs.join().unwrap(),
            requests.join().unwrap(),
            waiting.join().unwrap(),
        )
    });
    assert!(statuses.contains(&200), "{statuses:?}");
    // 3 枚目は復元したカタログにない（404）。
    assert!(
        statuses.iter().all(|s| matches!(s, 200 | 404 | 503)),
        "復元の最中のサムネイルは 503: {statuses:?}"
    );
    assert!(
        kinds.iter().all(|k| *k == ErrorKind::Closed),
        "閉じている以外の失敗: {kinds:?}"
    );
    let (opened, listed, inspected) = waited;
    assert_eq!(opened.unwrap(), path);
    assert!(
        listed.unwrap().iter().any(|b| b.path == backup.path),
        "復元に使ったバックアップは残る"
    );
    assert_eq!(inspected.unwrap(), CatalogFileState::Catalog);
    assert_eq!(stats(&state).assets, 2, "復元したカタログが開いている");
    assert_no_restored_copy(&path);
    let info = state
        .set_display_target(DisplayTarget::DisplayP3)
        .unwrap()
        .expect("開いている");
    assert_eq!(state.status().unwrap().unwrap().display, info);
    state.close_catalog(false).unwrap();
}

/// 開いているカタログを復元するとき、閉じる処理が「閉じた後の段階のエラー」（ここではワーカーがいなくなって
/// いて、終了を頼めない）を返しても、カタログはもう閉じているので、差し替えて開き直す（止めると、閉じたまま
/// 何もしないで返す）。
pub fn restore_goes_on_when_closing_fails_after_the_catalog_closed() {
    let env = Env::new();
    let switch = ExitSwitch::new(&env.root().join("合図"));
    let state = env.state_with_worker(env.config_dir(), env.events.sink(), switch.launch());
    let path = env.catalog("閉じる エラー.db");
    let backup = catalog_with_backup(&env, &state, &path);
    let before = state.core().unwrap();
    switch.exit_all();

    let outcome = state.restore(&path, &backup.path).unwrap();
    assert!(before.is_closed(), "前のコアは閉じた");
    assert!(outcome.open_error.is_none(), "{:?}", outcome.open_error);
    assert_eq!(outcome.status.unwrap().stats.assets, 2);
    assert!(!outcome.displaced.is_empty());
    assert_eq!(all_items(&state).len(), 2);
    assert_no_restored_copy(&path);
    drop(before);
    state.close_catalog(false).unwrap();
}
