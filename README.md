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
| [docs/catalog_schema.md](docs/catalog_schema.md) | カタログ（SQLite）のスキーマと、外部のツールから SQL で読む方法（公開ビュー。ORG-07） |
| [docs/composite_design.md](docs/composite_design.md) | 比較明・比較暗合成（DEV-34）の設計（設計案。未実装） |
| [docs/retouch_design.md](docs/retouch_design.md) | スポット修正（DEV-21。修復・コピースタンプ）の設計（O 座標・段階 A1 の出力に当てる・修復の計算・コピー元の自動の選択・確かめたことと人の確認が必要なこと） |

## 開発

Rust の Cargo workspace です（crate の構成は [docs/04_architecture.md](docs/04_architecture.md) の 1.4 節）。Rust の版は `rust-toolchain.toml` で 1.97.0 に固定しています（手元と CI で clippy の警告をそろえるため。`Cargo.toml` の最小の版は 1.88）。

### ビルドとテスト

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

オンラインの地図（LIB-21）の HTTPS（`genzo-geo`）は ureq の rustls（暗号の実装は `ring`）を使い、証明書の検証は OS に任せます（Windows は CryptoAPI、macOS は Security.framework、Linux は OS の証明書の保管場所）。`ring` は C とアセンブリを含むので、ビルドに C のコンパイラが要ります（SQLite・lcms2・BLAKE3 と同じく、ビルドの中で `cc` の crate がコンパイルします）。OpenSSL などのシステムの TLS のライブラリは要りません（[docs/third_party.md](docs/third_party.md) の 1.1 節）。

Linux（Ubuntu）から、Windows 向けのコード（`cfg(windows)`）の型と警告を確かめられます（コンパイルの検査だけで、リンク・実行はしません）。C のライブラリ（SQLite・lcms2 など）は mingw-w64 で Windows 向けにビルドします。

```sh
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo clippy --workspace --all-targets --keep-going --target x86_64-pc-windows-gnu -- -D warnings
```

CI の Windows のジョブは MSVC 向け（`x86_64-pc-windows-msvc`）です。MSVC 向けは Linux では C のライブラリをビルドできないので、gnu 向けで代わりに確かめます（Rust のコードは `target_env` で分けていないので、Windows 用のコードは同じように検査されます）。MSVC でのリンク・実行は、CI か Windows の実機で確かめてください。

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
- オンラインの地図（LIB-21 の第 2 段階。位置を付けるダイアログの地図と地名の検索）は **既定で無効** です。設定の「一般」か位置を付けるダイアログで有効にしたときだけ通信します。地図のタイルは OpenStreetMap Foundation のサーバー（`tile.openstreetmap.org`）から取ります。**地名の検索は、検索サービスを設定したときだけ使えます**（既定では使わない。設定の「一般」の「OSM の公開の地名の検索（Nominatim）を使う」を ON にすると OSM Foundation の公開の Nominatim（`nominatim.openstreetmap.org`）に、「詳細（地図の提供元・プロキシ）」で検索サービスの URL を指定するとそちらに送ります。どちらもなくても、座標の貼り付けと地図のクリックでは位置を付けられます）。守る利用ポリシーは、タイル（https://operations.osmfoundation.org/policies/tiles/ ）と Nominatim（https://operations.osmfoundation.org/policies/nominatim/ ）です: 地名の検索は 1 秒に 1 回まで（ポリシーの上限はアプリの全利用者の合計。アプリが守れるのは 1 つのプロセスの中の間隔だけ）・入力の途中で検索しない（オートコンプリートの禁止）・結果をキャッシュする、タイルは HTTP のキャッシュの指示に従い、先読み・一括取得をしない。開発で試すときもこれを守ってください。設定の「一般」の「詳細（地図の提供元・プロキシ）」で、タイルと地名の検索の提供元を切り替えられます（地名の検索の規則（1 秒に 1 回など）は、どの提供元にも当てます。https で、タイルは PNG のものだけ。URL の拡張子が `.jpg`・`.webp` などなら保存のときに断ります。その提供元の帰属の表示は「地図の帰属の表示」に入れると、地図の右下に OSM の帰属の表示と並べて出します）。通信のプロキシは、環境変数（`HTTPS_PROXY`・`ALL_PROXY`・`HTTP_PROXY` と `NO_PROXY`）か、Windows の手動のプロキシの設定（プロトコルごとの設定なら `https=` の欄）だけを読みます。自動構成スクリプト（PAC）・設定の自動検出（WPAD）と、Windows 以外のシステムの設定は読みません（それらだけが設定されているときは直接つなぐ）。プロキシが要るときは環境変数 `HTTPS_PROXY` を設定してください。SOCKS のプロキシは使えません。テスト・CI からは本物のサーバーに問い合わせません。公開の Nominatim を既定で使わないのは利用者の決定です（[docs/decision_log.md](docs/decision_log.md) の No.193）。ポリシーの順守の法的な最終の確認は人に残ります（[docs/implementation_status.md](docs/implementation_status.md) の人の判断の表 No.187〜189。設計は [docs/04_architecture.md](docs/04_architecture.md) の 3.16 節）。
- 配布物の作り方（`npm run app:worker`・`npm run app:build`、GitHub Actions の `release.yml`）と、FFmpeg・署名・ライセンスの注意は [docs/packaging.md](docs/packaging.md) を見てください。**`release.yml` は手動でだけ動き、公開のリポジトリでは、成果物を誰でも取得できる状態にしてよいかの確認の印がないと止まります**（2.5 節）。
- E2E（Linux の開発環境用）: `e2e/run.sh <アプリの実行ファイル> e2e/scenario.mjs [出力のフォルダ]`。Xvfb・WebKitWebDriver・`tauri-driver` が要ります（`e2e/run.sh` の冒頭）。写真のフォルダは `cargo run -p genzo-testkit --example e2e_fixtures -- <フォルダ>` で作り、動画を 1 本 FFmpeg で足して（合成の映像。例: `ffmpeg -f lavfi -i testsrc=duration=2:size=640x360:rate=30 -pix_fmt yuv420p "<フォルダ>/動画 01.mp4"`）、環境変数 `E2E_PHOTOS` で渡します。写真・動画の数は、シナリオが取り込んだ後にカタログから数えます（動画がなくても通りますが、そのときは動画の扱い（GPS を削除する設定では書き出さないこと）を確かめません）。アプリは `npm --prefix ui run build` の後に `cargo build -p genzo-app --features custom-protocol` で（実行ファイルは `target/debug/genzo-paris`）、ワーカーは同じフォルダに `cargo build -p genzo-worker --features libraw` でビルドします。出力のフォルダ（画面の保存先）の既定は `e2e/out` です（git の対象外）。Windows・macOS の WebView の確認の代わりにはなりません。

### CLI（`genzo`）

検証用の CLI です（ORG-05。UI なしで登録・検索・現像・書き出し・計測を行う）。結果は表か `--json`、進捗と警告は標準エラー、終了コードは 0（成功）・1（エラー。`trash` を `--yes` なしで中止した場合を含む）・2（使い方の誤り）・3（結果は出したが一部が失敗した・問題が見つかった: 書き出し・削除・サムネイルの作り直しの一部の失敗、取り込みで登録できなかったファイル、写真の移動・名前の変更・再リンクで飛ばした・失敗した写真、`catalog check` で見つかった問題、`missing check` で見つかった見つからない写真、`bench` での悪化、`profile import`・`preset import` で取り込めなかったファイル）・130（Ctrl+C で中断。1 回目は実行中の処理を取り消してカタログを閉じてから、2 回目はすぐに終了。Windows でも 130）です（[決定事項ログ](docs/decision_log.md) No.52）。スクリプトからは、3 のときに結果の JSON を確かめてください。詳しくは `genzo --help` と `crates/genzo-cli/src/lib.rs` の doc を見てください。

