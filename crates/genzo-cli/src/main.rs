//! 検証用の最小 CLI（`genzo`）。
//!
//! 登録・検索・現像・書き出し・計測をコマンドから実行する（ORG-05）。自動テストと、
//! AI による動作確認に使う（docs/04_architecture.md の 1.4 節）。

fn main() {
    println!("genzo {}", env!("CARGO_PKG_VERSION"));
}
