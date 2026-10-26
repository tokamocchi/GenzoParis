# 配布物（インストーラー）の作り方と注意（SYS-08）

GenzoParis を社内の数名に配るための、配布物の作り方・入れ方・注意をまとめます。

- 作成: AI（2026-10-12）。この Linux の開発環境では、同じスクリプトで Linux の .deb を作り、本体とワーカーが入ることを確かめました（6.1）。**Windows・macOS の配布物は、2026-10-12 に GitHub Actions（`release.yml`）で作り、ワーカーが同梱の LibRaw で起動することまで確かめました**（6.2）。**実機ではまだ確かめていません**（7 章の「未確認の事項」）。
- 関係する決定: [決定事項ログ](decision_log.md) の No.32（ワーカーを別の実行ファイルにし、本体を LibRaw に依存させない）・No.62（FFmpeg を同梱しない）・No.63（既定のカタログの場所）・No.73（Visual C++ のランタイムを同梱する）・No.74（macOS の配布物は macOS 26 以降向け）。
- **法的な事項**: LibRaw の LGPL の扱いと依存ライブラリのライセンスは、利用者が確認済みと回答しました（2026-10-12。implementation_status の判断の表 No.61・62）。**Visual C++ のランタイムの再頒布の条件（同梱するようにした。2.2）と FFmpeg は、人・専門家の確認待ちです**（8 章）。
- 署名・公証はしません（社内の数名への配布のため）。署名が必要かどうかは人が判断します（[01 の SYS-08](01_feature_inventory.md)）。

## 1. 配布物の構成

### 1.1 中身

