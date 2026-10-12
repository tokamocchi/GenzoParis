//! (d) バックアップと復元: BackupNow → 閉じる → カタログのファイルを壊す → 開けない（catalog_corrupt）→
//! バックアップの一覧（ファイル名で探す）→ 復元して開き直す・退避したファイルが残る。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use genzo_api::{BackupEntry, CatalogStats, ErrorKind, Response};
use genzo_app::catalog_file::{self, CatalogFileState};
use genzo_app::state::AppState;
use serde_json::json;

use crate::support::{Env, all_items, assert_kind, import, ok, write_image};

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
    let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n.contains("restored")),
        "復元の途中のファイルを残さない: {names:?}"
    );
    drop(core);
    state.close_catalog(false).unwrap();
}
