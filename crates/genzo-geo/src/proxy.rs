//! プロキシの設定を読む（環境変数と、Windows ではレジストリ）。[`crate::UreqClient::new`] が使う。
//!
//! 読む順:
//! 1. 環境変数（ureq と同じ順）: `ALL_PROXY`・`all_proxy`・`HTTPS_PROXY`・`https_proxy`・`HTTP_PROXY`・
//!    `http_proxy` のうち、空でない最初のもの。値が読めなければ、次の変数を見る。空でない変数があるのに 1 つも
//!    読めなければ、直接つながずに誤り（[`GeoError::Network`]）にする。
//! 2. Windows では、環境変数がなければ `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Internet
//!    Settings` の `ProxyEnable`・`ProxyServer`（設定の画面の「手動プロキシ セットアップ」）を読む。
//!    - すべてのプロトコルで同じプロキシ（`proxy.example:8080`）なら、それを HTTP のプロキシとして使う。
//!    - プロトコルごとの形（`http=proxy.example:8080;https=secure.example:8443;socks=...`）なら、`https=` を使う。
//!      `https=` がなく `socks=` があれば SOCKS（対応していないので、UreqClient が誤りにする）。どちらもなければ
//!      プロキシを使わない（https の要求にプロキシを割り当てていない設定。Windows の扱いに合わせたつもりの
//!      AI の仮決定で、実機では確かめていない）。
//!    - `ProxyEnable` が有効なのに `ProxyServer` が空・読めない形なら、直接つながずに誤りにする。
//! 3. どれもなければ直接つなぐ。
//!
//! 直接つなぐホスト: 環境変数 `NO_PROXY`・`no_proxy`（`,` 区切り）。なければ、Windows では `ProxyOverride`
//! （`;` 区切り。ドットのないホストを表す `<local>` は扱わない）。書き方は ureq の `NO_PROXY` と同じ
//! （`example.org`・`.example.org`・`*.example.org`・`10.*`・`*`。`10.0.0.0/8` のような範囲は扱わない）。
//!
//! 読まないもの（制限）:
//! - Windows の自動構成（`AutoConfigURL` の PAC と、「設定を自動的に検出する」の WPAD）。手動のプロキシが有効なら
//!   それを使い、なければ直接つなぐ（PAC があるときは warn のログを出す）。PAC の答えが DIRECT の環境もあるので、
//!   誤りにはしない。
//! - macOS のシステムの設定のプロキシ（環境変数だけ）。
//!
//! ログとエラーのメッセージには、プロキシの値（利用者名・パスワードを含みうる）を入れない。

use crate::GeoError;

/// 見る環境変数（ureq の `Proxy::try_from_env` と同じ順）。
const PROXY_ENV: [&str; 6] = [
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

/// 直接つなぐホストの環境変数。
const NO_PROXY_ENV: [&str; 2] = ["NO_PROXY", "no_proxy"];

/// Windows のインターネットの設定（レジストリの値。読めなかった値は `None`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WindowsInternetSettings {
    /// `ProxyEnable`（手動のプロキシを使う）。
    pub(crate) proxy_enable: bool,
    /// `ProxyServer`。
    pub(crate) proxy_server: Option<String>,
    /// `ProxyOverride`（プロキシを使わないホスト）。
    pub(crate) proxy_override: Option<String>,
    /// `AutoConfigURL`（自動構成のスクリプト。読まない）。
    pub(crate) auto_config_url: Option<String>,
}

/// この環境のプロキシ（`None` なら直接つなぐ）。モジュールの doc の順に読む。
pub(crate) fn system_proxy() -> Result<Option<ureq::Proxy>, GeoError> {
    resolve(
        &|name| std::env::var(name).ok(),
        windows_settings().as_ref(),
    )
}

