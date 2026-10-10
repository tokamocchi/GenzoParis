# 第三者のコード・データ・同梱物の台帳

M0 のタスク 6b（[05 PoC 計画とロードマップ](05_poc_and_roadmap.md) の 3 章）と、設計レビューの R-15（[06 設計レビュー](06_design_review.md)）に対応する台帳です。
cargo-deny では確認できない **C / C++ のライブラリ、同梱データ、外部の実行ファイル** と、移植・参考にするアルゴリズムの出典を記録します。

> **ライセンスの判断は人が最終確認します。**
> この台帳は AI が作成した記録で、個々のコードやデータの法的な適合性を確定するものではありません。
> わからないことは「要確認」と書き、推測で断定しません。特許の扱いは、ライセンスとは別の未決事項として扱います。

- 作成: 2026-10-09（AI）
- アプリのライセンス: GPL-3.0-or-later（[決定事項ログ](decision_log.md) No.27、[LICENSE](../LICENSE)）

## 1. 範囲と使い方

### 1.1 Rust のクレート（cargo-deny で確認する）

- Rust のクレートのライセンス・既知の脆弱性・取得元は、**cargo-deny で機械的に確認** します（[02 非機能要件](02_nonfunctional_requirements.md) の SEC-04）。この台帳には個別に書きません。
  - 設定: [deny.toml](../deny.toml)。許可するライセンスは GPL-3.0-or-later と両立するものだけです。新しいライセンスを許可するときは、両立するかを人が確認します。
  - CI: [.github/workflows/ci.yml](../.github/workflows/ci.yml) の `lint` ジョブで、push と pull request のたびに実行します。
  - ローカル: `cargo install cargo-deny --locked` で入れて、`cargo deny check` を実行します。ライセンスごとの一覧は `cargo deny list` で出せます。
- 2026-10-09 の確認結果: cargo-deny 0.20.2 で `cargo deny check` を実行し、advisories・bans・licenses・sources がすべて通りました。警告は、同じクレートの複数の版（hashbrown・miniz_oxide・syn）の 3 件だけです。
- 2026-10-10 の確認結果: `genzo-color`・`genzo-raw`・`genzo-media`・`genzo-catalog`・`genzo-jobs`・`genzo-testkit` の実装の後に再実行し、同じ結果でした（すべて通り、警告は同じ 3 件）。同日、`genzo-pipeline`・`genzo-worker`・`genzo-gpu` の実装の後（`naga` 30.0.1 を `genzo-gpu` の開発用の依存に、`libc` を `genzo-worker` の Linux 用の依存に追加）にも再実行し、同じ結果でした。
- **cargo-deny が見るのは、各クレートの `Cargo.toml` の `license` と、ライセンスのファイルだけです。** C のソースを同梱してビルドするクレートについて、同梱したソースの版やファイルのヘッダは確認しません。現在の依存では次の 3 つが該当します。
  - `lcms2-sys`（Little CMS 2）と `libsqlite3-sys`（SQLite）: 2 章の一覧に記録します。
  - `blake3`: BLAKE3 の作者たち自身による C とアセンブリの実装を同梱しています（`c/` ディレクトリ）。crate のライセンスは `Cargo.toml` で CC0-1.0 / Apache-2.0 / Apache-2.0 WITH LLVM-exception の選択です。`c/blake3.c` の先頭にはライセンスの表示がなく（1.8.7 で確認）、C の部分にも crate と同じライセンスが及ぶと考えていますが、要確認です。

### 1.2 この台帳で管理するもの

- C / C++ のライブラリ（LibRaw など）: リンクする版、ビルドの設定、配布物に含めるか
- C のソースを同梱する Rust のクレートのうち、同梱したソースの版を記録しておくべきもの（Little CMS 2、SQLite）
- 同梱データ（カメラ行列、lensfun のデータベース、ICC プロファイルなど）
- 外部の実行ファイル（FFmpeg / ffprobe）
- 移植・参考にするアルゴリズムとコード（RCD デモザイクなど）
- 開発環境だけで使うもの（配布物に含めないことを確認するため）

### 1.3 記入のルール

- 第三者のコード・データを取り込む前、または移植する前に、この台帳に記入します。**台帳への記入と人の確認が済むまで、第三者のコードを移植しません**（R-15）。
- 版は、パッケージの版（apt / Homebrew）、crate の版、またはコミットのハッシュで書きます。「最新」とは書きません。
- ライセンスは、確認した資料（個別の LICENSE ファイル、ソースのファイルのヘッダ、パッケージの copyright ファイル）を書きます。GPL / LGPL は、版だけのもの（-only）と「以降の版」を含むもの（-or-later）を区別します。
- 確認状況は「AI 記入（日付）」→「人が確認済み（日付・確認者）」の順に更新します。未確認の点は 3 章に「要確認」として挙げます。

