# 第三者のコード・データ・同梱物の台帳

M0 のタスク 6b（[05 PoC 計画とロードマップ](05_poc_and_roadmap.md) の 3 章）と、設計レビューの R-15（[06 設計レビュー](06_design_review.md)）に対応する台帳です。
cargo-deny では確認できない **C / C++ のライブラリ、同梱データ、外部の実行ファイル** と、移植・参考にするアルゴリズムの出典を記録します。

> **ライセンスの判断は人が最終確認します。**
> この台帳は AI が作成した記録で、個々のコードやデータの法的な適合性を確定するものではありません。
> わからないことは「要確認」と書き、推測で断定しません。特許の扱いは、ライセンスとは別の未決事項として扱います。

- 作成: 2026-10-09（AI）。2026-10-10 更新（判断の表への回答: LibRaw のライセンスの選択と Windows での指定方法、RCD の出典の調査、ColorChecker の照合、追加した依存。決定事項ログ No.32・34・37）
- アプリのライセンス: GPL-3.0-or-later（[決定事項ログ](decision_log.md) No.27、[LICENSE](../LICENSE)）

## 1. 範囲と使い方

### 1.1 Rust のクレート（cargo-deny で確認する）

- Rust のクレートのライセンス・既知の脆弱性・取得元は、**cargo-deny で機械的に確認** します（[02 非機能要件](02_nonfunctional_requirements.md) の SEC-04）。この台帳には個別に書きません。
  - 設定: [deny.toml](../deny.toml)。許可するライセンスは GPL-3.0-or-later と両立するものだけです。新しいライセンスを許可するときは、両立するかを人が確認します。
  - CI: [.github/workflows/ci.yml](../.github/workflows/ci.yml) の `lint` ジョブで、push と pull request のたびに実行します。
  - ローカル: `cargo install cargo-deny --locked` で入れて、`cargo deny check` を実行します。ライセンスごとの一覧は `cargo deny list` で出せます。
- 2026-10-09 の確認結果: cargo-deny 0.20.2 で `cargo deny check` を実行し、advisories・bans・licenses・sources がすべて通りました。警告は、同じクレートの複数の版（hashbrown・miniz_oxide・syn）の 3 件だけです。
- 2026-10-10 の確認結果: `genzo-color`・`genzo-raw`・`genzo-media`・`genzo-catalog`・`genzo-jobs`・`genzo-testkit` の実装の後に再実行し、同じ結果でした（すべて通り、警告は同じ 3 件）。同日、`genzo-pipeline`・`genzo-worker`・`genzo-gpu` の実装の後（`naga` 30.0.1 を `genzo-gpu` の開発用の依存に、`libc` を `genzo-worker` の Linux 用の依存に追加）にも再実行し、同じ結果でした。同日、全体レビューの修正の後（`ctrlc` 3.5.2 を `genzo-cli` の依存に追加し、`nix` 0.31.3 が新しく依存に入った。あわせて、既に依存のツリーにあった `rustix` を `genzo-media`（Unix）・`genzo-model`（macOS など Apple）の、`sha2`・`same-file` を `genzo-api` の、`libc` を `genzo-cli` の Unix の開発用の直接の依存にした）にも再実行し、同じ結果でした（すべて通り、警告は同じ 3 件）。
- 2026-10-10（判断の表への回答の実装の後）: 次の依存を加えた後に `cargo deny check` を再実行し、advisories・bans・licenses・sources がすべて通りました（cargo-deny 0.20.2。警告は以前と同じ重複の版 3 件（hashbrown・miniz_oxide・syn））。`Cargo.lock` は依存の行が増えただけで、新しい crate は入っていません。
  - `windows-sys` 0.61.2（Microsoft の `windows-rs` の crate。`Cargo.toml` の `license` は MIT OR Apache-2.0。既に依存のツリーにあった版）を、`genzo-worker`（ジョブオブジェクト。決定事項ログ No.39）と `genzo-api`（ごみ箱の確認 `SHQueryRecycleBinW`。No.48）の Windows 用の依存に追加。
  - `libc`（既存の版）を、`genzo-worker` の Linux 用の依存から Unix 用の依存に広げた（`killpg`。macOS でも使う）。
  - `tempfile`（既存の版）を、`genzo-pipeline` の開発用の依存に追加（実機のサンプルの回帰テスト）。
  - 新しい機能フラグ: `genzo-pipeline` の `libraw`（実機のサンプルの回帰テスト用）、`genzo-catalog` の `test-support`（`genzo-api` の開発用の依存だけで有効）。
- 2026-10-12（M4 の着手。デスクトップアプリ `genzo-app` の追加）: Tauri 2（`tauri` 2.11.6・`tauri-build` 2.6.3・`tauri-plugin-dialog` 2.7.3・`tauri-plugin-opener` 2.5.5。どれも `Cargo.toml` の `license` は Apache-2.0 OR MIT）を加えた。版は、公開から 2 週間以上たったもので、workspace の最小の Rust の版（1.88）に合う 2.11 系（Cargo の解決が MSRV に合わせて選んだ版）。`cargo deny check` の結果（cargo-deny 0.20.2）: advisories・bans・licenses・sources がすべて通った。ただし `deny.toml` に次の 2 つを加えた（理由と見直しの時期は `deny.toml` に書いた）。警告（同じクレートの複数の版）は Tauri の依存で 20 件ほどに増えた。
  - **MPL-2.0 の 5 crate を crate ごとに許可**（`cssparser`・`cssparser-macros`・`selectors`・`dtoa-short`・`option-ext`。Tauri の `tauri-utils` の HTML の処理と `dirs` の依存）。5 つとも、ソース・付属のファイルに MPL-2.0 の Exhibit B（"Incompatible With Secondary Licenses"）の表示がないことを確かめた（その文言は LICENSE の全文の中の定義とひな形にだけある）。MPL-2.0 は Exhibit B のないファイルなら GPL と組み合わせられる（MPL-2.0 の 3.3 節）とされるが、**法的な最終確認は人が行う**。
  - **メンテナンス終了の通知 6 件を除外**（RUSTSEC-2024-0370 `proc-macro-error`: Linux の GTK のバインディングのビルド時のマクロで、Windows・macOS の配布物には入らない。RUSTSEC-2025-0075・0080・0081・0098・0100 `unic-*`: `tauri-utils` の `urlpattern`）。どれも脆弱性ではなく、更新先がない。Tauri を上げるときに見直す。
