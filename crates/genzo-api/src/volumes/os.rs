//! OS のボリューム ID の取得（FILE-03。04 の 3.14 節）。OS ごとの実装を cfg で分ける。
//!
//! このファイルは std と OS の API（libc・windows-sys）だけを使う（Windows・macOS 向けにも型の確認ができる
//! ように。親のモジュールの型には依存しない）。
//!
//! - **Linux**: `/proc/self/mountinfo` で、パスの装置番号（`st_dev`）と同じマウントのうち、パスを含む最も深い
//!   マウント先を選ぶ。ID は `/dev/disk/by-uuid` の名前（udev がファイルシステムの UUID から作る。装置番号か、
//!   マウント元の装置の番号が同じもの）で `uuid:<UUID>`。マウントの根がファイルシステムの根でなければ（bind
//!   マウント・btrfs のサブボリューム）`uuid:<UUID>:<根>`。ネットワークのファイルシステム（NFS・CIFS など）は
//!   `net:<マウント元>`（CIFS・SMB は小文字にそろえる）。どれでもなければ（コンテナの中・tmpfs など）`None`
//!   （呼び出し側がパスのマウント先から `path:` の ID を作る）。ラベルは `/dev/disk/by-label`。
//! - **macOS**: `statfs` のマウント先（`f_mntonname`）・ファイルシステムの種類・`MNT_LOCAL`。ID は
//!   `getattrlist` の `ATTR_VOL_UUID`（`uuid:<UUID>`。大文字）、ネットワークのボリュームは `net:<マウント元>`
//!   （小文字）。APFS のデータのボリューム（`/System/Volumes/Data`。`/Users` などは firmlink でここにある）は、
//!   マウント先を `/` として扱い、`/`（読み取り専用のシステムのボリューム）とその中のパスもデータのボリュームと
//!   する（起動ディスクを 1 つのボリュームとして扱う）。ラベルはマウント先の最後の部分（`/Volumes/<名前>`）。
//!   **実機で未確認。**
//! - **Windows**: `GetVolumePathNameW` のマウント先（`E:\`・ボリュームを割り当てたフォルダ）と
//!   `GetVolumeInformationW` のシリアル番号・ファイルシステムの名前・ラベルで `winvol:<ファイルシステム>:<XXXX-XXXX>`
//!   （ドライブ文字が変わっても同じ。フォーマットし直すと変わる。ボリュームのクローンは同じ値になりうる）。UNC の
//!   共有と、割り当てたネットワークドライブ（`DRIVE_REMOTE`）は `None`（呼び出し側が `path:` の ID を作る。
//!   決定事項ログ No.106）。調べる間は、メディアのないドライブで OS の確認の画面を出さない
//!   （`SetThreadErrorMode`）。**実機で未確認。**

use std::path::{Path, PathBuf};

/// OS から読んだボリュームの情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsVolume {
    /// ボリュームの ID（`uuid:`・`net:`・`winvol:` で始まる）。
    pub uuid: String,
    /// マウント先（ボリュームのルートの絶対パス）。
    pub mount: PathBuf,
    /// ラベル（ボリュームの名前）。
    pub label: Option<String>,
    /// ファイルシステムの種類。
    pub fs_type: Option<String>,
    /// ネットワークのボリュームか。
    pub network: bool,
}

/// 実在するパス `path` のボリューム（分からなければ `None`）。
pub fn identify(path: &Path) -> Option<OsVolume> {
    imp::identify(path)
}

/// つながっているボリュームの一覧（ID を取れるものだけ）。
pub fn mounted() -> Vec<OsVolume> {
    imp::mounted()
}