```sh
export GENZO_CATALOG=/path/to/catalog.db      # または --catalog
cargo run -p genzo-cli -- catalog init
cargo run -p genzo-cli -- import ~/Pictures/2024-05-01
cargo run -p genzo-cli -- search --min-rating 3 --json
echo '{"exposure_ev": 0.5}' | cargo run -p genzo-cli -- develop set 12 --json -
cargo run -p genzo-cli -- export 12 --out ./out --long-edge 2048 --remove-gps
cargo run -p genzo-cli --features libraw -- render DSC00001.ARW --out out.tif   # カタログなしで 1 枚（RAW は LibRaw が必要）
cargo run --release -p genzo-cli -- bench preview --synthetic 7008x4672 --out bench-results
cargo run -p genzo-cli -- catalog schema            # カタログの構造（--sql で作る SQL、--json で表と列。ORG-07）
cargo run -p genzo-cli -- log info                  # ログ・クラッシュレポートの場所と中身（SYS-07）
cargo run -p genzo-cli -- log clear                 # ログ・クラッシュレポートを消す
cargo run -p genzo-cli -- metadata 12 13 --set title=清水寺 --set creator="山田 太郎" --clear city   # 説明のメタデータを一括で変える（LIB-15）
cargo run -p genzo-cli -- metadata 12 13            # 項目ごとの値（写真によって異なれば「複数の値」）
cargo run -p genzo-cli -- develop before 12 --out before.png --after after.png   # ビフォー / アフターのプレビュー（DEV-31）
cargo run -p genzo-cli -- develop auto 12 --dry-run   # 自動補正の値を見る（--dry-run を外すと保存。DEV-32）
cargo run -p genzo-cli -- develop compare 12 13 --cx 0.5 --cy 0.5 --width 800 --height 600 --out-dir ./比較   # 比較表示の同期ズームと同じ範囲を 2 枚で描く（LIB-03）
cargo run -p genzo-cli -- develop set 12 --lens-profile on --remove-ca on --distortion 10   # レンズ補正（埋め込みの補正の情報と手動の補正。DEV-14）
cargo run -p genzo-cli -- develop set 12 --upright vertical   # 自動の遠近補正（Upright 相当。水平・垂直・完全。直線を調べて角度と垂直・水平を入れる。DEV-16）
cargo run -p genzo-cli -- develop set 12 --vertical -20 --transform-scale 110   # 変形の手動の値（垂直・水平・縦横比・拡大縮小・ずらし。処理バージョン 2 から。DEV-16）
cargo run -p genzo-cli -- develop upgrade 12        # 処理バージョンを最新（2）に更新する（レンズ補正は処理バージョン 2 から。DATA-09）
cargo run -p genzo-cli -- develop upgrade 12 13 14  # まとめて更新する（すでに最新の写真・動画はそのまま）
cargo run -p genzo-cli -- develop copy 12 --to 13 14 --upgrade-process-version   # 設定をコピーし、コピー先が古い処理バージョンならコピー元の版に上げる（省略時は版を変えない。DEV-30）
cargo run -p genzo-cli -- develop set 12 --highlight-recovery reconstruct --nr-luminance 30 --nr-color 25   # ハイライト復元・ノイズ軽減（RAW だけ。処理バージョン 2 から。DEV-06・DEV-13）
cargo run -p genzo-cli -- develop set 12 --hsl-luminance blue=-40 --hsl-hue red=10 --grading-shadows 220,30 --grading-highlights 40,25   # カラーミキサー・カラーグレーディング（処理バージョン 2 から。DEV-09・DEV-10）
cargo run -p genzo-cli -- develop set 12 --black-white on --bw-mix blue=-50 --bw-mix orange=30   # 白黒と白黒ミキサー（処理バージョン 2 から。DEV-25）
cargo run -p genzo-cli -- develop set 12 --clarity 30 --dehaze 20 --sharpen-amount 50 --sharpen-masking 40   # 明瞭度・かすみの除去・シャープ（処理バージョン 2 から。DEV-11・DEV-12）
cargo run -p genzo-cli -- develop set 12 --vignette-amount -30 --vignette-highlights 50 --grain-amount 25   # 切り抜き後の周辺光量補正・粒子（処理バージョン 2 から。DEV-24）
cargo run -p genzo-cli -- develop copy 12 --to 13 14 --groups presence,effects   # 外観（テクスチャ・明瞭度・かすみの除去）と効果だけを写す
cargo run -p genzo-cli --features libraw -- develop source 12   # 元の画像の情報と、埋め込みのレンズ補正の情報・処理バージョン
cargo run -p genzo-cli -- profile import "$HOME/DCP/ILCE-7M4 標準.dcp" "$HOME/LUT/film.cube"   # カメラプロファイル（DCP）・ルック（3D LUT）を取り込む（DEV-17。元のファイルは変えない）
cargo run -p genzo-cli -- develop mask add 12 --linear 0.5,0,0.5,0.3 --exposure -0.7 --name 空   # 線形グラデーションのマスクで空を暗く（処理バージョン 2 から。DEV-18）
cargo run -p genzo-cli -- develop mask add-part 12 1 --luminance 50,100   # マスク 1 を明るい部分だけに絞る（輝度の範囲を交差。DEV-19）
cargo run -p genzo-cli -- develop mask overlay 12 1 --out mask.png   # マスクの重み（アプリの重ねて表示と同じ）を PNG に書く
cargo run -p genzo-cli --features libraw -- develop retouch add 12 --at 0.41,0.22 --size 0.008   # ゴミを修復（コピー元は自動で選ぶ。処理バージョン 2 から。DEV-21）
cargo run -p genzo-cli -- develop retouch add 12 --at 0.1,0.3 --at 0.6,0.28 --size 0.003 --source 0,0.01   # 電線を線で消す（コピー元のずれを指定）
cargo run -p genzo-cli -- profile list --for 12       # 取り込んだものの一覧（12 の機種用か）
cargo run -p genzo-cli -- develop set 12 --profile "ILCE-7M4 標準" --look film --look-amount 60   # プロファイル・ルックを選ぶ（処理バージョン 2 から）
cargo run -p genzo-cli --features libraw -- render DSC00001.ARW --out out.jpg --dcp camera.dcp --cube film.cube   # 取り込まずにファイルのプロファイル・ルックで 1 枚
cargo run -p genzo-cli -- keyword add 12 13 -k "場所>京都" -k 家族   # キーワードを付ける（なければ作る。LIB-09）
cargo run -p genzo-cli -- keyword list              # キーワードの一覧（階層のパス・直接付いている数・書き出すか）
cargo run -p genzo-cli -- search --keyword 場所      # 親のキーワードで絞り込む（下の階層を含む）
cargo run -p genzo-cli -- collection create "京都 2024" --add 12 13   # コレクションを作って写真を入れる（LIB-10）
cargo run -p genzo-cli -- collection create "よい写真" --rule rule.json   # スマートコレクション（LIB-11）
cargo run -p genzo-cli -- search --collection 1     # コレクションで絞り込む（スマートは条件、セットは中のいずれか）
cargo run -p genzo-cli -- stack auto --gap 2 --dry-run   # 撮影時刻の間隔 2 秒で自動スタックを作る見込み（LIB-12）
cargo run -p genzo-cli -- stack create 12 13 14 --top 13 # 写真をまとめて 1 つのスタックにする
cargo run -p genzo-cli -- search --expand-stacks    # 折りたたんだスタックもすべて展開して並べる
cargo run -p genzo-cli -- develop virtual-copy 12 --name 白黒   # 仮想コピーを作る（元の写真と同じスタックに入る。LIB-13）
cargo run -p genzo-cli -- develop make-master 21    # 仮想コピーを元の写真（マスター）にする
cargo run -p genzo-cli -- import /media/SD --keep-duplicates   # 内容が同じファイルが登録済みでも取り込む（既定は取り込まない。IMP-04）
cargo run -p genzo-cli -- import ~/Pictures/raw --separate-raw-jpeg   # RAW と同時記録の JPEG を別々の写真にする（既定は 1 枚。IMP-06）
cargo run -p genzo-cli -- import /media/SD --copy-to ~/Pictures --dry-run   # コピー先の名前を決めて表示するだけ（IMP-02）
cargo run -p genzo-cli -- import /media/SD --copy-to ~/Pictures --folders year-date --rename "{date}-{seq}"   # 撮影日別のフォルダへ名前を変えてコピーして登録する（IMP-02・IMP-03）
cargo run -p genzo-cli -- import /media/SD --copy-to ~/Pictures --keyword "場所>京都" --meta creator="山田 太郎"   # 新しく登録した写真にキーワード・作成者を付ける（IMP-05）
cargo run -p genzo-cli -- hash compute              # 内容のハッシュ（ファイル全体の BLAKE3）を計算して記録する（ORG-03）
cargo run -p genzo-cli -- hash status               # 計算済みの数・対象の数・内容が同じファイルの組の数
cargo run -p genzo-cli -- hash duplicates           # 内容が同じファイルの組
cargo run -p genzo-cli -- hash identify ~/moved/DSC00001.ARW   # 場所・名前によらず、同じ内容の登録済みの写真を探す
cargo run -p genzo-cli -- time devices              # 機材（カメラの機種・シリアル番号・写真か動画か）の一覧（LIB-16）
cargo run -p genzo-cli -- time sync 12 "2024-05-01 12:34:56" --dry-run   # 12 と同じ機材の写真を、12 の正しい時刻に合わせる見込み
cargo run -p genzo-cli -- time shift -1h --device-of 12   # 12 と同じ機材の写真を 1 時間前にずらす
cargo run -p genzo-cli -- time zone +02:00 --keep-instant 12 13   # 撮影した瞬間はそのままで、パリの時刻で表示する
cargo run -p genzo-cli -- time undo                 # 最後の撮影日時の修正を取り消す
cargo run -p genzo-cli -- timeline --gap 2h         # 撮影日とイベント（撮影の間隔 2 時間で区切る）のまとまり（ORG-01）
cargo run -p genzo-cli -- location set 12 13 --at "35.681236, 139.767125" --dry-run   # 位置を付ける見込み（LIB-21。--dry-run を外すと付ける）
cargo run -p genzo-cli -- location set 12 --at "-33.8568, 151.2153"   # 南緯・西経は負の値（度分秒・N/S/E/W・「北緯 …、東経 …」・Google マップの URL も読む。URL なら場所の URL（/place/…）がよい）
cargo run -p genzo-cli -- location clear 12         # 付けた位置を消す（カメラの位置があれば、それに戻る）
cargo run -p genzo-cli -- location undo             # 最後の位置の変更を取り消す
cargo run -p genzo-cli -- volume list --refresh     # ボリューム（ドライブ）の一覧とつながり（FILE-03）
cargo run -p genzo-cli -- folder create ~/Pictures 選別済み   # ディスクにフォルダを作り、カタログに登録する（FILE-02）
cargo run -p genzo-cli -- file move 12 13 --to ~/Pictures/選別済み --dry-run   # 写真を移す予定（移せない写真・別のドライブへのコピーの量）
cargo run -p genzo-cli -- file rename 12 13 --template "{date}-{seq}"   # 写真のファイルの名前を変える（RAW と同時記録の JPEG も）
cargo run -p genzo-cli -- missing check             # 見つからない写真を確かめる（見つからない写真があれば終了コード 3。FILE-04）
cargo run -p genzo-cli -- search --missing          # 見つからない写真だけ
cargo run -p genzo-cli -- missing relink --in /media/me/外付け --dry-run   # 見つからない写真の候補を探す（--dry-run を外すと結び付け直す）
cargo run -p genzo-cli -- folder relocate 7 /media/me/外付け/2024 --dry-run   # OS の側で移したフォルダの場所を指定し直す予定
cargo run -p genzo-cli -- folder remove 9           # 写真が登録されていない（見つからないまま残った）フォルダをカタログから除く
cargo run -p genzo-cli -- video info 21             # 動画の再生の情報（アプリが配信するファイル・再生用のファイル。VID-04）
cargo run -p genzo-cli -- video proxy 21 22         # 再生用のファイル（H.264 ＋ AAC の MP4。長辺 1280）を作る（ffmpeg が要る）
cargo run -p genzo-cli -- video cache               # 再生用のファイルのキャッシュの状況（video clear ですべて消す）
cargo run -p genzo-cli -- develop snapshot create 12 仕上げ前   # スナップショットを作る（今の状態。--from-history <履歴の ID> で履歴の段階から。DEV-28）
cargo run -p genzo-cli -- develop snapshot apply 3  # スナップショットを適用する（履歴に 1 件残る。develop undo で戻せる）
cargo run -p genzo-cli -- preset create-develop 明るく --from 12 --groups basic-tone,presence --folder 基本   # 現像プリセット（DEV-29）
cargo run -p genzo-cli -- preset apply 明るく --to 13 14   # 含める項目だけを写す（写真の処理バージョンは変えない）
cargo run -p genzo-cli -- preset create-export "Web 用" --format jpeg --quality 85 --long-edge 2048 --remove-gps --rename "{date}-{seq}"   # 書き出しプリセット（EXP-02）
cargo run -p genzo-cli -- export 12 13 --preset "Web 用" --out ./out   # プリセットで書き出す（指定したオプションがプリセットより優先）
cargo run -p genzo-cli -- preset export-files 明るく --out ./渡す   # ほかの PC へ渡すファイル（preset import で読み込む）
cargo run -p genzo-cli -- export 12 --out ./out --long-edge 2048 --output-sharpen screen --watermark-text "© 2026 山田" --watermark-font /path/to/font.ttf   # 出力シャープ・文字の透かし（EXP-03）
cargo run -p genzo-cli -- watermark preview --out ./見本.png --watermark-image ./logo.png --watermark-position bottom-right   # 透かしの見本（カタログは使わない）
cargo run -p genzo-cli -- watermark font-dirs        # OS の既定のフォントのフォルダ（フォントは同梱しない）
```