- 2026-10-13（v1 のパッケージ 1 infra。ログ・クラッシュレポート）: 既に workspace の依存だった `tracing-subscriber` 0.3.23 を `genzo-api` の直接の依存にし（`genzo-app` の直接の依存からは外した）、`tracing` 0.1.44（既存）を `genzo-worker` の依存に加えた。`Cargo.lock` は依存の行が変わっただけで、新しい crate は入っていない。この開発環境には cargo-deny がないため `cargo deny check` は実行していない（CI の `lint` ジョブで確かめる）。
- 2026-10-13（v1 のパッケージ 5 identity。内容による同一性・重複の検出）: 既に依存のツリーにあった `libc` 0.2.190（`genzo-worker`・`genzo-cli` と同じ版）を `genzo-api` の Unix 用の依存に加え（ボリュームがネットワークのファイルシステムかを `statfs` で確かめる）、`windows-sys` 0.61.2（既存の版）の `genzo-api` の機能に `Win32_Storage_FileSystem`（`GetDriveTypeW`。`tempfile` 3.27.0 が同じ版で既に有効にしている機能）を加えた。`Cargo.lock` は `genzo-api` の依存の行が 1 行増えただけで、新しい crate は入っていない。このコンテナには cargo-deny がないため `cargo deny check` は実行していない（CI の `lint` ジョブで確かめる）。
- 2026-10-13（v1 のパッケージ 6 import。コピー取り込み・リネーム規則・取り込み時のメタデータ）: 既に `genzo-api` の開発用の依存で、`genzo-media` が同じ版で使っている `tempfile` 3.27.0 を、`genzo-api` の通常の依存にした（コピーの一時ファイルと、上書きしない名前の変更 `persist_noclobber`。開発用の依存の行は外した）。`Cargo.lock` は変わらず、新しい crate は入っていない。このコンテナには cargo-deny がないため `cargo deny check` は実行していない（CI の `lint` ジョブで確かめる）。
- 2026-10-13（v1 のパッケージ 8 files。アプリ内でのファイル操作・外付けドライブの識別・見つからない写真）: 新しい crate は足していない。既に依存のツリーにあった `windows-sys` 0.61.2（`genzo-api`・`genzo-worker`・`tempfile` と同じ版。`Cargo.toml` の `license` は MIT OR Apache-2.0）を `genzo-media` の Windows 用の依存に加え（上書きしない名前の変更 `MoveFileExW`。機能は `Win32_Storage_FileSystem`）、`genzo-api` の `windows-sys` の機能に `Win32_System_Diagnostics_Debug`（ボリュームを調べる間の `SetThreadErrorMode`）を加えた。`genzo-media` の Unix 用の依存の `rustix`（既存）は、上書きしない名前の変更（`renameat_with`）にも使う。`Cargo.lock` は `genzo-media` の依存の行が 1 行増えただけ。このコンテナには cargo-deny がないため `cargo deny check` は実行していない（CI の `lint` ジョブで確かめる）。型の確認のため、このコンテナの rustup に Windows（x86_64-pc-windows-msvc）・macOS（aarch64-apple-darwin）向けの標準ライブラリを入れた（開発環境だけ。配布物・リポジトリには入らない）。
- **cargo-deny が見るのは、各クレートの `Cargo.toml` の `license` と、ライセンスのファイルだけです。** C のソースを同梱してビルドするクレートについて、同梱したソースの版やファイルのヘッダは確認しません。現在の依存では次の 3 つが該当します。
  - `lcms2-sys`（Little CMS 2）と `libsqlite3-sys`（SQLite）: 2 章の一覧に記録します。
  - `blake3`: BLAKE3 の作者たち自身による C とアセンブリの実装を同梱しています（`c/` ディレクトリ）。crate のライセンスは `Cargo.toml` で CC0-1.0 / Apache-2.0 / Apache-2.0 WITH LLVM-exception の選択です。`c/blake3.c` の先頭にはライセンスの表示がなく（1.8.7 で確認）、C の部分にも crate と同じライセンスが及ぶと考えていますが、要確認です。

### 1.1a 画面（`ui/`）の npm の依存（cargo-deny の対象外）

- 画面は React / TypeScript（Vite でビルド）。npm の依存は cargo-deny では確認できないため、次のルールで管理する（2026-10-12、AI 記入）。
  - `ui/package.json` で版を固定し（`save-exact`）、`ui/package-lock.json` をコミットする。CI は `npm ci` で lock のとおりに入れる。
  - 版は、公開から 2 週間以上たったものを選ぶ（公開の直後に乗っ取られた版を避けるため）。`legacy-peer-deps`（npm 10 の依存の解決の不具合の回避。`ui/.npmrc`）。
  - 新しい依存を足すときは、この節の表に追記し、ライセンスを確かめる。
- **配布物（`ui/dist` に入り、アプリに埋め込まれるもの）**: 次の 7 つ（2026-10-12 の `package-lock.json` と、各パッケージの `package.json` の `license` で確認。ファイルのヘッダ・LICENSE の本文は未確認）。

| パッケージ | 版 | ライセンス（package.json） | 用途 |
|---|---|---|---|
| react / react-dom / scheduler | 19.3.0 / 19.3.0 / 0.28.0 | MIT | 画面の部品 |
| @tanstack/react-virtual / @tanstack/virtual-core | 3.14.13 / 3.17.11 | MIT | グリッド・フィルムストリップの仮想スクロール |
| @tauri-apps/api | 2.11.1 | Apache-2.0 OR MIT | Tauri のコマンド・イベントの呼び出し |
| @tauri-apps/plugin-dialog | 2.7.3 | MIT OR Apache-2.0 | フォルダ・ファイルの選択と確認のダイアログ |

- **開発・ビルドだけで使うもの（配布物に入らない）**: Vite 8.3.1（Rolldown・lightningcss など）、@vitejs/plugin-react 6.1.1、TypeScript 6.0.3、vitest 5.0.2、@tauri-apps/cli 2.11.5 ほか。ライセンスは MIT が大半で、ほかに Apache-2.0（TypeScript・detect-libc・expect-type）、MPL-2.0（lightningcss。CSS の変換で、出力に lightningcss のコードは入らないと考えるが要確認）、ISC（picocolors）、BSD-3-Clause（source-map-js）。
- 配布物のライセンス表記（DIST-05）に、上の配布物の 7 つを含める必要がある（未対応。4 章）。

### 1.2 この台帳で管理するもの

- C / C++ のライブラリ（LibRaw など）: リンクする版、ビルドの設定、配布物に含めるか
- C のソースを同梱する Rust のクレートのうち、同梱したソースの版を記録しておくべきもの（Little CMS 2、SQLite）
- 同梱データ（カメラ行列、lensfun のデータベース、ICC プロファイルなど）
- 外部の実行ファイル（FFmpeg / ffprobe）
- 移植・参考にするアルゴリズムとコード（RCD デモザイクなど）
- テスト・評価の基準に使う公表データ（ColorChecker の基準値など）
- 開発環境だけで使うもの（配布物に含めないことを確認するため）

### 1.3 記入のルール

- 第三者のコード・データを取り込む前、または移植する前に、この台帳に記入します。**台帳への記入と人の確認が済むまで、第三者のコードを移植しません**（R-15）。
- 版は、パッケージの版（apt / Homebrew）、crate の版、またはコミットのハッシュで書きます。「最新」とは書きません。
- ライセンスは、確認した資料（個別の LICENSE ファイル、ソースのファイルのヘッダ、パッケージの copyright ファイル）を書きます。GPL / LGPL は、版だけのもの（-only）と「以降の版」を含むもの（-or-later）を区別します。
- 確認状況は「AI 記入（日付）」→「人が確認済み（日付・確認者）」の順に更新します。未確認の点は 3 章に「要確認」として挙げます。

