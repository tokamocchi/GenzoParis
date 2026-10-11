//! 計測の環境の情報（05 の 1.8 節「環境: OS のバージョン、GPU とドライバーのバージョン、主要な
//! ライブラリのバージョン、ビルドの設定」）。
//!
//! OS・CPU・ビルドの設定は [`EnvironmentInfo::detect`] で自動で取る（取れない項目は `None`）。
//! GPU・ドライバー・ライブラリの版は、この crate からは分からないので、呼び出し側が設定する
//! （例: genzo-gpu のアダプタの情報、genzo-raw の `libraw_version()`）。
//!
//! 公開リポジトリに結果を置くことがあるため、ホスト名・ユーザー名など個人を特定できる情報は記録しない。

use std::collections::BTreeMap;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// コミットの識別子を渡す環境変数（CI では `GITHUB_SHA` も使う）。
pub const BENCH_COMMIT_ENV: &str = "GENZO_BENCH_COMMIT";

/// ビルドの設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildProfile {
    /// デバッグ（`debug_assertions` が有効）。時間は参考にしかならない。
    Debug,
    /// リリース（`debug_assertions` が無効）。
    Release,
}

impl BuildProfile {
    /// この crate をビルドした設定（`cfg!(debug_assertions)` で判定する）。
    pub const fn current() -> Self {
        if cfg!(debug_assertions) {
            BuildProfile::Debug
        } else {
            BuildProfile::Release
        }
    }
}