| OS | 形式 | 入る場所と中身 |
|---|---|---|
| Windows（x64） | NSIS のインストーラー（`GenzoParis_<版>_x64-setup.exe`）。ユーザーごとのインストールで、管理者の権限は要らない（`tauri.conf.json` の `installMode: currentUser`） | `%LOCALAPPDATA%\GenzoParis\` に、本体 `genzo-paris.exe`・ワーカー `genzo-worker.exe`・LibRaw の DLL（vcpkg なら `raw_r.dll`）とその依存の DLL・Visual C++ のランタイムの DLL（`VCRUNTIME140.dll`・`MSVCP140.dll` など。2.2）・`uninstall.exe` |
| macOS（Apple Silicon） | `.dmg`（`GenzoParis_<版>_aarch64.dmg`）。中の `GenzoParis.app` を「アプリケーション」へ入れる | `GenzoParis.app/Contents/MacOS/` に本体 `genzo-paris` とワーカー `genzo-worker`、`Contents/Frameworks/` に LibRaw の dylib（`libraw_r.*.dylib`）とその依存（libomp・little-cms2・jpeg-turbo など） |
| Linux（確認用） | `.deb`（`GenzoParis_<版>_amd64.deb`） | `/usr/bin/genzo-paris`・`/usr/bin/genzo-worker`。LibRaw は同梱しない（OS のパッケージを使う。配布の対象ではない） |

ファイル名は Tauri の命名の決まりによるもので、Windows・macOS は未確認です（Linux の .deb は 6 章で確認）。

- **本体は LibRaw に依存しません。** LibRaw を読み込むのはワーカーだけです（決定 No.32）。本体はワーカーを「環境変数 `GENZO_WORKER_PATH` → 本体と同じフォルダ」の順に探します（`crates/genzo-worker/src/client.rs` の `locate_worker_executable`）。Tauri の `externalBin` は、ワーカーを本体と同じフォルダ（Windows はインストール先、macOS は `Contents/MacOS`、.deb は `/usr/bin`）に置くので、そのまま見つかります。
- **LibRaw のライブラリはワーカーから読める場所に置きます。** Windows は、実行ファイルと同じフォルダの DLL を最初に探すので、インストール先のフォルダに置きます（Tauri の `bundle.resources`。Windows の resource のフォルダはインストール先）。macOS は `Contents/Frameworks` に置き（Tauri の `bundle.macOS.frameworks`）、ワーカーと dylib の参照を `@executable_path/../Frameworks/…`・`@loader_path/…` に書き換えます（`install_name_tool`）。
- **LibRaw の LGPL-2.1 の全文・著作権表示・案内を同梱します**（2026-10-19。決定事項ログ No.175）。Windows はインストール先の `licenses\LibRaw\`、macOS は `GenzoParis.app/Contents/Resources/licenses/LibRaw/` に、`LICENSE.LGPL`（LGPL-2.1 の全文）・`COPYRIGHT`（LibRaw の著作権表示）・`README.txt`（使っている LibRaw の版・同梱したファイル・ソースの入手先・差し替えの方法）と、LibRaw をビルドした手順の写し `build-recipe/`（Windows は vcpkg の port「libraw」の定義とパッチ、macOS は Homebrew の formula。案内には vcpkg・homebrew-core のビルドの時点のコミットを固定した URL を書く。取れなければ「記録できなかった」と書く）、**ビルドに使った LibRaw のソースのアーカイブ `source/`**（2026-10-20 から。決定事項ログ No.196。vcpkg は portfile の LibRaw と LibRaw-cmake、Homebrew は formula の配布物。入手元が取得したものを使い、なければ同じ URL から取る。portfile の SHA512・formula の SHA-256 と違えば止まる）を置きます。`LICENSE.LGPL`・`COPYRIGHT` は上流の LibRaw（GitHub の 0.22.0 のタグ）のファイルをそのまま `crates/genzo-app/licenses/LibRaw/` に置いたもの、`README.txt` は `scripts/prepare-worker.mjs` がビルドのときに作るもの（版は LibRaw のヘッダ `libraw_version.h` から、`genzo-raw` の `build.rs` と同じ場所を探して読む。文面は `scripts/lib/notice.mjs`）。同じものを Actions の artifact（`crates/genzo-app/binaries/licenses/`）にも入れます。
- **アプリ自体のライセンス（GPL-3.0）の全文と、第三者のライセンスの文書を同梱します**（2026-10-20。決定事項ログ No.194・195、[第三者の台帳](third_party.md) の 1.4 節）。Windows はインストール先の `licenses\`、macOS は `GenzoParis.app/Contents/Resources/licenses/` に、`README.txt`（案内）・`GenzoParis/LICENSE.txt`（GPL-3.0 の全文）・`third-party/rust.txt`（本体とワーカーに組み込んだ Rust の crate の一覧とライセンスの本文）・`third-party/npm.txt`（画面に組み込んだ npm のパッケージの一覧と本文）を置きます。アプリの設定の「このアプリについて」のタブでも読めます。文書は `node scripts/third-party.mjs` で作ってリポジトリに置いたもので（Cargo.lock・ui/package-lock.json・about.toml を変えたら作り直す）、`scripts/build-app.mjs` は古い文書のまま配布物を作らないよう、ビルドの前後に確かめます（1.3）。**2026-10-20 から、同梱した C / C++ の共有ライブラリのライセンスの文書 `third-party/native.txt` も置きます**（決定事項ログ No.196。`scripts/prepare-worker.mjs` が、同梱したファイルごとに入手元（vcpkg の port・Homebrew の formula）を調べ、そのパッケージがインストールした文書を載せる。文書の見つからないライブラリがあれば止まる。Visual C++ のランタイムは名前・著作権者・入手元だけ。8 章の 2・3）。**この内容で各ライセンスの条件を満たすかは、人・専門家の確認待ちです**（8 章の 8、[license_compliance.md](license_compliance.md)）。
- **FFmpeg（ffmpeg・ffprobe）は同梱しません**（決定 No.62）。各自が入れます（4 章）。
- **AI マスク（DEV-20）の ONNX Runtime とモデルも同梱しません**（2026-10-26。判断の表 No.203）。各自がアプリのデータのフォルダの `models`（または実行ファイルのフォルダ）に置きます（[ai_masks.md](ai_masks.md) の 2 章）。置くまで AI マスクのボタンは押せません。
- 自動更新はありません（DIST-04 は later）。新しい版は、同じ手順で上から入れ直します。

### 1.2 設定のファイル

| ファイル | 内容 | いつ使うか |
|---|---|---|
| `crates/genzo-app/tauri.conf.json` | アプリの設定と、配布物の基本（種類・アイコン・NSIS・macOS の最小の版）。リポジトリにあるライセンスの文書（`licenses/`。1.1）の同梱（`bundle.resources`） | いつも |
| `crates/genzo-app/tauri.bundle.conf.json` | ワーカーの同梱（`bundle.externalBin: ["binaries/genzo-worker"]`）と、macOS のアドホック署名（`signingIdentity: "-"`・`hardenedRuntime: false`） | 配布物を作るときだけ（`tauri build --config`） |
| `crates/genzo-app/binaries/tauri.libs.conf.json` | LibRaw のライブラリの同梱（Windows は `bundle.resources`、macOS は `bundle.macOS.frameworks`）と、LibRaw のライセンスの文書・案内（両 OS とも `bundle.resources`）。`scripts/prepare-worker.mjs` が作る（コミットしない） | 配布物を作るときだけ |

`externalBin`・`resources`・`frameworks` を `tauri.conf.json` に書かないのは、Tauri のビルドスクリプト（tauri-build）が `cargo build` のたびにこれらのファイルを探してコピーし、ないと失敗するためです（tauri-build 2.6.3 の `lib.rs`）。分けておくと、ふだんの `cargo build`・`cargo clippy`・CI はワーカーのビルドなしで通ります。例外として、リポジトリにあるライセンスの文書（`crates/genzo-app/licenses/README.txt`・`licenses/third-party/rust.txt`・`npm.txt` とリポジトリの `LICENSE`。いつもある）は `tauri.conf.json` の `bundle.resources` に書きます（`cargo build` でも `target/<profile>/licenses/` に置かれるので、開発中の画面からも読める）。Tauri は `--config` の設定を RFC 7396 の merge で合わせるので、`tauri.libs.conf.json` の `resources`（LibRaw の文書）と足し合わされます。

macOS の `hardenedRuntime: false` は、Hardened Runtime のライブラリの検証（同じ Team ID の署名を求める）で、アドホック署名の dylib を読み込めなくなるのを避けるためです。Hardened Runtime は公証に必要なもので、公証しない今は要りません（公証する場合は有効にし、Developer ID で署名し直す）。

### 1.3 スクリプト（`scripts/`）

| スクリプト | 内容 | ui/ からの起動 |
|---|---|---|
| `scripts/prepare-worker.mjs` | ワーカーを LibRaw ありで release ビルドし（`cargo build --release --locked -p genzo-worker --features libraw`。macOS は同じ内容を `cargo rustc` で実行し、リンカーの引数を足す。2.3）、`crates/genzo-app/binaries/genzo-worker-<target triple>` に置く。LibRaw のライブラリを `binaries/libs/` に集め、`binaries/tauri.libs.conf.json` と同梱の一覧 `binaries/bundled-libs.txt` を作る | `npm run app:worker` |
| `scripts/build-app.mjs` | 第三者のライセンスの文書（`crates/genzo-app/licenses/third-party/`）が今の `Cargo.lock`・`ui/package-lock.json`・`about.toml` から作ったものかを確かめ（古ければ止める）、上の処理の後、`tauri build --config tauri.bundle.conf.json --config binaries/tauri.libs.conf.json <引数> -- --locked` を実行する。最後に、画面のバンドルに入った npm のパッケージ（Vite の `build.license` の出力）がすべて文書に載っているかを確かめる。`--skip-worker` でワーカーのビルドを省く | `npm run app:build -- --bundles nsis` など |
| `scripts/third-party.mjs` | 第三者のライセンスの文書（`rust.txt`・`npm.txt`）を作る（cargo-about 0.9.2 と `ui/node_modules` が要る）。`--check` で、リポジトリの文書と同じかを確かめる（CI） | — |
| `scripts/tauri.mjs` | Tauri の CLI を、この構成で起動する（下の説明） | `npm run app:dev`（`tauri dev`） |

スクリプトは Node（22）だけで動き、新しい npm の依存はありません（`scripts/third-party.mjs` の Rust の文書だけ、別に cargo-about が要る）。上の入口のスクリプトは入口だけで、処理の本体は `scripts/lib/`（`worker.mjs`・`build.mjs`・`tauri-cli.mjs` など）にあります（入口は「直接実行されたか」を判定せずに必ず処理を実行する。判定はシンボリックリンクを含むパスで起動したときに成り立たず、何もせずに成功で終わってしまうため）。テストは `node --test "scripts/**/*.test.mjs"`（CI の ui のジョブでも実行）。

**Tauri の CLI の起動のしかた**: Tauri の CLI（`ui/` の devDependencies の `@tauri-apps/cli` 2.11.5）は、`tauri.conf.json` を「今のフォルダとその下」から探します。この構成では `tauri.conf.json` が `ui/` の外（`crates/genzo-app/`）にあるため、`ui/` で `npx tauri build` を実行しても見つかりません（`npx tauri info` の「App」の欄が空になることで確認）。`scripts/tauri.mjs` は、環境変数 `TAURI_APP_PATH`（Tauri のアプリのフォルダが画面のフォルダの下にない構成のためのもの。@tauri-apps/cli の CHANGELOG）に `crates/genzo-app` を入れ、今のフォルダを `ui/` にして CLI を起動します（`beforeBuildCommand` の `npm run build` は `ui/` で実行される）。手で起動する場合は次のとおりです（先に `npm run app:worker` でワーカーと `binaries/tauri.libs.conf.json` を作っておく。`npm run tauri -- …` は `TAURI_APP_PATH` がないと設定を見つけられません）。

```sh
# macOS・Linux（ui/ で）
TAURI_APP_PATH=../crates/genzo-app npx tauri build --config ../crates/genzo-app/tauri.bundle.conf.json --config ../crates/genzo-app/binaries/tauri.libs.conf.json --bundles app,dmg
```

```powershell
# Windows（ui\ で）
$env:TAURI_APP_PATH = "..\crates\genzo-app"
npx tauri build --config ..\crates\genzo-app\tauri.bundle.conf.json --config ..\crates\genzo-app\binaries\tauri.libs.conf.json --bundles nsis
```

**開発中（`npm run app:dev`）のワーカー**: `tauri dev` は本体を `target/debug/genzo-paris` にビルドしますが、ワーカーはビルドしません。本体と同じフォルダにワーカーが要るので、先に `cargo build -p genzo-worker --features libraw` を実行してください（または `GENZO_WORKER_PATH` でワーカーの場所を指定する）。

## 2. 作り方

### 2.1 共通の準備

- Rust（版は `rust-toolchain.toml` の 1.97.0。rustup が自動で入れる）
- Node 22 と npm。`ui/` で `npm ci` を実行しておく
- 作業の前に、作業ツリーに未コミットの変更がないかを `git status` で確かめてください。`crates/genzo-app/binaries/libs/` はスクリプトが毎回消して作り直し、`binaries/` の中のワーカー・`tauri.libs.conf.json`・`bundled-libs.txt` は上書きします（手で置いたファイルは消えます。`binaries/` はコミットしない）。

成果物は `target/release/bundle/<種類>/` にできます。`target/` はコミットしません。

### 2.2 Windows

1. Visual Studio の C++ のビルドツール（MSVC）と、パッケージ管理の vcpkg を入れる。
2. LibRaw を入れる: `vcpkg install libraw:x64-windows`（依存の jasper・lcms・zlib・libjpeg-turbo もソースからビルドされる。vcpkg の libraw の port は 2026-10-12 の時点で 0.22.2）。
3. LibRaw の場所を指定する: 環境変数 `VCPKG_ROOT`（vcpkg のフォルダ）を設定すれば、スクリプトが `installed\x64-windows` の `include`・`lib`・`bin` を使います。`LIBRAW_INCLUDE_DIR`・`LIBRAW_LIB_DIR`（必要なら `LIBRAW_LIB_NAME`・`LIBRAW_DLL_DIR`）でも指定できますが、**配布物を作るときは、`VCPKG_ROOT` の `installed\<triplet>` の中の LibRaw（指定したフォルダ・同梱する DLL がすべてその中）でなければ止まります**（2026-10-20 から。同梱する DLL のライセンスの文書（`native.txt`）と LibRaw のソースを vcpkg から集めるため。決定事項ログ No.196）。vcpkg 以外で入手した LibRaw は、`cargo build`・`cargo test` では使えますが（`crates/genzo-raw/build.rs` の doc、README）、配布物は作れません。`LIBRAW_LIB_NAME` を指定しなければ、`.lib` のファイル名から選びます（`raw_r.lib` → `libraw_r.lib` → `libraw.lib` → `raw.lib` の順。vcpkg はスレッドセーフな版を `lib\raw_r.lib`、そうでない版を `lib\manual-link\raw.lib` に置く）。
4. 作る（PowerShell、`ui\` で）:

   ```powershell
   $env:VCPKG_ROOT = "C:\vcpkg"
   npm run app:build -- --bundles nsis
   ```

5. 成果物: `target\release\bundle\nsis\GenzoParis_<版>_x64-setup.exe`（版は `tauri.conf.json` の `version`。0.1.1 なら `GenzoParis_0.1.1_x64-setup.exe`）。同梱した DLL は `crates\genzo-app\binaries\bundled-libs.txt` で確かめられます。

スクリプトは、ワーカー（`genzo-worker.exe`）の PE のヘッダから読み込む DLL をたどり、LibRaw のフォルダ（vcpkg の `bin`）にある DLL と、Visual C++ のランタイムの DLL を集めます（`scripts/lib/pe.mjs`・`scripts/lib/bundle.mjs`。dumpbin は使わない）。OS の DLL（UCRT の `api-ms-win-crt-*`・`ucrtbase.dll` を含む。Windows 10 以降は OS の一部）は集めません。

**Visual C++ のランタイム**: 本体 `genzo-paris.exe` は、Visual C++ のランタイムを静的にリンクします（Tauri の CLI が `tauri build` のときに環境変数 `STATIC_VCRUNTIME` を設定し、tauri-build がリンクの設定を変える。@tauri-apps/cli の CHANGELOG と tauri-build 2.6.3 の `static_vcruntime.rs`）。一方、ワーカーと vcpkg の LibRaw の DLL は、ランタイムの DLL（`VCRUNTIME140.dll`・`MSVCP140.dll` など）を使います。**これらの DLL は、インストール先（ワーカーと同じフォルダ）に一緒に置きます**（アプリと一緒に置く配置。決定事項ログ No.73。2026-10-12 に利用者の指示で、利用者に再頒布可能パッケージを入れてもらう方式から変更）。Windows は実行ファイルと同じフォルダの DLL を先に探すので、再頒布可能パッケージが入っていない PC でもワーカーが起動します。

- DLL は、ビルドした PC（GitHub Actions ならランナー）の Visual Studio（Build Tools を含む）の再頒布用のフォルダ（`VC\Redist\MSVC\<版>\x64\Microsoft.VC*.CRT` など）から集めます。探す順は、環境変数 `GENZO_VC_REDIST_DIR`（DLL のあるフォルダ）→ `VCToolsRedistDir` → vswhere で見つけた Visual Studio です。見つからなければ止まります（`scripts/lib/worker.mjs`）。デバッグ版は使いません。
- **アプリと一緒に置いた DLL は、Windows Update では更新されません。** ランタイムにセキュリティの修正が出たときは、新しい Visual Studio でビルドし直したインストーラーを配ってください（再頒布可能パッケージが PC に入っていても、ワーカーは同じフォルダの DLL を使います）。
- 再頒布の条件: Visual Studio のライセンスで再頒布できるファイル（Visual Studio の `REDIST.TXT` の一覧）に当たります。**最終の判断は人が行ってください**（8 章）。

### 2.3 macOS

1. Xcode の Command Line Tools（`xcode-select --install`。`otool`・`install_name_tool`・`codesign` を使う）と Homebrew を入れる。
2. LibRaw を入れる: `brew install libraw pkgconf`（Homebrew の libraw は 2026-10-12 の時点で 0.22.2。依存は jpeg-turbo・little-cms2・libomp。formula で確認）。
3. 作る（`ui/` で）:

   ```sh
   npm run app:build -- --bundles app,dmg
   ```

4. 成果物: `target/release/bundle/dmg/GenzoParis_<版>_aarch64.dmg`（0.1.1 なら `GenzoParis_0.1.1_aarch64.dmg`）と `target/release/bundle/macos/GenzoParis.app`。

スクリプトは、`otool -l` でワーカーの依存をたどって Homebrew の dylib を `binaries/libs/` に写し、参照を書き換え（ワーカーは `@executable_path/../Frameworks/<名前>`、dylib どうしは `@loader_path/<名前>`、dylib の識別名は `@rpath/<名前>`）、`codesign --force --sign -` で署名し直します（書き換えると署名が壊れ、Apple Silicon では署名の壊れた実行ファイルは起動しないため）。書き換えた後に Homebrew の場所への参照が残っていればエラーにします。書き換えの余白のため、ワーカーはリンカーの `-headerpad_max_install_names` 付きでビルドします。Tauri は `.app` に入れた後、`signingIdentity: "-"` で中身を内側から順にアドホック署名し直します。

**動く macOS の版の注意**: Homebrew の dylib は、ビルドした Mac の macOS の版向けの bottle です（Homebrew は macOS の版ごとに bottle を分けて配っていて、`brew install` はその Mac の版向けのものを入れる。GitHub Actions なら macos-latest のランナーの macOS の版向け）。`tauri.conf.json` の `minimumSystemVersion` は 12.0 ですが、dylib の最小の版（`otool -l` の `LC_BUILD_VERSION` の `minos`）がそれより新しいと、古い macOS ではアプリは起動するのにワーカーが起動せず、RAW を開けません。

- スクリプトは、この場合 **既定ではエラーにして止めます**（どの dylib が何の版向けかを表示する）。
- 配る相手の Mac がすべてその版以降なら、環境変数 `GENZO_ALLOW_NEWER_MINOS=1` を付けて実行すると通ります。このとき、`.app` の最小の版（Info.plist の `LSMinimumSystemVersion`）を、同梱するものの `minos` の最大に上げます（`binaries/tauri.libs.conf.json` に書く。`tauri.conf.json` は変えない。Tauri はこの版を Info.plist と、本体のビルドの `MACOSX_DEPLOYMENT_TARGET` に使う。@tauri-apps/cli 2.11.5 の `config.schema.json` の説明）。それより古い Mac では、アプリが起動せず、OS がその旨を知らせます（文言は要確認）。
- どちらの場合も、`.app` の最小の版と同梱したものの `minos` の最大を `binaries/bundled-libs.txt` の先頭に書きます。
- 配る相手に古い macOS の人がいる場合は、LibRaw とその依存を `MACOSX_DEPLOYMENT_TARGET=12.0` でソースからビルドしたものが要ります（スクリプトは未対応。手順は未整備）。どうするかは人が決めてください（8 章）。
- **決定（2026-10-12。決定事項ログ No.74）**: 配る相手の Mac はすべて macOS 26 以降なので、`.app` の最小の版を上げて配ります。GitHub Actions の macos-latest で入る dylib は 4 つとも `minos` 26.0 で、`.app` の最小の版は 26.0 になりました（6.2）。

Intel の Mac 向け（x86_64）は作りません（要るかは人の判断。作る場合は Intel の Mac か、x86_64 の Homebrew の LibRaw が要る）。

### 2.4 Linux（確認用）

`ui/` で `npm run app:build -- --bundles deb`。LibRaw は同梱せず、.deb の依存にも書きません（Ubuntu 24.04 なら `sudo apt install libraw23t64` が要る。パッケージの名前は版によって違う）。Linux は配布の対象ではなく、スクリプトと設定の確認のためだけに使います。ビルドには GTK・WebKitGTK の開発用のパッケージが要ります。

### 2.5 GitHub Actions（`.github/workflows/release.yml`）

手動（Actions の画面の「Run workflow」）だけで、Windows（windows-latest）と macOS（macos-latest。Apple Silicon）の配布物を作ります（タグの push では動きません。2026-10-12 の独立レビューの指摘で変更）。手順は 2.2・2.3 と同じで、LibRaw は vcpkg（ランナーにある `VCPKG_INSTALLATION_ROOT`）と Homebrew で入れます。

- **公開の範囲の確認**: リポジトリが公開（public）のときは、Actions の artifact を GitHub にログインした誰でも取得できます（2026-10-12 時点で、このリポジトリは公開）。そのため、入力の「allow_public_artifacts」に印を付けないと、最初の段階で何も作らずに止まります。印を付けるのは、配布の責任者が、配布物を誰でも取得できる状態にしてよいこと（LibRaw の LGPL などの条件・署名のない配布物を社外に出してよいか。8 章）を確かめた後だけにしてください。配る相手だけに渡したい場合は、非公開のリポジトリで実行するか、手元で作って（2.2・2.3）社内の共有の場所に置いてください。非公開のリポジトリでは、この確認は行いません。
- 実行の画面で「Use workflow from」に `v` で始まるタグ（`v0.1.1` など）を選んだときは、最初にタグの版（`v0.1.1` なら 0.1.1）と、`tauri.conf.json` の `version`・Cargo の版（genzo-app・genzo-worker）が同じかを確かめ、違えば止めます（配布物のファイル名・中の版は `tauri.conf.json` の版になるため。版を直してからタグを付け直す）。
- macOS では、同梱する dylib が `.app` の最小の版より新しい macOS 向けだと止まります（2.3）。配る相手の Mac がすべてその版以降なら、実行の画面で「allow_newer_macos_libs」に印を付けて実行します。このとき、artifact の名前に `-macos<版>-or-later` が付き、ジョブの summary に各ファイルの `minos` が出ます。
- 配布物を作る前に、`licenses` ジョブ（Linux）で、Rust の crate のライセンスの文書（`crates/genzo-app/licenses/third-party/rust.txt`）を cargo-about 0.9.2 で作り直し、リポジトリのものと同じかを確かめます（CI の `third-party` ジョブと同じ。通らなければ配布物のジョブは動かない。[第三者の台帳](third_party.md) の 1.4 節）。
- 配布物を作った後、artifact に置く **前に**、配布物の中のワーカーが同梱の LibRaw で起動できるかを確かめます。
  - Windows: インストーラーを `/S` で無人インストールし、`PATH` を OS のフォルダだけにして `genzo-worker.exe --version` を実行する（終了コードと「genzo-worker <版>」の出力）。
  - 両 OS: `licenses/` の文書があるか、`third-party/native.txt` に同梱したファイル（Windows は `bundled-libs.txt` の DLL、macOS は `Contents/Frameworks` の dylib）がすべて載っているか、`LibRaw/source/` にソースのアーカイブがあるか（2026-10-20 から）。
  - macOS: `codesign --verify`、Info.plist の最小の版、`hdiutil verify`（.dmg）を見た後、`DYLD_PRINT_LIBRARIES=1` でワーカーを起動し、終了コード・「genzo-worker <版>」の出力・dyld のエラー（`Library not loaded`）がないこと・`Contents/Frameworks` のすべての dylib を `.app` の中から読み込んだこと・Homebrew の場所から読み込んでいないことを確かめる。
- 外部のアクション（actions/checkout など）は、コミットの SHA で固定しています（ci.yml も同じ。タグの付け替えで、配布物を作る処理が変わらないように）。SHA は 2026-10-12 に `git ls-remote` で、その時点の各タグ（コメントに書いた版）が指すコミットを調べたものです。更新するときは、新しい版のタグが指す SHA を同じ方法で調べ、差分を確かめてから書き換えてください。
- 成果物は Actions の artifact（`genzoparis-windows-x64`・`genzoparis-macos-arm64`。30 日で消える。仮置き）に置きます。GitHub の Releases には出しません。**artifact は、リポジトリを読める人なら誰でも取得できます。リポジトリの公開の範囲を確かめてから実行してください。**
- 途中で失敗したときは、調べるために `<名前>-unverified` の artifact（7 日で消える。仮置き）に、できたところまでの配布物と確認の記録を置きます。**`-unverified` の配布物は配らないでください。**
- 2026-10-12 に main で 2 回実行しました。2 回目（「allow_newer_macos_libs」にも印）で、両 OS の配布物ができ、確認が通りました（6.2）。**GitHub の macos-latest・windows-latest のランナーの版は変わります**。次に実行したときも、artifact の名前の `-macos<版>-or-later` が 26.0 のままか、ジョブの記録に警告・エラーがないかを確かめてください。

### 2.6 配る前の確認

1. 入れて起動し、ARW を 1 枚取り込んでサムネイルと現像の画面が出ることを確かめる（[M5 の受け入れの手順](m5_acceptance.md) の 0 章）。設定の画面の「カタログ」で、「RAW のデコーダ」が「なし（RAW を開けません）」でないこと（LibRaw の識別子が出る）、「ワーカーのエラー」が出ていないことも確かめる（ワーカーや LibRaw のライブラリが見つからない場合は、ここに出る。`ui/src/dialogs/SettingsCatalog.tsx` の表示。実機で未確認）。
2. 配布物の SHA-256 を記録し、配る相手に伝える（受け取った人が、改ざんされていないかを確かめられるように）:
   - Windows（PowerShell）: `Get-FileHash .\GenzoParis_0.1.1_x64-setup.exe -Algorithm SHA256`（版は配る配布物のもの）
   - macOS: `shasum -a 256 GenzoParis_0.1.1_aarch64.dmg`
3. 配る場所（社内の共有フォルダ・チャットなど）は、**見られる人の範囲が配る相手に限られているかを確かめてから** 置いてください。共有フォルダに置く場合は、上書きで前の版を壊さないよう、版ごとに別の名前（ファイル名に版が入る）で置いてください。

## 3. 署名なしのアプリを開く

署名・公証をしていないため、OS が警告を出します。**次の手順は OS の保護を一部外すことになります。2.6 の SHA-256 を確かめ、信頼できる経路で受け取ったファイルにだけ使ってください。** 画面の文言は OS の版で変わります（要確認）。

### 3.1 Windows（SmartScreen）

1. インストーラーを開くと「Windows によって PC が保護されました」と出る場合があります。
2. 「詳細情報」を押し、発行元が「不明な発行元」になっていることを確かめてから「実行」を押します。
3. 管理者の権限は求められません（ユーザーごとのインストール）。求められた場合は、別のファイルの可能性があるので中止してください。

注意:
- Windows 11 の「スマート アプリ コントロール」が有効な PC では、署名のないアプリが止められ、上の手順では開けない場合があります（要確認）。スマート アプリ コントロールは、一度オフにすると簡単にはオンに戻せない場合があるため（Windows の版による。要確認）、オフにするかは本人と管理者が判断してください。
- 会社の PC では、管理者の設定（アプリの実行の制限・ウイルス対策のソフト）で止められる場合があります。その場合は管理者に相談してください。
- 入れた後、インストールしたファイル（`genzo-worker.exe`）がウイルス対策のソフトに止められると、RAW を開けません。

### 3.2 macOS（Gatekeeper）

アプリはアドホック署名だけで、公証していません。ブラウザ・チャットなどで受け取ったファイルには「検疫」の印が付き、最初に開くときに止められます（Tauri の資料にも、アドホック署名でも「プライバシーとセキュリティ」での許可が要るとある）。

1. .dmg を開き、`GenzoParis.app` を「アプリケーション」へドラッグする。
2. 「アプリケーション」の GenzoParis を開く。「開けません」「Apple は、悪質なソフトウェアが含まれていないことを検証できませんでした」などと出たら、「完了」（または「キャンセル」）で閉じる。
3. 「システム設定」→「プライバシーとセキュリティ」を開き、下の方の「“GenzoParis”は開発元を確認できないため…」の横の「このまま開く」を押し、パスワード（または Touch ID）で許可する。
4. もう一度開き、確認の画面で「開く」を押す。以後は普通に開けます。

- macOS 14 以前では、Finder で Control キーを押しながらクリック →「開く」でも開けます。macOS 15 以降はこの方法が使えず、上の 3 の手順が要ります（Apple のサポートの「開発元が不明な Mac アプリを開く」。要確認）。
- 「壊れているため開けません」と出る場合は、署名が壊れている可能性があります（作り方の誤り）。配った人に知らせてください。端末で `xattr -dr com.apple.quarantine /Applications/GenzoParis.app` を実行すると検疫の印を外せますが、OS の確認を飛ばすことになるので、配った人に確かめてから使ってください。

## 4. FFmpeg の入れ方（動画のサムネイル・メタデータ）

FFmpeg は同梱しません（決定 No.62）。動画は FFmpeg がなくても登録できますが、サムネイルとメタデータ（長さ・fps など）が取れません。アプリは ffmpeg・ffprobe を、設定でフォルダを指定していればそのフォルダだけで、指定していなければ「`PATH`（絶対パスの項目だけ） → OS ごとの既定の場所」の順に探します（`crates/genzo-app/src/tools.rs`）。入れた後はアプリを起動し直してください。設定の画面の「一般」で、ffmpeg・ffprobe の横に「見つかりました」と出れば使えます。

| OS | 入れ方 | アプリが探す既定の場所 |
|---|---|---|
| Windows | `winget install --id Gyan.FFmpeg -e`（パッケージの名前は `winget search ffmpeg` で確かめる。要確認） | `%LOCALAPPDATA%\Microsoft\WinGet\Links`（winget がリンクを置く場所）・`%ProgramFiles%\ffmpeg\bin`（`C:\ffmpeg\bin` のような、ほかの利用者も書ける場所は探さない。そこに置いた場合は、設定でフォルダを指定する） |
| macOS | `brew install ffmpeg` | `/opt/homebrew/bin`（Apple Silicon の Homebrew）・`/usr/local/bin`・`/opt/local/bin`（Finder から起動したアプリには端末の `PATH` が渡らないため） |

ほかの場所に入れた場合は、設定の画面で ffmpeg のあるフォルダを指定してください。

注意: winget・Homebrew で入る FFmpeg のビルドの構成（GPL のコーデックを含むか）と、H.264 / H.265 の特許の扱いは確認していません。各自が入れる場合でも、会社で使ってよいかは人の確認待ちです（8 章、[第三者の台帳](third_party.md) の 3.4 節）。

## 5. カタログ・設定の場所と、アンインストール

| もの | 場所（Windows） | 場所（macOS） |
|---|---|---|
| 既定のカタログ（決定 No.63） | OS の「ピクチャ」のフォルダの `GenzoParis\catalog.db`（通常は `%USERPROFILE%\Pictures\GenzoParis\catalog.db`） | `~/Pictures/GenzoParis/catalog.db` |
| カタログのデータ（サムネイル・プレビュー・自動バックアップ） | カタログの隣の `catalog-data` | 同じ |
| アプリの設定（前回のカタログ・FFmpeg の場所・GPU） | `%APPDATA%\io.github.tokamocchi.genzoparis\settings.json` | `~/Library/Application Support/io.github.tokamocchi.genzoparis/settings.json` |
| 画面（WebView）のデータ | `%LOCALAPPDATA%\io.github.tokamocchi.genzoparis\`（要確認） | `~/Library/WebKit/` などの下（要確認） |

- 最初の起動で、カタログを作る場所を確かめる画面が出ます。ピクチャのフォルダが OneDrive・iCloud などで同期されている場合、カタログ（SQLite のデータベース）を同期のフォルダに置くと、同期の衝突で壊れるおそれがあります（要確認）。その場合は、同期されない場所を選んでください。
- **アンインストールしても、カタログ・写真・カタログのデータは消えません。**
  - Windows:「設定」→「アプリ」→ GenzoParis →「アンインストール」。アンインストーラーの「アプリのデータを削除する」に印を付けた場合は、上の表のアプリの設定と画面のデータ（`%APPDATA%` と `%LOCALAPPDATA%` の `io.github.tokamocchi.genzoparis`）も消えますが、ピクチャのフォルダのカタログは消えません（Tauri の NSIS のテンプレート（GitHub の tauri-apps/tauri の dev の版）で確認。実機で未確認）。
  - macOS: 「アプリケーション」の GenzoParis.app をゴミ箱に入れる。設定とカタログは残ります。
  - カタログも消す場合は、`Pictures/GenzoParis` を手で消してください（写真の元ファイルは消えませんが、評価・現像の設定は失われます。消す前に、必要ならフォルダごとコピーしてバックアップしてください）。
- 入れ直し・新しい版への更新の前にも、念のためカタログのフォルダ（`Pictures/GenzoParis`）をコピーしてバックアップしておくことを勧めます。

## 6. 確認の結果

### 6.1 この環境（Linux）

2026-10-12、Ubuntu 24.04 のコンテナ（4 コア）で、LibRaw（apt の libraw-dev。`pkg-config --modversion libraw_r` は 0.21.2）を入れて確かめました。

1. `ui/` で `npm run app:build -- --bundles deb` を実行した（このときは、並行して作業中の画面のコードに型の誤りがあったため、`--config '{"build":{"beforeBuildCommand":"npx vite build"}}'` を足して型の検査を省いた。配布物の中身には関係しない）。ワーカーのビルド（ビルド済みで 0.25 秒）→ Tauri のビルド（本体の release ビルドを含めて 13 分 28 秒）で、`target/release/bundle/deb/GenzoParis_0.1.0_amd64.deb` ができた。
2. `dpkg -c` で、`usr/bin/genzo-paris`（本体）と `usr/bin/genzo-worker`（ワーカー）が入っていることを確かめた。ワーカーは `crates/genzo-app/binaries/genzo-worker-x86_64-unknown-linux-gnu` と同じファイル（`cmp` で一致）。
3. `dpkg -x` で取り出し、`ldd` で、ワーカーは `libraw_r.so.23` に依存し、本体は LibRaw に依存しないことを確かめた（決定 No.32）。取り出したワーカーの `--version` は「genzo-worker 0.1.0」を出して終了コード 0。
4. `bundle.resources` の対応表（Windows で DLL を置くのと同じ書き方。`"binaries/libs/<名前>": "<名前>"`）に試しのファイルを足して作り直し、.deb の resource のフォルダの直下（`usr/lib/GenzoParis/<名前>`）に入ることを確かめた（パスは `crates/genzo-app` から解決される）。確かめた後、試しのファイルを除いて作り直した。
5. `--config` は複数指定でき、順にマージされること、`--` の後の引数（`--locked -v`）が cargo に渡ることを確かめた（`-v` の出力が出た）。
6. `.deb` の依存（`Depends`）は `libwebkit2gtk-4.1-0, libgtk-3-0` だけで、LibRaw は入っていない（2.4 のとおり）。
7. スクリプトを入口と `scripts/lib/` に分けた後（1.3）、`ui/` で `npm run app:build -- --bundles deb` を差し替えなし（`beforeBuildCommand` の `npm run build` で型の検査を含む）で実行し直し、通ることを確かめた（2 分 3 秒。本体は増分のビルド）。`dpkg -c` で本体とワーカーが入り、取り出したワーカーの `--version` が終了コード 0 で、`binaries` の中のファイルと `cmp` で一致した。シンボリックリンクを含むパスで `scripts/tauri.mjs` を起動しても処理が実行されることも確かめた（`scripts/entry.test.mjs`）。

### 6.2 GitHub Actions（`release.yml`。2026-10-12）

main の c0681eb（PR #2 をマージしたもの）で、手動で 2 回実行しました。リポジトリは公開で、artifact は GitHub にログインした誰でも取得できます（2.5。利用者の指示で、「allow_public_artifacts」に印を付けて実行した）。数値・ファイル名は、ジョブの記録から写したものです。

1. 1 回目（run 38263152398。「allow_newer_macos_libs」は印なし）
   - Windows: 成功（artifact `genzoparis-windows-x64`）。
   - macOS: 「ワーカーのビルドと LibRaw の収集」で止まった。同梱する 4 つの dylib（`libraw_r.25.dylib`・`libomp.dylib`・`libjpeg.8.dylib`・`liblcms2.2.dylib`）が、いずれも `minos` 26.0 だったため（2.3 のとおり）。
   - 配布物ができる前に止まったので、`-unverified` の artifact には何も置かれなかった。
2. 2 回目（run 38276847388。「allow_newer_macos_libs」にも印を付けた。決定事項ログ No.74）: 両 OS とも成功。
   - **Windows**（windows-latest）
     - 配布物: `GenzoParis_0.1.0_x64-setup.exe`。artifact は `genzoparis-windows-x64`（zip で 7,778,435 バイト）。
     - 同梱した DLL は 6 つ。
       - vcpkg の `C:\vcpkg\installed\x64-windows\bin` から: `raw_r.dll`・`lcms2-2.dll`・`z.dll`
       - Visual Studio 18 Enterprise の `VC\Redist\MSVC\14.51.36231\x64\Microsoft.VC145.CRT` から: `VCRUNTIME140.dll`・`VCRUNTIME140_1.dll`・`MSVCP140.dll`
     - 確認: `/S` で無人インストールし、`%LOCALAPPDATA%\GenzoParis` に次のものが入った。
       - `genzo-paris.exe`・`genzo-worker.exe`・`uninstall.exe`
       - 上の 6 つの DLL
     - `PATH` を OS のフォルダだけにした `genzo-worker.exe --version` が「genzo-worker 0.1.0」を出した。
   - **macOS**（macos-latest。Xcode 26.6）
     - 配布物: `GenzoParis_0.1.0_aarch64.dmg` と `GenzoParis.app.tar.gz`。artifact は `genzoparis-macos-arm64-macos26.0-or-later`（zip で 21,019,829 バイト）。
     - 同梱した dylib は 4 つで、いずれも `minos` 26.0。元は Homebrew の次の版。
       - `libraw_r.25.dylib`: libraw 0.22.2
       - `libomp.dylib`: libomp 23.1.0
       - `libjpeg.8.dylib`: jpeg-turbo 3.2.0
       - `liblcms2.2.dylib`: little-cms2 2.19.1
     - `GENZO_ALLOW_NEWER_MINOS=1` により、`.app` の最小の版を 12.0 から 26.0 に上げた（ジョブの警告として出る）。
     - 確認で通ったこと:
       - `codesign --verify --deep --strict`（valid on disk・satisfies its Designated Requirement）
       - Info.plist の最小の版が 26.0 であること
       - `hdiutil verify`（VALID）
       - `DYLD_PRINT_LIBRARIES=1` で起動したワーカーが「genzo-worker 0.1.0」を出し、4 つの dylib を `.app` の `Contents/Frameworks` から読み込んだこと（Homebrew の場所からは読み込んでいない）
     - dyld の記録の形は `dyld[<pid>]: <UUID> <パス>` で、展開したパスは `…/GenzoParis.app/Contents/Frameworks/<名前>` の形だった（7 章の No.7a）。
     - 公証は行わない（Tauri は「skipping app notarization」の警告を出す。3.2）。
     - 確認の記録の「LSMinimumSystemVersion: …」の行には、版が出なかった。macOS のランナーの bash が、変数名のすぐ後の全角の括弧の先頭のバイトまで変数名として読んだため（1 バイトの文字を Latin-1 として分類するロケールの bash で、同じ出力になることを確かめた。macOS で起きる仕組みは推測）。比べる処理は引用符で囲んだ変数で行うので、正しく行われている。2026-10-12 に `${名前}` で囲むよう直し、ワークフローを調べるテスト（`scripts/workflows.test.mjs`）を足した。
3. artifact は、30 日で消える（2 回目のものは 2026-11-11 の 16:29〜16:32 UTC。1 回目の Windows の artifact は同じ日の 14:46 UTC）。

## 7. 未確認の事項

| No | 事項 | 確かめ方 |
|---|---|---|
| 1 | ~~release.yml を GitHub Actions で一度も実行していない~~ **確認済み（2026-10-12。6.2）**: 両 OS の成果物ができ、最後の確認の手順が通った | — |
| 2 | Windows: vcpkg の libraw の `.lib`・DLL の名前（`raw_r`）と、スレッドセーフな版であること（vcpkg の port のファイルで確認しただけ）。**名前は 2026-10-12 の Actions で確認**（`raw_r.dll`。6.2）。スレッドセーフな版であることは未確認 | 2.2 の手順で作り、`bundled-libs.txt` と ARW の展開を確かめる |
| 3 | Windows: 集めた DLL だけで、別の PC（vcpkg のない PC）でワーカーが起動するか | 別の PC に入れて ARW を開く。release.yml の確認は vcpkg のあるランナーで、DLL は PATH にないが、OS の DLL の一覧の漏れは見つけられない |
| 4 | Windows: Visual C++ のランタイムがない PC での動き（2.2。ランタイムの DLL を同梱するようにした。決定事項ログ No.73） | 再頒布可能パッケージを入れていない PC か、新しく入れた Windows で、RAW を開けるか確かめる |
| 5 | Windows: NSIS のインストーラーの `/S`（無人インストール）、インストール先、アンインストーラーの「アプリのデータを削除する」の動き。**`/S` とインストール先（`%LOCALAPPDATA%\GenzoParis`）は 2026-10-12 の Actions で確認**（6.2） | アンインストーラーを実機で確かめる |
| 6 | Windows: SmartScreen・スマート アプリ コントロールでの動き（3.1） | 実機で確かめる |
| 7 | macOS: `otool -l` の出力の読み取り、dylib の書き換え・署名（スクリプトは出力の例でテストしただけ）。**2026-10-12 の Actions で確認**（6.2。ワーカーが `.app` の中の dylib だけを読み込んだ） | 2.3 の手順で作り、`otool -L` と `DYLD_PRINT_LIBRARIES=1 …/genzo-worker --version` で、`.app` の中の dylib を読むことを確かめる |
| 7a | macOS: release.yml の確認が前提とする `DYLD_PRINT_LIBRARIES` の出力の形（`dyld[<pid>]: <UUID> <パス>`）と、`@executable_path/../Frameworks` を展開したパスの形（`MacOS/../Frameworks` のままか）。形が違うと、正しい配布物でも確認で止まる（壊れた配布物を通すことはない）。**2026-10-12 の Actions（macOS 26 のランナー）で確認**: `dyld[<pid>]: <UUID> <パス>` の形で、パスは `Contents/Frameworks/<名前>` の形だった（6.2） | release.yml を実行し、確認のステップの記録（`target/verify-logs/`）を見る |
| 8 | macOS: Tauri のアドホック署名と `hardenedRuntime: false` で、`codesign --verify --deep --strict` が通るか、Gatekeeper の手順（3.2）のとおりに開けるか。**`codesign --verify` は 2026-10-12 の Actions で通った**（6.2）。Gatekeeper の手順は未確認 | 実機で確かめる |
| 9 | ~~macOS: Homebrew の dylib の実際の最小の版と、配る相手の macOS の版~~ **確認済み（2026-10-12）**: 4 つとも `minos` 26.0 で、既定では止まった。配る相手はすべて macOS 26 以降（決定事項ログ No.74）。`GENZO_ALLOW_NEWER_MINOS=1` で Info.plist の最小の版が 26.0 に上がった（6.2） | 次に実行したときも、artifact の名前の版を確かめる（2.5） |
| 10 | ~~macOS: GitHub Actions のランナーでの .dmg の作成（Finder の操作を使う）~~ **確認済み（2026-10-12。6.2）**: .dmg ができ、`hdiutil verify` が通った | — |
| 11 | ~~Tauri の出力のファイル名~~ **確認済み（2026-10-12。6.2）**: `GenzoParis_0.1.0_x64-setup.exe`・`GenzoParis_0.1.0_aarch64.dmg` | — |
| 12 | 画面（WebView）のデータの場所（5 章の表） | 実機で確かめる |
| 13 | winget の FFmpeg のパッケージの名前（4 章） | `winget search ffmpeg` |
| 14 | ピクチャのフォルダが同期のフォルダのときの、カタログの扱い（5 章） | 実機で確かめる。必要ならアプリで警告するかを決める |

## 8. 人・専門家の確認待ちの事項（法的な事項を含む）

**次の事項は AI では決められません。社外に配る前（社内でも、必要と判断した場合は配る前）に、人（必要なら専門家）が確認し、結果を [決定事項ログ](decision_log.md) と [第三者の台帳](third_party.md) に書いてください。**

1. **LibRaw の同梱と LGPL-2.1**（決定 No.32・175、台帳の 3.1 節）: 配布物に LibRaw の共有ライブラリを入れるときに、LGPL の条件を満たす方法（ライセンスの文書を同梱するか、対応するソースの提供の方法、利用者がライブラリを差し替えられること）。**2026-10-19 から、LGPL-2.1 の全文・LibRaw の著作権表示・案内（ソースの入手先として上流の LibRaw の版のタグと、ビルドの手順（vcpkg の port・Homebrew の formula）の場所、差し替えの方法）を配布物に入れます**（1.1）。**2026-10-20 から、ビルドに使った LibRaw のソースのアーカイブ（vcpkg は LibRaw と LibRaw-cmake、Homebrew は LibRaw の配布物）も `licenses/LibRaw/source/` に入れます**（決定事項ログ No.196。portfile の SHA512・formula の SHA-256 と同じかを確かめる）。これで LGPL-2.1 の第 4 節・第 6 節（ソースの提供）などの条件を満たすか（ソースの範囲を含む）は、人（必要なら専門家）が確認してください（[license_compliance.md](license_compliance.md) の 4 章の 3）。macOS では、`.app` の中の dylib を差し替えると署名が壊れるため、差し替えた人がアドホック署名し直す必要があります（案内に書いた）。
2. **LibRaw の依存ライブラリのライセンス**: 2026-10-12 の配布物で同梱したのは、Windows（vcpkg）は lcms2（`lcms2-2.dll`）・zlib（`z.dll`）、macOS（Homebrew）は libomp・little-cms2・jpeg-turbo（6.2。一覧と SHA-256 は artifact の中の `bundled-libs.txt`）。**2026-10-20 から、同梱したものごとに入手元のパッケージがインストールしたライセンスの文書を `licenses/third-party/native.txt` に載せます**（決定事項ログ No.196。文書が見つからなければ止まる。libjpeg を同梱したときは IJG の一文を書く）。ライセンスごとの条件と対応は [license_compliance.md](license_compliance.md)。この内容で足りるかは人が確認してください。
3. **Visual C++ のランタイムの扱い**（2.2）: DLL をインストール先に同梱する（決定事項ログ No.73。利用者の指示、2026-10-12）。Microsoft の再頒布の条件の確認と、Windows Update で更新されないことの扱い（ビルドし直して配る）は人が判断する。**2026-10-20 に AI が調べた Visual Studio 2026 の条項（第三者の写しで読んだ）には、配布する人と利用者に保護の条件への同意を求めること・Microsoft を補償すること、という文書だけでは満たせない条件があり、GPL との関係も含めて、Windows の配布物を公開する前に人（専門家）の確認が要る**（implementation_status の判断の表 No.200、[license_compliance.md](license_compliance.md) の 4 章の 1）。`native.txt` には名前・著作権者・入手元だけを書く。**2026-10-20、利用者が方式 A（同梱のまま、インストーラーで使用条件への同意を求める）を選んだ**（決定事項ログ No.199）: Windows の配布のビルドで、インストーラーの同意の画面の文書を、使用条件（`crates/genzo-app/licenses/windows/vcruntime-terms.txt`。AI の文案）と GPL-3.0 の全文をまとめたものに差し替え、使用条件をインストール先の `licenses/third-party/vcruntime-terms.txt` にも入れる。文言の法的な確認と、補償の義務の扱いは専門家の確認待ち。
4. **FFmpeg**（決定 No.62、台帳の 3.4 節）: 各自が入れる FFmpeg のビルドの構成と、H.264 / H.265 の特許の扱い。
5. ~~**macOS の最小の版**（2.3）~~ **決定済み（2026-10-12。決定事項ログ No.74）**: 配る相手の Mac はすべて macOS 26 以降なので、`.app` の最小の版を 26.0 に上げて配る。
6. **署名・公証の要否**（SYS-08）: 社内の数名への配布なら不要としていますが、配る範囲が広がる場合や、会社の規則で署名が求められる場合は、Windows のコード署名の証明書（有償の場合あり）と、Apple Developer Program（有償）が要ります。費用・手続きは確認していません。
7. **成果物を置く場所の公開の範囲**（2.5・2.6）: GitHub Actions の artifact、社内の共有フォルダなどを、配る相手だけが見られるか。
8. **第三者のライセンスの文書**（2026-10-20。決定事項ログ No.194〜197、台帳の 1.4 節、implementation_status の判断の表 No.198〜201、照合の資料 [license_compliance.md](license_compliance.md)）: 配布物の `licenses/` に入れた Rust の crate・npm のパッケージ・同梱の C / C++ の共有ライブラリの一覧と文書（ライセンスの選び方、本文のファイルがない crate の扱いを含む）、LibRaw のソース、アプリ自体の GPL-3.0 の全文・ソースの場所の書き方で、各ライセンスの条件を満たすか。**2026-10-19 に公開した v0.1.0 の配布物には、これらの文書が入っていない**（v0.1.0 の添付を差し替えるか、次の版を待つかは人の判断）。
