//! ボリュームの識別（外付けドライブ。FILE-03）と、つながっているかの確認。04 の 3.3 節・3.14 節。
//!
//! - **ボリュームの ID**: OS のボリューム ID（`os` の doc。Linux・macOS は `uuid:`、Windows は `winvol:`、
//!   ネットワークのボリュームは `net:`）。取れない（コンテナの中・tmpfs・UNC の共有・割り当てたネットワーク
//!   ドライブなど）ときは、MVP と同じくパスのマウント先から作る（`path:`。`paths::split_volume`）。
//! - **場所**: フォルダはボリュームの中の相対パスで持つので、ドライブ文字・マウント先が変わっても、同じ ID の
//!   ボリュームが見つかれば、そのマウント先で同じファイルを指す（[`refresh_volumes`] が `last_mount_path` を
//!   新しいマウント先にする）。
//! - **つながっていないボリューム**: 記録したマウント先に同じ ID のボリュームがなく、つながっているボリュームの
//!   中にもなければ、つながっていないとし、カタログの場所の問い合わせ（genzo-catalog の
//!   `Catalog::set_offline_volumes`）でマウント先を `None` にする（記録したマウント先に別のドライブがつながって
//!   いても、そのファイルを読まない・見つからないと記録しない）。`path:` のボリュームは ID で確かめられないので、
//!   マウント先のフォルダがあればつながっているとする（MVP と同じ）。
//! - **以前の形式のボリュームの移し替え**（[`rebind_path_volumes`]）: 版 11 までの `path:` のボリュームの
//!   フォルダのうち、いま見えて、登録したファイルが（あれば）そこにあるものを、OS のボリューム ID のボリュームへ
//!   移す（カタログを開いたとき。移せなかったものは、次に開いたとき・ボリュームを確かめ直したときに移す）。
//!
//! Windows・macOS の ID の取り方は実機で未確認（`os` の doc）。

mod os;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use genzo_catalog::{Catalog, FolderRebind, RebindReport};
use genzo_model::VolumeId;

use crate::error::ApiError;
use crate::paths::{PATH_VOLUME_PREFIX, VolumePath, join_rel, split_volume};

pub use os::OsVolume as VolumeIdentity;

/// ボリュームの問い合わせ（[`crate::CoreHooks::volumes`]。テストでは偽物に差し替えて、ドライブの付け外し・
/// マウント先の変化を確かめる）。
pub trait VolumeProbe: Send + Sync + 'static {
    /// 実在するパス `path` のボリューム（OS のボリューム ID を取れなければ `None`。呼び出し側がパスの
    /// マウント先から `path:` の ID を作る）。
    fn identify(&self, path: &Path) -> Option<VolumeIdentity>;
    /// つながっているボリュームの一覧（つながっていないとされたボリュームの、新しいマウント先を探す）。
    fn mounted(&self) -> Vec<VolumeIdentity>;
}

/// OS のボリューム ID（`os` の doc）。
#[derive(Debug, Clone, Copy, Default)]
pub struct OsVolumes;

impl VolumeProbe for OsVolumes {
    fn identify(&self, path: &Path) -> Option<VolumeIdentity> {
        os::identify(path)
    }

    fn mounted(&self) -> Vec<VolumeIdentity> {
        os::mounted()
    }
}

/// ボリュームのつながりの状態（コアのメモリだけ。[`refresh_volumes`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VolumeState {
    /// つながっているか。
    pub online: bool,
    /// ネットワークのボリュームか（分からなければ `false`）。
    pub network: bool,
}

/// パスの部分を比べる（Windows はドライブ文字・名前の大文字・小文字を区別しない）。
fn same_component(a: Component<'_>, b: Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// `path` がマウント先 `mount` の中にあれば、残りの部分（UTF-8 の名前）を返す。
fn strip_mount(path: &Path, mount: &Path) -> Option<Vec<String>> {
    let mut p = path.components();
    for m in mount.components() {
        let c = p.next()?;
        if !same_component(c, m) {
            return None;
        }
    }
    p.map(|c| match c {
        Component::Normal(s) => s.to_str().map(str::to_owned),
        _ => None,
    })
    .collect()
}

/// 2 つのパスが同じ場所を表すか（[`same_component`] で比べる。末尾の区切りは無視）。
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let (mut x, mut y) = (a.components(), b.components());
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(c), Some(d)) if same_component(c, d) => {}
            _ => return false,
        }
    }
}