## 2. 一覧

| 名前 | 用途 | 版またはコミット | 取得元 | ライセンス（確認した資料） | リンク方法・ビルドの設定 | 配布するバイナリと対応するソース | 必要な表記 | 確認状況 |
|---|---|---|---|---|---|---|---|---|
| LibRaw | RAW の展開とメタデータの取得。ワーカープロセスだけで使う（[04](04_architecture.md) の 1.2 節、`genzo-raw` の機能フラグ `libraw`） | 開発環境は 0.21.2。macOS の CI は実行時点の Homebrew の版（2026-10-10 の時点で 0.22 系。コミット 966b379 の記録による。CI では版を記録していない）。Windows は未定。0.21 以降を想定（`genzo-raw` の build.rs で 0.21 以上を要求） | Ubuntu 24.04 の apt（`libraw-dev` 0.21.2-2.1ubuntu0.24.04.2）、macOS は Homebrew（`libraw`）、Windows は未定。上流は https://www.libraw.org/ | LGPL-2.1 または CDDL-1.0 の選択（`libraw.h` のヘッダで確認）。上流の配布物の LICENSE ファイルは未確認。どちらを選ぶかは要確認（3.1 節） | 動的リンク。pkg-config で `libraw_r`（スレッドセーフ版）を探し、なければ `libraw`（この場合は LibRaw の使用をプロセスの中で 1 つずつに制限する）。C++ のシム（`crates/genzo-raw/src/shim/`）を cc でビルドする。pkg-config が返す lcms2 はリンクしない（3.2 節）。Windows は環境変数 `LIBRAW_INCLUDE_DIR`・`LIBRAW_LIB_DIR` で指定する（未確認） | 未定。同梱する場合は、共有ライブラリと、対応するソースの提供方法を決める | 選んだライセンスの全文と著作権表示（要確認） | AI 記入（2026-10-09）。人の確認待ち |
| Little CMS 2 | ICC プロファイルの変換と 3D LUT の作成（`genzo-color`。04 の 2.6 節・5 章） | 2.19（`lcms2-sys` 4.0.7 が同梱するソース。Rust のラッパーは `lcms2` 6.2.0） | crates.io の `lcms2-sys`（`vendor/` に上流のソースを同梱）。上流は Marti Maria Saguer による Little CMS | MIT（`vendor/LICENSE` と、`vendor/src/*.c` のファイルのヘッダで確認）。ラッパーの crate も MIT | 静的リンク。workspace の `Cargo.toml` で `lcms2` の機能フラグ `static` を有効にし、同梱のソースを cc でビルドする（3.2 節） | 実行ファイルに含まれる。対応するソースは crates.io の `lcms2-sys` 4.0.7 | MIT の著作権表示と許諾文 | AI 記入（2026-10-09）。人の確認待ち |
| SQLite | カタログ DB とサムネイル DB（`genzo-catalog`。04 の 3 章） | 3.53.2（`libsqlite3-sys` 0.38.2 が同梱する amalgamation。`rusqlite` 0.40.2） | crates.io の `libsqlite3-sys`（`sqlite3/` に同梱）。上流は https://sqlite.org/ | パブリックドメイン（`sqlite3.c` のヘッダの「The author disclaims copyright to this source code」で確認）。バインディングの crate は MIT | 静的リンク。`rusqlite` の機能フラグ `bundled` と `backup`。コンパイルの設定は `libsqlite3-sys` の build.rs の既定（3.3 節）。システムの SQLite は使わない | 実行ファイルに含まれる。対応するソースは crates.io の `libsqlite3-sys` 0.38.2 | SQLite 自体は不要とされる（パブリックドメイン）。バインディングの MIT の表記は Rust のクレートとして扱う | AI 記入（2026-10-09）。人の確認待ち |
| FFmpeg / ffprobe | 動画のメタデータの取得とサムネイルの生成（`genzo-media`）。子プロセスとして実行する（04 の 1.2 節） | 開発環境は 6.1.1。配布する版は未定 | 開発環境は Ubuntu 24.04 の apt（`ffmpeg` 7:6.1.1-3ubuntu5）。配布物での入手方法は未定 | ビルドの構成で LGPL-2.1-or-later・GPL-2.0-or-later・(L)GPL-3.0-or-later のいずれかになり、`--enable-nonfree` のビルドは再配布できないとされる。開発環境の版は `--enable-gpl` のビルド（`ffmpeg -version` の configure の引数と、Ubuntu の copyright ファイルで確認）。上流の LICENSE.md は未確認 | リンクしない（子プロセス）。同梱する場合は configure の引数を記録する | 未定（同梱するか、利用者に入れてもらうか）。同梱する場合は対応するソースとビルドの構成の提供が必要 | 同梱する場合は、ライセンスの全文、ビルドの構成、組み込んだ外部ライブラリの表記（要確認） | AI 記入（2026-10-09）。配布の方針は人の判断待ち |
| RCD デモザイク | デモザイク（04 の 2.1 節のステージ 5。最終品質） | CPU 版は自前の実装（`crates/genzo-pipeline/src/sensor/demosaic/rcd.rs`。2026-10-10）。GPU 版は未実装（書き出しのセンサー処理は CPU 版を使う）。第三者のファイルは取り込んでいない | アルゴリズムの考案者は Luis Sanz Rodríguez。実装者（AI）の報告では「公開されているアルゴリズムの説明」をもとに書いたが、**その説明の出典（URL・版）は未確認**。実装の例は RawTherapee と darktable にあるとされる | 自前の実装として、アプリと同じ GPL-3.0-or-later。実装者は他のプロジェクトのソースを参照・複製していないと報告しているが、独立性は人が確認していない（3.5 節）。RawTherapee / darktable の実装は GPL-3.0 系とされるが、個別のファイルのヘッダは未確認 | 該当なし（自前の Rust のコード） | アプリのソースに含まれる | アルゴリズムの出典（考案者の名前）を doc コメントに記載済み。説明の出典が分かれば追記する | AI 記入（2026-10-10）。説明の出典と実装の独立性は人の確認待ち（3.5 節） |
| カメラ行列（α7 IV / α7C） | カメラ RGB から XYZ への変換（04 の 2.5 節の `render_deps`、2.6 節） | 未作成 | 候補は LibRaw の内蔵の表、DNG の ColorMatrix、カラーチャートからの自作（3.6 節） | 要確認（出典ごとに利用条件を確認する） | アプリのデータファイルとして持ち、ID とハッシュで参照する | アプリに同梱する予定 | 出典による（要確認） | 未着手 |
| Mesa（llvmpipe） | 開発環境（Linux のコンテナ）で wgpu を動かすための、ソフトウェアの Vulkan | 25.2.8（`mesa-vulkan-drivers` 25.2.8-0ubuntu0.24.04.4）。Vulkan のローダーは `libvulkan1` 1.3.275.0 | Ubuntu 24.04 の apt | 主に MIT とされる（未確認。配布しないため優先度は低い） | wgpu が実行時に Vulkan のローダー経由で読み込む。ビルド時のリンクはない | **配布物に含めない** | 不要（配布しない） | 開発環境だけで使う |