- `--json` のときは、失敗（使い方の誤りを含む）も標準出力に `{"error": {"kind": ...}}` の JSON で出します。
- `import` は、取り込みの後に現像結果からサムネイル・プレビューを作り直しません（埋め込みの JPEG のまま。現像設定を変えたときに作ります）。作り直すときは `--previews` を付けてください（決定事項ログ No.49）。
- `--gpu auto`（既定）でも、ソフトウェアの GPU（Mesa の llvmpipe・Windows の WARP など）は使わず CPU 版で処理します。使うときは環境変数 `GENZO_GPU_ALLOW_SOFTWARE=1` を付けてください（決定事項ログ No.46）。
- `develop region <ID>` で、表示範囲の描画（ルーペの等倍表示・拡大の入口。`Core::render_region`）を試せます（決定事項ログ No.58。既にあるファイルには書きません）。`--before <SOURCE>` を付けると、ビフォー / アフターのビフォーの設定で描きます（`Core::render_before_region`）。
- `metadata <ID>...`（LIB-15）: タイトル・キャプション・作成者と連絡先・著作権・撮影場所などの説明のメタデータを、カタログの中だけで変えます（元のファイルには書きません。書き出したファイルには XMP と、ASCII で表せる値は Exif にも入ります）。`--set 項目=値`（空の値で削除）・`--clear 項目` を複数指定でき、複数の写真には一括で書きます。項目の名前は `genzo metadata --help` の末尾にあります（`creator_email` は `creator-email` とも書けます）。`--set`・`--clear` がなければ、項目ごとの値を表示します。`show` にも値のある項目が出ます（決定事項ログ No.78）。
- `keyword`（LIB-09）: `list`・`add <ID>... -k <パス>`（なければ作る）・`remove <ID>... -k <キーワード>`・`show <ID>...`・`create <パス>`・`rename <キーワード> <名前>`・`move <キーワード> [--to <キーワード>]`・`delete <キーワード>`（下の階層と付与も消す。写真は消さない）・`export <キーワード> --on|--off`（書き出すファイルの XMP の `dc:subject` に書くか）。キーワードは「場所>京都」のような階層のパス（名前は全角・半角、大文字・小文字を区別しない）か `id:12` の形の ID で指定します（名前が数字だけのキーワードと取り違えないため）。`search --keyword <キーワード>`（複数指定ですべてを持つ）は下の階層を含めて探します。キーワードの名前は 200 文字までで、区切りに使う「>」「＞」「|」「,」「，」「、」と改行などの制御文字は使えません（決定事項ログ No.85）。書き出しでキーワードを書かないときは `export --remove-keywords`（決定事項ログ No.81）。
- `collection`（LIB-10・LIB-11）: `list`（スマートコレクション・セットの数も数える）・`create <名前> [--in <セットの ID>] [--kind manual|smart|set] [--rule <FILE>] [--add <ID>...]`・`add <コレクション> <ID>...`・`remove <コレクション> <ID>...`（写真は消さない）・`rename`・`move <コレクション> [--to <セットの ID>]`・`delete`（セットなら中のものも。削除するものを条件で使っているスマートコレクションがあれば警告する）・`rule <コレクション> [--set <FILE>]`（条件の表示・変更）。名前は 200 文字までです。コレクションは ID で指定します。`search --collection <ID>` で絞り込み、`search --rule <FILE>` で保存していない条件で探せます。条件の JSON の形は [docs/04_architecture.md](docs/04_architecture.md) の 3.9 節と [docs/catalog_schema.md](docs/catalog_schema.md) の 2.5 節（例: `{"version": 1, "match": "all", "rules": [{"type": "rating", "op": "gte", "value": 3}, {"type": "keyword", "op": "has", "keyword_id": 1}]}`）。条件が自分自身を参照する（循環する）と保存しません（終了コード 1。決定事項ログ No.82・83）。
- `stack`（LIB-12）: `create <ID> <ID>... [--top <ID>]`（2 枚以上。既にスタックに入っている写真があれば、そのスタックの写真もまとめる。順は撮影日時の順で、作ったスタックは折りたたむ）・`add <スタック> <ID>...`（ほかのスタックの写真は移す）・`remove <ID>...`（写真は消えない。1 枚になったスタックは消える）・`unstack <ID>...`（写真が入っているスタックを解除する）・`top <ID>`（一番上にする）・`collapse <スタック>... | --all`・`expand <スタック>... | --all`・`show <ID>...`（スタックの ID・写真の順・折りたたみ）・`auto --gap <秒> [--dry-run] [search と同じ条件]`（条件に合う写真のうち、スタックに入っていない、撮影日時の分かるものを撮影日時の順に並べ、前の写真との間隔が `--gap`（0〜3,600 秒）以下のものをまとめる）。`search` は画面と同じく、折りたたんだスタックを一番上の写真（一番上の写真が条件に合わなければ、最初に合った写真）だけで出し、ほかに条件に合う写真の数を `hidden_in_stacks`（`--json`）・末尾の文に出します。`--expand-stacks` ですべて展開して並べます（折りたたみの状態は変えない）。展開したスタックの写真は、一番上の写真の位置にスタックの順でまとめて並びます（決定事項ログ No.86・87）。
- 仮想コピー（LIB-13）: `develop virtual-copy <ID> [--name <名前>]`（元の写真と同じスタックに入れ、スタックを展開する。名前を省くと「コピー n」）・`develop rename-copy <ID> <名前>`（200 文字まで。制御文字は使えない）・`develop make-master <ID>`（仮想コピーを元の写真にする。前のマスターは仮想コピーになり、名前を受け継ぐ。現像設定・評価・キーワードなどはそれぞれのまま）・`develop delete-copy <ID>`（仮想コピーだけを削除。マスターは消せない）。マスターを `remove`・`trash` すると、その写真の仮想コピーもまとめて消えます。書き出すファイルの名前は「元のファイル名-仮想コピーの名前」です（決定事項ログ No.88）。
- 重複と思われるもの（IMP-04）: `import` は、新しく登録するファイルについて、大きさと先頭・末尾の 64 KB（クイックハッシュ）が同じ登録済みのファイルを候補にし、ファイル全体の BLAKE3 が同じなら重複として登録しません（結果の `duplicates` と表に、同じ内容の登録済みのファイルと写真を出す）。同じ取り込みの中の同じ内容のファイルも、2 つ目からを重複とします。見つからない（`missing`）ファイルは候補にしません。`--keep-duplicates` で別の写真として登録し、`--skip-duplicates` で取り込みません（どちらも省くとカタログの設定。その既定は取り込まない。決定事項ログ No.91）。
- RAW と JPEG（IMP-06）: 同じフォルダで拡張子だけが違う RAW と JPEG は、既定で 1 枚の写真（RAW が主、JPEG は付属。`search` の `has_sidecar_jpeg`）にまとめ、取り込みの結果に組にした数（`paired`）を出します。`--separate-raw-jpeg` で別々の写真に、`--pair-raw-jpeg` でまとめます（省くとカタログの設定。新しく登録するファイルだけに効き、登録済みの写真は分けない・まとめない）。`remove`・`trash` は RAW と JPEG の両方が対象で、書き出しは RAW から現像します（決定事項ログ No.92）。
- コピー取り込み（IMP-02）: `import <DIR> --copy-to <DEST>` は、DIR の写真・動画をコピー先（あるフォルダ）へコピーしてから登録します（元のファイルは読むだけで、変更・移動・削除しません）。コピーは一時ファイルに書いてディスクに確定し、元のファイルと内容（BLAKE3）が同じことを確かめてから名前を付け、同じ名前があっても上書きしません（`-1` などを付ける。同じ名前で同じ内容の登録していないファイルがあれば、コピーせずにそれを登録する）。中のフォルダは `--folders`（`year-date`: 2026/2026-10-13（既定）・`year-month-day`: 2026/10/13・`year-month`: 2026/2026-10・`date`: 2026-10-13・`date-compact`: 20261013・`single`: コピー先にそのまま・`source`: 元のフォルダの構成のまま。撮影日時が分からなければファイルの更新日時を既定のタイムゾーンで読んだ日付）。RAW と同時記録の JPEG は同じ名前で一緒にコピーします。失敗・取り消しのときは、この取り込みで作ったファイル（一時ファイル・失敗した組のコピー・作った空のフォルダ）だけを片付けます。`--dry-run` はコピー先の名前を決めて表示するだけです。結果の `copy`（`--json`）にコピーしたファイル（元 → コピー先）と次の連番（`next_sequence`）が入ります（決定事項ログ No.94・95）。
- リネーム規則（IMP-03）: `--rename <テンプレート>` の項目は `{date}`（20261013）・`{time}`（143052）・`{year}`・`{month}`・`{day}`・`{hour}`・`{minute}`・`{second}`・`{seq}`（連番。`--seq-start`（既定 1）・`--seq-digits`（1〜9、既定 4））・`{name}`（元の名前）・`{camera}`（機種）・`{text}`（`--text` の文字）。`{{`・`}}` は波かっこそのものです。連番は撮影日時の順に付け、RAW と JPEG の組は 1 つに数えます。テンプレートに直接書く文字と `--text` には `< > : " / \ | ? *` と制御文字を使えません。作る名前は、どの OS でも使えるように、Windows の予約名（`CON`・`NUL`・`COM1` など）に `_` を足し、先頭の `.`・末尾の `.` と空白を除き、Unicode を NFC にし、200 バイトまでにします（決定事項ログ No.95）。
- 取り込み時のキーワード・メタデータ（IMP-05）: `--keyword <パス>`（「場所>京都」。複数可。なければ作る）と `--meta 項目=値`（`metadata --set` と同じ項目。空の値は付けない）を、新しく登録した写真（その場で登録する取り込みでも）に、登録と同じトランザクションで付けます。登録済みの写真（取り込み直し）には付けません（決定事項ログ No.96）。
- `hash`（ORG-03）: `compute [--include-network]`（全体のハッシュのないファイルを読んで記録する。ネットワークのボリュームのファイルは `--include-network` のときだけ。読めなかったファイルがあれば終了コード 3）・`status`・`duplicates [--limit N]`（記録したファイルの中で内容が同じファイルの組）・`identify <FILE>`（ファイルの内容で、場所・名前によらず同じ内容の登録済みの写真を探す。ファイルは読むだけ）。CLI はバックグラウンドで自動の計算をしません（アプリはカタログを開いている間に計算します。決定事項ログ No.90）。
- `time`（LIB-16。撮影日時の一括修正。カタログの中だけを変え、元のファイルには書きません。書き出したファイルのメタデータには修正後の撮影日時が入ります）: `devices`（機材の一覧。数・撮影日時の範囲（既定のタイムゾーン）・修正のある数）・`shift <量> <対象>`（時計のずれの補正に足す。量は `+1h`・`-90s`・`+1d2h3m4s`・`3600`（秒））・`sync <基準の ID> <正しい日時> [--zone +09:00] [<対象>]`（基準の写真の正しい撮影日時との差を求めて、対象をその差だけずらす。対象を省くと基準の写真と同じ機材のすべて）・`zone <オフセット> [--keep-instant] <対象>`（タイムゾーンの指定。既定は記録の時刻をそのタイムゾーンの時刻として読む（UTC の印だけの動画も現地時刻として読む）、`--keep-instant` は撮影した瞬間はそのままで表示だけを変える）・`clear-zone <対象>`・`reset <対象>`（ファイルの値に戻す）・`history`・`undo`（最後の修正から順に取り消す。履歴は新しい 20 件）。対象は variant の ID か `--device-of <ID>`（その写真と同じ機材のすべて。複数可）で、`--dry-run` で変わる数・撮影日時の記録の内訳・例だけを出します（決定事項ログ No.102〜104）。
- `timeline [--gap <間隔>] [--desc] [search と同じ条件]`（ORG-01）: 撮影日時の順に並べ、撮影日（写真ごとの表示のタイムゾーンの現地の日付）と、撮影の間隔（既定 2h。0 はイベントで分けない。24h まで）で区切ったイベントのまとまりを表に出します（`--json` はコア API の形。決定事項ログ No.105）。
- `location`（LIB-21。位置情報の後付け。カタログの中だけを変え、元のファイルには書きません。カメラ（ファイル）が記録した位置も変えません）: `set <ID>... --at <座標> [--dry-run]`（位置を付ける。付けた位置があれば置き換える）・`clear <ID>... [--dry-run]`（付けた位置を消す。カメラの位置があれば、それを使う）・`history`（新しい 20 件）・`undo`（最後の変更から順に取り消す。やり直しはない）。座標は `35.681236, 139.767125`（Google マップで地点を右クリックしてコピーできる形。記号のない 2 つの数は緯度・経度の順）・`-33.8568 151.2153`・`35.681236N 139.767125E`・`35°40'52.4"N 139°46'01.7"E`・`北緯 35.681236°、東経 139.767125°`・Google マップの URL（`q=緯度,経度`・`ll=緯度,経度`・場所の URL のピン `data=…!3d緯度!4d経度`・`@緯度,経度` の順に探す。`@` は地図の中心で、ピンとずれることがあるので、場所の URL か、ピンを右クリックしてコピーした座標を使ってください）を読みます（画面の貼り付けと同じ規則。読めない・範囲外（緯度 -90〜90、経度 -180〜180）は使い方の誤り）。同じ写真の仮想コピーと、RAW と同時記録の JPEG の組は位置を共有します。動画は対象外です（除いた数を警告に出す）。`show` の GPS は使う位置（付けた位置があればそれ、なければカメラの位置）で、付けた位置なら「後から付けた位置。カメラの位置: …」と添えます。書き出しの Exif には使う位置が入ります（`export --remove-gps` では入りません）。設計は [docs/04_architecture.md](docs/04_architecture.md) の 3.16 節です。
- ボリューム（FILE-03）: `volume list [--refresh]` は、ボリュームの ID（Linux はファイルシステムの UUID（`uuid:…`）、macOS はボリュームの UUID、Windows はボリュームのシリアル番号（`winvol:…`）、ネットワークは `net:…`、分からなければマウント先のパス（`path:…`））・ラベル・マウント先・つながっているかを出します。カタログはボリュームの ID と、その中の相対パスで場所を覚えるので、外付けのドライブのマウント先（ドライブ文字など）が変わっても写真の登録は変わりません。つながっていないドライブの写真は、そのまま検索・評価できます（表示・現像・書き出しはつないでから）。版 12 より前のカタログのパスのボリュームは、開いたときに、今の場所にあり、登録したファイルが 1 つ以上同じ大きさで見つかるフォルダ（写真のないフォルダは、フォルダがあれば）だけを OS のボリューム ID に移します（決定事項ログ No.106・107）。
- `folder`（FILE-02・FILE-04）: `list`（ID・状態（つながっていない・見つからない）・見つからない写真の数）・`create <場所> <名前>`・`rename <フォルダ> <名前>`・`move <フォルダ> <移動先>`（ディスクの上でフォルダごと。同じドライブの中だけ）・`relocate <フォルダ> <新しい場所> [--dry-run]`（OS の側で移した・名前を変えたフォルダを結び付け直す。カタログの中だけを変え、ファイルは動かさない）・`remove <フォルダ>`（写真が登録されていないフォルダを配下ごとカタログから除く。見つからないまま残ったフォルダ・再リンクで空になったフォルダの片付け。ディスクのフォルダには触れない。写真があれば終了コード 1）。配下のフォルダが、名前の書き方（大文字・小文字・正規化）だけが違う別のフォルダにある場合は、`rename`・`move` をしません。フォルダは ID か、登録したフォルダのパスで指定します（数字だけの名前のフォルダは `./2024` のように書く）。名前は取り込みのリネームと同じく、どの OS でも使える名前だけです（使えない文字・Windows の予約名・先頭の「.」・末尾の「.」と空白はエラー）。同じ名前のものがあれば変えません（決定事項ログ No.108・109・113）。
- `file`（FILE-02）: `move <ID>... --to <フォルダ> [--dry-run]`・`rename <ID>... (--name <名前> | --template <テンプレート> [--text] [--seq-start] [--seq-digits]) [--dry-run]`（テンプレートの項目は `import --rename` と同じ。連番は指定した順）。RAW と同時記録の JPEG は一緒に動かし、名前もそろえます。**上書きしません**（移動先に同じ名前のファイルがあれば、その写真は飛ばして終了コード 3）。コアは操作の前に意図をカタログに記録し（file_op）、同じドライブの中は名前の変更（大文字・小文字だけの変更も一時的な名前を経由して）、別のドライブへはコピーして内容（BLAKE3）を照合してから登録し、元のファイルをゴミ箱へ移します（ゴミ箱へ移せなければ元の場所に残し、終了コード 3）。途中で失敗した写真は元に戻し、途中で終了したときは次に開いたときに実際のファイルと突き合わせて確定します（決定事項ログ No.108・110）。
- `missing`（FILE-04）: `check [--folder <フォルダ>]`（登録したファイルがあるかを確かめ、見つからない印を付ける・外す。つながっていないドライブは飛ばす。アプリはカタログを開いたときと、つないだドライブを見つけたとき（ウィンドウに戻ったときにつながりを確かめ直す）にも確かめる）・`relink --in <DIR> [<ID>...] [--no-recursive] [--accept-changed] [--dry-run]`（見つからない写真の候補を、名前・大きさ・撮影日時・内容のハッシュで探し、確からしい候補（内容が同じ・大きさと先頭と末尾が同じ）が 1 つだけのものを結び付け直す。名前と撮影日時だけが同じ（内容が変わっている）候補は `--accept-changed` のとき、それしかなければ結び付けて読み直す）・`link <ファイルの ID> <パス> [--accept-changed]`。`search --missing` で見つからない写真だけを出します（決定事項ログ No.111・112）。
- `video`（VID-04）: `info <ID>`（元のファイルを読めるか・MIME の型・コーデック・アプリが `genzo://video` で配信するファイル・今の内容の再生用のファイルの有無・作るときのエンコーダー（作れなければ理由））・`proxy <ID>...`（再生用のファイルを 1 本ずつ作って待つ。今の内容のものがあれば作らない。作れなかった動画があれば終了コード 3）・`cache`（フォルダ・数・合計・上限）・`clear`（すべて消す。作り直せる）。再生用のファイルは、WebView（OS の動画の機能）で再生できない動画のために、利用者の ffmpeg（`GENZO_FFMPEG`・`GENZO_FFPROBE`、なければ PATH）で H.264（8 bit・4:2:0。長辺 1280）＋ AAC の MP4 に変換したもので、データのフォルダの `video-proxies/` に置きます。上限（既定 5 GB。アプリの設定の「カタログ」で変えられる）を超えると、最後に使ったものが古い順に消えます。元の動画は読むだけです（決定事項ログ No.117・118）。
- `composite <ID>... [--mode lighten|darken|lighter-color|darker-color] [--format tiff|png16|png8|jpeg] [--quality N] [--color-space srgb|p3|adobergb] [--out <DIR>] [--no-stack] [--remove-gps] [--check]`（DEV-34。比較明・比較暗合成）: 選んだ写真（2〜999 枚。動画は除く）を撮影日時の順に、それぞれの現像の設定で 16bit・ディザなしで現像し、画素ごとに比べて 1 枚にします。既定は 16bit の TIFF・Adobe RGB・1 枚目と同じフォルダ（`<1 枚目の名前>-<モード>-<枚数>.tif`。同じ名前は連番）で、カタログに登録して元の写真とスタックにまとめ、一番上にします。1 枚でも現像に失敗したら（寸法が違うときを含む）何も保存しません。`--check` は合成せずに、写真の並び・設定の違い・保存先を表示します（[docs/composite_design.md](docs/composite_design.md)）。
- `develop auto <ID> [--dry-run]`（DEV-32）: 露光量・コントラスト・ハイライト・シャドウ・白レベル・黒レベルを写真に合わせて決め、設定に入れて保存します（履歴に 1 件「自動補正」。ほかの項目は変えません。`--dry-run` は値を表示するだけ）。アプリでは現像の「基本補正」の「自動」のボタン（Ctrl / ⌘ ＋ U）。値の決め方は `crates/genzo-pipeline/src/engine/auto_tone.rs` の doc（目標の値・重みは仮置き。決定事項ログ No.200）
- `develop before <ID> --out <FILE>`（DEV-31）: ビフォー / アフターの表示のビフォーのプレビューを PNG に書きます。`--source` は `initial`（取り込んだ直後の設定。既定）・`current`（今の設定）・`history:<履歴の ID>`・`snapshot:<ID>`。切り抜き・角度は今の設定と同じです。`--after <FILE>` でアフターも同じ寸法で書きます（決定事項ログ No.79。既にあるファイルには書きません）。
- `develop compare <ID> [<ID>] --width <W> --height <H> [--cx <0〜1>] [--cy <0〜1>] [--scale <倍率>] --out-dir <DIR>`（LIB-03）: 比較表示の同期ズームと同じ範囲（中心は写真ごとの出力の画像の大きさに対する割合、大きさは出力の画素でどの写真も同じ）を、2 枚までの写真について比較表示の面（`Core::render_view_region`。現像のセッションは開かない）に同時に描き、`compare-<面>-<ID>.png` に書きます（既にあるファイルには書きません。決定事項ログ No.114）。
- レンズ補正（DEV-14）: `develop set <ID>` に `--lens-profile on|off`（RAW に埋め込まれた補正の情報で歪曲・周辺減光を補正）・`--remove-ca on|off`（倍率色収差）・`--distortion-scale`・`--vignetting-scale`（埋め込みの情報の適用量。0〜200 %）・`--distortion`・`--vignetting`・`--vignetting-midpoint`・`--ca-red-cyan`・`--ca-blue-yellow`（手動の補正。-100〜100、中点は 0〜100）を指定できます（`--json <FILE>` と一緒なら JSON の後に当てます。JSON の `lens` と同じ項目です）。補正の情報は Sony の ARW のタグと DNG の opcode だけを使い、lensfun などの外部のデータは使いません。周辺減光・色収差・埋め込みの情報による補正は RAW だけで、RAW 以外の写真には手動の歪曲だけが効きます。レンズ補正は**処理バージョン 2** から使われます。処理バージョン 1 の写真（これまでに現像した写真）は `develop upgrade <ID>` で更新してください（ほかの設定は変えず、履歴に「処理バージョンを 2 に更新」が残ります。`develop undo` で戻せます）。`develop source <ID>` は写真を開いて、元の画像の情報（RAW か・大きさ・デコーダ・撮影時の WB・埋め込みのレンズ補正の情報）と処理バージョンを出します（決定事項ログ No.120〜125）。
- 変形（遠近補正。DEV-16）: `develop set <ID>` に `--upright level|vertical|full`（写真を開いて直線を調べ、求めた角度補正と垂直・水平の値を入れる。水平は角度だけ。直線が見つからなければエラー）、`--vertical`・`--horizontal`（-100〜100。負の垂直で上辺が広がる）・`--transform-aspect`（-100〜100）・`--transform-scale`（50〜150 %）・`--offset-x`・`--offset-y`（-100〜100）を指定できます（JSON の `transform` と同じ項目）。変形は**処理バージョン 2** から使われます（版 1 の写真では警告）。`develop copy --groups transform` で変形だけを写せます（決定事項ログ No.202〜206）。
- 処理バージョン（DATA-09）: `develop copy`（設定の一括適用）は**コピー先の処理バージョンを変えません**（すべての項目を写す場合も。版 1 と 2 の写真の間でも `--groups` で選んだ項目だけを写せます）。版 1 の写真に写したレンズ補正・ハイライト復元・ノイズ軽減・カラーミキサー・明瞭度・シャープ・効果など（処理バージョン 2 の項目）は保存しますが使われないので、警告と JSON の `needs_upgrade`（その写真の数）で知らせます。`--upgrade-process-version` を付けると、コピー元より古い写真をコピー元の版に上げます（下げることはありません。JSON の `upgraded`）。`develop set` は JSON に `process_version` を書いたときだけ版を変えます（`--replace` でも、書かなければ今の版のまま）。`develop upgrade <ID>...` は ID を複数受け取り、まとめて更新します（ID が 1 つなら `{"variant_id", "changed", "settings"}`、複数なら `{"updated"}`）。`develop source <ID>` は、埋め込みのレンズ補正の情報が壊れていて使えなかったとき、その理由を出します（JSON は `source.lens_correction_note`。決定事項ログ No.130・131）。
- ハイライト復元（DEV-06）・ノイズ軽減（DEV-13）: `develop set <ID>` に `--highlight-recovery clip|neutral|reconstruct`（白で切りそろえる（既定）・明るさだけ復元・色を保って復元）・`--highlight-amount`（0〜100。既定 100）・`--nr-luminance`・`--nr-luminance-detail`（既定 50）・`--nr-luminance-contrast`（既定 0）・`--nr-color`・`--nr-color-detail`（既定 50）（いずれも 0〜100）を指定できます（`--json <FILE>` と一緒なら JSON の後に当てます。JSON の `highlight_recovery`・`detail.noise_reduction` と同じ項目です）。どちらも RAW の写真だけで、**処理バージョン 2** から使われます（処理バージョン 1 の写真では警告を出して使いません。`develop upgrade <ID>` で更新してください）。`bench preview` / `bench export` にも同じオプションがあり、計測の設定に当てます（決定事項ログ No.126〜129）。
- 白黒（DEV-25）・カラーミキサー（DEV-09）・カラーグレーディング（DEV-10）: `develop set <ID>` に `--black-white on|off`（白黒にする・カラーに戻す。画面の V キーと同じ）・`--hsl-hue`・`--hsl-saturation`・`--hsl-luminance`（カラーミキサーの色相・彩度・輝度）・`--bw-mix`（白黒ミキサー）（いずれも `帯=値`。帯は `red`・`orange`・`yellow`・`green`・`aqua`・`blue`・`purple`・`magenta`、値は -100〜100。繰り返して指定でき、同じ帯は後の値）・`--grading-shadows`・`--grading-midtones`・`--grading-highlights`・`--grading-global`（`色相,強さ[,輝度]`。色相 0〜360（0 がレッド）・強さ 0〜100・輝度 -100〜100）・`--grading-blending`（0〜100。既定 50）・`--grading-balance`（-100〜100）を指定できます（`--json <FILE>` と一緒なら JSON の後に当てます。JSON の `color.black_white`・`color.mixer`・`color.grading` と同じ項目です）。白黒のときは、彩度・自然な彩度・カラーミキサーは使われません（値は保ちます）。RAW 以外の写真にも使えます。**処理バージョン 2** から使われます（処理バージョン 1 の写真では警告を出して使いません。`develop upgrade <ID>` で更新してください）。`bench preview` / `bench export` にも同じオプションがあり、計測の設定に当てます（決定事項ログ No.132〜136・142）。
- テクスチャ・明瞭度・かすみの除去（DEV-11）・シャープ（DEV-12）・効果（DEV-24）: `develop set <ID>` に `--texture`・`--clarity`・`--dehaze`（-100〜100。かすみの除去の負の値はかすみを足す）・`--sharpen-amount`（0〜150）・`--sharpen-radius`（0.5〜3.0。写真のフル解像度の画素。既定 1.0）・`--sharpen-detail`（0〜100。既定 25）・`--sharpen-masking`（0〜100。既定 0）・`--vignette-amount`（切り抜き後の周辺光量補正の量。-100〜100。負で暗く、正で明るく）・`--vignette-midpoint`（0〜100。既定 50）・`--vignette-roundness`（-100〜100。既定 0）・`--vignette-feather`（0〜100。既定 50）・`--vignette-highlights`（ハイライトの保護。0〜100。既定 0。暗くするときだけ効く）・`--vignette-center`（中心。`X,Y` で手動の位置（切り抜く前の画像の幅・高さに対する割合。0〜1）、`auto` で切り抜きの中心に戻す。既定は切り抜きの中心）・`--grain-amount`（0〜100）・`--grain-size`（0〜100。既定 25）・`--grain-roughness`（0〜100。既定 50）を指定できます（`--json <FILE>` と一緒なら JSON の後に当てます。JSON の `presence`・`detail.sharpening`・`effects` と同じ項目です）。シャープは縮小したプレビューでは効きが小さいので、等倍か書き出した画像で確かめてください。粒子の模様は写真（元ファイル）ごとに決まり、プレビューと書き出しで同じです。RAW 以外の写真にも使えます。`develop copy --groups` では `presence`（テクスチャ・明瞭度・かすみの除去）・`effects`（効果）で選べます（シャープは `detail`）。**処理バージョン 2** から使われます（処理バージョン 1 の写真では警告を出して使いません。`develop upgrade <ID>` で更新してください）。`bench preview` / `bench export` にも同じオプションがあり、計測の設定に当てます（例: `bench preview --synthetic 7008x4672 --clarity 30 --grain-amount 25`。範囲の外の値は計測の前にエラー。決定事項ログ No.137〜142）。
- カメラプロファイル・ルック（DEV-17）: `profile import <ファイル>... [--replace]` は、利用者が用意したカメラプロファイル（`.dcp`）とルック（3D LUT の `.cube`）を、カタログのデータのフォルダの `profiles` に写して取り込みます（元のファイルは変えません。ID は拡張子を除いたファイルの名前。同じ ID で内容が違うものは `--replace` のときだけ置き換えます。取り込めなかったファイルがあれば終了コード 3）。`profile list [--kind camera|look] [--camera <機種> | --for <ID>]`（その機種用か）・`profile show <camera|look> <ID>`（説明・使わない情報・著作権の表示・埋め込みの方針・SHA-256）・`profile remove <camera|look> <ID>`・`profile inspect <ファイル>`（取り込まずに確かめる。カタログなし）。現像設定は `develop set <ID>` の `--profile <ID|standard>`（カメラプロファイル。RAW だけ）・`--look <ID|none>`・`--look-amount <0〜100>`（JSON の `profile` と同じ項目。取り込んでいない ID・処理バージョン 1 の写真は警告）、`develop copy --groups profile` で写せます。`render` は `--dcp <ファイル>`・`--cube <ファイル>`・`--look-amount` で、取り込まずにファイルを使います（現像の警告を標準エラーに出します）。プロファイルのトーンカーブは使いません。**Adobe のプロファイルなどは同梱していません**（決定事項ログ No.143〜149）。`profile import` は、一覧にある同じ内容のファイルでも、写しが消えた・壊れたときは書き直します（「写しを書き直した」。「読めません。取り込み直してください」と出たときは、同じファイルを取り込み直してください。決定事項ログ No.160）。`bench preview` / `bench export` も `--dcp`・`--cube`・`--look-amount` で、計測の設定にプロファイル・ルックを当てます。
- ローカル補正（マスク。DEV-18・DEV-19）: `develop mask list <ID>`（番号・名前・有効か・部品・調整）・`add <ID> <形> [--mode add|subtract|intersect] [--invert] [--name <名前>] [調整]`・`add-part <ID> <マスク> <形> [--mode] [--invert]`・`remove-part <ID> <マスク> <部品>`・`set <ID> <マスク> [--name] [--enabled on|off] [--reset-adjustments] [調整]`・`remove <ID> <マスク>`・`overlay <ID> <マスク> --out <FILE> [--uncropped]`（重みを 8bit のグレーの PNG に。白が 100%。既にあるファイルには書きません）・`pick-color <ID> --x <0〜1> --y <0〜1>`（プレビューの点の G 座標と OKLab）。マスクと部品は **1 から数えた番号**（アプリの「マスク 1」と同じ）で指定します。形は `--linear X0,Y0,X1,Y1`（100% の点と 0% の点）・`--radial CX,CY,RX,RY[,角度[,ぼかし]]`（角度は時計回り）・`--brush <FILE|->`（ストロークの JSON。`size` は半径、`points` は点の列）・`--color-at X,Y`（プレビューの点の色を見本にする。5 つまで繰り返せる。`--color-amount` で許容量）・`--luminance MIN,MAX[,滑らかさ]`（表示の明るさの 0〜100）のどれか 1 つで、色域・輝度の範囲の既定の合成は交差（`intersect`）、ほかは加える（`add`）です。位置・半径・ブラシの大きさは **G 座標**（ジオメトリ補正後・切り抜く前の全体の外接矩形の長辺を 1 とする。左上が 0,0）なので、切り抜きや書き出しの大きさを変えてもマスクの位置は変わりません。調整は `--temperature`・`--tint`・`--exposure`（EV。-5〜5）・`--contrast`・`--highlights`・`--shadows`・`--whites`・`--blacks`・`--texture`・`--clarity`・`--dehaze`・`--saturation`・`--sharpness`（-100〜100。負のシャープはぼかす）で、全体の値に重みを掛けて足します。変更は履歴に 1 件残ります（例: 「マスクを追加」「マスク「空」の露光量 -0.70」）。`develop copy --groups masks` で写せます。**処理バージョン 2** から使われます（処理バージョン 1 の写真では警告を出して使いません。`develop upgrade <ID>` で更新してください。決定事項ログ No.150〜158）。`bench preview` / `bench export` は `--masks <FILE>`（現像設定の `masks` と同じ形の JSON。`develop get` の出力をそのまま使えます）または `--sample-masks`（線形・円形・画像を塗りつぶすブラシ・輝度範囲の見本）で、マスクのある設定を計測します。設定にブラシの部品があれば、`bench preview` はブラシで 1 本描いたときの時間（`cli_preview_brush_stroke`）も測ります（決定事項ログ No.159）。
- スポット修正（DEV-21。修復・コピースタンプ）: `develop retouch list <ID>`（番号・有効か・直し方・形・大きさ・ぼかし・不透明度・コピー元のずれ）・`add <ID> --at X,Y [--at X,Y ...] [--mode heal|clone] [--size R] [--feather N] [--opacity N] [--source DX,DY]`（`--at` が 1 つなら円、2 つ以上なら点を順に結んだ線。`--source` を省くと写真を開いて、まわりのよく似た場所を自動で選びます）・`set <ID> <スポット> [--enabled on|off] [値]`・`remove <ID> <スポット>|--all`。スポットは **1 から数えた番号** で指定します。位置・大きさ（半径）・コピー元のずれは **O 座標**（RAW の向きを反映した画像の長辺を 1 とする。左上が 0,0。角度補正・切り抜きの前の画像）なので、角度補正・レンズの歪曲補正・切り抜きを後から変えてもスポットは写っているものの上からずれません。変更は履歴に 1 件残ります（例: 「スポット修正を追加」「スポット 2 の不透明度」）。`develop copy --groups retouch` で写せます（一括適用・プリセットの既定では写しません）。**処理バージョン 2** から使われます（決定事項ログ No.207〜210。設計は [docs/retouch_design.md](docs/retouch_design.md)）。
- スナップショット（DEV-28）: `develop snapshot list <ID>`・`create <ID> <名前> [--from-history <履歴の ID>]`（既定は今の状態。履歴の段階から作っても、今の設定・履歴の位置は変えません。名前は 200 文字まで）・`rename <スナップショットの ID> <名前>`・`delete <スナップショットの ID>`（写真の設定・履歴は変えません）・`apply <スナップショットの ID>`（処理バージョンを含めて設定を置き換え、履歴に「スナップショット「名前」を適用」を 1 件残します。`develop undo` で戻せます）。ビフォーにするのは `develop before <ID> --source snapshot:<ID>` です（決定事項ログ No.164）。
- プリセット（DEV-29・EXP-02）: `preset dir`（保存のフォルダ）・`list [--kind develop|export]`・`show <プリセット>`（`--json` の `settings` は `develop set --json` にそのまま渡せます）・`create-develop <名前> --from <ID> [--groups <項目>,...] [--folder <フォルダ>]`（`--groups` を省くと、切り抜きと角度・マスクを除くすべて）・`update-develop <プリセット> --from <ID> [--groups]`・`apply <プリセット> --to <ID>... [--upgrade-process-version]`（含める項目だけを写し、写真の処理バージョンは変えません。`develop copy` と同じく、版 1 の写真に版 2 の項目を写したときは警告と JSON の `needs_upgrade` で知らせます。履歴に「プリセット「名前」を適用」を 1 件。動画は除き、警告と JSON の `skipped_videos` で知らせます）・`create-export <名前> [export と同じオプション] [--dest <フォルダ>]`・`update-export <プリセット> [...]`（指定しなかった項目は既定値に戻ります）・`rename <プリセット> <名前>`・`move <プリセット> --to <フォルダ>`（`""` でフォルダなし）・`rename-folder develop|export <フォルダ> <名前>`・`delete <プリセット>`・`import <ファイル>...`（元のファイルは変えず、同じフォルダに同じ名前があれば「 (2)」などを付けます。読み込めなかったファイルがあれば終了コード 3）・`export-files <プリセット>... --out <フォルダ>`（`<名前>.json`。既にあるファイルには書かず `-1` などを付けます）。プリセットは ID（16 桁の 16 進数）か名前（全角・半角、大文字・小文字を区別しない）で指定し、同じ名前が複数あれば `--folder`・`--kind` で絞ります。保存のフォルダは `--presets-dir`・環境変数 `GENZO_PRESETS_DIR`、なければアプリと同じ場所（Windows は `%APPDATA%\io.github.tokamocchi.genzoparis\presets`、macOS は `~/Library/Application Support/io.github.tokamocchi.genzoparis/presets`、Linux は `~/.local/share/io.github.tokamocchi.genzoparis/presets`）で、カタログをまたいで使えます。1 件 1 ファイル（版付きの JSON）です。写真の設定を使うもの（`create-develop`・`update-develop`・`apply`）だけカタログを開きます（決定事項ログ No.161〜163・165）。
- 書き出しのファイル名（EXP-02）: `export` に `--rename <テンプレート>`（`import --rename` と同じ項目。連番 `{seq}` は指定した写真・動画の順（書き出さない動画は数えない）、仮想コピーは「-名前」を足します）・`--text`・`--seq-start`・`--seq-digits` を指定できます。`--preset <書き出しプリセット>` を付けると、プリセットの形式・大きさ・色空間・品質・メタデータ・ファイル名・書き出し先を使い、指定したオプションがそれより優先します（`--remove-gps`・`--remove-keywords` はオンにするだけ。プリセットに書き出し先があれば `--out` を省けます。`--keep-names` で元のファイル名。別の OS で作ったプリセットの書き出し先・透かしのファイルは使わずに警告するので、`--out`・`--watermark-font`・`--watermark-image` で指定してください）。同じ名前のファイルの扱い（`--on-conflict`）はこれまでどおりです（決定事項ログ No.163）。
- 書き出しの仕上げ（EXP-03）: `export`・`preset create-export`・`update-export` に、`--ppi <1〜1200>`（解像度。既定 72。Exif と PNG に書くだけで画素数は変えません。印刷用の出力シャープの半径にも使います）・`--output-sharpen screen|matte|glossy`（出力シャープ。縮小の後に、画面・マット紙・光沢紙に合わせてかけます）・`--sharpen-amount low|standard|high`（既定 standard）・`--no-output-sharpen` と、透かしの `--watermark-text <文字>`（`\n` で改行。5 行・200 文字まで）＋ `--watermark-font <フォントのファイル>`（TrueType・OpenType・.ttc。アプリはフォントを同梱しません。`genzo watermark font-dirs` で OS のフォントのフォルダを表示）・`--watermark-font-index <N>`（.ttc の番号）・`--watermark-color <#RRGGBB>`、または `--watermark-image <PNG・JPEG・TIFF>`、`--watermark-position <top-left〜bottom-right の 9 か所。既定 bottom-right>`・`--watermark-size <%>`（写真の短辺に対する割合。既定は文字 4・画像 20）・`--watermark-opacity <%>`（既定 60）・`--watermark-margin <%>`（既定 2）・`--no-watermark` を指定できます。透かしのフォント・画像を読めない写真は、透かしなしでは書き出さずに失敗にします。カーニング・合字などの文字の組み・縦書きは扱いません。`genzo watermark preview --out <PNG> [--width] [--height] <透かしのオプション>` で、灰色の濃淡の見本の画像に重ねた見本を書けます（同じ名前があれば連番。決定事項ログ No.166・167・169）。
- 動画の書き出し（EXP-05）: `export` に動画を含めると、再エンコードせずに元のファイルのままコピーします（コピーを確かめてから公開し、既にあるファイルは上書きしません。`--on-conflict overwrite` でも連番を付けます）。`--skip-videos` で動画を書き出しません。**`--remove-gps` では、動画の位置情報を消せないため、動画を書き出しません**（決定事項ログ No.168）。
- Windows: 現像設定の JSON は、引数に直接書かずにファイル（`--json settings.json`）で渡すことをおすすめします（PowerShell・cmd.exe では引用符の扱いが違うため）。ファイルは UTF-8（BOM 付きも可）か BOM 付きの UTF-16 で読めます。PowerShell で `--json` の出力を受け取るときに日本語が化ける場合は、`[Console]::OutputEncoding = [Text.Encoding]::UTF8` を先に実行してください（出力は UTF-8）。
- `bench` は release ビルドで実行してください（debug ビルドの時間は記録に「debug」と残り、参考値になります）。
- ログ（SYS-07。外部へは送信しません）: CLI は標準エラー（`--log`・`GENZO_LOG`）に加えて、ログのフォルダの `cli.log` に info の水準で書き、パニックのときはクラッシュレポートを `crash/` に残します。ログのフォルダは `--log-dir`・環境変数 `GENZO_LOG_DIR`、なければアプリと同じ場所（Windows は `%LOCALAPPDATA%\io.github.tokamocchi.genzoparis\logs`、macOS は `~/Library/Logs/io.github.tokamocchi.genzoparis`、Linux は `~/.local/share/io.github.tokamocchi.genzoparis/logs`）です。ファイルに書かないときは `--no-log-file`（環境変数 `GENZO_LOG_FILE=off`）を付けてください。ログには写真のファイル名・フォルダの場所が含まれます。実行したコマンドは、サブコマンドの名前と引数の数だけを書きます（`--set` のメタデータの値・パスなどの引数の値は書きません）。
- カタログを外部のツールから読むときは、読み取り専用で開き（`sqlite3 -readonly catalog.db`）、公開ビュー（`photos_v1`・`photo_keywords_v1`・`photo_collections_v1`・`photo_stacks_v1`・`photo_files_v1`）を使ってください（[docs/catalog_schema.md](docs/catalog_schema.md)）。`photos_v1` の `gps_lat`・`gps_lon` はカメラ（ファイル）が記録した位置で、後から付けた位置（LIB-21）を含む使う位置は `location_lat`・`location_lon`（出どころは `location_source`。スキーマの版 13 から）です。

