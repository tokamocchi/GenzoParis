# 配布物（インストーラー）の作り方と注意（SYS-08）

GenzoParis を社内の数名に配るための、配布物の作り方・入れ方・注意をまとめます。

- 作成: AI（2026-10-12）。**この Linux の開発環境で確かめたのは、同じスクリプトで Linux の .deb を作り、本体とワーカーが入ることまで** です（6 章）。Windows・macOS の配布物は、実機でも GitHub Actions でもまだ作っていません（7 章の「未確認の事項」）。
- 関係する決定: [決定事項ログ](decision_log.md) の No.32（ワーカーを別の実行ファイルにし、本体を LibRaw に依存させない）・No.62（FFmpeg を同梱しない）・No.63（既定のカタログの場所）。
- **法的な事項（LibRaw の LGPL の扱い、依存ライブラリ・Visual C++ のランタイムの再頒布、FFmpeg）は、人・専門家の確認待ちです**（8 章）。確認が済むまでは、社外に配らないでください。
- 署名・公証はしません（社内の数名への配布のため）。署名が必要かどうかは人が判断します（[01 の SYS-08](01_feature_inventory.md)）。

## 1. 配布物の構成

### 1.1 中身

| OS | 形式 | 入る場所と中身 |
|---|---|---|
| Windows（x64） | NSIS のインストーラー（`GenzoParis_<版>_x64-setup.exe`）。ユーザーごとのインストールで、管理者の権限は要らない（`tauri.conf.json` の `installMode: currentUser`） | `%LOCALAPPDATA%\GenzoParis\` に、本体 `genzo-paris.exe`・ワーカー `genzo-worker.exe`・LibRaw の DLL（vcpkg なら `raw_r.dll`）とその依存の DLL・`uninstall.exe` |
| macOS（Apple Silicon） | `.dmg`（`GenzoParis_<版>_aarch64.dmg`）。中の `GenzoParis.app` を「アプリケーション」へ入れる | `GenzoParis.app/Contents/MacOS/` に本体 `genzo-paris` とワーカー `genzo-worker`、`Contents/Frameworks/` に LibRaw の dylib（`libraw_r.*.dylib`）とその依存（libomp・little-cms2・jpeg-turbo など） |
| Linux（確認用） | `.deb`（`GenzoParis_<版>_amd64.deb`） | `/usr/bin/genzo-paris`・`/usr/bin/genzo-worker`。LibRaw は同梱しない（OS のパッケージを使う。配布の対象ではない） |

ファイル名は Tauri の命名の決まりによるもので、Windows・macOS は未確認です（Linux の .deb は 6 章で確認）。

- **本体は LibRaw に依存しません。** LibRaw を読み込むのはワーカーだけです（決定 No.32）。本体はワーカーを「環境変数 `GENZO_WORKER_PATH` → 本体と同じフォルダ」の順に探します（`crates/genzo-worker/src/client.rs` の `locate_worker_executable`）。Tauri の `externalBin` は、ワーカーを本体と同じフォルダ（Windows はインストール先、macOS は `Contents/MacOS`、.deb は `/usr/bin`）に置くので、そのまま見つかります。
- **LibRaw のライブラリはワーカーから読める場所に置きます。** Windows は、実行ファイルと同じフォルダの DLL を最初に探すので、インストール先のフォルダに置きます（Tauri の `bundle.resources`。Windows の resource のフォルダはインストール先）。macOS は `Contents/Frameworks` に置き（Tauri の `bundle.macOS.frameworks`）、ワーカーと dylib の参照を `@executable_path/../Frameworks/…`・`@loader_path/…` に書き換えます（`install_name_tool`）。
- **FFmpeg（ffmpeg・ffprobe）は同梱しません**（決定 No.62）。各自が入れます（4 章）。
- 自動更新はありません（DIST-04 は later）。新しい版は、同じ手順で上から入れ直します。

### 1.2 設定のファイル

| ファイル | 内容 | いつ使うか |
|---|---|---|
| `crates/genzo-app/tauri.conf.json` | アプリの設定と、配布物の基本（種類・アイコン・NSIS・macOS の最小の版） | いつも |
| `crates/genzo-app/tauri.bundle.conf.json` | ワーカーの同梱（`bundle.externalBin: ["binaries/genzo-worker"]`）と、macOS のアドホック署名（`signingIdentity: "-"`・`hardenedRuntime: false`） | 配布物を作るときだけ（`tauri build --config`） |
| `crates/genzo-app/binaries/tauri.libs.conf.json` | LibRaw のライブラリの同梱（Windows は `bundle.resources`、macOS は `bundle.macOS.frameworks`）。`scripts/prepare-worker.mjs` が作る（コミットしない） | 配布物を作るときだけ |

`externalBin`・`resources`・`frameworks` を `tauri.conf.json` に書かないのは、Tauri のビルドスクリプト（tauri-build）が `cargo build` のたびにこれらのファイルを探してコピーし、ないと失敗するためです（tauri-build 2.6.3 の `lib.rs`）。分けておくと、ふだんの `cargo build`・`cargo clippy`・CI はワーカーのビルドなしで通ります。

macOS の `hardenedRuntime: false` は、Hardened Runtime のライブラリの検証（同じ Team ID の署名を求める）で、アドホック署名の dylib を読み込めなくなるのを避けるためです。Hardened Runtime は公証に必要なもので、公証しない今は要りません（公証する場合は有効にし、Developer ID で署名し直す）。

### 1.3 スクリプト（`scripts/`）

| スクリプト | 内容 | ui/ からの起動 |
|---|---|---|
| `scripts/prepare-worker.mjs` | ワーカーを LibRaw ありで release ビルドし（`cargo build --release --locked -p genzo-worker --features libraw`）、`crates/genzo-app/binaries/genzo-worker-<target triple>` に置く。LibRaw のライブラリを `binaries/libs/` に集め、`binaries/tauri.libs.conf.json` と同梱の一覧 `binaries/bundled-libs.txt` を作る | `npm run app:worker` |
| `scripts/build-app.mjs` | 上の処理の後、`tauri build --config tauri.bundle.conf.json --config binaries/tauri.libs.conf.json <引数> -- --locked` を実行する。`--skip-worker` でワーカーのビルドを省く | `npm run app:build -- --bundles nsis` など |
| `scripts/tauri.mjs` | Tauri の CLI を、この構成で起動する（下の説明） | `npm run app:dev`（`tauri dev`） |

スクリプトは Node（22）だけで動き、新しい npm の依存はありません。テストは `node --test "scripts/**/*.test.mjs"`（CI の ui のジョブでも実行）。

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
3. LibRaw の場所を指定する: 環境変数 `VCPKG_ROOT`（vcpkg のフォルダ）を設定すれば、スクリプトが `installed\x64-windows` の `include`・`lib`・`bin` を使います。vcpkg 以外で入手した場合は `LIBRAW_INCLUDE_DIR`・`LIBRAW_LIB_DIR`（必要なら `LIBRAW_LIB_NAME`・`LIBRAW_DLL_DIR`）を設定します（`crates/genzo-raw/build.rs` の doc、README）。`LIBRAW_LIB_NAME` を指定しなければ、`.lib` のファイル名から選びます（`raw_r.lib` → `libraw_r.lib` → `libraw.lib` → `raw.lib` の順。vcpkg はスレッドセーフな版を `lib\raw_r.lib`、そうでない版を `lib\manual-link\raw.lib` に置く）。
4. 作る（PowerShell、`ui\` で）:

   ```powershell
   $env:VCPKG_ROOT = "C:\vcpkg"
   npm run app:build -- --bundles nsis
   ```

5. 成果物: `target\release\bundle\nsis\GenzoParis_0.1.0_x64-setup.exe`。同梱した DLL は `crates\genzo-app\binaries\bundled-libs.txt` で確かめられます。

スクリプトは、ワーカー（`genzo-worker.exe`）の PE のヘッダから読み込む DLL をたどり、LibRaw のフォルダ（vcpkg の `bin`）にある DLL だけを集めます（`scripts/lib/pe.mjs`。dumpbin は使わない）。OS の DLL と Visual C++ のランタイムは集めず、一覧に表示します。

**Visual C++ のランタイム**: 本体 `genzo-paris.exe` は、Visual C++ のランタイムを静的にリンクします（Tauri の CLI が `tauri build` のときに環境変数 `STATIC_VCRUNTIME` を設定し、tauri-build がリンクの設定を変える。@tauri-apps/cli の CHANGELOG と tauri-build 2.6.3 の `static_vcruntime.rs`）。一方、ワーカーと vcpkg の LibRaw の DLL は、ランタイムの DLL（`VCRUNTIME140.dll`・`MSVCP140.dll` など）を使います。**利用者の PC に「Microsoft Visual C++ 再頒布可能パッケージ（x64）」が入っていないと、ワーカーが起動せず RAW を開けません**（多くの PC には他のアプリと一緒に入っていますが、入っているかは PC によります）。対応は人が決めてください（8 章）:
(a) 利用者に再頒布可能パッケージを入れてもらう（今の方式）、(b) ランタイムの DLL をインストール先に一緒に置く、(c) インストーラーのフック（`bundle.windows.nsis.installerHooks`）で再頒布可能パッケージを入れる（Tauri の資料に例がある）。

### 2.3 macOS

1. Xcode の Command Line Tools（`xcode-select --install`。`otool`・`install_name_tool`・`codesign` を使う）と Homebrew を入れる。
2. LibRaw を入れる: `brew install libraw pkgconf`（Homebrew の libraw は 2026-10-12 の時点で 0.22.2。依存は jpeg-turbo・little-cms2・libomp。formula で確認）。
3. 作る（`ui/` で）:

   ```sh
   npm run app:build -- --bundles app,dmg
   ```

4. 成果物: `target/release/bundle/dmg/GenzoParis_0.1.0_aarch64.dmg` と `target/release/bundle/macos/GenzoParis.app`。

スクリプトは、`otool -l` でワーカーの依存をたどって Homebrew の dylib を `binaries/libs/` に写し、参照を書き換え（ワーカーは `@executable_path/../Frameworks/<名前>`、dylib どうしは `@loader_path/<名前>`、dylib の識別名は `@rpath/<名前>`）、`codesign --force --sign -` で署名し直します（書き換えると署名が壊れ、Apple Silicon では署名の壊れた実行ファイルは起動しないため）。書き換えた後に Homebrew の場所への参照が残っていればエラーにします。書き換えの余白のため、ワーカーはリンカーの `-headerpad_max_install_names` 付きでビルドします。Tauri は `.app` に入れた後、`signingIdentity: "-"` で中身を内側から順にアドホック署名し直します。

**動く macOS の版の注意**: Homebrew の dylib は、ビルドした macOS（GitHub Actions なら macos-latest のランナーの版）向けにできています。`tauri.conf.json` の `minimumSystemVersion` は 12.0 ですが、dylib の最小の版（`otool -l` の `LC_BUILD_VERSION` の `minos`）がそれより新しいと、古い macOS ではワーカーが起動せず RAW を開けません。スクリプトはこの場合に警告を出します。対応は人が決めてください: 配る相手の macOS の版に合わせて `minimumSystemVersion` を上げる、または LibRaw とその依存を `MACOSX_DEPLOYMENT_TARGET=12.0` でソースからビルドする（スクリプトは未対応）。

Intel の Mac 向け（x86_64）は作りません（要るかは人の判断。作る場合は Intel の Mac か、x86_64 の Homebrew の LibRaw が要る）。

### 2.4 Linux（確認用）

`ui/` で `npm run app:build -- --bundles deb`。LibRaw は同梱せず、.deb の依存にも書きません（Ubuntu 24.04 なら `sudo apt install libraw23t64` が要る。パッケージの名前は版によって違う）。Linux は配布の対象ではなく、スクリプトと設定の確認のためだけに使います。ビルドには GTK・WebKitGTK の開発用のパッケージが要ります。

### 2.5 GitHub Actions（`.github/workflows/release.yml`）

手動（Actions の画面の「Run workflow」）か、`v` で始まるタグ（`v0.1.0` など）の push で、Windows（windows-latest）と macOS（macos-latest。Apple Silicon）の配布物を作ります。手順は 2.2・2.3 と同じで、LibRaw は vcpkg（ランナーにある `VCPKG_INSTALLATION_ROOT`）と Homebrew で入れます。

- 成果物は Actions の artifact（`genzoparis-windows-x64`・`genzoparis-macos-arm64`。30 日で消える。仮置き）に置きます。GitHub の Releases には出しません。**artifact は、リポジトリを読める人なら誰でも取得できます。リポジトリの公開の範囲を確かめてから実行してください。**
- 最後に、配布物の中のワーカーが同梱の LibRaw で起動できるかを確かめます（Windows はインストーラーを `/S` で無人インストールして `genzo-worker.exe --version`、macOS は `DYLD_PRINT_LIBRARIES` で `.app` の中の dylib を読み込んだことを確認）。
- **一度も実行していません**（7 章）。最初の実行の結果を見て、ワークフローとこの資料を直してください。

### 2.6 配る前の確認

1. 入れて起動し、ARW を 1 枚取り込んでサムネイルと現像の画面が出ることを確かめる（[M5 の受け入れの手順](m5_acceptance.md) の 0 章）。設定の画面のカタログの欄で、「RAW のデコーダ」が「なし」でないこと（LibRaw の識別子が出る）、「ワーカーのエラー」が出ていないことも確かめる（ワーカーや LibRaw のライブラリが見つからない場合は、ここに出る。画面は作成中のため要確認）。
2. 配布物の SHA-256 を記録し、配る相手に伝える（受け取った人が、改ざんされていないかを確かめられるように）:
   - Windows（PowerShell）: `Get-FileHash .\GenzoParis_0.1.0_x64-setup.exe -Algorithm SHA256`
   - macOS: `shasum -a 256 GenzoParis_0.1.0_aarch64.dmg`
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

FFmpeg は同梱しません（決定 No.62）。動画は FFmpeg がなくても登録できますが、サムネイルとメタデータ（長さ・fps など）が取れません。アプリは ffmpeg・ffprobe を「設定で指定したフォルダ → `PATH` → OS ごとの既定の場所」の順に探します（`crates/genzo-app/src/tools.rs`）。入れた後はアプリを起動し直してください。設定の画面の「一般」で「見つかった」と出れば使えます。

| OS | 入れ方 | アプリが探す既定の場所 |
|---|---|---|
| Windows | `winget install --id Gyan.FFmpeg -e`（パッケージの名前は `winget search ffmpeg` で確かめる。要確認） | `%LOCALAPPDATA%\Microsoft\WinGet\Links`（winget がリンクを置く場所）・`%ProgramFiles%\ffmpeg\bin`・`C:\ffmpeg\bin` |
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

## 6. この環境（Linux）での確認の結果

2026-10-12、Ubuntu 24.04 のコンテナで、LibRaw（apt の libraw-dev 0.21.2）を入れて確かめました。

- 結果はこの節の末尾に記録します（実行のログから写す）。

## 7. 未確認の事項

| No | 事項 | 確かめ方 |
|---|---|---|
| 1 | release.yml を GitHub Actions で一度も実行していない | 手動で実行し、両 OS の成果物と最後の確認の手順が通るかを見る |
| 2 | Windows: vcpkg の libraw の `.lib`・DLL の名前（`raw_r`）と、スレッドセーフな版であること（vcpkg の port のファイルで確認しただけ） | 2.2 の手順で作り、`bundled-libs.txt` と ARW の展開を確かめる |
| 3 | Windows: 集めた DLL だけで、別の PC（vcpkg のない PC）でワーカーが起動するか | 別の PC に入れて ARW を開く。release.yml の確認は vcpkg のあるランナーで、DLL は PATH にないが、OS の DLL の一覧の漏れは見つけられない |
| 4 | Windows: Visual C++ のランタイムがない PC での動き（2.2） | ランタイムのない PC か、新しく入れた Windows で確かめる |
| 5 | Windows: NSIS のインストーラーの `/S`（無人インストール）、インストール先、アンインストーラーの「アプリのデータを削除する」の動き | 実機で確かめる |
| 6 | Windows: SmartScreen・スマート アプリ コントロールでの動き（3.1） | 実機で確かめる |
| 7 | macOS: `otool -l` の出力の読み取り、dylib の書き換え・署名（スクリプトは出力の例でテストしただけ） | 2.3 の手順で作り、`otool -L` と `DYLD_PRINT_LIBRARIES=1 …/genzo-worker --version` で、`.app` の中の dylib を読むことを確かめる |
| 8 | macOS: Tauri のアドホック署名と `hardenedRuntime: false` で、`codesign --verify --deep --strict` が通るか、Gatekeeper の手順（3.2）のとおりに開けるか | 実機で確かめる |
| 9 | macOS: Homebrew の dylib の最小の版（2.3）と、配る相手の macOS の版 | `otool -l` の `minos` を見る |
| 10 | macOS: GitHub Actions のランナーでの .dmg の作成（Finder の操作を使う） | release.yml を実行する |
| 11 | Tauri の出力のファイル名（`GenzoParis_<版>_x64-setup.exe`・`_aarch64.dmg`） | 実機・Actions で作る |
| 12 | 画面（WebView）のデータの場所（5 章の表） | 実機で確かめる |
| 13 | winget の FFmpeg のパッケージの名前（4 章） | `winget search ffmpeg` |
| 14 | ピクチャのフォルダが同期のフォルダのときの、カタログの扱い（5 章） | 実機で確かめる。必要ならアプリで警告するかを決める |

## 8. 人・専門家の確認待ちの事項（法的な事項を含む）

**次の事項は AI では決められません。社外に配る前（社内でも、必要と判断した場合は配る前）に、人（必要なら専門家）が確認し、結果を [決定事項ログ](decision_log.md) と [第三者の台帳](third_party.md) に書いてください。**

1. **LibRaw の同梱と LGPL-2.1**（決定 No.32、台帳の 3.1 節）: 配布物に LibRaw の共有ライブラリを入れるときに、LGPL の条件を満たす方法（ライセンスの文書を同梱するか、対応するソースの提供の方法、利用者がライブラリを差し替えられること）。今の配布物には **ライセンスの文書を入れていません**。macOS では、`.app` の中の dylib を差し替えると署名が壊れるため、差し替えた人がアドホック署名し直す必要がある点も含めて確認してください。
2. **LibRaw の依存ライブラリのライセンス**: Windows（vcpkg）は jasper・lcms・zlib・libjpeg-turbo、macOS（Homebrew）は libomp・little-cms2・jpeg-turbo（実際に同梱したものは `bundled-libs.txt`）。それぞれのライセンスと、表記の要否を確認し、台帳に追加してください。
3. **Visual C++ のランタイムの扱い**（2.2）: 利用者に入れてもらうか、DLL を同梱するか（同梱する場合は Microsoft の再頒布の条件の確認）。
4. **FFmpeg**（決定 No.62、台帳の 3.4 節）: 各自が入れる FFmpeg のビルドの構成と、H.264 / H.265 の特許の扱い。
5. **署名・公証の要否**（SYS-08）: 社内の数名への配布なら不要としていますが、配る範囲が広がる場合や、会社の規則で署名が求められる場合は、Windows のコード署名の証明書（有償の場合あり）と、Apple Developer Program（有償）が要ります。費用・手続きは確認していません。
6. **成果物を置く場所の公開の範囲**（2.5・2.6）: GitHub Actions の artifact、社内の共有フォルダなどを、配る相手だけが見られるか。
