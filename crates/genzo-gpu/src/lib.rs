//! 現像パイプラインの GPU 版（wgpu・WGSL）。
//!
//! `genzo-pipeline` が定義する処理ステージの GPU 実装と、CPU 版との一致テスト
//! （docs/04_architecture.md の 2.3 節）を担当する。GPU を使う処理は本体プロセスだけで
//! 行い、GPU スレッド 1 本がデバイスとキューを専有する（04 の 1.2 節・1.3 節）。
//!
//! # 依存の向き（04 の 1.4 節の図との違い）
//! 1.4 節の図は `genzo-pipeline` → `genzo-gpu` の向きだが、この workspace では
//! **`genzo-gpu` が `genzo-pipeline` に依存する**。実装の計画（docs/implementation_status.md）で
//! CPU 基準実装（`genzo-pipeline`）を先に作り、GPU 版と CPU 版との一致テストをこの crate に
//! 置くため。`genzo-pipeline` は wgpu に依存しない。そのため、7.1 節の `Stage::gpu()` が返す
//! GPU 版の抽象は、wgpu の型を使わずに `genzo-pipeline` に置くか、`Stage::gpu()` の代わりに
//! この crate の登録表でステージの ID と GPU 版を対応付ける（どちらにするかは、
//! `genzo-pipeline` で `Stage` を定義するときに決める）。