### 依存ライブラリのライセンスと脆弱性の確認

```sh
cargo install cargo-deny --locked
cargo deny check
```

設定は [deny.toml](deny.toml) です。cargo-deny で確認できない C / C++ のライブラリ・同梱データ・外部の実行ファイル（FFmpeg）は、[docs/third_party.md](docs/third_party.md) の台帳で管理します。

### 配布物に入れる第三者のライセンスの文書（DIST-05）

`Cargo.lock`・`ui/package-lock.json`・`about.toml` を変えたら、配布物に入れるライセンスの文書（`crates/genzo-app/licenses/third-party/rust.txt`・`npm.txt`）を作り直してコミットします（作り直していないと、CI と配布物のビルドが止まります）。

```sh
cargo install cargo-about --version 0.9.2 --locked --features cli   # 初回だけ
npm --prefix ui ci                                                  # npm の文書に要る
node scripts/third-party.mjs                                        # 作り直す（--check で確かめるだけ）
```

設定は [about.toml](about.toml)、方式と人の確認待ちの点は [docs/third_party.md](docs/third_party.md) の 1.4 節です。配布物では `licenses/` に入り、アプリの設定の「このアプリについて」でも読めます。

### CI

GitHub Actions（[.github/workflows/ci.yml](.github/workflows/ci.yml)）で、push と pull request のたびに次を実行します。

- Linux: フォーマットの確認と cargo-deny。画面（`ui/`）の型の検査・テスト・ビルドと、配布のスクリプト（`scripts/`）のテスト。第三者のライセンスの文書が最新か（cargo-about で作り直して比べる）
- Windows と macOS（Apple Silicon）: clippy（警告をエラーとして扱う）とテスト（デスクトップアプリの `genzo-app` を含む）。macOS では LibRaw を入れて、機能フラグ `libraw` を有効にした確認も行う

外部のアクションはコミットの SHA で固定しています（[docs/packaging.md](docs/packaging.md) の 2.5 節）。配布物は別のワークフロー（`release.yml`。手動で実行）で作ります。

## ライセンス

GenzoParis is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

全文は [LICENSE](LICENSE) を参照してください。

SPDX-License-Identifier: GPL-3.0-or-later
