//! GenzoParis のデスクトップアプリの入口（処理は lib の `genzo_app::run`）。

// Windows の配布版でコンソールのウィンドウを出さない。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    genzo_app::run();
}
