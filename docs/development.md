# 開発の手順

GenzoParis を手元でビルド・テスト・起動するための手順です。機能の概要は [README](../README.md)、CLI（`genzo`）の使い方は [cli.md](cli.md)、配布物の作り方は [packaging.md](packaging.md) にあります。

Rust の Cargo workspace です（crate の構成は [docs/04_architecture.md](04_architecture.md) の 1.4 節）。Rust の版は `rust-toolchain.toml` で 1.97.0 に固定しています（手元と CI で clippy の警告をそろえるため。`Cargo.toml` の最小の版は 1.88）。

## ビルドとテスト

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

オンラインの地図（LIB-21）の HTTPS（`genzo-geo`）は ureq の rustls（暗号の実装は `ring`）を使い、証明書の検証は OS に任せます（Windows は CryptoAPI、macOS は Security.framework、Linux は OS の証明書の保管場所）。`ring` は C とアセンブリを含むので、ビルドに C のコンパイラが要ります（SQLite・lcms2・BLAKE3 と同じく、ビルドの中で `cc` の crate がコンパイルします）。OpenSSL などのシステムの TLS のライブラリは要りません（[docs/third_party.md](third_party.md) の 1.1 節）。

Linux（Ubuntu）から、Windows 向けのコード（`cfg(windows)`）の型と警告を確かめられます（コンパイルの検査だけで、リンク・実行はしません）。C のライブラリ（SQLite・lcms2 など）は mingw-w64 で Windows 向けにビルドします。

```sh
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo clippy --workspace --all-targets --keep-going --target x86_64-pc-windows-gnu -- -D warnings
```

CI の Windows のジョブは MSVC 向け（`x86_64-pc-windows-msvc`）です。MSVC 向けは Linux では C のライブラリをビルドできないので、gnu 向けで代わりに確かめます（Rust のコードは `target_env` で分けていないので、Windows 用のコードは同じように検査されます）。MSVC でのリンク・実行は、CI か Windows の実機で確かめてください。

## LibRaw（機能フラグ `libraw`）

RAW の展開には LibRaw（0.21 以降。スレッドセーフ版の `libraw_r`）を使います。機能フラグ `libraw` は既定では無効で、LibRaw がなくてもビルドとテストができます。LibRaw の関数を呼ぶのはワーカープロセスだけで、本体からは呼びません（[docs/04_architecture.md](04_architecture.md) の 1.2 節）。

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

実行するときは、LibRaw の DLL（と、その依存の DLL）を、ワーカーとして動く実行ファイル（開発用の CLI では `genzo.exe`、配布するアプリでは `genzo-worker.exe`）と同じフォルダか `PATH` の通ったフォルダに置いてください。vcpkg が作る `.lib` のファイル名と、スレッドセーフな版かどうかは未確認です（[docs/third_party.md](third_party.md) の 3.1 節）。CI の Windows は LibRaw なしの構成だけを確かめます。

`genzo-cli/libraw` を指定すると、`genzo-api`・`genzo-worker`・`genzo-raw` の `libraw` も有効になります。LibRaw は pkg-config で `libraw_r` を探し、なければ `libraw` を使います（`libraw` はスレッドセーフでないため、LibRaw の使用をプロセスの中で 1 つずつに制限します）。

開発用の CLI の `genzo` は、自分自身を `genzo __worker` で起動してワーカーを兼ねます。そのため、`libraw` を有効にした `genzo` は LibRaw の共有ライブラリに依存します（Linux の `ldd` で確認）。LibRaw の関数を呼ぶのは、ワーカーとして動くプロセスだけです。**配布するアプリでは、ワーカーを別の実行ファイル（`genzo-worker`）にし、本体の実行ファイルは LibRaw に依存させません**（[決定事項ログ](decision_log.md) No.32。`genzo-api` の `WorkerLaunch::Executable`。既定の設定）。Cargo の機能フラグは同じビルドの中で共通になるので、本体は `libraw` なしでビルドし、ワーカーは別に `cargo build --release -p genzo-worker --features libraw` でビルドします（Linux の `ldd` で、`genzo-worker` だけが `libraw_r.so.23` に依存することを確認）。LibRaw のライセンスは LGPL-2.1 を選ぶ方針ですが、**法的な事項のため、公開・配布の前に人（必要なら専門家）の確認が必要**です（[docs/third_party.md](third_party.md) の 3.1 節、[docs/implementation_status.md](implementation_status.md) の「人の判断・確認が必要な事項」の No.4）。

## 実機のサンプルの回帰テスト（任意）