/// 環境変数（`env`）と Windows の設定（`windows`。Windows 以外は `None`）から、使うプロキシを決める。
pub(crate) fn resolve(
    env: &dyn Fn(&str) -> Option<String>,
    windows: Option<&WindowsInternetSettings>,
) -> Result<Option<ureq::Proxy>, GeoError> {
    let no_proxy = no_proxy_entries(env, windows);

    let mut unreadable_env = None;
    for name in PROXY_ENV {
        let Some(value) = env(name) else { continue };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match ureq::Proxy::new(value) {
            Ok(proxy) => return with_no_proxy(&proxy, &no_proxy).map(Some),
            Err(_) => {
                unreadable_env.get_or_insert(name);
            }
        }
    }
    if let Some(name) = unreadable_env {
        return Err(GeoError::Network(format!(
            "プロキシの設定（環境変数 {name}）を読めません"
        )));
    }

    let Some(windows) = windows else {
        return Ok(None);
    };
    if !windows.proxy_enable {
        if windows
            .auto_config_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty())
        {
            tracing::warn!(
                "Windows の自動構成スクリプト（PAC）は読みません。プロキシを使わずに直接つなぎます"
            );
        }
        return Ok(None);
    }
    match windows_proxy_server(windows.proxy_server.as_deref().unwrap_or(""))? {
        Some(proxy) => with_no_proxy(&proxy, &no_proxy).map(Some),
        None => Ok(None),
    }
}

/// Windows の `ProxyServer` の値から、https の要求に使うプロキシを決める（`None` は直接つなぐ）。
fn windows_proxy_server(value: &str) -> Result<Option<ureq::Proxy>, GeoError> {
    let unreadable =
        || GeoError::Network("Windows のプロキシの設定（ProxyServer）を読めません".to_string());
    let value = value.trim();
    if value.is_empty() {
        return Err(unreadable());
    }
    if !value.contains('=') {
        // すべてのプロトコルで同じプロキシ。
        return proxy_from_windows_entry(value, "http").map(Some);
    }
    let mut https = None;
    let mut socks = None;
    for entry in value.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let (scheme, address) = entry.split_once('=').ok_or_else(unreadable)?;
        let address = address.trim();
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = https.or(Some(address)),
            "socks" => socks = socks.or(Some(address)),
            _ => {}
        }
    }
    if let Some(address) = https {
        return proxy_from_windows_entry(address, "http").map(Some);
    }
    if let Some(address) = socks {
        return proxy_from_windows_entry(address, "socks4").map(Some);
    }
    Ok(None)
}

/// `host:port`（`http://host:port` のように種類が付いていてもよい）を、種類がなければ `default_scheme` の
/// プロキシにする。
fn proxy_from_windows_entry(address: &str, default_scheme: &str) -> Result<ureq::Proxy, GeoError> {
    let unreadable =
        || GeoError::Network("Windows のプロキシの設定（ProxyServer）を読めません".to_string());
    if address.is_empty() {
        return Err(unreadable());
    }
    let uri = if address.contains("://") {
        address.to_string()
    } else {
        format!("{default_scheme}://{address}")
    };
    ureq::Proxy::new(&uri).map_err(|_| unreadable())
}

/// 直接つなぐホストの書き方の一覧（環境変数を先に、なければ Windows の `ProxyOverride`）。
fn no_proxy_entries(
    env: &dyn Fn(&str) -> Option<String>,
    windows: Option<&WindowsInternetSettings>,
) -> Vec<String> {
    let entries = |value: &str, separator: char| -> Vec<String> {
        value
            .split(separator)
            .map(str::trim)
            .filter(|e| !e.is_empty() && *e != "<local>")
            .map(str::to_string)
            .collect()
    };
    if let Some(value) = NO_PROXY_ENV.iter().find_map(|name| env(name)) {
        return entries(&value, ',');
    }
    windows
        .and_then(|w| w.proxy_override.as_deref())
        .map(|value| entries(value, ';'))
        .unwrap_or_default()
}

/// `proxy` に直接つなぐホストの一覧を付ける（ureq の `Proxy::new` は `NO_PROXY` を付けないため、作り直す）。
fn with_no_proxy(proxy: &ureq::Proxy, no_proxy: &[String]) -> Result<ureq::Proxy, GeoError> {
    let mut builder = ureq::Proxy::builder(proxy.protocol())
        .host(proxy.host())
        .port(proxy.port())
        .resolve_target(proxy.resolve_target());
    if let Some(username) = proxy.username() {
        builder = builder.username(username);
    }
    if let Some(password) = proxy.password() {
        builder = builder.password(password);
    }
    for entry in no_proxy {
        builder = builder.no_proxy(entry);
    }
    builder
        .build()
        .map_err(|_| GeoError::Network("プロキシの設定を読めません".to_string()))
}