## 3. 項目ごとの詳細と要確認の点

### 3.1 LibRaw

- **確認した資料**
  - 開発環境の `/usr/include/libraw/libraw.h` のヘッダ: 「GNU LESSER GENERAL PUBLIC LICENSE version 2.1」と「COMMON DEVELOPMENT AND DISTRIBUTION LICENSE (CDDL) Version 1.0」のどちらかを選べる、と書かれている。
  - `/usr/include/libraw/libraw_version.h`: 0.21.2。
  - Ubuntu の `/usr/share/doc/libraw23t64/copyright`: `Files: *` は「LGPL-2.1 or CDDL-1.0」。
- **要確認**
  - **どちらのライセンスを選ぶか。** GPL-3.0-or-later のアプリと組み合わせるため、LGPL-2.1 を選ぶ案を考えています。CDDL-1.0 は GPL と両立しないとされているためです。最終的な判断は人が行います。
  - **LGPL-2.1 に「以降の版」が含まれるか。** 上流のヘッダには「version 2.1」とだけ書かれています。一方、Ubuntu の copyright ファイルの LGPL-2.1 の節には「or (at your option) any later version」とあります。上流の配布物の `LICENSE.LGPL`・`COPYRIGHT` で確認してください。
  - Ubuntu の copyright ファイルには、`RawSpeed/rawspeed_xmldata.cpp`（CC-BY-SA-3.0）の記載があります。0.21.2 のソースにこのファイルが含まれるか、ビルドで使われるかを確認してください。
  - LibRaw は dcraw を元にしています。dcraw 由来の部分の扱いを、上流の `COPYRIGHT` で確認してください。
  - Ubuntu の `libraw.so.23` は、lcms2・libjpeg・libgomp（OpenMP）に動的リンクしています（`ldd` で確認）。Homebrew の LibRaw の依存と、Windows で使う LibRaw のビルドの設定（OpenMP や lcms2 を使うか）を記録してください。同梱する場合は、これらの依存ライブラリも台帳に追加します。
  - LibRaw とアプリ（`genzo-color`）の両方が lcms2 を使うため、ワーカーのプロセスに 2 つの版の lcms2 が入る可能性があります（3.2 節）。シンボルの衝突や、どちらの版が使われるかを PoC-2 で確認してください。
  - Windows / macOS の配布物に LibRaw を同梱する場合、LGPL の条件（利用者がライブラリを差し替えられること、対応するソースの提供）を満たす方法を決めてください。
  - **本体の実行ファイルに LibRaw が入るか。** Cargo の機能フラグは同じビルドの中で共通になるため、`genzo-cli/libraw` を有効にすると本体の実行ファイル（`genzo`）が使う `genzo-raw` でも `libraw` が有効になります。LibRaw の関数を呼ぶのはワーカーだけですが、OS とリンカーによっては本体の実行ファイルも LibRaw の共有ライブラリに依存する可能性があります。配布物の構成と LGPL の条件に関わります。
    - Linux（Ubuntu 24.04）: `--features genzo-cli/libraw` でビルドした `genzo` と `genzo-worker`（どちらも現時点では LibRaw の関数を呼ばない）が `libraw_r.so` に依存しないことを、`ldd` で確認しました（2026-10-10）。Linux では rustc（と Ubuntu の gcc）がリンカーに `--as-needed` を渡し、使わない共有ライブラリを記録しないためと考えています。
    - macOS: ld64 は既定では、指定したライブラリ（dylib）を使われなくても実行ファイルに記録するとされるため、本体も LibRaw に依存する可能性があります（未確認）。`otool -L` で確認し、必要なら本体の crate のリンクの引数に `-Wl,-dead_strip_dylibs` を加えることを検討してください。
    - Windows: 未確認（LibRaw の入手方法が未定）。
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
- **実装の状況**（2026-10-10、AI 記入）
  - CPU 版を `crates/genzo-pipeline/src/sensor/demosaic/rcd.rs` に自前で実装しました。ファイルの doc コメントに、考案者の名前、手順と式、余白（10 画素）を書いています。
  - 実装者（AI）の報告: 公開されているアルゴリズムの説明（手順の構成と考え方）をもとに書き、RawTherapee・darktable・考案者の参照実装などのソースは参照・複製していない。説明と違う点として、2 方向の推定の重み付き平均を線形補間（lerp）の形で書いたこと、比の補正の ε を分子にも入れたこと、上限で切り詰めないこと（下限 0 だけ）、近傍との比較に斜めの 4 画素を使ったことを挙げています（同じファイルの「参照の説明からの変更点」）。
  - 第三者のファイルは取り込んでいないので、1.3 節の「移植する前の記入」には当たらないと考えています。ただし、下の要確認の点が済むまでは確定しません。
  - GPU 版（WGSL）は未実装です（`genzo-gpu`）。作る場合も同じ方針で、この節に追記します。
