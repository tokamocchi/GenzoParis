# 配布物のライセンスの条件の照合（DIST-05。判断の表 No.198・199）

2026-10-20、AI 記入。版 0.1.1 の配布物（Windows のインストーラー・macOS の `.dmg`）に入る第三者のソフトウェアについて、ライセンスごとに「実行形式で配るときの主な条件」と「配布物の文書でどう対応しているか」を並べたものです。

> **この資料は法的な判断ではありません。** 条件の要旨は AI が原文を読んでまとめたもので、「AI の読み」は確定ではありません。**配る前に、人（必要なら専門家）が各ライセンスの原文で確かめてください。** 特に 4 章の事項は、文書の側だけでは決められません。条文・版・日付・URL は、引用元の原本で確かめてください。

## 1. 配布物に入るものと文書

配布物の `licenses/`（Windows はインストール先 `%LOCALAPPDATA%\GenzoParis\licenses`、macOS は `GenzoParis.app/Contents/Resources/licenses`）に次の文書を入れます。設定の「このアプリについて」のタブからも読めます。

| 区分 | 中身（2026-10-20 の時点） | 文書 | 作り方 |
|---|---|---|---|
| アプリ自体 | GenzoParis（GPL-3.0-or-later） | `GenzoParis/LICENSE.txt`（全文）・`README.txt`（ソースの場所） | リポジトリのファイル |
| Rust の crate | 本体とワーカーに静的にリンクした 395 crate | `third-party/rust.txt` | `node scripts/third-party.mjs`（cargo-about 0.9.2） |
| npm のパッケージ | 画面に組み込んだ 8 パッケージ | `third-party/npm.txt` | 同上 |
| C / C++ の共有ライブラリ | Windows: LibRaw（`raw_r.dll`）・Little CMS（`lcms2-2.dll`）・zlib（`z.dll`）。macOS: LibRaw（`libraw_r.25.dylib`）・Little CMS（`liblcms2.2.dylib`）・libjpeg-turbo（`libjpeg.8.dylib`）・LLVM の OpenMP（`libomp.dylib`）。2026-10-12 の配布物の記録で、実際に入るものはビルドの環境で決まる | `third-party/native.txt` | 配布のビルド（`scripts/prepare-worker.mjs`。vcpkg・Homebrew がインストールした文書を載せる。**2026-10-20 に追加**） |
| Visual C++ のランタイム（Windows） | `VCRUNTIME140.dll`・`VCRUNTIME140_1.dll`・`MSVCP140.dll`（2026-10-12 の記録） | `third-party/native.txt` の 3 章（名前・著作権者・入手元だけ）と、使用条件 `third-party/vcruntime-terms.txt`（インストーラーで同意を求める。2026-10-20 に追加） | 同上（使用条件はリポジトリの `crates/genzo-app/licenses/windows/vcruntime-terms.txt`） |
| LibRaw | LGPL-2.1 の全文・著作権表示・案内・ビルドの手順の写し・**ソースのアーカイブ** | `LibRaw/`（`source/` は **2026-10-20 に追加**） | 配布のビルド |
| AI マスクのモデル（2026-10-26 に追加） | U²-Net の小型版（`models/u2netp.onnx`。rembg の配布物を変更せずに）・DeepLab-v3（`models/deeplab_v3.onnx`。MediaPipe の tflite を ONNX に変換） | `third-party/ai-models.txt`（入手元・SHA-256・変更の表示・Apache-2.0 と MIT（rembg）の全文） | リポジトリのファイル（`crates/genzo-app/models`・`licenses/third-party/ai-models.txt`） |
| ONNX Runtime（2026-10-26 に追加。Windows・macOS） | `models/onnxruntime.dll`・`models/libonnxruntime.dylib`（1.31.0。Microsoft の公式の配布物を変更せずに） | `third-party/onnxruntime/LICENSE.txt`（MIT）・`third-party/onnxruntime/ThirdPartyNotices.txt` | 配布のビルド（`scripts/lib/onnxruntime.mjs`。公式の配布物の SHA-256 を固定） |