/// 計測の環境。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    /// OS（`std::env::consts::OS`。例: `"windows"`・`"macos"`・`"linux"`）。
    pub os: String,
    /// OS のバージョン（取れなければ `None`）。
    #[serde(default)]
    pub os_version: Option<String>,
    /// CPU のアーキテクチャ（`std::env::consts::ARCH`。例: `"x86_64"`・`"aarch64"`）。
    pub arch: String,
    /// CPU の名前（取れなければ `None`）。
    #[serde(default)]
    pub cpu_model: Option<String>,
    /// 使える論理 CPU の数（`std::thread::available_parallelism`。コンテナの制限を反映する）。
    pub logical_cpus: u32,
    /// ビルドの設定。
    pub build_profile: BuildProfile,
    /// GPU の名前（呼び出し側が設定する）。
    #[serde(default)]
    pub gpu: Option<String>,
    /// GPU のドライバーのバージョン（呼び出し側が設定する）。
    #[serde(default)]
    pub gpu_driver: Option<String>,
    /// 主要なライブラリのバージョン（例: `"libraw" → "0.21.2"`）。
    #[serde(default)]
    pub libraries: BTreeMap<String, String>,
    /// コミットの識別子（環境変数 `GENZO_BENCH_COMMIT`、なければ `GITHUB_SHA`）。
    #[serde(default)]
    pub commit: Option<String>,
    /// その他の情報（電源の状態、表示倍率など）。
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl EnvironmentInfo {
    /// 今の環境の情報を取る。
    pub fn detect() -> Self {
        Self {
            os: std::env::consts::OS.to_owned(),
            os_version: detect_os_version(),
            arch: std::env::consts::ARCH.to_owned(),
            cpu_model: detect_cpu_model(),
            logical_cpus: std::thread::available_parallelism()
                .map(|n| u32::try_from(n.get()).unwrap_or(u32::MAX))
                .unwrap_or(1),
            build_profile: BuildProfile::current(),
            gpu: None,
            gpu_driver: None,
            libraries: BTreeMap::new(),
            commit: non_empty_env(BENCH_COMMIT_ENV).or_else(|| non_empty_env("GITHUB_SHA")),
            extra: BTreeMap::new(),
        }
    }

    /// GPU の名前とドライバーを設定する。
    pub fn with_gpu(mut self, name: impl Into<String>, driver: Option<String>) -> Self {
        self.gpu = Some(name.into());
        self.gpu_driver = driver;
        self
    }

    /// ライブラリのバージョンを追加する。
    pub fn with_library(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.libraries.insert(name.into(), version.into());
        self
    }

    /// その他の情報を追加する。
    pub fn with_extra(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// 時間を比べてよい環境か（同じ OS・アーキテクチャ・CPU・CPU の数・ビルドの設定・GPU）。
    ///
    /// OS・ドライバー・ライブラリのバージョンとコミットは比べない（その変化による悪化を検出したい
    /// ため）。
    pub fn comparable_with(&self, other: &EnvironmentInfo) -> bool {
        self.os == other.os
            && self.arch == other.arch
            && self.cpu_model == other.cpu_model
            && self.logical_cpus == other.logical_cpus
            && self.build_profile == other.build_profile
            && self.gpu == other.gpu
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// コマンドを実行して標準出力の 1 行目を取る（失敗したら `None`）。
fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

fn detect_os_version() -> Option<String> {
    match std::env::consts::OS {
        "linux" => {
            let pretty = std::fs::read_to_string("/etc/os-release")
                .ok()
                .and_then(|t| parse_os_release_pretty_name(&t));
            let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            match (pretty, kernel) {
                (Some(p), Some(k)) => Some(format!("{p} (kernel {k})")),
                (p, k) => p.or(k),
            }
        }
        "macos" => {
            let version = command_line("sw_vers", &["-productVersion"])?;
            match command_line("sw_vers", &["-buildVersion"]) {
                Some(build) => Some(format!("macOS {version} ({build})")),
                None => Some(format!("macOS {version}")),
            }
        }
        // 例: "Microsoft Windows [Version 10.0.22631.4317]"
        "windows" => command_line("cmd", &["/C", "ver"]),
        _ => None,
    }
}

/// /etc/os-release の PRETTY_NAME を取り出す。
fn parse_os_release_pretty_name(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .find(|v| !v.is_empty())
}

fn detect_cpu_model() -> Option<String> {
    match std::env::consts::OS {
        "linux" => std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|t| parse_cpuinfo_model(&t)),
        "macos" => command_line("sysctl", &["-n", "machdep.cpu.brand_string"]),
        "windows" => non_empty_env("PROCESSOR_IDENTIFIER"),
        _ => None,
    }
}

/// /proc/cpuinfo の CPU の名前（x86 は `model name`、ARM は `Model` や `Hardware`）。
fn parse_cpuinfo_model(text: &str) -> Option<String> {
    for key in ["model name", "Model", "Hardware", "cpu model"] {
        let found = text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_owned())
        });
        if let Some(v) = found.filter(|v| !v.is_empty()) {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_fills_basic_fields() {
        let e = EnvironmentInfo::detect();
        assert_eq!(e.os, std::env::consts::OS);
        assert_eq!(e.arch, std::env::consts::ARCH);
        assert!(e.logical_cpus >= 1);
        assert_eq!(e.build_profile, BuildProfile::current());
        // テストは通常 debug でビルドする（--release のときは Release）。
        assert_eq!(
            e.build_profile == BuildProfile::Debug,
            cfg!(debug_assertions)
        );
        assert!(e.gpu.is_none());
        assert!(e.comparable_with(&e.clone()));
    }

    #[test]
    fn comparable_ignores_versions_but_not_hardware() {
        let base = EnvironmentInfo::detect();
        let newer = base
            .clone()
            .with_library("libraw", "0.22.0")
            .with_extra("note", "x");
        let mut newer = newer;
        newer.os_version = Some("other".to_owned());
        newer.commit = Some("abc".to_owned());
        assert!(base.comparable_with(&newer));
        let gpu = base.clone().with_gpu("RTX 3080", Some("560.94".to_owned()));
        assert!(!base.comparable_with(&gpu));
        let mut other = base.clone();
        other.build_profile = match base.build_profile {
            BuildProfile::Debug => BuildProfile::Release,
            BuildProfile::Release => BuildProfile::Debug,
        };
        assert!(!base.comparable_with(&other));
        let mut other = base.clone();
        other.logical_cpus += 1;
        assert!(!base.comparable_with(&other));
    }

    #[test]
    fn parsers() {
        let os = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(
            parse_os_release_pretty_name(os).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert_eq!(parse_os_release_pretty_name("ID=x\n"), None);
        let x86 = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Xeon(R) CPU\n";
        assert_eq!(
            parse_cpuinfo_model(x86).as_deref(),
            Some("Intel(R) Xeon(R) CPU")
        );
        let arm = "processor\t: 0\nBogoMIPS\t: 48.00\n\nModel\t\t: Raspberry Pi 4 Model B\n";
        assert_eq!(
            parse_cpuinfo_model(arm).as_deref(),
            Some("Raspberry Pi 4 Model B")
        );
        assert_eq!(parse_cpuinfo_model("processor : 0\n"), None);
    }

    #[test]
    fn json_roundtrip() {
        let e = EnvironmentInfo::detect()
            .with_gpu("llvmpipe", None)
            .with_library("lcms2", "2.16");
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"build_profile\":\""));
        let back: EnvironmentInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }
}
