# GenzoParis

Lightroom Classic 相当の写真現像（RAW 現像・非破壊編集）とカタログ管理を行う、Windows / macOS 向けのデスクトップアプリです。

現在は MVP（ロードマップの M5）の実装を終え、**人による実機での受け入れの確認の前** の段階です。コアの crate（現像エンジン・カタログ・ワーカー・コア API）、検証用の CLI（`genzo`）、デスクトップアプリ（`genzo-app`。Tauri 2 ＋ React / TypeScript。取り込み・選別・絞り込み・現像・書き出し・バックアップと復元）を実装しました。動作は Linux（Xvfb と WebKitWebDriver）の E2E のシナリオで確かめています。配布物（Windows のインストーラー、macOS 26 以降向けの .dmg）は GitHub Actions で作り、ワーカーが同梱の LibRaw で起動することまで確かめました（2026-10-12）。**Windows / macOS の実機での確認と PoC の計測はこれから** です（手順は [docs/m5_acceptance.md](docs/m5_acceptance.md)・[docs/packaging.md](docs/packaging.md)）。進捗と、人の判断・確認が必要な事項は [docs/implementation_status.md](docs/implementation_status.md) にあります。

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
| [docs/m5_acceptance.md](docs/m5_acceptance.md) | M5（MVP）の受け入れの確認の手順（Windows / macOS の実機で人が行う） |
| [docs/packaging.md](docs/packaging.md) | 配布物（インストーラー）の作り方・入れ方・注意 |

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
| Windows | vcpkg（`vcpkg install libraw:x64-windows`）などで入手し、環境変数 `LIBRAW_INCLUDE_DIR`・`LIBRAW_LIB_DIR`（必要なら `LIBRAW_LIB_NAME`）で指定する（下の手順。**実機で未確認**） |

```sh
cargo test --workspace --features genzo-cli/libraw
```

Windows（PowerShell）の手順（実機で未確認。詳しくは `crates/genzo-raw/build.rs` の doc）:

```powershell
vcpkg install libraw:x64-windows
$env:LIBRAW_INCLUDE_DIR = "C:\vcpkg\installed\x64-windows\include"   # libraw\libraw.h のある include
$env:LIBRAW_LIB_DIR     = "C:\vcpkg\installed\x64-windows\lib"
$env:LIBRAW_LIB_NAME    = "<.lib のファイル名から .lib を除いたもの>"   # 既定は libraw。入手したファイル名を確かめる
cargo build -p genzo-cli --features libraw
```

実行するときは、LibRaw の DLL（と、その依存の DLL）を、ワーカーとして動く実行ファイル（開発用の CLI では `genzo.exe`、配布するアプリでは `genzo-worker.exe`）と同じフォルダか `PATH` の通ったフォルダに置いてください。vcpkg が作る `.lib` のファイル名と、スレッドセーフな版かどうかは未確認です（[docs/third_party.md](docs/third_party.md) の 3.1 節）。CI の Windows は LibRaw なしの構成だけを確かめます。

`genzo-cli/libraw` を指定すると、`genzo-api`・`genzo-worker`・`genzo-raw` の `libraw` も有効になります。LibRaw は pkg-config で `libraw_r` を探し、なければ `libraw` を使います（`libraw` はスレッドセーフでないため、LibRaw の使用をプロセスの中で 1 つずつに制限します）。

開発用の CLI の `genzo` は、自分自身を `genzo __worker` で起動してワーカーを兼ねます。そのため、`libraw` を有効にした `genzo` は LibRaw の共有ライブラリに依存します（Linux の `ldd` で確認）。LibRaw の関数を呼ぶのは、ワーカーとして動くプロセスだけです。**配布するアプリでは、ワーカーを別の実行ファイル（`genzo-worker`）にし、本体の実行ファイルは LibRaw に依存させません**（[決定事項ログ](docs/decision_log.md) No.32。`genzo-api` の `WorkerLaunch::Executable`。既定の設定）。Cargo の機能フラグは同じビルドの中で共通になるので、本体は `libraw` なしでビルドし、ワーカーは別に `cargo build --release -p genzo-worker --features libraw` でビルドします（Linux の `ldd` で、`genzo-worker` だけが `libraw_r.so.23` に依存することを確認）。LibRaw のライセンスは LGPL-2.1 を選ぶ方針ですが、**法的な事項のため、公開・配布の前に人（必要なら専門家）の確認が必要**です（[docs/third_party.md](docs/third_party.md) の 3.1 節、[docs/implementation_status.md](docs/implementation_status.md) の「人の判断・確認が必要な事項」の No.4）。

### 実機のサンプルの回帰テスト（任意）