## 2. 一覧

| 名前 | 用途 | 版またはコミット | 取得元 | ライセンス（確認した資料） | リンク方法・ビルドの設定 | 配布するバイナリと対応するソース | 必要な表記 | 確認状況 |
|---|---|---|---|---|---|---|---|---|
| LibRaw | RAW の展開とメタデータの取得。ワーカープロセスだけで使う（[04](04_architecture.md) の 1.2 節、`genzo-raw` の機能フラグ `libraw`） | 開発環境は 0.21.2。macOS の CI は実行時点の Homebrew の版（2026-10-10 の時点で 0.22 系。コミット 966b379 の記録による。CI では版を記録していない）。Windows は vcpkg などで入手する（2026-10-10 に手順を記載。版は未定・実機で未確認）。0.21 以降を想定（`genzo-raw` の build.rs で 0.21 以上を要求） | Ubuntu 24.04 の apt（`libraw-dev` 0.21.2-2.1ubuntu0.24.04.2）、macOS は Homebrew（`libraw`）、Windows は vcpkg（`libraw:x64-windows`）など（3.1 節）。上流は https://www.libraw.org/ | LGPL-2.1 または CDDL-1.0 の選択（`libraw.h` のヘッダで確認）。**LGPL-2.1 を選ぶ**（2026-10-10、AI の推奨。決定事項ログ No.32。**人・専門家の確認待ち**）。上流の配布物の LICENSE ファイルは未確認（3.1 節） | 動的リンク。pkg-config で `libraw_r`（スレッドセーフ版）を探し、なければ `libraw`（この場合は LibRaw の使用をプロセスの中で 1 つずつに制限する）。C++ のシム（`crates/genzo-raw/src/shim/`）を cc でビルドする。pkg-config が返す lcms2 はリンクしない（3.2 節）。Windows は環境変数 `LIBRAW_INCLUDE_DIR`・`LIBRAW_LIB_DIR`（必要なら `LIBRAW_LIB_NAME`）で指定する（実機で未確認） | 配布するアプリでは、ワーカーの実行ファイル（`genzo-worker`）だけが LibRaw に依存し、本体の実行ファイルは依存しない（決定事項ログ No.32）。同梱する場合の共有ライブラリと、対応するソースの提供方法は未定 | 選んだライセンス（LGPL-2.1）の全文と著作権表示（要確認） | AI 記入（2026-10-09、2026-10-10 更新）。ライセンスの選択は人・専門家の確認待ち |
| Little CMS 2 | ICC プロファイルの変換と 3D LUT の作成（`genzo-color`。04 の 2.6 節・5 章） | 2.19（`lcms2-sys` 4.0.7 が同梱するソース。Rust のラッパーは `lcms2` 6.2.0） | crates.io の `lcms2-sys`（`vendor/` に上流のソースを同梱）。上流は Marti Maria Saguer による Little CMS | MIT（`vendor/LICENSE` と、`vendor/src/*.c` のファイルのヘッダで確認）。ラッパーの crate も MIT | 静的リンク。workspace の `Cargo.toml` で `lcms2` の機能フラグ `static` を有効にし、同梱のソースを cc でビルドする（3.2 節） | 実行ファイルに含まれる。対応するソースは crates.io の `lcms2-sys` 4.0.7 | MIT の著作権表示と許諾文 | AI 記入（2026-10-09）。人の確認待ち |
| SQLite | カタログ DB とサムネイル DB（`genzo-catalog`。04 の 3 章） | 3.53.2（`libsqlite3-sys` 0.38.2 が同梱する amalgamation。`rusqlite` 0.40.2） | crates.io の `libsqlite3-sys`（`sqlite3/` に同梱）。上流は https://sqlite.org/ | パブリックドメイン（`sqlite3.c` のヘッダの「The author disclaims copyright to this source code」で確認）。バインディングの crate は MIT | 静的リンク。`rusqlite` の機能フラグ `bundled` と `backup`。コンパイルの設定は `libsqlite3-sys` の build.rs の既定（3.3 節）。システムの SQLite は使わない | 実行ファイルに含まれる。対応するソースは crates.io の `libsqlite3-sys` 0.38.2 | SQLite 自体は不要とされる（パブリックドメイン）。バインディングの MIT の表記は Rust のクレートとして扱う | AI 記入（2026-10-09）。人の確認待ち |
| FFmpeg / ffprobe | 動画のメタデータの取得とサムネイルの生成（`genzo-media`）。子プロセスとして実行する（04 の 1.2 節） | 開発環境は 6.1.1。配布する版は未定 | 開発環境は Ubuntu 24.04 の apt（`ffmpeg` 7:6.1.1-3ubuntu5）。配布物での入手方法は未定 | ビルドの構成で LGPL-2.1-or-later・GPL-2.0-or-later・(L)GPL-3.0-or-later のいずれかになり、`--enable-nonfree` のビルドは再配布できないとされる。開発環境の版は `--enable-gpl` のビルド（`ffmpeg -version` の configure の引数と、Ubuntu の copyright ファイルで確認）。上流の LICENSE.md は未確認 | リンクしない（子プロセス）。同梱する場合は configure の引数を記録する | 未定（同梱するか、利用者に入れてもらうか）。同梱する場合は対応するソースとビルドの構成の提供が必要 | 同梱する場合は、ライセンスの全文、ビルドの構成、組み込んだ外部ライブラリの表記（要確認） | AI 記入（2026-10-09）。配布の方針は人の判断待ち |
| RCD デモザイク | デモザイク（04 の 2.1 節のステージ 5。最終品質。既定のまま。決定事項ログ No.34） | CPU 版は `crates/genzo-pipeline/src/sensor/demosaic/rcd.rs`（2026-10-10。調査の時点の SHA-256 `3fe730d9b5bf8e4a876aae7d7c5bbed0075e3778c5271d1edf0dff7b471313fc`、追加したコミット 527f911）。GPU 版は未実装（書き出しのセンサー処理は CPU 版を使う）。第三者のファイルは取り込んでいない | アルゴリズムの考案者は Luis Sanz Rodríguez。考案者の実装は https://github.com/LuisSR/RCD-Demosaicing （`rcd_demosaicing.c`、Release 2.3）。改変した実装は RawTherapee・darktable・librtprocess にある（3.5 節に URL・コミット）。**`rcd.rs` は、これらの実装（特に 2021 年以降の改変版）と手順・定数・式の書き方・変数名が一致する**（2026-10-10 の AI の調査。3.5 節） | 自前のファイルとして、アプリと同じ GPL-3.0-or-later。比べた実装のファイルの表示は、RawTherapee・darktable・librtprocess が GPL-3.0-or-later、考案者の実装は LICENSE が GPL 第 3 版の全文でソースに表示なし（3.5 節）。由来の扱いと元の著作権表示の書き方は**人・専門家の確認待ち** | 該当なし（自前の Rust のコード） | アプリのソースに含まれる | 考案者の名前は doc コメントに記載済み。RawTherapee・darktable の版を参考にした実装として扱い、元の著作権表示（RawTherapee: Luis Sanz Rodriguez・Ingo Weyrich、darktable: darktable developers）と出典を `rcd.rs` のヘッダに記載した（2026-10-10。3.5 節）。書き方の妥当性は人・専門家の確認待ち | AI 記入（2026-10-10）。出典の調査は済み（2026-10-10）。扱いは人・専門家の確認待ち |
| ColorChecker Classic の基準値 | PoC-4 の色の評価の基準とテスト（`crates/genzo-testkit/src/colorchecker.rs`。2014 年 11 月以降の版と、より前の版の 24 色 × L\*a\*b\*（D50）） | X-Rite の公表値（CGATS のファイル。2014 年 11 月以降の版の測定日は 2015-04-28）。値は colour-science 0.4.6 のデータから写したもの | X-Rite（現在は Calibrite）の公表資料。2026-10-10 の照合には、GitHub 上の第三者のリポジトリにある CGATS ファイルの写しを使った（3.8 節） | 利用条件は未確認（**要確認**） | 該当なし（テストのデータ） | `genzo-testkit` はテストと計測用の crate。開発用の CLI（`genzo`）は計測の記録のために依存するが、配布するアプリに含めるかは未定（含める場合は利用条件の確認が要る） | 出典（X-Rite）を doc に記載 | AI 記入（2026-10-10）。値は写しと一致、一次資料とは未照合（人の確認待ち） |
| カメラ行列（α7 IV / α7C） | カメラ RGB から XYZ への変換（04 の 2.5 節の `render_deps`、2.6 節） | 未作成 | 候補は LibRaw の内蔵の表、DNG の ColorMatrix、カラーチャートからの自作（3.6 節） | 要確認（出典ごとに利用条件を確認する） | アプリのデータファイルとして持ち、ID とハッシュで参照する | アプリに同梱する予定 | 出典による（要確認） | 未着手 |
| Mesa（llvmpipe） | 開発環境（Linux のコンテナ）で wgpu を動かすための、ソフトウェアの Vulkan | 25.2.8（`mesa-vulkan-drivers` 25.2.8-0ubuntu0.24.04.4）。Vulkan のローダーは `libvulkan1` 1.3.275.0 | Ubuntu 24.04 の apt | 主に MIT とされる（未確認。配布しないため優先度は低い） | wgpu が実行時に Vulkan のローダー経由で読み込む。ビルド時のリンクはない | **配布物に含めない** | 不要（配布しない） | 開発環境だけで使う |
| LibRaw の同梱（配布物） | 配布物のワーカーが読み込む LibRaw と、その依存の共有ライブラリ（決定事項ログ No.64） | 配布のビルドで入る版。2026-10-12 の release.yml の実行（run 38276847388）では、macOS は Homebrew の libraw 0.22.2・libomp 23.1.0・jpeg-turbo 3.2.0・little-cms2 2.19.1（ログの元のファイルのパス）。Windows の vcpkg の版はログに出ない（要確認）。実際に入ったファイルと SHA-256 は `crates/genzo-app/binaries/bundled-libs.txt`（配布のビルドが作り、artifact に入る） | Windows: vcpkg。同梱したのは `raw_r.dll`・`lcms2-2.dll`・`z.dll`（ワーカーと DLL の読み込みをたどった結果。jasper・libjpeg-turbo の DLL はたどった先になかった）。macOS: Homebrew。同梱したのは `libraw_r.25.dylib`・`libomp.dylib`・`libjpeg.8.dylib`・`liblcms2.2.dylib` | LibRaw は LGPL-2.1 / CDDL-1.0 の選択（3.1 節）。依存はそれぞれのライセンス（未確認） | 動的リンク（ワーカーだけ。本体は依存しない） | Windows: インストール先のフォルダ。macOS: `.app` の `Contents/Frameworks`（参照を書き換えてアドホック署名） | **ライセンスの文書を同梱していない（未対応）**。LGPL のソースの入手方法の案内も要る（人・専門家の確認待ち） | AI 記入（2026-10-12）。配布物は release.yml で作成（2026-10-12。[packaging.md](packaging.md) の 6.2 節） |
| Visual C++ のランタイム | ワーカーと LibRaw の DLL が使う（Windows） | ビルドした Visual Studio の版による（同梱の一覧 `bundled-libs.txt` に元の場所と SHA-256 を書く）。2026-10-12 の実行では、windows-latest の Visual Studio 18 Enterprise の `VC\Redist\MSVC\14.51.36231\x64\Microsoft.VC145.CRT` から | Visual Studio の再頒布用のフォルダ（`VC\Redist\MSVC\<版>\x64\Microsoft.VC*.CRT` など） | Microsoft のライセンス（Visual Studio の `REDIST.TXT` の再頒布可能なファイル。条件は人が確認） | **インストール先に同梱する**（決定事項ログ No.73。本体は静的にリンク）。Windows Update では更新されない | 同梱する（ワーカーと LibRaw の DLL が読み込むもの。2026-10-12 の実行では `VCRUNTIME140.dll`・`VCRUNTIME140_1.dll`・`MSVCP140.dll`） | — | AI 記入（2026-10-12）。利用者の指示で同梱に変更。再頒布の条件は人の確認待ち |
| WebView（Windows: WebView2、macOS: WKWebView） | アプリの画面の表示（Tauri 2。決定事項ログ No.60） | OS のもの（同梱しない） | Windows 10 / 11 の WebView2 ランタイム（Microsoft）、macOS の WebKit | OS の構成要素。WebView2 のランタイムがない Windows では、インストーラー（NSIS）が Microsoft の配布元から入れる（Tauri の既定の `downloadBootstrapper`。要確認） | 動的に使う（同梱しない） | 同梱しない | 要確認（WebView2 の配布の条件） | AI 記入（2026-10-12） |
| NSIS（Windows のインストーラー） | Windows のインストーラーの作成（`npx tauri build`。SYS-08） | Tauri の CLI がビルドのときに取得する版（未記録） | Tauri の CLI が GitHub の tauri-apps/binary-releases から取得（要確認） | zlib/libpng のライセンスとされる（要確認） | インストーラーの実行ファイルに NSIS のスタブが入る | インストーラー | 要確認 | AI 記入（2026-10-12）。版と取得元は配布のビルドの記録で確かめる |
| WebKitGTK・WebKitWebDriver | 開発環境（Linux のコンテナ）でアプリを動かす・E2E（`e2e/`） | 2.52.6（`libwebkit2gtk-4.1-dev`・`webkit2gtk-driver` 2.52.6-0ubuntu0.24.04.1） | Ubuntu 24.04 の apt | LGPL-2.1 / BSD とされる（要確認） | 開発環境だけ | 配布しない | — | AI 記入（2026-10-12） |
| tauri-driver | E2E の WebDriver の中継（開発環境だけ） | 2.0.6（`cargo install tauri-driver --version 2.0.6 --locked`） | crates.io | Apache-2.0 OR MIT（crates.io の表示） | 開発環境だけ | 配布しない | — | AI 記入（2026-10-12） |
| アプリのアイコン | アプリ・インストーラーのアイコン（`crates/genzo-app/icons/icon.svg` から `tauri icon` で作成） | — | このリポジトリ（自作） | アプリと同じ GPL-3.0-or-later | 同梱 | アプリ・インストーラー | — | AI 記入（2026-10-12） |

