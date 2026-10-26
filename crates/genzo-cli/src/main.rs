//! 検証用の CLI（`genzo`）。処理は lib（`genzo_cli`）にある（ORG-05。04 の 1.4 節）。

fn main() -> std::process::ExitCode {
    // AI マスクの ONNX Runtime のテレメトリー（Microsoft への送信）を止める（genzo-ml の onnx モジュールの doc）。
    // SAFETY: main の最初で、まだほかのスレッドはない。
    unsafe { genzo_api::disable_onnxruntime_telemetry() };
    genzo_cli::main_entry()
}