## 2. ライセンスごとの条件と対応

「数」は 2026-10-20 の `rust.txt`・`npm.txt` の値（選んだライセンスで数える。AND のものは両方に数える）。

| ライセンス | 対象 | 実行形式で配るときの主な条件（原文の要旨） | 配布物での対応 | AI の読み（確定ではない） |
|---|---|---|---|---|
| MIT | crate 350・npm 7・Little CMS（両 OS）・libomp（Homebrew の表示） | 著作権表示と許諾の文を、ソフトウェアのすべての複製・重要な部分に含める | 各 crate・パッケージの本文（著作権の行を含むファイル）を載せる。本文のファイルがない crate は、上流のリポジトリのその版のコミットのファイルで補う（下の 3 章）。それでもない objc2 系 12 crate は、SPDX の標準の文と、上流の `LICENSE.md` を NOTICE として載せる | 満たすと考える。ただし objc2 系 12 crate は著作権の行が雛形のまま（4 章の 5） |
| BSD-2-Clause・BSD-3-Clause | crate 2・7、npm 1（leaflet）、libjpeg-turbo の TurboJPEG の部分（macOS） | バイナリの再頒布では、著作権表示・条件・免責の文を、文書か添付の資料に再現する。BSD-3 は、著作権者の名前を推奨・宣伝に使わない | 本文（著作権の行を含む）を載せる。名前を宣伝に使っていない | 満たすと考える |
| ISC | crate 4（ring・rustls-webpki など） | 著作権表示と許諾の文を、すべての複製に含める | 本文と、ring のファイルごとの表示（cargo-about が拾ったもの）を載せる | 満たすと考える。ファイルごとの表示をどこまで載せるかは 4 章の 6 |
| Zlib | crate 1・zlib（Windows）・libjpeg-turbo の SIMD の部分（macOS） | 出所を偽らない・変えたソースはそう明示する・ソースの配布物から表示を消さない（バイナリには表示の義務はない） | 本文を載せる（義務を超えるが、載せる） | 満たすと考える |
| Unicode-3.0 | crate 19（ICU4X） | 著作権表示と許諾の文を、複製とともに、または関連の文書に載せる | 本文を載せる | 満たすと考える |
| CC0-1.0 | crate 3 | 条件なし | 本文を載せる | 該当する条件なし |
| Apache-2.0 | crate 8（codespan-reporting・dpi・gl_generator・glutin_wgl_sys・khronos_api・ring・spirv・tao） | 4 条: (a) ライセンスの写しを渡す、(b) 変えたファイルにはその旨、(c) ソースの形の表示を残す、(d) NOTICE のファイルがあれば、その帰属の表示を含める | 全文を載せる。8 crate の直下に NOTICE のファイルはない（2026-10-20 に確かめた）。gl_generator・khronos_api・spirv は crate に本文のファイルがないので、上流の LICENSE を載せる | 満たすと考える（変更はしていない） |
| Apache-2.0 WITH LLVM-exception | libomp（macOS。LLVM の OpenMP） | Apache-2.0 の 4 条。例外は、コンパイルの結果に埋め込まれた部分を配るときに 4 (a)(b)(d) を免除する（libomp はライブラリそのものを配るので、例外に頼らない読み） | keg の直下の `LICENSE.TXT`（Homebrew がソースの最上位から写したもの）を載せる | 満たすと考える。ただし keg の `LICENSE.TXT` は llvm-project の最上位のもので、`openmp/LICENSE.TXT` にある OpenMP の旧ライセンス（MIT との二重）と Intel の許諾の文は含まない（4 章の 7） |
| MPL-2.0 | crate 5（cssparser・cssparser-macros・dtoa-short・option-ext・selectors） | 3.2 節: 実行形式で配るときは、ソースを入手できるようにし、入手の方法を受け取った人に知らせる。実行形式のライセンスで、ソースの形の権利を制限しない | 全文を載せ、**`rust.txt` の「ソースコードの入手先」の節に crates.io の同じ版の URL を書く（2026-10-20 に追加）**。変更していない。GenzoParis は GPL-3.0 で、ソースの形の権利を制限しない | 満たすと考える。入手先が第三者（crates.io）の場所でよいかは 4 章の 4 |
| LGPL-2.1（LibRaw。CDDL-1.0 との選択で LGPL-2.1 を選んだ） | `raw_r.dll`・`libraw_r.25.dylib` | 4 節: ライブラリを実行形式で配るときは、対応するソースを添える（同じ場所からの入手でもよい）。6 節: ライブラリを使っていることとライセンスを目立つように知らせ、写しを渡す。実行中に著作権表示を出すなら、ライブラリの表示とライセンスへの案内も出す。a〜e のどれか（b: 共有ライブラリとして使い、互換のある差し替えで動く）。利用者が改変・リバースエンジニアリングできる条件で配る | 全文（`LICENSE.LGPL`）・著作権表示（`COPYRIGHT`）・案内（使っていること・同梱したファイル・差し替えの方法）を入れる。**ビルドに使ったソースのアーカイブを `LibRaw/source/` に入れる（vcpkg は LibRaw と LibRaw-cmake、Homebrew は LibRaw の配布物。2026-10-20 に追加）**。ビルドの手順の写し（vcpkg の port とパッチ・Homebrew の formula）とコミットを固定した URL も入れる。ワーカーが共有ライブラリとして読み込む（差し替えられる）。**設定の「このアプリについて」に LibRaw の著作権表示と LGPL-2.1 の文書への案内を出す（2026-10-20 に追加）**。GenzoParis は GPL-3.0 で、改変を妨げない | 満たすと考える。ただしソースの範囲（4 章の 3）と、macOS の差し替えで署名し直す必要があること（案内に書いた）は人の確認が要る |
| IJG（libjpeg-turbo の libjpeg の部分。macOS） | `libjpeg.8.dylib` | 実行形式だけを配るときは、添付の文書に "this software is based in part on the work of the Independent JPEG Group" と書く。名前を宣伝に使わない | **`native.txt` の冒頭にこの一文を書き、`README.ijg`（LEGAL ISSUES を含む）と `LICENSE.md` を載せる（2026-10-20 に追加。README.ijg が見つからなければ配布物を作らずに止める）** | 満たすと考える |
| Microsoft の Visual C++ のランタイム（Windows） | `VCRUNTIME140.dll`・`VCRUNTIME140_1.dll`・`MSVCP140.dll` | Visual Studio のライセンス条項の Distributable Code の節（下の注）。著作権表示を出す義務はない | `native.txt` に名前・著作権者・入手元を書き、変更せずに同梱する。**2026-10-20 から、インストーラーの同意の画面で使用条件（`licenses/third-party/vcruntime-terms.txt`）への同意を求める**（利用者が方式 A を選んだ。決定事項ログ No.199。文言は AI の案） | 同意を求める条件は、使用条件で対応する形にした（文言が足りるかは専門家の確認待ち）。**補償（indemnify）の義務は文書では満たせず、配布する人が負う**（4 章の 1） |
| MIT（ONNX Runtime・rembg。2026-10-26 に追加） | `onnxruntime.dll`・`libonnxruntime.dylib`、`u2netp.onnx`（rembg の配布物） | 著作権表示と許諾の文を、すべての複製・重要な部分に含める | ONNX Runtime の配布物の LICENSE と ThirdPartyNotices.txt をそのまま入れる。rembg の LICENSE の全文を `ai-models.txt` に載せる | 満たすと考える。ThirdPartyNotices.txt の中の各ライセンスの条件は個別には照合していない（4 章の 11） |
| Apache-2.0（AI マスクのモデル。2026-10-26 に追加） | `u2netp.onnx`（U²-Net の重み）・`deeplab_v3.onnx`（MediaPipe の DeepLab-v3） | 4 条（上の行と同じ） | 全文と入手元を `ai-models.txt` に載せる。DeepLab-v3 は ONNX に変換したので、変えたこと（4 条 (b)）を書いた。NOTICE のファイルは、U-2-Net のリポジトリにはない（2026-10-26 の取得で LICENSE だけ） | U²-Net は満たすと考える。**DeepLab-v3 は、配布元のファイルにライセンスの明記がなく、Apache-2.0 という前提そのものが人の確認待ち**（4 章の 11） |
| GPL-3.0-or-later（GenzoParis 自体） | 本体・ワーカー・画面 | 6 条: 実行形式を配るときは、対応するソースを提供する（同じ場所からの入手でもよい）。ライセンスの写しを渡す | 全文を `GenzoParis/LICENSE.txt` に入れ、ソースの場所（GitHub のリポジトリと版のタグ）を `README.txt` と設定の画面に書く。GitHub の Release にはタグのソースのアーカイブが付く | 4 章の 8 |