/// `/proc/self/mountinfo` などの 8 進数のエスケープ（`\040` = 空白など）を戻す。
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn unescape_octal(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let v = (b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..=i + 3].iter().all(|c| (b'0'..=b'7').contains(c)))
        .then(|| {
            u32::from(b[i + 1] - b'0') * 64
                + u32::from(b[i + 2] - b'0') * 8
                + u32::from(b[i + 3] - b'0')
        })
        .and_then(|v| u8::try_from(v).ok());
        if let Some(v) = v {
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// udev の名前（`/dev/disk/by-label` など）の 16 進数のエスケープ（`\x20` = 空白など）を戻す。
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn unescape_hex(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1] == b'x'
            && b[i + 2].is_ascii_hexdigit()
            && b[i + 3].is_ascii_hexdigit()
        {
            let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(b[i + 2]) * 16 + hex(b[i + 3]));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `/proc/self/mountinfo` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) struct MountInfo {
    /// 装置番号（major・minor）。
    pub major: u32,
    pub minor: u32,
    /// ファイルシステムの中の、マウントの根。
    pub root: String,
    /// マウント先。
    pub mount_point: PathBuf,
    /// ファイルシステムの種類。
    pub fs_type: String,
    /// マウント元（装置のパスなど）。
    pub source: String,
}

/// `/proc/self/mountinfo` の内容を読む（読めない行は飛ばす）。
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn parse_mountinfo(text: &str) -> Vec<MountInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let l: Vec<&str> = left.split(' ').collect();
        let r: Vec<&str> = right.split(' ').collect();
        if l.len() < 5 || r.len() < 2 {
            continue;
        }
        let Some((major, minor)) = l[2].split_once(':') else {
            continue;
        };
        let (Ok(major), Ok(minor)) = (major.parse(), minor.parse()) else {
            continue;
        };
        out.push(MountInfo {
            major,
            minor,
            root: unescape_octal(l[3]),
            mount_point: PathBuf::from(unescape_octal(l[4])),
            fs_type: unescape_octal(r[0]),
            source: unescape_octal(r[1]),
        });
    }
    out
}

/// Linux のネットワークのファイルシステムの種類か（`/proc/self/mountinfo` の名前）。
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn is_network_fs_type(t: &str) -> bool {
    matches!(
        t,
        "nfs"
            | "nfs4"
            | "cifs"
            | "smb3"
            | "smbfs"
            | "afs"
            | "ceph"
            | "coda"
            | "ncpfs"
            | "9p"
            | "glusterfs"
    ) || t.starts_with("fuse.sshfs")
}

/// マウント `m` の、`dev`（装置番号）のパスのマウントとしての情報から、ボリュームの ID を作る。`by_uuid` は
/// （UUID, その装置の番号）の一覧。
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn linux_volume_id(
    m: &MountInfo,
    dev: (u32, u32),
    source_dev: Option<(u32, u32)>,
    by_uuid: &[(String, (u32, u32))],
) -> Option<String> {
    if is_network_fs_type(&m.fs_type) {
        let source = if matches!(m.fs_type.as_str(), "cifs" | "smb3" | "smbfs") {
            m.source.to_lowercase()
        } else {
            m.source.clone()
        };
        return Some(format!("net:{source}"));
    }
    let uuid = by_uuid
        .iter()
        .find(|(_, d)| *d == dev)
        .or_else(|| source_dev.and_then(|sd| by_uuid.iter().find(|(_, d)| *d == sd)))
        .map(|(u, _)| u.clone())?;
    Some(if m.root == "/" || m.root.is_empty() {
        format!("uuid:{uuid}")
    } else {
        format!("uuid:{uuid}:{}", m.root)
    })
}