## 3. 項目ごとの詳細と要確認の点

### 3.1 LibRaw

- **確認した資料**
  - 開発環境の `/usr/include/libraw/libraw.h` のヘッダ: 「GNU LESSER GENERAL PUBLIC LICENSE version 2.1」と「COMMON DEVELOPMENT AND DISTRIBUTION LICENSE (CDDL) Version 1.0」のどちらかを選べる、と書かれている。
  - `/usr/include/libraw/libraw_version.h`: 0.21.2。
  - Ubuntu の `/usr/share/doc/libraw23t64/copyright`: `Files: *` は「LGPL-2.1 or CDDL-1.0」。
- **要確認**
  - **ライセンスの選択**（2026-10-10、決定事項ログ No.32）: 利用者の指示により、AI の推奨で **LGPL-2.1** を選びました（CDDL-1.0 は GPL と両立しないとされるため）。**法的な事項のため確定ではありません。公開・配布の前に人（必要なら専門家）が確認し、結果を決定事項ログ No.32 の「再検討の条件」の欄に書いてください。**
  - **LGPL-2.1 に「以降の版」が含まれるか。** 上流のヘッダには「version 2.1」とだけ書かれています。一方、Ubuntu の copyright ファイルの LGPL-2.1 の節には「or (at your option) any later version」とあります。上流の配布物の `LICENSE.LGPL`・`COPYRIGHT` で確認してください。
  - Ubuntu の copyright ファイルには、`RawSpeed/rawspeed_xmldata.cpp`（CC-BY-SA-3.0）の記載があります。0.21.2 のソースにこのファイルが含まれるか、ビルドで使われるかを確認してください。
  - LibRaw は dcraw を元にしています。dcraw 由来の部分の扱いを、上流の `COPYRIGHT` で確認してください。
  - Ubuntu の `libraw.so.23` は、lcms2・libjpeg・libgomp（OpenMP）に動的リンクしています（`ldd` で確認）。Homebrew の LibRaw の依存と、Windows で使う LibRaw のビルドの設定（OpenMP や lcms2 を使うか）を記録してください。同梱する場合は、これらの依存ライブラリも台帳に追加します。
  - LibRaw とアプリ（`genzo-color`）の両方が lcms2 を使うため、ワーカーのプロセスに 2 つの版の lcms2 が入る可能性があります（3.2 節）。シンボルの衝突や、どちらの版が使われるかを PoC-2 で確認してください。
  - Windows / macOS の配布物に LibRaw を同梱する場合、LGPL の条件（利用者がライブラリを差し替えられること、対応するソースの提供）を満たす方法を決めてください。
  - **本体の実行ファイルに LibRaw が入るか**（2026-10-10、決定事項ログ No.32 で決定）: **配布するアプリでは、ワーカーを別の実行ファイル（`genzo-worker`。`genzo-api` の `WorkerLaunch::Executable`。既定）にし、本体の実行ファイルは LibRaw に依存させません。** 本体は機能フラグ `libraw` なしでビルドし、ワーカーは別の cargo の実行でビルドします（`cargo build --release -p genzo-worker --features libraw`。Cargo の機能フラグは同じビルドの中で共通になるため）。
    - Linux（Ubuntu 24.04）: 別々にビルドすると、`genzo-worker`（libraw あり）は `libraw_r.so.23` に依存し、`genzo`（libraw なし）は依存しないことを `ldd` で確認しました（2026-10-10、実装の報告による）。
    - 開発用の CLI（`genzo`）は、今のまま自分自身を `genzo __worker` で起動してワーカーを兼ねます（`WorkerLaunch::current_exe`）。そのため `--features genzo-cli/libraw` でビルドした `genzo` は `libraw_r.so.23` に依存します（2026-10-10、第 3 波の統合の確認）。LibRaw の関数を呼ぶのはワーカーとして動くプロセスだけですが、本体として動くプロセスにも LibRaw（と、その依存ライブラリ）が読み込まれます。開発用の CLI を配布するかは未定です。
    - macOS・Windows: 未確認。
  - **Windows での LibRaw の入手と指定**（2026-10-10 に手順を記載。決定事項ログ No.32。**実機で未確認**）: Windows には pkg-config の LibRaw がないので、LibRaw（0.21 以降。スレッドセーフな版）を別に入手し、環境変数で場所を指定します（詳細は `crates/genzo-raw/build.rs` の doc と [README](../README.md)）。CI の Windows は LibRaw なし（既定の機能フラグ）だけで確かめます。
    1. 入手する: パッケージ管理の vcpkg（`vcpkg install libraw:x64-windows`）など。上流のソースを `nmake -f Makefile.msvc` でビルドしてもよい。
    2. 環境変数を設定する: `LIBRAW_INCLUDE_DIR`（`libraw/libraw.h` のある include のディレクトリ）、`LIBRAW_LIB_DIR`（`.lib` のあるディレクトリ）、必要なら `LIBRAW_LIB_NAME`（リンクする `.lib` のファイル名から `.lib` を除いたもの。既定は `libraw`）。静的ライブラリなら `LIBRAW_STATIC=1`（未確認）。
    3. ビルドする: `cargo build -p genzo-cli --features libraw`（開発用の CLI）、または `cargo build --release -p genzo-worker --features libraw`（配布するアプリのワーカー）。
    4. 実行する: LibRaw の DLL（と、その依存の DLL）を、ワーカーとして動く実行ファイルと同じフォルダか `PATH` の通ったフォルダに置く。
    - **要確認**: vcpkg・上流の `Makefile.msvc` が作るライブラリのファイル名、スレッドセーフな版か（`genzo-raw` は Windows の LibRaw をスレッドセーフとみなす）、OpenMP・lcms2 などの依存、同梱する場合のライセンスの表記と対応するソースの提供方法。
  - **スレッドセーフな版（`libraw_r`）を使うこと。** autotools でビルドした `libraw`（`_r` なし）は `LIBRAW_NOTHREADS` 付きで、展開の関数が静的変数を使うため、別のインスタンスでも並行して展開するとデータが壊れます。`genzo-raw` は `libraw_r` が見つからず `libraw` にリンクする場合、ビルド時に警告を出し、LibRaw の使用をプロセスの中で 1 つずつに制限します（`crates/genzo-raw/build.rs`）。Windows の LibRaw（`Makefile.msvc` のビルド）は `LIBRAW_NOTHREADS` を使わないとみなしていますが、未確認です。