注（Microsoft の条項）: 調べた条文は、Visual Studio 2026 Community のライセンス条項（"Last Updated: October 1, 2025"）の 8 節 DISTRIBUTABLE CODE を、第三者のリポジトリにある写しで読んだものです（Microsoft の原本は、この環境から開けなかった）。2026-10-12 の配布物は、GitHub Actions のランナーの Visual Studio 18 Enterprise の `VC\Redist\MSVC\14.51.36231\x64\Microsoft.VC145.CRT` から DLL を取っており、Enterprise 版の条項の本文は確かめていません。要旨: (a) Distributable List（`aka.ms/vs/18/redistribution`。`VC\redist` の下のファイル。デバッグ版を除く）のファイルを、変更せずにアプリとともに配れる。(b) 配るときは、アプリに重要な主たる機能を足す、**配布する人と外部の利用者に、Distributable Code をこの条項と同じ以上に保護する条件に同意させる**、**アプリの配布・使用に関する請求から Microsoft を補償・防御する**。(c) Microsoft の商標をアプリの名前に使わない、Distributable Code のソースを、ソースの開示や改変の権利を求めるライセンス（Excluded License）の対象にしない。

## 3. 2026-10-20 に直した不足

| No | 不足 | 直したこと |
|---|---|---|
| 1 | 同梱の C / C++ の共有ライブラリ（LibRaw 以外）と Visual C++ のランタイムのライセンスの文書が、配布物になかった（判断の表 No.199） | 配布のビルドで `licenses/third-party/native.txt` を作る（`scripts/lib/native-notice.mjs`）。同梱したファイルごとに入手元のパッケージを調べ、そのパッケージがインストールした文書（vcpkg の `share/<port>/copyright`、Homebrew の keg の `LICENSE`・`COPYING`・`NOTICE`・`README.ijg` など）を載せる。文書が見つからないライブラリがあれば止める。`release.yml` の確認で、同梱したファイルがすべて載っているかを見る |
| 2 | libjpeg（IJG）を同梱しているのに、IJG のライセンスが求める一文がなかった（macOS） | `native.txt` の冒頭に書く |
| 3 | LibRaw（LGPL-2.1）のソースを配布物に入れていなかった（上流の場所を案内するだけ） | ビルドに使ったソースのアーカイブを `LibRaw/source/` に入れる（vcpkg の portfile の SHA512・Homebrew の formula の SHA-256 と同じかを確かめる） |
| 4 | MPL-2.0 の 5 crate のソースの入手先を書いていなかった | `rust.txt` に「ソースコードの入手先」の節（crates.io の同じ版）を足す |
| 5 | crate にライセンスのファイルがない 27 crate を、著作権の行のない標準の文で載せていた | 14 crate は上流のリポジトリの、その版を公開したコミット（crate の `.cargo_vcs_info.json`）のファイルで補う（`RUST_SUPPLEMENTS`。版と SHA-256 を固定）。rust-unic の 5 crate は `COPYRIGHT.md`（著作権者）も載せる。objc2 系 12 crate は上流にも MIT の本文のファイルがなく、`LICENSE.md`（ライセンスと Apple の SDK についての説明）を NOTICE として載せる。selectors（servo/stylo）は上流にもファイルがない |
| 6 | アプリの画面に LibRaw の著作権表示がなかった | 設定の「このアプリについて」に、LibRaw の著作権表示と LGPL-2.1 の文書への案内を出す（LibRaw の文書がある配布物だけ） |

