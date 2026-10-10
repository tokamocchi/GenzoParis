//! genzo-raw のビルドスクリプト。
//!
//! 機能フラグ `libraw` が有効なときだけ、LibRaw を探して C++ のシム
//! （`src/shim/genzo_libraw_shim.cpp`）を cc でビルドする。既定（`libraw` なし）では何もしない。
//!
//! # LibRaw の探し方
//!
//! 1. pkg-config で `libraw_r`（スレッドセーフ版）を探す。なければ `libraw` を探す。
//!    pkg-config を使わない場合は、pkg-config crate の環境変数
//!    `LIBRAW_R_NO_PKG_CONFIG` / `LIBRAW_NO_PKG_CONFIG` を設定する。
//! 2. 見つからなければ、環境変数を使う（Windows 向け）。
//!    - `LIBRAW_INCLUDE_DIR`: `libraw.h` のあるディレクトリ、または `libraw/libraw.h` のある
//!      ディレクトリ（LibRaw のソースの最上位）。両方を include パスに加える。
//!    - `LIBRAW_LIB_DIR`: ライブラリのあるディレクトリ。
//!    - `LIBRAW_LIB_NAME`（任意）: リンクするライブラリの名前。既定は Windows で `libraw`、
//!      それ以外で `raw_r`。
//!    - `LIBRAW_STATIC`（任意）: `1` なら静的ライブラリとしてリンクし、シムを `LIBRAW_NODLL`
//!      付きでコンパイルする（Windows の LibRaw の静的ビルド用。未確認）。
//!
//! # リンクするライブラリ
//!
//! pkg-config の結果のうち、LibRaw 自身（`raw_r` / `raw`）とライブラリの検索パスだけを
//! Cargo に渡す。pkg-config は `Requires: lcms2` からシステムの lcms2 も返すが、それを本体の
//! 実行ファイルに直接リンクすると、`genzo-color` が同梱のソースから静的にビルドする lcms2
//! （docs/third_party.md の 3.2 節）の代わりにシステムの lcms2 が使われるおそれがあるため、
//! 渡さない。LibRaw の共有ライブラリは自分の依存（lcms2・OpenMP など）を自分で読み込む。
//! LibRaw を静的にリンクする場合（`LIBRAW_STATIC=1`）は、依存するライブラリを
//! `RUSTFLAGS` などで別に指定する必要がある（未確認）。

use std::env;
use std::path::PathBuf;

/// シムのソース。
const SHIM_SOURCE: &str = "src/shim/genzo_libraw_shim.cpp";
/// シムのヘッダ。
const SHIM_HEADER: &str = "src/shim/genzo_libraw_shim.h";
/// 必要な LibRaw の最小の版（04 の 1.2 節の前提は 0.21 系。docs/third_party.md）。
const MIN_LIBRAW_VERSION: &str = "0.21";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if env::var_os("CARGO_FEATURE_LIBRAW").is_none() {
        return;
    }
    println!("cargo::rerun-if-changed={SHIM_SOURCE}");
    println!("cargo::rerun-if-changed={SHIM_HEADER}");
    for var in [
        "LIBRAW_INCLUDE_DIR",
        "LIBRAW_LIB_DIR",
        "LIBRAW_LIB_NAME",
        "LIBRAW_STATIC",
    ] {
        println!("cargo::rerun-if-env-changed={var}");
    }

    let found = find_with_pkg_config()
        .or_else(find_with_env)
        .unwrap_or_else(|| {
            panic!(
                "LibRaw が見つかりません。pkg-config で libraw_r / libraw を探せるようにするか、\
             環境変数 LIBRAW_INCLUDE_DIR と LIBRAW_LIB_DIR を設定してください（0.21 以降）。"
            )
        });

    for dir in &found.link_dirs {
        println!("cargo::rustc-link-search=native={}", dir.display());
    }
    let kind = if found.static_link { "static" } else { "dylib" };
    println!("cargo::rustc-link-lib={kind}={}", found.lib_name);

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++14")
        .file(SHIM_SOURCE)
        .include("src/shim");
    for dir in &found.include_dirs {
        build.include(dir);
    }
    if found.static_link {
        build.define("LIBRAW_NODLL", None);
    }
    build.compile("genzo_libraw_shim");
}

/// 見つかった LibRaw。
struct LibRawLocation {
    include_dirs: Vec<PathBuf>,
    link_dirs: Vec<PathBuf>,
    lib_name: String,
    static_link: bool,
}

/// pkg-config で `libraw_r`、なければ `libraw` を探す。
fn find_with_pkg_config() -> Option<LibRawLocation> {
    let mut errors = Vec::new();
    for name in ["libraw_r", "libraw"] {
        let probe = pkg_config::Config::new()
            .atleast_version(MIN_LIBRAW_VERSION)
            .cargo_metadata(false)
            .env_metadata(true)
            .probe(name);
        let lib = match probe {
            Ok(lib) => lib,
            Err(e) => {
                errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        // LibRaw 自身のライブラリ（raw_r / raw）だけを選ぶ（lcms2 などは渡さない。冒頭の説明）。
        let lib_name = lib
            .libs
            .iter()
            .find(|l| l.as_str() == "raw_r" || l.as_str() == "raw")
            .cloned()
            .unwrap_or_else(|| if name == "libraw_r" { "raw_r" } else { "raw" }.to_owned());
        return Some(LibRawLocation {
            include_dirs: lib.include_paths,
            link_dirs: lib.link_paths,
            lib_name,
            static_link: false,
        });
    }
    for e in errors {
        // 環境変数での指定に切り替える前に、pkg-config で見つからなかった理由を残す。
        println!(
            "cargo::warning=pkg-config で LibRaw が見つかりません（{}）",
            e.replace('\n', " ")
        );
    }
    None
}

/// 環境変数 `LIBRAW_INCLUDE_DIR` / `LIBRAW_LIB_DIR` から作る。
fn find_with_env() -> Option<LibRawLocation> {
    let include = PathBuf::from(env::var_os("LIBRAW_INCLUDE_DIR")?);
    let lib_dir = PathBuf::from(env::var_os("LIBRAW_LIB_DIR")?);
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let default_name = if target_os == "windows" {
        "libraw"
    } else {
        "raw_r"
    };
    let lib_name = env::var("LIBRAW_LIB_NAME").unwrap_or_else(|_| default_name.to_owned());
    let static_link = env::var("LIBRAW_STATIC").is_ok_and(|v| v == "1");
    // libraw.h を直接含むディレクトリと、libraw/libraw.h を含むディレクトリの両方に対応する。
    let include_dirs = vec![include.join("libraw"), include];
    Some(LibRawLocation {
        include_dirs,
        link_dirs: vec![lib_dir],
        lib_name,
        static_link,
    })
}