- **CI**: macOS では Homebrew の `libraw` を入れて、機能フラグ `libraw` を有効にしたビルドとテスト（合成 DNG を LibRaw で展開するテストを含む）も実行します。Windows の CI は LibRaw なしの構成だけです。

### 3.2 Little CMS 2

- **確認した資料**（crates.io の `lcms2-sys` 4.0.7 の中身）
  - `vendor/LICENSE`: MIT License、「Copyright (c) 2023 Marti Maria Saguer」。
  - `vendor/src/cmscnvrt.c` のヘッダ: MIT と同じ許諾文、「Copyright (c) 1998-2026 Marti Maria Saguer」。
  - `vendor/include/lcms2.h`: 「Version 2.19」、`LCMS_VERSION` は 2190。なお、crate の README には「2.19.1」と書かれています（要確認: 2.19 と 2.19.1 のどちらか）。
- **ビルドの設定**
  - workspace の `Cargo.toml` で `lcms2 = { version = "6.2.0", features = ["static"] }` とし、同梱のソースから常にビルドして静的リンクします。
  - `lcms2-sys` の既定の機能フラグ（`dynamic` と `static-fallback`）のままだと、pkg-config でシステムの lcms2 が見つかる環境では、その版に動的リンクします。開発環境の Ubuntu では、システムの lcms2 2.14（`liblcms2-2` 2.14-2ubuntu0.1）にリンクしていたことを、ビルドスクリプトの出力で確認しました。macOS でも、Homebrew の `little-cms2` が入っている環境（LibRaw の依存として入る場合がある。Homebrew の LibRaw の依存は未確認）では、その版にリンクする可能性があります。OS ごとに Little CMS の版が変わると色の計算結果が変わりうるため、`static` にしました。
  - 注意: 環境変数 `LCMS2_LIB_DIR` を設定すると、`static` を有効にしていても、その場所のライブラリが使われます（`lcms2-sys` の build.rs）。CI と配布用のビルドでは設定しないでください。