手持ちの ARW などで展開・既定の切り抜き・飽和・壊れたファイルの扱い・既定の現像の統計を確かめます（`crates/genzo-pipeline/tests/private_samples.rs`。LibRaw が必要）。フォルダは **リポジトリの外** に置いてください。サンプルとそこから作った画像はリポジトリに入れません（`.gitignore` に `*.ARW`・`*.arw` があります。期待値の `crates/genzo-pipeline/tests/private_samples.json` には、ハッシュと写真の内容を明かさない数値だけを置きます。撮影日時・GPS・撮影場所も書きません）。サンプルはハッシュ（SHA-256）で期待値と対応づけ、期待値のないファイルは共通の性質（展開の成否・既定の切り抜きの範囲・端の帯がないことなど）だけを確かめます。壊したファイルは OS の一時フォルダに作り、終わったら消します。
`GENZO_PRIVATE_SAMPLES=<フォルダ> cargo test -p genzo-pipeline --features libraw --release --test private_samples -- --nocapture`（未指定なら何もせずに成功。`GENZO_REQUIRE_PRIVATE_SAMPLES=1` で実行できない場合を失敗に、`GENZO_UPDATE_PRIVATE_EXPECT=1` で期待値を書き直し）。
Windows（PowerShell）では `$env:GENZO_PRIVATE_SAMPLES="<フォルダ>"` を先に設定してください。期待値を書き直したときは、git の差分を確かめ、数値を別の人が確認してからコミットしてください。CI では実行しません（手で実行するか自前のランナーに入れるかは未決。[docs/implementation_status.md](docs/implementation_status.md) の表 No.58）。

### デスクトップアプリ（`genzo-app`）

Tauri 2（`crates/genzo-app`）と React / TypeScript の画面（`ui/`）です（[決定事項ログ](docs/decision_log.md) No.60〜66）。Node 22 と、Tauri の前提のパッケージ（Linux は GTK・WebKitGTK の開発用のパッケージ。Tauri の資料の「Prerequisites」）が要ります。

```sh
cd ui
npm ci
npm test                 # 画面のテスト（vitest）
npm run build            # 型の検査と画面のビルド（ui/dist）
# 開発用の起動: ワーカー（LibRaw あり）を先にビルドしておく（本体は LibRaw に依存しない。決定事項ログ No.32）
cargo build -p genzo-worker --features libraw
npm run app:dev
```

- 本体（`genzo-paris`）は、ワーカー（`genzo-worker`）を「環境変数 `GENZO_WORKER_PATH` → 本体と同じフォルダ」の順に探します。開発用の起動では、同じ `target/debug` に置かれたものを使います。
- 配布物の作り方（`npm run app:worker`・`npm run app:build`、GitHub Actions の `release.yml`）と、FFmpeg・署名・ライセンスの注意は [docs/packaging.md](docs/packaging.md) を見てください。**`release.yml` は手動でだけ動き、公開のリポジトリでは、成果物を誰でも取得できる状態にしてよいかの確認の印がないと止まります**（2.5 節）。
- E2E（Linux の開発環境用）: `e2e/run.sh <アプリの実行ファイル> e2e/scenario.mjs [出力のフォルダ]`。Xvfb・WebKitWebDriver・`tauri-driver` が要ります（`e2e/run.sh` の冒頭）。写真のフォルダは `cargo run -p genzo-testkit --example e2e_fixtures -- <フォルダ>` で作り、環境変数 `E2E_PHOTOS` で渡します。アプリは `npm --prefix ui run build` の後に `cargo build -p genzo-app --features custom-protocol` で、ワーカーは同じフォルダに `cargo build -p genzo-worker --features libraw` でビルドします。Windows・macOS の WebView の確認の代わりにはなりません。

### CLI（`genzo`）

検証用の CLI です（ORG-05。UI なしで登録・検索・現像・書き出し・計測を行う）。結果は表か `--json`、進捗と警告は標準エラー、終了コードは 0（成功）・1（エラー。`trash` を `--yes` なしで中止した場合を含む）・2（使い方の誤り）・3（結果は出したが一部が失敗した・問題が見つかった: 書き出し・削除・サムネイルの作り直しの一部の失敗、取り込みで登録できなかったファイル、`catalog check` で見つかった問題、`bench` での悪化）・130（Ctrl+C で中断。1 回目は実行中の処理を取り消してカタログを閉じてから、2 回目はすぐに終了。Windows でも 130）です（[決定事項ログ](docs/decision_log.md) No.52）。スクリプトからは、3 のときに結果の JSON を確かめてください。詳しくは `genzo --help` と `crates/genzo-cli/src/lib.rs` の doc を見てください。

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
- `import` は、取り込みの後に現像結果からサムネイル・プレビューを作り直しません（埋め込みの JPEG のまま。現像設定を変えたときに作ります）。作り直すときは `--previews` を付けてください（決定事項ログ No.49）。
- `--gpu auto`（既定）でも、ソフトウェアの GPU（Mesa の llvmpipe・Windows の WARP など）は使わず CPU 版で処理します。使うときは環境変数 `GENZO_GPU_ALLOW_SOFTWARE=1` を付けてください（決定事項ログ No.46）。
- `develop region <ID>` で、表示範囲の描画（ルーペの等倍表示・拡大の入口。`Core::render_region`）を試せます（決定事項ログ No.58。既にあるファイルには書きません）。
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

- Linux: フォーマットの確認と cargo-deny。画面（`ui/`）の型の検査・テスト・ビルドと、配布のスクリプト（`scripts/`）のテスト
- Windows と macOS（Apple Silicon）: clippy（警告をエラーとして扱う）とテスト（デスクトップアプリの `genzo-app` を含む）。macOS では LibRaw を入れて、機能フラグ `libraw` を有効にした確認も行う

外部のアクションはコミットの SHA で固定しています（[docs/packaging.md](docs/packaging.md) の 2.5 節）。配布物は別のワークフロー（`release.yml`。手動で実行）で作ります。

## ライセンス

GenzoParis is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

全文は [LICENSE](LICENSE) を参照してください。

SPDX-License-Identifier: GPL-3.0-or-later
