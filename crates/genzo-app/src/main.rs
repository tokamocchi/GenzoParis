//! GenzoParis のデスクトップアプリの入口（処理は lib の `genzo_app::run`）。

// Windows の配布版でコンソールのウィンドウを出さない。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // AI マスクの ONNX Runtime のテレメトリー（Microsoft への送信）を止める。環境変数を変えるので、ほかのスレッドが
    // 動き出す前に行う（genzo-ml の onnx モジュールの doc。決定事項ログ No.216）。
    // SAFETY: main の最初で、まだほかのスレッドはない。
    unsafe { genzo_api::disable_onnxruntime_telemetry() };
    genzo_app::run();
}