### 3.3 SQLite

- **確認した資料**（crates.io の `libsqlite3-sys` 0.38.2 の中身）
  - `sqlite3/sqlite3.h`: `SQLITE_VERSION` は "3.53.2"、`SQLITE_SOURCE_ID` は "2026-06-03 19:12:13 d6e03d8c777cfa2d35e3b60d8ec3e0187f3e9f99d8e2ee9cac695fd6fcdf1a24"。
  - `sqlite3/sqlite3.c` のヘッダ: 「The author disclaims copyright to this source code.」
- **ビルドの設定**: `rusqlite` の機能フラグ `bundled`（同梱のソースを cc でビルドして静的リンク）と `backup`（オンラインバックアップ。04 の 3.4 節）。コンパイルの設定は `libsqlite3-sys` の build.rs の既定で、`SQLITE_ENABLE_FTS5`、`SQLITE_DEFAULT_FOREIGN_KEYS=1`、`SQLITE_THREADSAFE=1`、`SQLITE_ENABLE_LOAD_EXTENSION=1` などを含みます。開発環境のシステムの SQLite（3.45.1）は使いません。
- 補足: `libsqlite3-sys` の crate には SQLCipher のソース（`sqlcipher/`）も入っていますが、機能フラグ `bundled-sqlcipher` を有効にしていないため、ビルドしていません。

### 3.4 FFmpeg / ffprobe

- **確認した資料**
  - 開発環境の `ffmpeg -version`: 6.1.1-3ubuntu5。configure の引数に `--enable-gpl` を含み、`libx264`・`libx265` などを組み込んでいる。`--enable-nonfree` は含まない。
  - Ubuntu の `/usr/share/doc/ffmpeg/copyright`: 多くのファイルは LGPL-2.1-or-later で、任意の一部が GPL-2.0-or-later。Debian のパッケージは GPL の部分を使うため、バイナリは GPL-2.0-or-later。libavcodec・libavfilter には Apache-2.0 のライブラリとリンクした別の版もあり、それは実質的に GPL-3.0-or-later。非自由のライブラリと組み合わせたバイナリは再配布できない（Debian のパッケージでは組み合わせていない）。
  - 上流の説明: https://ffmpeg.org/legal.html （R-15 で参照）。上流の `LICENSE.md` そのものは未確認です。
- **方針と要確認**
  - 子プロセスとして実行することだけを根拠に、ライセンスの義務がなくなるとは判断しません（R-15）。
  - 配布物に同梱するかは未定です（02 の 2 章の 4、COMPAT-02・PERF-11）。同梱する場合は、configure の引数、組み込む外部ライブラリ（libx264・libx265 など）とそれぞれのライセンス、`--enable-nonfree` を使っていないことを記録し、対応するソースの提供方法を決めてください。
  - **H.264 / H.265（HEVC）などのコーデックの特許** は、ライセンスとは別の問題です。配布の範囲も含めて、必要に応じて専門家に確認してください（03 の 2.1 節）。

### 3.5 RCD デモザイク

- **出典**: RCD（Ratio Corrected Demosaicing）の考案者は Luis Sanz Rodríguez です。
- **既定のデモザイクは RCD のまま**です（2026-10-10、決定事項ログ No.34。判断の表 No.6 の回答）。下の調査の結果の扱い（由来をどう記録するか、元の著作権表示をどう書くか）は、**人（必要なら専門家）の確認待ち** です。

**実装の状況**（2026-10-10、AI 記入）
- CPU 版を `crates/genzo-pipeline/src/sensor/demosaic/rcd.rs` に実装しました（追加したコミット 527f911。調査の時点の SHA-256 `3fe730d9b5bf8e4a876aae7d7c5bbed0075e3778c5271d1edf0dff7b471313fc`）。ファイルの doc コメントに、考案者の名前、手順と式、余白（10 画素）を書いています。
- 実装者（AI）の報告と、`rcd.rs` の doc には「公開されているアルゴリズムの説明（手順の構成と考え方）をもとに書き、RawTherapee・darktable・考案者の参照実装などのソースは参照・複製していない」とあります。**この記述は、下の調査の結果と合いません**（下の「`rcd.rs` との比較」）。
- 第三者のファイルは取り込んでいません。GPU 版（WGSL）は未実装です（`genzo-gpu`）。

**出典の調査の結果**（2026-10-10、AI の調査。取得日はすべて 2026-10-10。GitHub の公開リポジトリを匿名で取得し、ページは WebFetch・WebSearch で確認した。取得したソースはリポジトリの外に置き、リポジトリにはコピーしていない（GPL のコードのため）。**法的な評価（派生物に当たるかなど）はしていません**。コミット・日付・ライセンスの表記は、記録に使う前に元のページで確かめてください）

