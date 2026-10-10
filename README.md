# GenzoParis

Lightroom Classic 相当の写真現像（RAW 現像・非破壊編集）とカタログ管理を行う、Windows / macOS 向けのデスクトップアプリです。

現在は開発の初期段階です。コアの crate（現像エンジン・カタログ・ワーカー・コア API）と検証用の CLI（`genzo`）を実装しましたが、UI はまだなく、アプリとしてはまだ使えません。Windows / macOS の実機での確認と PoC の計測もこれからです。進捗は [docs/implementation_status.md](docs/implementation_status.md) にあります。

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
| [docs/implementation_status.md](docs/implementation_status.md) | 実装の進め方と進捗、人の判断・確認が必要な事項 |
| [docs/third_party.md](docs/third_party.md) | 第三者のコード・データ・同梱物の台帳 |
| [docs/poc/README.md](docs/poc/README.md) | PoC の記録（計測と記録のルール、PoC-1〜7 の記録欄） |

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

CLI の `genzo` は、自分自身を `genzo __worker` で起動してワーカーを兼ねます（ワーカーの実行ファイルを別に配布しない）。そのため、`libraw` を有効にした `genzo` は LibRaw の共有ライブラリに依存します（Linux の `ldd` で確認）。LibRaw の関数を呼ぶのは、ワーカーとして動くプロセスだけです。配布物の構成とライセンスの扱いは未定です（[docs/third_party.md](docs/third_party.md) の 3.1 節、[docs/implementation_status.md](docs/implementation_status.md) の「人の判断・確認が必要な事項」の No.4）。

### CLI（`genzo`）

検証用の CLI です（ORG-05。UI なしで登録・検索・現像・書き出し・計測を行う）。結果は表か `--json`、進捗と警告は標準エラー、終了コードは 0（成功）・1（エラー）・2（使い方の誤り）・130（Ctrl+C で中断。1 回目は実行中の処理を取り消してカタログを閉じてから、2 回目はすぐに終了）です。詳しくは `genzo --help` と `crates/genzo-cli/src/lib.rs` の doc を見てください。

```sh
export GENZO_CATALOG=/path/to/catalog.db      # または --catalog
cargo run -p genzo-cli -- catalog init
cargo run -p genzo-cli -- import ~/Pictures/2024-05-01
cargo run -p genzo-cli -- search --min-rating 3 --json
echo '{"exposure_ev": 0.5}' | cargo run -p genzo-cli -- develop set 12 --json -
cargo run -p genzo-cli -- export 12 --out ./out --long-edge 2048 --remove-gps
cargo run -p genzo-cli --features libraw -- render DSC00001.ARW --out out.tif   # カタログなしで 1 枚（RAW は LibRaw が必要）
cargo run --release -p genzo-cli -- bench preview --synthetic 7008x4672 --out bench-results
```

- `--json` のときは、失敗（使い方の誤りを含む）も標準出力に `{"error": {"kind": ...}}` の JSON で出します。
- Windows: 現像設定の JSON は、引数に直接書かずにファイル（`--json settings.json`）で渡すことをおすすめします（PowerShell・cmd.exe では引用符の扱いが違うため）。ファイルは UTF-8（BOM 付きも可）か BOM 付きの UTF-16 で読めます。PowerShell で `--json` の出力を受け取るときに日本語が化ける場合は、`[Console]::OutputEncoding = [Text.Encoding]::UTF8` を先に実行してください（出力は UTF-8）。
- `bench` は release ビルドで実行してください（debug ビルドの時間は記録に「debug」と残り、参考値になります）。

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