手持ちの ARW などで展開・既定の切り抜き・飽和・壊れたファイルの扱い・既定の現像の統計を確かめます（`crates/genzo-pipeline/tests/private_samples.rs`。LibRaw が必要）。フォルダは **リポジトリの外** に置いてください。サンプルとそこから作った画像はリポジトリに入れません（`.gitignore` に `*.ARW`・`*.arw` があります。期待値の `crates/genzo-pipeline/tests/private_samples.json` には、ハッシュと写真の内容を明かさない数値だけを置きます。撮影日時・GPS・撮影場所も書きません）。サンプルはハッシュ（SHA-256）で期待値と対応づけ、期待値のないファイルは共通の性質（展開の成否・既定の切り抜きの範囲・端の帯がないことなど）だけを確かめます。壊したファイルは OS の一時フォルダに作り、終わったら消します。
`GENZO_PRIVATE_SAMPLES=<フォルダ> cargo test -p genzo-pipeline --features libraw --release --test private_samples -- --nocapture`（未指定なら何もせずに成功。`GENZO_REQUIRE_PRIVATE_SAMPLES=1` で実行できない場合を失敗に、`GENZO_UPDATE_PRIVATE_EXPECT=1` で期待値を書き直し）。
Windows（PowerShell）では `$env:GENZO_PRIVATE_SAMPLES="<フォルダ>"` を先に設定してください。期待値を書き直したときは、git の差分を確かめ、数値を別の人が確認してからコミットしてください。CI では実行しません（手で実行するか自前のランナーに入れるかは未決。[docs/implementation_status.md](implementation_status.md) の表 No.58）。

## デスクトップアプリ（`genzo-app`）

Tauri 2（`crates/genzo-app`）と React / TypeScript の画面（`ui/`）です（[決定事項ログ](decision_log.md) No.60〜66）。Node 22 と、Tauri の前提のパッケージ（Linux は GTK・WebKitGTK の開発用のパッケージ。Tauri の資料の「Prerequisites」）が要ります。

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
- オンラインの地図（LIB-21 の第 2 段階。位置を付けるダイアログの地図と地名の検索。LIB-18 の表示モード「地図」（Shift + M。写真を撮影地に並べ、地図から探す。初めて開くと写真を撮った場所の付近のタイルを要求する。設計は [docs/04_architecture.md](04_architecture.md) の 3.17 節）も同じ地図）は **既定で無効** です。設定の「一般」か位置を付けるダイアログで有効にしたときだけ通信します。地図のタイルは OpenStreetMap Foundation のサーバー（`tile.openstreetmap.org`）から取ります。**地名の検索は、検索サービスを設定したときだけ使えます**（既定では使わない。設定の「一般」の「OSM の公開の地名の検索（Nominatim）を使う」を ON にすると OSM Foundation の公開の Nominatim（`nominatim.openstreetmap.org`）に、「詳細（地図の提供元・プロキシ）」で検索サービスの URL を指定するとそちらに送ります。どちらもなくても、座標の貼り付けと地図のクリックでは位置を付けられます）。守る利用ポリシーは、タイル（https://operations.osmfoundation.org/policies/tiles/ ）と Nominatim（https://operations.osmfoundation.org/policies/nominatim/ ）です: 地名の検索は 1 秒に 1 回まで（ポリシーの上限はアプリの全利用者の合計。アプリが守れるのは 1 つのプロセスの中の間隔だけ）・入力の途中で検索しない（オートコンプリートの禁止）・結果をキャッシュする、タイルは HTTP のキャッシュの指示に従い、先読み・一括取得をしない。開発で試すときもこれを守ってください。設定の「一般」の「詳細（地図の提供元・プロキシ）」で、タイルと地名の検索の提供元を切り替えられます（地名の検索の規則（1 秒に 1 回など）は、どの提供元にも当てます。https で、タイルは PNG のものだけ。URL の拡張子が `.jpg`・`.webp` などなら保存のときに断ります。その提供元の帰属の表示は「地図の帰属の表示」に入れると、地図の右下に OSM の帰属の表示と並べて出します）。通信のプロキシは、環境変数（`HTTPS_PROXY`・`ALL_PROXY`・`HTTP_PROXY` と `NO_PROXY`）か、Windows の手動のプロキシの設定（プロトコルごとの設定なら `https=` の欄）だけを読みます。自動構成スクリプト（PAC）・設定の自動検出（WPAD）と、Windows 以外のシステムの設定は読みません（それらだけが設定されているときは直接つなぐ）。プロキシが要るときは環境変数 `HTTPS_PROXY` を設定してください。SOCKS のプロキシは使えません。テスト・CI からは本物のサーバーに問い合わせません。公開の Nominatim を既定で使わないのは利用者の決定です（[docs/decision_log.md](decision_log.md) の No.193）。ポリシーの順守の法的な最終の確認は人に残ります（[docs/implementation_status.md](implementation_status.md) の人の判断の表 No.187〜189。設計は [docs/04_architecture.md](04_architecture.md) の 3.16 節）。
- 配布物の作り方（`npm run app:worker`・`npm run app:build`、GitHub Actions の `release.yml`）と、FFmpeg・署名・ライセンスの注意は [docs/packaging.md](packaging.md) を見てください。**`release.yml` は手動でだけ動き、公開のリポジトリでは、成果物を誰でも取得できる状態にしてよいかの確認の印がないと止まります**（2.5 節）。
- E2E（Linux の開発環境用）: `e2e/run.sh <アプリの実行ファイル> e2e/scenario.mjs [出力のフォルダ]`。Xvfb・WebKitWebDriver・`tauri-driver` が要ります（`e2e/run.sh` の冒頭）。写真のフォルダは `cargo run -p genzo-testkit --example e2e_fixtures -- <フォルダ>` で作り、動画を 1 本 FFmpeg で足して（合成の映像。例: `ffmpeg -f lavfi -i testsrc=duration=2:size=640x360:rate=30 -pix_fmt yuv420p "<フォルダ>/動画 01.mp4"`）、環境変数 `E2E_PHOTOS` で渡します。写真・動画の数は、シナリオが取り込んだ後にカタログから数えます（動画がなくても通りますが、そのときは動画の扱い（GPS を削除する設定では書き出さないこと）を確かめません）。アプリは `npm --prefix ui run build` の後に `cargo build -p genzo-app --features custom-protocol` で（実行ファイルは `target/debug/genzo-paris`）、ワーカーは同じフォルダに `cargo build -p genzo-worker --features libraw` でビルドします。出力のフォルダ（画面の保存先）の既定は `e2e/out` です（git の対象外）。Windows・macOS の WebView の確認の代わりにはなりません。