| 対象 | URL | 版・コミット（取得時点） | ファイルのライセンスの表示 | 著作権の表示・帰属 | SHA-256 |
|---|---|---|---|---|---|
| 考案者の実装（RCD 2.3） | https://github.com/LuisSR/RCD-Demosaicing （`rcd_demosaicing.c`） | ブランチ master `0aa9d6e`（2017-11-30）。ヘッダは「Release 2.3 @ 171125」。コミットは 15 件、タグはない | リポジトリの `LICENSE` は GPL 第 3 版の全文。**`rcd_demosaicing.c` にはライセンスの表示も「or any later version」の記載もない** | ヘッダに考案者の名前と版。「Copyright」の行はない | `rcd_demosaicing.c` `49de57f000b9ab9a9e07ebb1dfa40b8d9afb1ad13c87526b206c6ed70cf7b84c` |
| RawTherapee | https://github.com/RawTherapee/RawTherapee （`rtengine/rcd_demosaic.cc`） | ブランチ dev `5f486d3`（2026-10-05） | GPL-3.0-or-later | 「Copyright (c) 2017-2020 Luis Sanz Rodriguez and Ingo Weyrich」。元の実装の URL と「Licensed under the GNU GPL version 3」の注記 | `646262a75c72ff8bad8ef199bf5edb48b8d1a230b196914cc07c59be8547b083` |
| darktable | https://github.com/darktable-org/darktable （`src/iop/demosaicing/rcd.c`） | ブランチ master `8c64bf7`（2026-10-07） | GPL-3.0-or-later | 「Copyright (C) 2010-2026 darktable developers」。Ingo Weyrich・Luis Sanz Rodríguez・Hanno Schwalm への帰属 | `b0f53a902f29b6ad462aa8229d657da05ba09c691e81f1db8eeeb167ccac85ba` |
| librtprocess | https://github.com/CarVac/librtprocess （`src/demosaic/rcd.cc`） | ブランチ master `9a85827`（2023-06-27） | GPL-3.0-or-later（ヘッダは RawTherapee のもの） | 「Copyright (c) 2017-2018 Luis Sanz Rodriguez and Ingo Weyrich」 | `eab58c2bf6aeff51697e118664d01a6e2a97948c757042d2eb68cdaabfd92683` |

- 考案者のリポジトリの README にあるのは、4 段の概要、3 × 3 の低域通過フィルタの係数（1 2 1 / 2 4 2 / 1 2 1）、比の補正の式の例だけです。ε の値（1e-5・1e-10）、勾配の式、方向の統計量、近傍との比較の規則は README にありません。査読のある論文は見つかりませんでした（存在しないとは断定しません）。
- 2021 年に、方向の統計量を短い式に書き換えるなどの改変が入りました（RawTherapee の issue #6054。RawTherapee 5.9 に収録。issue の内容は WebFetch の要約で確かめただけで、本文を全部は読んでいない）。

**`rcd.rs` との比較**（事実。詳細はリポジトリの外の調査の記録）
- 手順は、考案者の 2.3・RawTherapee・darktable と数学的に一致します。README にない細部（ε の値、勾配の各項、近傍との比較の規則）も一致します。2.3 の展開した 2 次式は「高域通過フィルタの 2 乗を 3 か所足した形」と同じで、縦と斜めの 40 項すべての係数を比べて確かめました。
- 2021 年以降の改変版（RawTherapee 5.9 以降・darktable・librtprocess）に特有の書き方と、次の点で一致します: 方向の統計量を「高域通過の 2 乗の和」で書くこと、低域通過の尺度（中心の重みが 1。2.3 の 4 倍）、比の補正の分子の `(lc + lc)`（RawTherapee の `(lpfi + lpfi)`）、近傍の平均の括弧の付け方 `0.25 * ((a + b) + (c + d))`（RawTherapee と同じ）。
- 変数名の多くは、RawTherapee・darktable の名前を snake_case にしたものです（`VH_Dir` → `vh_dir`、`VH_Disc` → `vh_disc`、`PQ_Disc` → `pq_disc`、`N_Grad` → `n_grad`、`N_Est` → `n_est`、`epssq` → `EPS_SQ` など）。
- 一致しない点: コードの構造（帯と窓の分け方、バッファの持ち方、クロージャ、段ごとに 0 以上へ切り詰めること）とコメント（日本語、独自の番号付け）は違います。言語が違うので、行がそのまま一致する箇所はありません。
- `rcd.rs` の doc の「参照の説明からの変更点」には誤りがあります: 「近傍との比較に斜めの 4 画素を使う」は、2.3・RawTherapee・darktable と同じで変更点ではありません。「比の補正の ε を分子にも入れた」は、2.3 の式 `1 + (a − b) / (ε + a + b)` と数学的に同じです。

**推測**（AI の推測で、確かめる方法はない）
- `rcd.rs` は、学習データに含まれていた 2021 年以降の版の実装の知識をもとに書かれた可能性が高いと考えます。行をそのまま写したものではなく、変数名と一部の式の書き方が一致する、Rust への書き直しに見えます。
- 考案者の 2.3 に由来するとみなす場合、「GPL-3.0 だけ」か「以降の版も可」かは、ファイルの表示からは決められません。RawTherapee の版は考案者本人が著作権者に入っていて、GPL-3.0-or-later で配布されています（本人が「or later」に同意した記録そのものは確認していない）。
- RawTherapee・darktable の版に由来するとみなす場合も、どちらも GPL-3.0-or-later なので、GenzoParis のライセンス（GPL-3.0-or-later）とは組み合わせられます。その場合は、元の著作権表示と出典の記録が必要です。

**要確認**（人・必要なら専門家）
- 由来の扱い。AI の提案（決めるのは人）: RawTherapee・darktable の版を参考にした実装として扱うのが保守的。`rcd.rs` のヘッダとこの台帳に、元の著作権表示（Luis Sanz Rodríguez、Ingo Weyrich、Hanno Schwalm、darktable developers など）と上の出典を書く。
- 確認の後に行うコードの変更（**未対応**。この資料の更新ではコードを変えていない）: `rcd.rs` の doc の「他のプロジェクトのソースは参照・複製していない」という断定を消すか「確認できない」と書き直す。「変更点」の 2 つの誤りを直す。ヘッダに出典と著作権表示を書く。
- 確認できなかったこと: pixls.us のフォーラムと RawPedia（この環境から名前の解決ができず読めなかった）、実装者（AI）が実際に何を参照したか、ART・vkdt・Siril などほかの実装。
- 結果は決定事項ログ No.34 の「再検討の条件」の欄に書いてください。

- **移植・参考にするときの手順**（04 の 9 章の AR-8）
  1. 使うファイルのパス、リポジトリ、コミット（または版）を、この台帳に記録する。
  2. ファイルのヘッダのライセンスの表示を記録する。GPL-3.0-only と GPL-3.0-or-later を区別する。GPL-3.0-only のコードを取り込むと、配布物全体は事実上 GPL-3.0 だけで配布することになる（「以降の版」を選べなくなる）とされるため、取り込む前に人が判断する。
  3. 移植したコードには元の著作権表示を残し、ファイルのヘッダに出典を書く。
  4. 人の確認を受けてから取り込む。

