//! genzo-raw のビルドスクリプト。
//!
//! 機能フラグ `libraw` が有効なときに、システムの LibRaw を pkg-config で探し、
//! C++ のシムを cc でビルドする（LibRaw の FFI の作業で実装する）。
//! 既定（`libraw` なし）では何もしない。

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_LIBRAW").is_some() {
        // LibRaw の FFI はまだない。FFI の作業で、ここに pkg-config と cc の処理を書く。
    }
}