## CLI（`genzo`）

検証用の CLI の使い方（コマンドの一覧・終了コード・注意）は [cli.md](cli.md) にあります。

## 依存ライブラリのライセンスと脆弱性の確認

```sh
cargo install cargo-deny --locked
cargo deny check
```

設定は [deny.toml](../deny.toml) です。cargo-deny で確認できない C / C++ のライブラリ・同梱データ・外部の実行ファイル（FFmpeg）は、[docs/third_party.md](third_party.md) の台帳で管理します。

## 配布物に入れる第三者のライセンスの文書（DIST-05）

`Cargo.lock`・`ui/package-lock.json`・`about.toml` を変えたら、配布物に入れるライセンスの文書（`crates/genzo-app/licenses/third-party/rust.txt`・`npm.txt`）を作り直してコミットします（作り直していないと、CI と配布物のビルドが止まります）。

```sh
cargo install cargo-about --version 0.9.2 --locked --features cli   # 初回だけ
npm --prefix ui ci                                                  # npm の文書に要る
node scripts/third-party.mjs                                        # 作り直す（--check で確かめるだけ）
```

設定は [about.toml](../about.toml)、方式と人の確認待ちの点は [docs/third_party.md](third_party.md) の 1.4 節です。配布物では `licenses/` に入り、アプリの設定の「このアプリについて」でも読めます。

## CI

GitHub Actions（[.github/workflows/ci.yml](../.github/workflows/ci.yml)）で、push と pull request のたびに次を実行します。

- Linux: フォーマットの確認と cargo-deny。画面（`ui/`）の型の検査・テスト・ビルドと、配布のスクリプト（`scripts/`）のテスト。第三者のライセンスの文書が最新か（cargo-about で作り直して比べる）
- Windows と macOS（Apple Silicon）: clippy（警告をエラーとして扱う）とテスト（デスクトップアプリの `genzo-app` を含む）。macOS では LibRaw を入れて、機能フラグ `libraw` を有効にした確認も行う

外部のアクションはコミットの SHA で固定しています（[docs/packaging.md](packaging.md) の 2.5 節）。配布物は別のワークフロー（`release.yml`。手動で実行）で作ります。

## スクリーンショット

README の画面のスクリーンショット（`docs/images/*.jpg`）は、画面（`ui/`）を Tauri なしでモックのデータ（`ui/screenshot/main.tsx`。架空のカタログ 30 枚）で描き、Playwright の Chromium で撮ったものです。写真は `scripts/screenshots/gen_photos.py` が手続き的に作る合成の画像で、実在の写真・撮影日時・位置情報は使いません（[m5_acceptance.md](m5_acceptance.md) の冒頭の注意と同じく、個人の写真をリポジトリに入れないため）。画面を変えたときに撮り直してください。

Python 3（numpy・Pillow）と Playwright（このリポジトリの依存には入れていない）が要ります。リポジトリのルートで実行します。

```sh
npm --prefix ui ci
python3 scripts/screenshots/gen_photos.py /tmp/genzo-photos 30
NODE_PATH="$(npm root -g)" node scripts/screenshots/shoot.cjs /tmp/genzo-photos   # docs/images に書く
```

- 撮る画面と操作は `scripts/screenshots/shoot.cjs` に、モックの応答は `ui/screenshot/main.tsx` にあります。モックが応えなかったコマンドは、最後に一覧で出ます（画面が新しいコマンドを使うようになったら、応答を足してください）。
- 現像のプレビューは画像をそのまま返すだけで、現像の処理はしません（スライダーの値と画像の見た目は対応しません）。アプリの動作・画質の確認の代わりにはなりません。
- `ui/screenshot/` はアプリのビルド（`ui/index.html`）と型の検査（`tsconfig.json` の `include`）の対象外です。