/// 絶対パス（正規化済み）を、ボリュームとボリュームの中の相対パスに分ける（FILE-03）。
///
/// まだないパス（作るフォルダ・コピー先）は、実在する最も近い親のボリュームとする。OS のボリューム ID を取れない、
/// またはパスがマウント先の中にない（リンクを解決しても）ときは、パスの先頭から作る（`path:`）。
pub(crate) fn locate(probe: &dyn VolumeProbe, abs: &Path) -> Result<VolumePath, ApiError> {
    let mut existing = abs.to_path_buf();
    let mut rest: Vec<String> = Vec::new();
    while std::fs::symlink_metadata(&existing).is_err() {
        match (existing.file_name(), existing.parent()) {
            (Some(n), Some(p)) => {
                let Some(n) = n.to_str() else {
                    return split_volume(abs);
                };
                rest.push(n.to_owned());
                existing = p.to_path_buf();
            }
            _ => return split_volume(abs),
        }
    }
    let Some(id) = probe.identify(&existing) else {
        return split_volume(abs);
    };
    let parts = strip_mount(&existing, &id.mount).or_else(|| {
        std::fs::canonicalize(&existing)
            .ok()
            .and_then(|c| strip_mount(&c, &id.mount))
    });
    let Some(mut parts) = parts else {
        return split_volume(abs);
    };
    parts.extend(rest.into_iter().rev());
    let rel = genzo_catalog::normalize_rel_path(&parts.join("/"))?;
    Ok(VolumePath {
        uuid: id.uuid,
        mount: id.mount,
        rel,
        label: id.label,
        fs_type: id.fs_type,
    })
}

/// マウント先の文字列（UTF-8 で表せなければエラー）。
pub(crate) fn mount_str(mount: &Path) -> Result<String, ApiError> {
    mount
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| ApiError::InvalidArgument("UTF-8 で表せないパスです".to_owned()))
}

/// カタログのボリュームのつながりを確かめ、マウント先と最後につながった日時を記録し、つながっていない
/// ボリュームをカタログに知らせる（モジュールの doc）。ボリュームごとの状態を返す。
pub(crate) fn refresh_volumes(
    cat: &mut Catalog,
    probe: &dyn VolumeProbe,
) -> Result<HashMap<VolumeId, VolumeState>, ApiError> {
    let mut mounted: Option<Vec<VolumeIdentity>> = None;
    let mut offline = Vec::new();
    let mut out = HashMap::new();
    for v in cat.volume_records()? {
        let found: Option<(PathBuf, Option<String>, Option<String>, bool)> =
            if v.uuid.starts_with(PATH_VOLUME_PREFIX) {
                v.last_mount_path
                    .as_deref()
                    .filter(|m| Path::new(m).is_dir())
                    .map(|m| (PathBuf::from(m), None, None, false))
            } else {
                let here = v.last_mount_path.as_deref().and_then(|m| {
                    probe
                        .identify(Path::new(m))
                        .filter(|id| id.uuid == v.uuid && same_path(&id.mount, Path::new(m)))
                });
                here.or_else(|| {
                    mounted
                        .get_or_insert_with(|| probe.mounted())
                        .iter()
                        .find(|id| id.uuid == v.uuid)
                        .cloned()
                })
                .map(|id| (id.mount, id.label, id.fs_type, id.network))
            };
        match found {
            Some((mount, label, fs_type, network)) => {
                cat.note_volume_seen(
                    v.id,
                    &mount_str(&mount)?,
                    label.as_deref(),
                    fs_type.as_deref(),
                )?;
                out.insert(
                    v.id,
                    VolumeState {
                        online: true,
                        network,
                    },
                );
            }
            None => {
                offline.push(v.id);
                out.insert(
                    v.id,
                    VolumeState {
                        online: false,
                        network: false,
                    },
                );
            }
        }
    }
    cat.set_offline_volumes(offline);
    Ok(out)
}

/// フォルダに登録したファイルのうち、1 つでも記録と同じ大きさで今の場所にあるか（ファイルのないフォルダは
/// `true`）。以前の形式のボリュームは、同じマウント先に別のドライブがつながっていても区別できないので、移す前に
/// 確かめる。
fn has_registered_file_here(cat: &Catalog, folder: genzo_model::FolderId, dir: &Path) -> bool {
    let Ok(files) = cat.files_in_folder(folder) else {
        return false;
    };
    files.is_empty()
        || files.iter().any(|f| {
            std::fs::metadata(dir.join(&f.name))
                .is_ok_and(|m| m.is_file() && m.len() == f.facts.size)
        })
}

