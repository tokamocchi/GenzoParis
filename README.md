# GenzoParis

Lightroom Classic 相当の写真現像（RAW 現像・非破壊編集）とカタログ管理を行う、Windows / macOS 向けのデスクトップアプリです。

現在は開発の初期段階です（コアの crate を実装中。アプリとしてはまだ使えません）。進捗は [docs/implementation_status.md](docs/implementation_status.md) にあります。

## 設計資料

| 資料 | 内容 |
|---|---|
| [prompts/premise_sheet.md](prompts/premise_sheet.md) | 前提条件 |
| [docs/01_feature_inventory.md](docs/01_feature_inventory.md) | 機能の棚卸しと MVP の範囲 |
| [docs/02_nonfunctional_requirements.md](docs/02_nonfunctional_requirements.md) | 非機能要件 |
| [docs/03_tech_selection.md](docs/03_tech_selection.md) | 技術選定 |
| [docs/04_architecture.md](docs/04_architecture.md) | アーキテクチャ設計 |
| [docs/05_poc_and_roadmap.md](docs/05_poc_and_roadmap.md) | PoC 計画とロードマップ |
| [docs/06_design_review.md](docs/06_design_review.md) | 設計レビュー |
| [docs/07_review_response.md](docs/07_review_response.md) | 設計レビューへの対応 |
| [docs/decision_log.md](docs/decision_log.md) | 決定事項ログ |

## 開発

Rust の Cargo workspace です（crate の構成は [docs/04_architecture.md](docs/04_architecture.md) の 1.4 節）。Rust の版は `rust-toolchain.toml` で 1.97.0 に固定しています（手元と CI で clippy の警告をそろえるため。`Cargo.toml` の最小の版は 1.88）。

### ビルドとテスト

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

### LibRaw（機能フラグ `libraw`）

RAW の展開には LibRaw（0.21 以降。スレッドセーフ版の `libraw_r`）を使います。機能フラグ `libraw` は既定では無効で、LibRaw がなくてもビルドとテストができます。LibRaw の関数を呼ぶのはワーカープロセスだけで、本体からは呼びません（[docs/04_architecture.md](docs/04_architecture.md) の 1.2 節）。

| OS | LibRaw の導入方法 |
|---|---|
| Ubuntu | `sudo apt install libraw-dev pkg-config` |
| macOS | `brew install libraw pkgconf` |
| Windows | 未定（環境変数 `LIBRAW_INCLUDE_DIR`・`LIBRAW_LIB_DIR` で指定する方法を用意しているが、未確認。`crates/genzo-raw/build.rs`） |

```sh
cargo test --workspace --features genzo-cli/libraw
```

`genzo-cli/libraw` を指定すると、`genzo-api`・`genzo-worker`・`genzo-raw` の `libraw` も有効になります。LibRaw は pkg-config で `libraw_r` を探し、なければ `libraw` を使います（`libraw` はスレッドセーフでないため、LibRaw の使用をプロセスの中で 1 つずつに制限します）。

Cargo の機能フラグは同じビルドの中で共通になるため、`libraw` を有効にすると、本体の実行ファイル（`genzo`）が使う `genzo-raw` でも `libraw` が有効になります。Linux では本体の実行ファイルが LibRaw の共有ライブラリに依存しないことを確認しましたが、macOS・Windows は未確認です（[docs/third_party.md](docs/third_party.md) の 3.1 節）。

### 依存ライブラリのライセンスと脆弱性の確認

```sh
cargo install cargo-deny --locked
cargo deny check
```

設定は [deny.toml](deny.toml) です。cargo-deny で確認できない C / C++ のライブラリ・同梱データ・外部の実行ファイル（FFmpeg）は、[docs/third_party.md](docs/third_party.md) の台帳で管理します。

### CI

GitHub Actions（[.github/workflows/ci.yml](.github/workflows/ci.yml)）で、push と pull request のたびに次を実行します。

- Linux: フォーマットの確認と cargo-deny
- Windows と macOS（Apple Silicon）: clippy（警告をエラーとして扱う）とテスト。macOS では LibRaw を入れて、機能フラグ `libraw` を有効にした確認も行う

## ライセンス

GenzoParis is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

全文は [LICENSE](LICENSE) を参照してください。

SPDX-License-Identifier: GPL-3.0-or-later