### 3.6 カメラ行列（α7 IV / α7C）

- **方針**（04 の 2.5 節）: LibRaw が内蔵する表に頼らず、対象機種（α7 IV / α7C）の値をアプリ側のデータファイルとして持ち、ID とハッシュで参照します。LibRaw を更新しても色が変わらないようにするためです。
- **出典の候補と、確認すること**
  - LibRaw の内蔵の表: LibRaw のライセンス（LGPL-2.1 / CDDL-1.0）が、表の値を取り出してデータファイルにすることに及ぶか。表の値の元の出典（要確認）。
  - DNG の ColorMatrix / ForwardMatrix（Adobe DNG Converter で変換したファイルから読む）: Adobe の利用条件。Adobe のカメラプロファイル（DCP）は、利用条件を確認するまで同梱しません（03 の 2.2 節）。
  - カラーチャートを撮影して自作する（PoC-4）: 自前の測定値。チャートの基準値（メーカーの公表値など）の利用条件は要確認。
- **確認状況**: 未着手です。データファイルを作るときに、出典ごとにこの台帳へ記入します。

### 3.7 Mesa（llvmpipe）

- 開発環境（Linux のコンテナ）で、GPU のない状態で wgpu を動かすためだけに使います。wgpu が、Vulkan のローダー経由で Mesa のソフトウェア実装（llvmpipe）を実行時に読み込みます。
- Windows / macOS の配布物には含めません（実機では OS の GPU ドライバを使います）。

### 3.8 ColorChecker Classic の基準値

- **対象**: `crates/genzo-testkit/src/colorchecker.rs` の 2 つの版（2014 年 11 月以降の版と、より前の版。各 24 色 × L\*・a\*・b\*（D50）、計 144 値）と、パッチの並び（`PATCH_NAMES`）。値は colour-science 0.4.6 のデータから写したもので、doc の出典は「X-Rite (2016)」（colour-science の引用に合わせたもの）。
- **照合の結果**（2026-10-10、AI。決定事項ログ No.37。判断の表 No.10 の回答）: 144 値すべてが、GitHub 上の第三者のリポジトリにある X-Rite の CGATS ファイルの写しと一致しました（どの値も差は 0。値は 10 進数のまま比べ、CGATS の小数点のカンマは同じ値として扱った）。パッチの並び（CGATS の ID の A〜F が列、1〜4 が行）も合っています。colour-science 0.4.6 とも、値とパッチの名前がすべて一致しました。2 つの版の差は ΔE2000 で最小 0.182、最大 1.951（white 9.5）、平均 0.848 です。
- **一次資料とは未照合**: X-Rite・Calibrite のサイト（一次資料）からは取得できませんでした（この環境からは xrite.com・xritephoto.com・calibrite.com・zenodo.org・web.archive.org・babelcolor.com に届かなかった）。X-Rite の告知のページは、検索の結果で存在と要旨を確かめただけで、原文は読めていません。**一次資料と照合できるまで、照合を「済み」にしません。**
- 照合に使った写し（取得日 2026-10-10）:

  | 版 | ファイル | 写しの場所 | SHA-256 | 大きさ |
  |---|---|---|---|---|
  | 2014 年 11 月以降 | `ColorChecker24_After_Nov2014.txt` | 持ち主の違う 4 つのリポジトリにバイト単位で同じファイル（V7CN/Colorful など。git blob `46de15fb…`）。ヘッダーに `MANUFACTURER "X-Rite - http://www.xrite.com"`。測定日 2015-04-28、i1Pro 2、M0（UV フィルターなし） | `2d110e3e8c33cb46fad920817efcf3efa504c2a9eb9cf60c9079b9c111557d29` | 946 B |
  | 2014 年 11 月より前 | `ColorChecker24_Before_Nov2014.txt` | thatcherfreeman/rgb-matrix-finder（見つかった写しはこの 1 つだけ） | `0594f920f656331ca878e5a207173e22c80217dcaa00ae5d45013ccf7f86275f` | 948 B |

  ほかに、改行と空白だけが違う写し 1 つと、Zenodo 3245895（colour-science が X-Rite のファイルを保管した記録）から作ったと明記された別の写しとも、値がすべて一致しました。
- **人の確認が必要な事項**
  1. **原本との照合**: 公式ページから CGATS ファイルを 2 つ取得し、SHA-256 を上の値と比べてください（一致すれば照合は完了）。改行などが違う場合は値を比べます（比べるスクリプトはリポジトリの外の調査の記録にある）。
  2. **光源と視野**: CGATS の写しには「D50・2° 視野」の記述がありません。`colorchecker.rs` の doc の記述は公式ページで確かめてください。D50 の白を ICC の値にするか Lindbloom の値にするかで、D65 にした Lab は最大で ΔE2000 0.0045 しか変わりません（計算値。PoC-4 の評価には効かない大きさ）。
  3. **出典の年**: doc の「X-Rite (2016)」は colour-science の引用に合わせたものです。CGATS の測定日は 2015-04-28 で、Web ページの公開日は確認できていません。
  4. **版の境界**: 新しい版のファイルの `ORIGINATOR` は「ColorChecker24 - November2014 edition and newer」で、2014 年 11 月製造のチャートも新しい版に入ります（`colorchecker.rs` の書き方と合う）。使うチャートの裏面の製造日を確かめてください（PoC-4 の 4-1）。
  5. **測定条件（参考）**: 新しい版は i1Pro 2、M0 で測られた平均値で、チャートの個体差は含まれていません。
  6. **利用条件**: 公表値をテストのデータとして使う条件は未確認です。
- 結果は決定事項ログ No.37 の「再検討の条件」の欄に書いてください。確認の後に `colorchecker.rs` の doc の「照合」の記述を更新します（コードの変更。未対応）。

## 4. 今後記入する対象

使うことが決まった時点で、2 章の一覧に追加します。

- lensfun（使う場合。ライブラリは LGPL-3.0、データベースは CC BY-SA 3.0 とされる。03 の 2.2 節）
- ICC プロファイル（sRGB・Display P3 などのファイルを同梱する場合）
- Exiv2（使う場合。GPL-2.0-or-later とされる。03 の 2.1 節）
- LibRaw が依存するライブラリ（libjpeg、OpenMP のランタイム、lcms2 など）を同梱する場合
- Windows の DirectX のシェーダーコンパイラ（wgpu の Direct3D 12 のバックエンドで、DXC のライブラリを同梱するか。要確認）
- UI の npm の依存の、配布物のライセンス表記への反映（確認の方法は 1.1a 節。2026-10-12）
- OS のランタイム（Visual C++ のランタイムなど）を同梱する場合
- ONNX Runtime（将来の ML 推論）
- 配布物のライセンス表記の自動生成（02 の DIST-05）。Rust のクレートについては、生成するツールを検討する（要検討）