/// 以前の形式（`path:`）のボリュームのフォルダを、OS のボリューム ID のボリュームへ移す（モジュールの doc）。
/// 移すものがなければ `None`。
pub(crate) fn rebind_path_volumes(
    cat: &mut Catalog,
    probe: &dyn VolumeProbe,
) -> Result<Option<RebindReport>, ApiError> {
    let volumes: HashMap<VolumeId, genzo_catalog::VolumeRecord> = cat
        .volume_records()?
        .into_iter()
        .filter(|v| v.uuid.starts_with(PATH_VOLUME_PREFIX))
        .map(|v| (v.id, v))
        .collect();
    if volumes.is_empty() {
        return Ok(None);
    }
    let mut rebinds = Vec::new();
    let mut targets: HashMap<String, VolumeId> = HashMap::new();
    for r in cat.folder_records()? {
        let Some(v) = volumes.get(&r.folder.volume_id) else {
            continue;
        };
        let Some(mount) = v.last_mount_path.as_deref() else {
            continue;
        };
        let dir = join_rel(Path::new(mount), &r.folder.rel_path, "");
        if !dir.is_dir() || !has_registered_file_here(cat, r.folder.id, &dir) {
            continue;
        }
        let vp = locate(probe, &dir)?;
        if vp.uuid == v.uuid && vp.rel == r.folder.rel_path {
            continue;
        }
        let target = match targets.get(&vp.uuid) {
            Some(&id) => id,
            None => {
                let m = mount_str(&vp.mount)?;
                let id = cat.ensure_volume(&vp.uuid, vp.label.as_deref(), Some(&m))?;
                cat.note_volume_seen(id, &m, vp.label.as_deref(), vp.fs_type.as_deref())?;
                targets.insert(vp.uuid.clone(), id);
                id
            }
        };
        rebinds.push(FolderRebind {
            folder_id: r.folder.id,
            volume_id: target,
            rel_path: vp.rel,
        });
    }
    if rebinds.is_empty() {
        return Ok(None);
    }
    Ok(Some(cat.rebind_folders(&rebinds)?))
}

/// カタログのボリュームのつながりを確かめ直す（[`refresh_volumes`]。コアのボリュームの状態も更新し、変わったら
/// フォルダの変更を知らせる）。つながったボリュームがあれば `true`。
pub(crate) fn refresh(inner: &crate::core::Inner) -> Result<bool, ApiError> {
    let states = inner.with_catalog_api(|c| refresh_volumes(c, inner.volume_probe.as_ref()))?;
    let (changed, came_online) = {
        let mut cur = inner.volume_states.lock();
        let came_online = states
            .iter()
            .any(|(id, s)| s.online && !cur.get(id).is_some_and(|c| c.online));
        let changed = *cur != states;
        *cur = states;
        (changed, came_online)
    };
    if changed {
        crate::fileops::announce_folders(inner);
    }
    Ok(came_online)
}

impl crate::core::Core {
    /// 登録されているボリューム（FILE-03。最後に確かめたつながりの状態。確かめ直すには
    /// [`crate::Core::refresh_volumes`]）。
    pub fn volumes(&self) -> Result<Vec<crate::types::VolumeInfo>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let states = inner.volume_states.lock().clone();
        inner.with_catalog(|c| {
            Ok(c.volume_records()?
                .into_iter()
                .map(|v| {
                    let state = states.get(&v.id);
                    crate::types::VolumeInfo {
                        volume_id: v.id,
                        identified: !v.uuid.starts_with(PATH_VOLUME_PREFIX),
                        // 確かめた後に作ったボリューム（取り込み）は、つながっている。
                        online: !c.is_volume_offline(v.id) && state.is_none_or(|s| s.online),
                        network: state.is_some_and(|s| s.network),
                        uuid: v.uuid,
                        label: v.label,
                        mount_path: v.last_mount_path.map(PathBuf::from),
                        fs_type: v.fs_type,
                        last_seen_at: v.last_seen_at,
                    }
                })
                .collect())
        })
    }

    /// ボリュームのつながりを確かめ直す（ドライブをつないだ・外した後。マウント先が変わったボリュームは新しい
    /// マウント先にする）。以前の形式のボリュームのフォルダも、見えるものから移す。つながったボリュームがあれば、
    /// 見つからない写真の確認を頼む（アプリが許したとき）。
    pub fn refresh_volumes(&self) -> Result<Vec<crate::types::VolumeInfo>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let rebound = {
            let _ops = inner.file_ops.lock();
            inner.with_catalog_api(|c| rebind_path_volumes(c, inner.volume_probe.as_ref()))?
        };
        let came_online = refresh(inner)?;
        if let Some(r) = rebound {
            inner.events.emit(crate::events::Event::Warning {
                code: crate::events::WarningCode::VolumesRebound,
                message: rebind_message(&r),
                variant_id: None,
                path: None,
            });
            crate::fileops::announce_folders(inner);
        }
        if came_online {
            crate::relink::request_background_check(inner);
        }
        self.volumes()
    }
}

