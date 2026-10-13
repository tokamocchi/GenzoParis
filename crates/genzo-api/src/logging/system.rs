//! クラッシュレポート・ログの最初の行に書く OS の説明（`linux x86_64（Ubuntu 24.04 LTS、カーネル 6.8.0）`
//! など）。
//!
//! 外部のプログラムを起動せず、OS の版を記録したファイル・API だけを読む（読めなければ OS の種類と CPU だけ）。
//!
//! | OS | 読むもの |
//! |---|---|
//! | Linux | `/etc/os-release` の `PRETTY_NAME` と `/proc/sys/kernel/osrelease` |
//! | macOS | `/System/Library/CoreServices/SystemVersion.plist` の `ProductVersion`・`ProductBuildVersion` |
//! | Windows | `ntdll.dll` の `RtlGetVersion`（アプリの互換性の設定で値が変わらない。ビルド番号 22000 以上が Windows 11） |

/// OS の説明（起動時に 1 回求めて使い回す）。
pub(crate) fn describe() -> String {
    let base = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    match detail() {
        Some(d) if !d.is_empty() => format!("{base}（{d}）"),
        _ => base,
    }
}

#[cfg(target_os = "linux")]
fn detail() -> Option<String> {
    let name = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| os_release_pretty_name(&t));
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());
    let parts: Vec<String> = name
        .into_iter()
        .chain(kernel.map(|k| format!("カーネル {k}")))
        .collect();
    Some(parts.join("、"))
}

#[cfg(target_os = "macos")]
fn detail() -> Option<String> {
    let text = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
    let version = plist_string(&text, "ProductVersion")?;
    Some(match plist_string(&text, "ProductBuildVersion") {
        Some(build) => format!("macOS {version}、ビルド {build}"),
        None => format!("macOS {version}"),
    })
}

#[cfg(windows)]
fn detail() -> Option<String> {
    // OSVERSIONINFOW（Windows SDK の winnt.h）。RtlGetVersion は ntdll.dll にあり、すべての版の Windows で使える。
    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform_id: u32,
        csd_version: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }
    let mut info = OsVersionInfoW {
        size: std::mem::size_of::<OsVersionInfoW>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform_id: 0,
        csd_version: [0; 128],
    };
    // SAFETY: info は RtlGetVersion が求める OSVERSIONINFOW と同じ配置で、size に大きさを入れてある。
    let status = unsafe { RtlGetVersion(&mut info) };
    (status == 0).then(|| windows_version_text(info.major, info.minor, info.build))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn detail() -> Option<String> {
    None
}

/// `/etc/os-release` の `PRETTY_NAME` の値（引用符を外す）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn os_release_pretty_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("PRETTY_NAME=")?;
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// プロパティリスト（XML）の `<key>{key}</key>` の次の `<string>` の値。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn plist_string(text: &str, key: &str) -> Option<String> {
    let after = text.split_once(&format!("<key>{key}</key>"))?.1;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    let value = after[start..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Windows の版の説明（ビルド番号 22000 以上を Windows 11 とする）。
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_version_text(major: u32, minor: u32, build: u32) -> String {
    let name = match (major, minor, build) {
        (10, 0, b) if b >= 22000 => "Windows 11",
        (10, 0, _) => "Windows 10",
        _ => "Windows",
    };
    format!("{name}、版 {major}.{minor}.{build}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_description_starts_with_the_os_and_arch() {
        let d = describe();
        assert!(d.starts_with(std::env::consts::OS), "{d}");
        assert!(d.contains(std::env::consts::ARCH), "{d}");
    }

    #[test]
    fn os_release_and_plist_are_parsed() {
        let os_release = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(
            os_release_pretty_name(os_release).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert_eq!(os_release_pretty_name("ID=x\n"), None);
        let plist = "<dict>\n\t<key>ProductBuildVersion</key>\n\t<string>25A354</string>\n\t<key>ProductVersion</key>\n\t<string>26.0.1</string>\n</dict>";
        assert_eq!(
            plist_string(plist, "ProductVersion").as_deref(),
            Some("26.0.1")
        );
        assert_eq!(
            plist_string(plist, "ProductBuildVersion").as_deref(),
            Some("25A354")
        );
        assert_eq!(plist_string(plist, "Missing"), None);
    }

    #[test]
    fn windows_build_numbers_are_named() {
        assert_eq!(
            windows_version_text(10, 0, 22631),
            "Windows 11、版 10.0.22631"
        );
        assert_eq!(
            windows_version_text(10, 0, 19045),
            "Windows 10、版 10.0.19045"
        );
        assert_eq!(windows_version_text(6, 3, 9600), "Windows、版 6.3.9600");
    }
}