- **要確認**
  - **説明の出典**: 実装の元にした「公開されているアルゴリズムの説明」の所在（URL・版・日付）を、この環境では確認できていません。人が特定して記録してください。
  - **実装の独立性**: AI が書いたコードは、学習したデータに含まれうる既存の実装（RawTherapee・darktable など。GPL-3.0 系とされる）に似る可能性を否定できません。必要に応じて既存の実装と比べ、表現（変数の構成・コメント・特徴的な定数の並び）まで似ている部分がないかを人が確認してください。似ている部分が見つかった場合は、下の「移植・参考にするときの手順」に従って元の著作権表示とライセンスを記録します（GPL-3.0-only のコードであれば、配布物全体の「以降の版」の扱いにも関わります）。
  - 考案者自身による実装も公開されているとされますが、所在とライセンスは未確認です。
- **実装の例**（参考。いずれも未確認）
  - RawTherapee と darktable に実装があり、GPL-3.0 系とされています。ファイルのパス・版・ヘッダは未確認です。
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

## 4. 今後記入する対象

使うことが決まった時点で、2 章の一覧に追加します。

- lensfun（使う場合。ライブラリは LGPL-3.0、データベースは CC BY-SA 3.0 とされる。03 の 2.2 節）
- ICC プロファイル（sRGB・Display P3 などのファイルを同梱する場合）
- Exiv2（使う場合。GPL-2.0-or-later とされる。03 の 2.1 節）
- LibRaw が依存するライブラリ（libjpeg、OpenMP のランタイム、lcms2 など）を同梱する場合
- Windows の DirectX のシェーダーコンパイラ（wgpu の Direct3D 12 のバックエンドで、DXC のライブラリを同梱するか。要確認）
- UI（案 A の場合）の npm の依存。cargo-deny の対象外なので、別の確認方法が必要
- OS のランタイム（Visual C++ のランタイムなど）を同梱する場合
- ONNX Runtime（将来の ML 推論）
- 配布物のライセンス表記の自動生成（02 の DIST-05）。Rust のクレートについては、生成するツールを検討する（要検討）