/// 以前の形式のボリュームの移し替えの知らせの文言。
pub(crate) fn rebind_message(r: &RebindReport) -> String {
    let mut m = format!(
        "以前の形式（パスで識別）のボリュームのフォルダ {} 件を、ドライブの ID で識別するようにしました（ドライブ文字・マウント先が変わっても同じドライブとして扱います）",
        r.moved + r.merged
    );
    if !r.conflicting_files.is_empty() {
        m.push_str(&format!(
            "。同じ場所に二重に登録されていたファイル {} 件は、元の形式のボリュームに残しました（カタログから除いてください）",
            r.conflicting_files.len()
        ));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_prefixes_are_stripped_component_wise() {
        assert_eq!(
            strip_mount(
                Path::new("/media/me/SD/DCIM/100"),
                Path::new("/media/me/SD")
            ),
            Some(vec!["DCIM".to_owned(), "100".to_owned()])
        );
        assert_eq!(
            strip_mount(Path::new("/media/me/SD"), Path::new("/media/me/SD")),
            Some(vec![])
        );
        // 名前の途中で切らない（/media/me/SD2 は /media/me/SD の中ではない）。
        assert_eq!(
            strip_mount(Path::new("/media/me/SD2/a"), Path::new("/media/me/SD")),
            None
        );
        assert_eq!(
            strip_mount(Path::new("/a/b"), Path::new("/")),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert!(same_path(
            Path::new("/media/me/SD/"),
            Path::new("/media/me/SD")
        ));
        assert!(!same_path(
            Path::new("/media/me/SD"),
            Path::new("/media/me")
        ));
    }

    /// 偽のボリューム（`root` の中を `uuid:T` とする）。
    struct Fake {
        root: PathBuf,
    }

    impl VolumeProbe for Fake {
        fn identify(&self, path: &Path) -> Option<VolumeIdentity> {
            path.starts_with(&self.root).then(|| VolumeIdentity {
                uuid: "uuid:T".to_owned(),
                mount: self.root.clone(),
                label: Some("T".to_owned()),
                fs_type: Some("exfat".to_owned()),
                network: false,
            })
        }
        fn mounted(&self) -> Vec<VolumeIdentity> {
            self.identify(&self.root).into_iter().collect()
        }
    }

    /// まだないパスは、実在する最も近い親のボリュームで、残りの部分を相対パスに足す。ID を取れなければ
    /// パスの先頭（`path:`）。
    #[test]
    fn locate_uses_the_nearest_existing_parent() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("drive");
        std::fs::create_dir_all(root.join("DCIM")).unwrap();
        let probe = Fake { root: root.clone() };
        let v = locate(&probe, &root.join("DCIM").join("新しい").join("x")).unwrap();
        assert_eq!(v.uuid, "uuid:T");
        assert_eq!(v.mount, root);
        assert_eq!(v.rel, "DCIM/新しい/x");
        assert_eq!(v.label.as_deref(), Some("T"));
        let v = locate(&probe, &root).unwrap();
        assert_eq!(v.rel, "");
        // ID を取れない場所はパスの先頭から。
        let other = d.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let v = locate(&probe, &other).unwrap();
        assert!(v.uuid.starts_with(PATH_VOLUME_PREFIX), "{}", v.uuid);
        assert_eq!(v, split_volume(&other).unwrap());
    }
}