## 4. 人の判断が要る事項（AI は決めない）

1. **Visual C++ のランタイムの同梱（Windows）と Microsoft の条項**（決定事項ログ No.73 で、利用者の指示によりインストール先に同梱している）。調べた条文（2 章の注）には、文書を同梱するだけでは満たせない条件がある。
   - (a) 配布する人と外部の利用者に、Distributable Code を条項と同じ以上に保護する条件へ同意させること（今のインストーラーは同意を求めていない）。
   - (b) Microsoft を補償・防御する義務（個人で公開する人が負う）。
   - (c) GPL との関係: GPLv3 の 10 条（受け取った人の権利の行使に、さらに制限を課さない）と (a) の関係、Excluded License の範囲、FSF の GPL FAQ（Windows のランタイムと GPL。検索の抜粋で "You may not distribute these libraries in compiled DLL form with the program." と読めた）と GPLv3 のシステムライブラリの定義の関係。GenzoParis の著作権者が GPLv3 の 7 条の追加の許可を出す方法もある（他の人の GPL のコードを含むなら、その人の同意が要る）。
   - (d) どのエディション・版の条項が適用されるか（ランナーの Visual Studio Enterprise のライセンスで、使う人が再頒布できるか。runner-images の Issue #12945 では、ランナーの利用者が別にライセンスを持つ必要はないとの説明と、それを疑う声の両方があった）。
   - 選べる方法（AI の整理）: 今のまま同梱し、条項の (a) を満たす同意の手順をインストーラーに足す（方式 A） ／ 同梱をやめ、利用者に Microsoft の再頒布可能パッケージを入れてもらう（方式 B。2026-10-12 以前の方式。管理者の権限が要る） ／ ワーカーと LibRaw を、ランタイムを静的にリンクしてビルドする（方式 C）。
   - **2026-10-20、利用者が方式 A を選んだ**（決定事項ログ No.199）。AI が使用条件の文案（`crates/genzo-app/licenses/windows/vcruntime-terms.txt`）を作り、Windows の配布のビルドで、インストーラーの同意の画面の文書を「使用条件と GPL-3.0 の全文」に差し替える形にした。使用条件は Microsoft のファイルだけに適用し、GenzoParis（GPL-3.0）とほかのソフトウェアの権利を制限しないと明記した（(c) の GPL との関係への対応）。
   - **残る点（専門家の確認待ち）**: 文言が「同じ以上に保護する」を満たすか（Microsoft の原本・Enterprise 版の条項で確かめる）、GPL との関係、(b) の補償の義務を配布する人が負うことを受け入れるか、(d) のエディションの扱い、インストーラーの同意の画面を出さない配り方（`/S` など）での扱い（使用条件の 5 で、インストールする人が同意を得ると書いた）。