/// Windows のレジストリのインターネットの設定（読めなければ `None`）。
#[cfg(windows)]
fn windows_settings() -> Option<WindowsInternetSettings> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

    const REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(REGISTRY_PATH, KEY_READ)
        .ok()?;
    Some(WindowsInternetSettings {
        proxy_enable: key.get_value::<u32, _>("ProxyEnable").is_ok_and(|v| v != 0),
        proxy_server: key.get_value::<String, _>("ProxyServer").ok(),
        proxy_override: key.get_value::<String, _>("ProxyOverride").ok(),
        auto_config_url: key.get_value::<String, _>("AutoConfigURL").ok(),
    })
}

/// Windows 以外はレジストリを読まない。
#[cfg(not(windows))]
fn windows_settings() -> Option<WindowsInternetSettings> {
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ureq::ProxyProtocol;

    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| vars.get(name).cloned()
    }

    fn windows(server: &str) -> WindowsInternetSettings {
        WindowsInternetSettings {
            proxy_enable: true,
            proxy_server: Some(server.to_string()),
            ..Default::default()
        }
    }

    fn uri(s: &str) -> ureq::http::Uri {
        s.parse().unwrap()
    }

    fn host_port(proxy: &ureq::Proxy) -> (ProxyProtocol, String, u16) {
        (proxy.protocol(), proxy.host().to_string(), proxy.port())
    }

    #[test]
    fn nothing_configured_means_direct() {
        assert_eq!(resolve(&env(&[]), None).unwrap(), None);
        assert_eq!(
            resolve(&env(&[]), Some(&WindowsInternetSettings::default())).unwrap(),
            None
        );
    }

    #[test]
    fn environment_variables_are_read_in_order_with_no_proxy() {
        let p = resolve(
            &env(&[
                ("HTTP_PROXY", "http://plain.example:3128"),
                ("HTTPS_PROXY", "http://user:secret@secure.example:8443"),
                ("NO_PROXY", "localhost, .corp.example"),
            ]),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            host_port(&p),
            (ProxyProtocol::Http, "secure.example".to_string(), 8443)
        );
        assert_eq!(p.username(), Some("user"));
        assert_eq!(p.password(), Some("secret"));
        assert!(p.is_no_proxy(&uri("https://localhost/")));
        assert!(p.is_no_proxy(&uri("https://tiles.corp.example/")));
        assert!(!p.is_no_proxy(&uri("https://tile.openstreetmap.org/")));
        // 空の変数は、ないものとして次を見る。
        let p = resolve(
            &env(&[("ALL_PROXY", " "), ("http_proxy", "proxy.example:8080")]),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            host_port(&p),
            (ProxyProtocol::Http, "proxy.example".to_string(), 8080)
        );
    }

    #[test]
    fn unreadable_environment_variables_are_errors_instead_of_connecting_directly() {
        let e = resolve(&env(&[("HTTPS_PROXY", "http://a b:1")]), None).unwrap_err();
        let GeoError::Network(message) = &e else {
            panic!("{e:?}");
        };
        assert!(message.contains("HTTPS_PROXY"), "{message}");
        // 値（利用者名・パスワードを含みうる）はメッセージに入れない。
        assert!(!message.contains("a b"), "{message}");
        // 読める変数が後にあれば、それを使う（ureq と同じ）。
        let p = resolve(
            &env(&[
                ("ALL_PROXY", "nope://x"),
                ("HTTP_PROXY", "proxy.example:3128"),
            ]),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(p.host(), "proxy.example");
    }

    #[test]
    fn environment_variables_come_before_the_windows_settings() {
        let p = resolve(
            &env(&[("HTTPS_PROXY", "http://env.example:1")]),
            Some(&windows("registry.example:2")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(p.host(), "env.example");
    }

    #[test]
    fn windows_one_proxy_for_all_protocols() {
        let p = resolve(&env(&[]), Some(&windows("proxy.corp.example:8080")))
            .unwrap()
            .unwrap();
        assert_eq!(
            host_port(&p),
            (ProxyProtocol::Http, "proxy.corp.example".to_string(), 8080)
        );
        let p = resolve(&env(&[]), Some(&windows("http://proxy.corp.example:3128")))
            .unwrap()
            .unwrap();
        assert_eq!(p.port(), 3128);
    }

    #[test]
    fn windows_per_protocol_settings_use_the_https_entry() {
        // ureq の win-system-proxy は、この形を読めずに（':' が 2 つ）直接つないでいた。
        for server in [
            "http=proxy.corp.example:8080;https=secure.corp.example:8443",
            "ftp=ftp.corp.example:21; https=secure.corp.example:8443 ;http=proxy.corp.example:8080",
            "HTTPS=secure.corp.example:8443",
            "https=http://secure.corp.example:8443;socks=socks.corp.example:1080",
        ] {
            let p = resolve(&env(&[]), Some(&windows(server)))
                .unwrap()
                .unwrap_or_else(|| panic!("{server}"));
            assert_eq!(
                host_port(&p),
                (ProxyProtocol::Http, "secure.corp.example".to_string(), 8443),
                "{server}"
            );
        }
    }

    #[test]
    fn windows_per_protocol_settings_without_https_use_socks_or_connect_directly() {
        let p = resolve(
            &env(&[]),
            Some(&windows(
                "http=proxy.corp.example:8080;socks=socks.corp.example:1080",
            )),
        )
        .unwrap()
        .unwrap();
        // SOCKS は UreqClient が誤りにする（直接つながない）。
        assert_eq!(
            host_port(&p),
            (
                ProxyProtocol::Socks4,
                "socks.corp.example".to_string(),
                1080
            )
        );
        // https にプロキシを割り当てていない設定（http・ftp だけ）は、プロキシを使わない。
        assert_eq!(
            resolve(
                &env(&[]),
                Some(&windows(
                    "http=proxy.corp.example:8080;ftp=ftp.corp.example:21"
                ))
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn unreadable_windows_settings_are_errors_instead_of_connecting_directly() {
        for server in ["", "  ", "https=", "https=a b:1", "a b:1", "http=x:1;oops"] {
            let e = resolve(&env(&[]), Some(&windows(server))).unwrap_err();
            let GeoError::Network(message) = &e else {
                panic!("{server}: {e:?}");
            };
            assert!(message.contains("ProxyServer"), "{server}: {message}");
        }
        let missing = WindowsInternetSettings {
            proxy_enable: true,
            ..Default::default()
        };
        assert!(resolve(&env(&[]), Some(&missing)).is_err());
    }

    #[test]
    fn windows_disabled_proxy_and_auto_config_scripts_mean_direct() {
        let disabled = WindowsInternetSettings {
            proxy_enable: false,
            proxy_server: Some("proxy.corp.example:8080".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve(&env(&[]), Some(&disabled)).unwrap(), None);
        // PAC は読まない（直接つなぐ。制限としてモジュールの doc に書いた）。
        let pac = WindowsInternetSettings {
            auto_config_url: Some("http://wpad.corp.example/proxy.pac".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve(&env(&[]), Some(&pac)).unwrap(), None);
        // 手動のプロキシも有効なら、それを使う。
        let both = WindowsInternetSettings {
            auto_config_url: Some("http://wpad.corp.example/proxy.pac".to_string()),
            ..windows("proxy.corp.example:8080")
        };
        assert_eq!(
            resolve(&env(&[]), Some(&both)).unwrap().unwrap().host(),
            "proxy.corp.example"
        );
    }

    #[test]
    fn windows_proxy_override_is_used_unless_no_proxy_is_set() {
        let settings = WindowsInternetSettings {
            proxy_override: Some("<local>;*.corp.example; 10.*".to_string()),
            ..windows("proxy.corp.example:8080")
        };
        let p = resolve(&env(&[]), Some(&settings)).unwrap().unwrap();
        assert!(p.is_no_proxy(&uri("https://tiles.corp.example/")));
        assert!(p.is_no_proxy(&uri("https://10.1.2.3/")));
        assert!(!p.is_no_proxy(&uri("https://tile.openstreetmap.org/")));
        // 環境変数の NO_PROXY があれば、そちらを使う（ureq と同じ）。
        let p = resolve(
            &env(&[("NO_PROXY", "tile.openstreetmap.org")]),
            Some(&settings),
        )
        .unwrap()
        .unwrap();
        assert!(p.is_no_proxy(&uri("https://tile.openstreetmap.org/")));
        assert!(!p.is_no_proxy(&uri("https://tiles.corp.example/")));
    }
}