/// `mounts` のうち、装置番号が `dev` で、`canonical`（リンクを解決したパス）を含む最も深いマウント。
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn pick_mount<'a>(
    mounts: &'a [MountInfo],
    dev: (u32, u32),
    canonical: &Path,
) -> Option<&'a MountInfo> {
    mounts
        .iter()
        .filter(|m| (m.major, m.minor) == dev && canonical.starts_with(&m.mount_point))
        .max_by_key(|m| m.mount_point.components().count())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::path::Path;

    use super::{OsVolume, linux_volume_id, parse_mountinfo, pick_mount, unescape_hex};

    fn split_dev(dev: u64) -> (u32, u32) {
        (
            libc::major(dev as libc::dev_t),
            libc::minor(dev as libc::dev_t),
        )
    }

    /// `/dev/disk/by-uuid`（`by-label`）の名前と、その先の装置の番号。
    fn disk_links(dir: &str) -> Vec<(String, (u32, u32))> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for e in entries.flatten() {
            let Ok(meta) = std::fs::metadata(e.path()) else {
                continue;
            };
            if !meta.file_type().is_block_device() {
                continue;
            }
            let Some(name) = e.file_name().to_str().map(unescape_hex) else {
                continue;
            };
            out.push((name, split_dev(meta.rdev())));
        }
        out
    }

    fn source_dev(source: &str) -> Option<(u32, u32)> {
        if !source.starts_with("/dev/") {
            return None;
        }
        let meta = std::fs::metadata(source).ok()?;
        meta.file_type()
            .is_block_device()
            .then(|| split_dev(meta.rdev()))
    }

    pub(super) fn identify(path: &Path) -> Option<OsVolume> {
        let meta = std::fs::metadata(path).ok()?;
        let dev = split_dev(meta.dev());
        let canonical = std::fs::canonicalize(path).ok()?;
        let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
        let mounts = parse_mountinfo(&text);
        let m = pick_mount(&mounts, dev, &canonical)?;
        let by_uuid = disk_links("/dev/disk/by-uuid");
        let sdev = source_dev(&m.source);
        let uuid = linux_volume_id(m, dev, sdev, &by_uuid)?;
        let label = if uuid.starts_with("uuid:") {
            let labels = disk_links("/dev/disk/by-label");
            labels
                .iter()
                .find(|(_, d)| *d == dev || Some(*d) == sdev)
                .map(|(l, _)| l.clone())
        } else {
            None
        };
        Some(OsVolume {
            network: uuid.starts_with("net:"),
            uuid,
            mount: m.mount_point.clone(),
            label,
            fs_type: Some(m.fs_type.clone()),
        })
    }

    pub(super) fn mounted() -> Vec<OsVolume> {
        let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return Vec::new();
        };
        let mut out: Vec<OsVolume> = Vec::new();
        for m in parse_mountinfo(&text) {
            // 装置に結び付いたものとネットワークのものだけ（proc・sysfs・cgroup などは飛ばす）。
            if !(m.source.starts_with("/dev/") || super::is_network_fs_type(&m.fs_type)) {
                continue;
            }
            if let Some(v) = identify(&m.mount_point)
                && v.mount == m.mount_point
                && !out.iter().any(|o| o.uuid == v.uuid && o.mount == v.mount)
            {
                out.push(v);
            }
        }
        out
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use super::OsVolume;

    /// APFS のデータのボリュームのマウント先（`/Users` などは firmlink でここにある）。
    const DATA_VOLUME: &str = "/System/Volumes/Data";

    /// statfs の文字列の欄（NUL で終わる。終わらなければ欄の長さで止める）。
    fn c_text(buf: &[libc::c_char]) -> String {
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn statfs(path: &Path) -> Option<libc::statfs> {
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `statfs` は C の構造体で、すべて 0 のビット列は有効な値。
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` は NUL で終わる文字列、`st` は書き込める構造体で、どちらも呼び出しの間は生きている。
        let r = unsafe { libc::statfs(c.as_ptr(), &mut st) };
        (r == 0).then_some(st)
    }

    /// `getattrlist` の `ATTR_VOL_UUID`（ボリュームの UUID。大文字の 8-4-4-4-12 の形）。
    fn volume_uuid(mount: &Path) -> Option<String> {
        #[repr(C)]
        struct Buf {
            length: u32,
            uuid: [u8; 16],
        }
        let c = CString::new(mount.as_os_str().as_bytes()).ok()?;
        // SAFETY: attrlist は C の構造体で、すべて 0 のビット列は有効な値。
        let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
        list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
        list.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID;
        let mut buf = Buf {
            length: 0,
            uuid: [0; 16],
        };
        // SAFETY: c は NUL で終わる文字列、list と buf は書き込める構造体で、buf の大きさを渡す。
        let r = unsafe {
            libc::getattrlist(
                c.as_ptr(),
                (&mut list as *mut libc::attrlist).cast(),
                (&mut buf as *mut Buf).cast(),
                std::mem::size_of::<Buf>(),
                0,
            )
        };
        if r != 0 || (buf.length as usize) < std::mem::size_of::<Buf>() || buf.uuid == [0; 16] {
            return None;
        }
        let u = buf.uuid;
        Some(format!(
            "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            u[0],
            u[1],
            u[2],
            u[3],
            u[4],
            u[5],
            u[6],
            u[7],
            u[8],
            u[9],
            u[10],
            u[11],
            u[12],
            u[13],
            u[14],
            u[15]
        ))
    }

    fn from_statfs(st: &libc::statfs) -> Option<OsVolume> {
        let on = c_text(&st.f_mntonname);
        let from = c_text(&st.f_mntfromname);
        let fs_type = c_text(&st.f_fstypename);
        let network = st.f_flags & (libc::MNT_LOCAL as u32) == 0;
        let real_mount = PathBuf::from(&on);
        let uuid = if network {
            format!("net:{}", from.to_lowercase())
        } else {
            format!("uuid:{}", volume_uuid(&real_mount)?)
        };
        // データのボリュームは `/` として扱う（`/Users/...` を `/` からの相対パスで持つ）。
        let mount = if on == DATA_VOLUME {
            PathBuf::from("/")
        } else {
            real_mount
        };
        let label = if mount == Path::new("/") {
            None
        } else {
            mount.file_name().map(|n| n.to_string_lossy().into_owned())
        };
        Some(OsVolume {
            uuid,
            mount,
            label,
            fs_type: (!fs_type.is_empty()).then_some(fs_type),
            network,
        })
    }

    /// 起動ディスクの `/`（macOS 10.15 以降は読み取り専用のシステムのボリューム）を、データのボリュームに読み
    /// 替える。[`from_statfs`] はデータのボリュームのマウント先を `/` とするので、`/` とシステムのボリュームの
    /// パスも同じボリュームとして扱う（記録したマウント先 `/` を問い合わせ直したときに、システムのボリュームの
    /// 別の ID を返さないため。検証のラウンド 1 の指摘）。データのボリュームがなければ（10.14 まで）そのまま。
    fn boot_volume_as_data(st: libc::statfs) -> libc::statfs {
        if c_text(&st.f_mntonname) != "/" {
            return st;
        }
        match statfs(Path::new(DATA_VOLUME)) {
            Some(data) if c_text(&data.f_mntonname) == DATA_VOLUME => data,
            _ => st,
        }
    }

    pub(super) fn identify(path: &Path) -> Option<OsVolume> {
        from_statfs(&boot_volume_as_data(statfs(path)?))
    }

    pub(super) fn mounted() -> Vec<OsVolume> {
        let mut ptr: *mut libc::statfs = std::ptr::null_mut();
        // SAFETY: getmntinfo は OS が持つ配列へのポインタを書く（解放しない。次の呼び出しまで有効）。
        let n = unsafe { libc::getmntinfo(&mut ptr, libc::MNT_NOWAIT) };
        if n <= 0 || ptr.is_null() {
            return Vec::new();
        }
        // SAFETY: ptr は n 個の statfs の配列を指す。
        let list = unsafe { std::slice::from_raw_parts(ptr, n as usize) };
        let mut out: Vec<OsVolume> = Vec::new();
        for st in list {
            if let Some(v) = from_statfs(&boot_volume_as_data(*st))
                && !out.iter().any(|o| o.uuid == v.uuid && o.mount == v.mount)
            {
                out.push(v);
            }
        }
        out
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, GetVolumePathNameW,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetThreadErrorMode,
    };

    use super::OsVolume;

    /// `GetDriveTypeW`: ネットワークのドライブ。
    const DRIVE_REMOTE: u32 = 4;
    /// `GetDriveTypeW`: ルートのパスが正しくない。
    const DRIVE_NO_ROOT_DIR: u32 = 1;

    /// メディアのないドライブで OS の確認の画面を出さないようにする間の印（戻すときに前の値にする）。
    struct ErrorModeGuard(u32);

    impl ErrorModeGuard {
        fn new() -> Self {
            let mut old = 0;
            // SAFETY: old は書き込める変数。
            unsafe {
                SetThreadErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX, &mut old)
            };
            Self(old)
        }
    }

    impl Drop for ErrorModeGuard {
        fn drop(&mut self) {
            // SAFETY: 前の値に戻すだけ。
            unsafe { SetThreadErrorMode(self.0, std::ptr::null_mut()) };
        }
    }

    fn wide(path: &Path) -> Option<Vec<u16>> {
        let abs = std::path::absolute(path).ok()?;
        let mut w: Vec<u16> = abs.as_os_str().encode_wide().collect();
        if w.contains(&0) {
            return None;
        }
        w.push(0);
        Some(w)
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        std::ffi::OsString::from_wide(&buf[..end])
            .to_string_lossy()
            .into_owned()
    }

    /// `\\?\C:\` を `C:\` に、`\\?\UNC\server\share\` を `\\server\share\` にする。
    fn plain_form(s: &str) -> String {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            rest.to_owned()
        } else {
            s.to_owned()
        }
    }

    fn info(root: &str) -> Option<OsVolume> {
        if root.starts_with(r"\\") {
            // UNC の共有は、呼び出し側がパスから ID を作る。
            return None;
        }
        let w: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: w は NUL で終わる UTF-16 の文字列。
        let kind = unsafe { GetDriveTypeW(w.as_ptr()) };
        if kind == DRIVE_REMOTE || kind == DRIVE_NO_ROOT_DIR {
            return None;
        }
        let mut name = [0u16; 261];
        let mut fs = [0u16; 261];
        let mut serial = 0u32;
        let mut max_len = 0u32;
        let mut flags = 0u32;
        // SAFETY: w は NUL で終わる文字列、ほかは書き込める変数と、長さを渡した配列。
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
                &mut serial,
                &mut max_len,
                &mut flags,
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let fs_name = from_wide(&fs);
        let label = from_wide(&name);
        Some(OsVolume {
            uuid: format!(
                "winvol:{}:{:04X}-{:04X}",
                fs_name,
                serial >> 16,
                serial & 0xFFFF
            ),
            mount: PathBuf::from(root),
            label: (!label.is_empty()).then_some(label),
            fs_type: (!fs_name.is_empty()).then_some(fs_name),
            network: false,
        })
    }

    pub(super) fn identify(path: &Path) -> Option<OsVolume> {
        let _mode = ErrorModeGuard::new();
        let w = wide(path)?;
        let mut buf = vec![0u16; w.len() + 64];
        // SAFETY: w は NUL で終わる文字列、buf は長さを渡した書き込める配列。
        let ok = unsafe { GetVolumePathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            return None;
        }
        info(&plain_form(&from_wide(&buf)))
    }

    pub(super) fn mounted() -> Vec<OsVolume> {
        let _mode = ErrorModeGuard::new();
        // SAFETY: 引数のない関数。
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = char::from(b'A' + i as u8);
            if let Some(v) = info(&format!("{letter}:\\")) {
                out.push(v);
            }
        }
        out
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    windows
)))]
mod imp {
    use std::path::Path;

    use super::OsVolume;

    pub(super) fn identify(_path: &Path) -> Option<OsVolume> {
        None
    }

    pub(super) fn mounted() -> Vec<OsVolume> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_lines_are_parsed_with_escapes() {
        let text = "\
28 1 254:0 / / rw,relatime - ext4 /dev/vda rw
40 28 8:17 / /media/me/My\\040SD rw,nosuid shared:5 - exfat /dev/sdb1 rw,uid=1000
41 28 0:45 /@home /home rw - btrfs /dev/sda2 rw
42 28 0:50 / /mnt/nas rw - cifs //NAS/Photos rw
broken line
";
        let m = parse_mountinfo(text);
        assert_eq!(m.len(), 4);
        assert_eq!(m[1].mount_point, PathBuf::from("/media/me/My SD"));
        assert_eq!((m[1].major, m[1].minor), (8, 17));
        assert_eq!(m[1].fs_type, "exfat");
        assert_eq!(m[1].source, "/dev/sdb1");
        assert_eq!(m[2].root, "/@home");
        assert_eq!(unescape_hex(r"My\x20SD"), "My SD");
        assert_eq!(unescape_octal(r"a\134b"), r"a\b");
        assert_eq!(unescape_octal(r"end\04"), r"end\04");
    }

    /// ID の作り方: 装置番号か、マウント元の装置の番号が by-uuid の装置と同じなら UUID（根がファイルシステムの
    /// 根でなければ根を付ける）。ネットワークは net:（CIFS は小文字）。どれでもなければ None。
    #[test]
    fn linux_volume_ids() {
        let m = parse_mountinfo(
            "\
40 28 8:17 / /media/me/SD rw - exfat /dev/sdb1 rw
41 28 0:45 /@home /home rw - btrfs /dev/sda2 rw
42 28 0:50 / /mnt/nas rw - cifs //NAS/Photos rw
43 28 0:51 / /tmp rw - tmpfs tmpfs rw
",
        );
        let by_uuid = vec![
            ("1234-ABCD".to_owned(), (8, 17)),
            ("b1f0c8aa-0000".to_owned(), (8, 2)),
        ];
        assert_eq!(
            linux_volume_id(&m[0], (8, 17), Some((8, 17)), &by_uuid).as_deref(),
            Some("uuid:1234-ABCD")
        );
        // btrfs のサブボリューム（装置番号は無名、マウント元の装置で探す）。
        assert_eq!(
            linux_volume_id(&m[1], (0, 45), Some((8, 2)), &by_uuid).as_deref(),
            Some("uuid:b1f0c8aa-0000:/@home")
        );
        assert_eq!(
            linux_volume_id(&m[2], (0, 50), None, &by_uuid).as_deref(),
            Some("net://nas/photos")
        );
        assert_eq!(linux_volume_id(&m[3], (0, 51), None, &by_uuid), None);
        // パスを含む最も深いマウント。
        let all = parse_mountinfo(
            "\
1 0 8:1 / / rw - ext4 /dev/sda1 rw
2 1 8:17 / /media/me/SD rw - exfat /dev/sdb1 rw
",
        );
        let p = pick_mount(&all, (8, 17), Path::new("/media/me/SD/DCIM")).unwrap();
        assert_eq!(p.mount_point, PathBuf::from("/media/me/SD"));
        assert!(pick_mount(&all, (8, 17), Path::new("/home/me")).is_none());
        assert!(is_network_fs_type("nfs4"));
        assert!(!is_network_fs_type("ext4"));
    }

    /// この環境のボリュームを調べても失敗しない（ID を取れない環境では None）。
    #[test]
    fn identifying_existing_paths_does_not_fail() {
        let d = std::env::temp_dir();
        if let Some(v) = identify(&d) {
            assert!(
                d.starts_with(&v.mount) || std::fs::canonicalize(&d).unwrap().starts_with(&v.mount)
            );
            assert!(!v.uuid.is_empty());
        }
        let _ = mounted();
        assert_eq!(identify(Path::new("/存在しない/フォルダ")), None);
    }
}