2. **公開してよいか**: GitHub の Release の公開の範囲（決定事項ログ No.1・3・181）と、上の 1 の結論。2026-10-19 に公開した v0.1.0 の配布物には、ここに書いた文書がどれも入っていない（判断の表 No.198 の (7)）。
3. **LibRaw のソースの範囲**（LGPL-2.1 の 4 節・6 節）: 同梱したのは LibRaw のソース、vcpkg のビルドの CMake のファイル（LibRaw-cmake）とパッチ、port の定義（`build-recipe/`）、Homebrew の formula。vcpkg の道具（`vcpkg_cmake_configure` などの共通のスクリプト）・コンパイラーは入れていない。依存の lcms2・zlib・libjpeg-turbo などは許容的なライセンスで、ソースを入れていない。これで「対応するソース」として足りるか。
4. **MPL-2.0 のソースの入手先**を、第三者の場所（crates.io）の URL で知らせることでよいか（GenzoParis 自身の Release に置く方法もある）。
5. **著作権の行のない crate**: objc2 系 12 crate（MIT と表示。上流の `LICENSE.md` には「Apple の SDK から派生したもので、派生物の配布が許されるかは Xcode のライセンスから明らかでない」とある）と selectors（MPL-2.0。本文は SPDX の標準の文）。著作権者の表示がないまま載せてよいか。Apple の SDK の扱い。
6. **ソースのファイルのヘッダの、ファイルごとの著作権表示**（ring の BoringSSL・OpenSSL 由来のファイルなど）を、どこまで載せる必要があるか（判断の表 No.198 の (5b)）。
7. **libomp の文書**: keg にあるのは llvm-project の最上位の `LICENSE.TXT`（Apache-2.0 WITH LLVM-exception と、LLVM の旧ライセンス（University of Illinois/NCSA））で、`openmp/LICENSE.TXT` の OpenMP の旧ライセンス（NCSA と MIT の二重）と Intel の許諾の文は含まない。Homebrew の formula は "MIT" と表示している。これで足りるか。
8. **GPL-3.0 の対応するソース**: GitHub のリポジトリと版のタグ（Release のソースのアーカイブ）で足りるか。著作権者が利用者自身だけなら、GPL の条件は再頒布する人を縛るもので、著作権者自身は縛らない、という読みでよいか（他の人の GPL のコードを含むかの確認を含む）。
9. **複数のライセンスから選ぶ順**（MIT を Apache-2.0 より先。決定事項ログ No.195）と、brotli-decompressor を BSD-3-Clause として扱うこと。
10. **未確認の環境**: `native.txt`・`LibRaw/source/` を作る処理は、Linux で偽の vcpkg・brew を使ったテストでだけ確かめた。Windows・macOS の配布のビルド（`release.yml`）では未実行。実際に同梱されるライブラリ（vcpkg の libraw の依存の jasper が入るかなど）とその文書は、実行したときの `native.txt` で確かめる。
11. **AI マスクのモデルと ONNX Runtime（2026-10-26 に同梱。判断の表 No.207）**:
   - DeepLab-v3（MediaPipe の `deeplab_v3.tflite`）のライセンスの根拠。ファイル自体と、この環境から読めた一次資料には明記がない（MediaPipe のリポジトリが Apache-2.0 で、似たテスト用のファイルを Apache-2.0 と宣言していることからの推定）。Google の MediaPipe の案内のページ・モデルカードで確かめる。確かめられなければ、人物のモデルを同梱から外すか、別のモデルに替える。
   - 学習データ（U²-Net の小型版: DUTS-TR、DeepLab-v3: ImageNet・MS-COCO・PASCAL VOC 2012）の利用条件が、モデルの配布・利用に及ぶか。
   - Apache-2.0 の 4 条 (b) の変更の表示（ONNX への変換）の書き方が足りるか。
   - ONNX Runtime の ThirdPartyNotices.txt に載っている第三者のソフトウェアの条件（個別には照合していない）。
   - ONNX Runtime のテレメトリーを止める処理（環境変数と API）の効果は、実測では確かめていない。社内の情報の扱いの規程に照らして足りるか。
   - モデルのファイルを公開のリポジトリ（GitHub）に置くこと（再頒布）の可否と範囲。

## 5. 更新のしかた

- 依存（`Cargo.lock`・`ui/package-lock.json`・`about.toml`）を変えたら `node scripts/third-party.mjs` を実行し、作り直した文書をコミットする（[third_party.md](third_party.md) の 1.4 節）。上流の写し（`RUST_SUPPLEMENTS`・`NPM_SUPPLEMENTS`）は版を固定しており、版が上がると使われずに警告が出る。その版のファイルを確かめて足し直す。
- `native.txt` と `LibRaw/source/` は配布のビルドで毎回作る。入手元のパッケージ（vcpkg の port・Homebrew の formula）の版が変わると中身も変わる。`release.yml` の artifact の `crates/genzo-app/binaries/licenses/` で確かめられる。
