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
- 2026-10-13（v1 のパッケージ 10 video。動画のプレビュー再生）: 新しい crate・npm のパッケージは足していない（Range の解釈・ffmpeg の進捗の読み取り・キャッシュの管理は自作。`Cargo.lock`・`package-lock.json` は変わらない）。動画のデコードは WebView（OS の動画の機能）が行い、WebView で再生できない動画は、利用者が入れた ffmpeg で再生用のファイルに変換する（同梱しない。下の 3.4 節と WebView の行）。
- 2026-10-14（v1 のパッケージ 18 export。出力シャープ・透かし・動画の書き出し）: **`ttf-parser` 0.25.1 を新しく足した**（workspace の依存。`default-features = false`・機能は `std` だけ。`genzo-media` の通常の依存）。透かしの文字（EXP-03）を描くために、利用者が選んだ OS のフォントのファイル（TrueType・OpenType）の字形の輪郭を読む。取得元は crates.io（`Cargo.lock` の checksum `d2df906b07856748fa3f6e0ad0cbaa047052d4a7dd609e231c4f72cee8c36f31`）、リポジトリは https://github.com/harfbuzz/ttf-parser（HarfBuzz のプロジェクト）。`Cargo.toml` の `license` は **MIT OR Apache-2.0**（どちらも deny.toml で許可済み。crate に `LICENSE-MIT`・`LICENSE-APACHE` がある）。`#![forbid(unsafe_code)]` で、この機能の組み合わせでは依存の crate がない（`Cargo.lock` に増えたのは `ttf-parser` の 1 つだけ）。選んだ理由と代わりの案は決定事項ログ No.167。**フォントのファイル自体は同梱・ダウンロードしない**（利用者の OS のフォントを使う。フォントの使用許諾は人の判断の表 No.176）。テストのフォントは `genzo-media` の `testing`（機能フラグ `test-support`。他の crate の開発用の依存だけで有効）がその場で作り、リポジトリにフォントのファイルは置いていない。ほかに、既に workspace の依存の `image`（既存の版）を `genzo-worker` の開発用の依存に加えた（透かしの画像のテストの入力）。画面（`ui/`）の npm のパッケージは足していない。このコンテナには cargo-deny がないため `cargo deny check` は実行していない（CI の `lint` ジョブで確かめる）。**ライセンスの表記の要否・方法（配布物に MIT・Apache-2.0 の文を含めるか）は、ほかの Rust の crate と同じ扱いで、AI の判断で確定しない。** **この `ttf-parser` は、同じ日に `skrifa` へ置き換えて外した**（次の項目。RUSTSEC-2026-0192。決定事項ログ No.173）。
- 2026-10-14（CI の修正。透かしの文字のフォントの解析の置き換え。決定事項ログ No.173）: CI の `lint` ジョブの cargo-deny（`check licenses advisories bans sources`）が、**RUSTSEC-2026-0192（`ttf-parser` is unmaintained。作者が保守の終了を表明し、代わりに `skrifa` を勧めている。更新先なし）** で失敗していた。**利用者の決定により、`ttf-parser` 0.25.1 を外して `skrifa` 0.47.0 に置き換えた**（workspace の依存。`default-features = false`・機能は `std` だけ。既定の `autohint_shaping` は自動ヒンティングの機能で、ヒンティングをしないので使わない。`genzo-media` の通常の依存）。`Cargo.toml`・`Cargo.lock` から `ttf-parser` はなくなった（`cargo tree -i ttf-parser` は該当なし）。新しく入った crate は次の 3 つで、どれも Google Fonts の fontations（https://github.com/googlefonts/fontations）の crate。取得元は crates.io で、`Cargo.lock` の checksum は crates.io の API の値と一致した。`Cargo.toml` の `license` はどれも **MIT OR Apache-2.0**（どちらも deny.toml で許可済み。crate に `LICENSE-MIT`・`LICENSE-APACHE` がある）で、skrifa と read-fonts は `#![forbid(unsafe_code)]`（font-types は宣言がなく、bytemuck の印の trait の `unsafe impl` が 3 か所ある）。MSRV はどれも 1.85（workspace の 1.88 以下）。
  - `skrifa` 0.47.0（2026-09-08 公開。checksum `526e654b303ff4ad73a5a40b25de2b3ce3d039c5e3b6da5f89ae6e57004cb40f`）: 透かしの文字の字形の輪郭（TrueType の `glyf`（合成の字形を含む）・CFF・CFF2。ヒンティングなし・フォントの単位）、行の高さ、送り幅。
  - `read-fonts` 0.44.0（2026-09-08 公開。checksum `d5a0cf4bb6cbce4c29756270d189a5e400ac05e91192161d83a0bcb6e68492e8`）: skrifa の下のフォントのファイルの解析（表の一覧・`head`・`hhea`・`maxp`・`cmap`）。`genzo-media` は skrifa の `raw` から、必須の表の確認と `cmap` の引き方に直接使う。
  - `font-types` 0.12.5（2026-09-08 公開。checksum `b8eb065f3251655b3c90e22e5e363f310fc5332fb3402e37bbc94752283248f6`）: read-fonts の基本の型（字形の番号・固定小数点など）。
  - 版は、公開から 2 週間以上たった最新の安定版にした。最新の skrifa 0.48.0・read-fonts 0.45.0・font-types 0.12.6 は 2026-10-03 の公開（11 日前）のため使わない。`font-types` は Cargo の解決では 0.12.6 になるので、`cargo update -p font-types --precise 0.12.5` で 0.12.5 にした（read-fonts 0.44.0 の要求は `^0.12.5`）。
  - ほかに一緒に使う `bytemuck` 1.25.2・`bytemuck_derive` 1.12.1（Zlib OR Apache-2.0 OR MIT）と `once_cell` 1.21.4（MIT OR Apache-2.0）は、依存のツリーに既にあった版で、新しい crate ではない。
  - cargo-deny 0.20.2 をこのコンテナに入れ（`cargo install cargo-deny --version 0.20.2 --locked`。EmbarkStudios の公式の crate。2026-07-09 公開）、CI と同じ `cargo deny check licenses advisories bans sources` を実行した: **advisories・bans・licenses・sources がすべて通った**。置き換えの前（同じ版の cargo-deny）は advisories が RUSTSEC-2026-0192 の 1 件で失敗した。警告（同じクレートの複数の版）は置き換えの前後で同じ 38 件で、新しい crate は含まない。deny.toml は変えていない。
  - 描画の違い（置き換えの前後で画素を比べた結果）と、壊れたフォントの扱いは決定事項ログ No.173。フォントのファイル自体は、これまでどおり同梱・ダウンロードしない。
  - **ライセンスの表記の要否・方法は、ほかの Rust の crate と同じ扱いで、AI の判断で確定しない**（DIST-05）。
- 2026-10-19（CI の修正。マスクの重ねて表示の修正の PR #5 で見つかった）: CI の `lint` ジョブの cargo-deny（advisories）が、新しく公開された **メンテナンス終了の通知 3 件**（RUSTSEC-2026-0370 `zvariant`・RUSTSEC-2026-0371 `zbus_names`・RUSTSEC-2026-0372 `zvariant_derive`。どれも zbus 6 に統合されたため）で失敗した。依存の木は `genzo-app` → `tauri-plugin-opener` 2.5.5 → `zbus` 5.19.0 で、**Linux だけ**の依存（Windows・macOS の配布物には入らない。`cargo tree -i zbus --target` で確かめた）。`tauri-plugin-opener` の最新の安定版は 2.5.5（3.0 は alpha だけ）で更新先がないため、2026-10-12 の Tauri の依存と同じく `deny.toml` で除外した（理由と見直しの時期は `deny.toml`）。脆弱性ではない。依存の crate・版・`Cargo.lock` は変えていない。**除外でよいかは人の確認待ち**。
- 2026-10-20（位置情報の後付け LIB-21 の第 2 段階。オンラインの地図の HTTPS。決定事項ログ No.187）: **新しい crate を 24 個足した**（LIB-21 の前のコミット `2fd05a1` の `Cargo.lock` と比べた数。workspace の crate の `genzo-geo` は数えない。消えた crate はない）。workspace の依存は `ureq` 3.4.2（`default-features = false`・機能は `rustls-no-provider`・`platform-verifier`）と `rustls` 0.23.45（`default-features = false`・機能は `ring`・`std`・`tls12`。ureq 3.4.2 の rustls の依存が `logging` も付ける）で、使うのは新しい crate `genzo-geo` だけ（`genzo-geo` を使うのは `genzo-app` だけ）。TLS は rustls で、暗号の実装は `ring`、証明書の検証は OS（`rustls-platform-verifier`。Windows は CryptoAPI、macOS は Security.framework、Linux は OS の証明書の保管場所を読み、rustls-webpki で確かめる）。`webpki-roots`・`aws-lc-rs`・`aws-lc-sys`・`native-tls`・`openssl`・`openssl-sys` は `Cargo.lock` にない。公開日（crates.io の `created_at`。UTC）・ライセンス・checksum は crates.io の API（`https://crates.io/api/v1/crates/<名前>/<版>`）で確かめ、`Cargo.lock` の checksum は 24 個とも API の値と一致した。どの対象の依存の木に入るかは `cargo tree --workspace --target <対象>` で確かめた（Linux は `x86_64-unknown-linux-gnu`、Windows は `x86_64-pc-windows-msvc`・`aarch64-pc-windows-msvc`、macOS は `aarch64-apple-darwin`・`x86_64-apple-darwin`）。どれも 2026-10-20 に確かめた（下の表）。
  - **経緯**: 初めは ureq の TLS の機能を使わず、`native-tls` 0.2.18（OS の TLS。Windows は SChannel、macOS は Security.framework、Linux はシステムの OpenSSL に動的リンク）と、ureq の unversioned の API（semver の約束の外）で接続を組む `genzo-geo` の `tls.rs` にした（コミット `8cfeb81`。このとき新しく入った crate は 13 個）。同じ日のレビューの後、unversioned の API に頼らないよう、ureq の公開の機能の rustls に置き換えた（コミット `8ac4eda`。決定事項ログ No.187）。そのときに `Cargo.lock` から消えたもの: `native-tls` 0.2.18・`openssl` 0.10.81・`openssl-sys` 0.9.117・`openssl-macros` 0.1.1・`foreign-types` 0.3.2・`foreign-types-shared` 0.1.1。Linux の開発環境でも OpenSSL（`libssl-dev`）は要らなくなった。`tls.rs` も消したが、git の履歴（`8cfeb81` から `8ac4eda` の前まで）には残る（3.14 節）。
  - 既存の crate の行で変わったのは、`genzo-app` の依存に `genzo-geo` が増えたことと、`jni` が 2 つの版（0.21.1・0.22.4）になったための名前の付け直し（`tao`・`tauri`・`tauri-runtime`・`tauri-runtime-wry`・`wry` の依存の `"jni"` が `"jni 0.21.1"` になった）だけ。既存の crate の版（`windows-sys` など）は `2fd05a1` と同じ。
  - 版は、公開から 2 週間以上たったものにした。Cargo の解決が選ぶ版のうち、2 週間たっていないものは前の版に下げた: `rustls` 0.23.46（2026-10-14 公開）→ 0.23.45、`rustls-native-certs` 0.8.5（2026-10-13）→ 0.8.4、`ureq-proto` 0.6.5（2026-10-17）→ 0.6.4、`schannel` 0.1.30（2026-10-16）→ 0.1.29、`webpki-root-certs` 1.0.10（2026-10-19）→ 1.0.9、`zeroize` 1.9.1（2026-10-06）→ 1.9.0（公開日は crates.io の API の値。UTC）。
  - 既に `Cargo.lock` にあって、新しく使うもの（新しい crate ではない）: `winreg` 0.56.0（MIT。2026-03-14 公開。`embed-resource` と同じ版。Windows で `genzo-geo` の直接の依存にした。プロキシの設定を読む。ureq の機能 `win-system-proxy` は使わない）、`windows-sys` 0.61.2（Windows で `rustls-platform-verifier` と `winreg` が使う）、`core-foundation` 0.10.1・`core-foundation-sys` 0.8.7（macOS で `rustls-platform-verifier`・`security-framework` が使う）、`log`・`once_cell`・`percent-encoding`・`base64` 0.23.1・`http`・`httparse`（ureq・rustls の依存）、`cc`（ring のビルド）、`tempfile`（キャッシュの書き込み）など。
  - `ring` 0.17.14 は C とアセンブリを同梱してビルドする（下の「C のソースを同梱してビルドするクレート」と 2 章の一覧）。
  - cargo-deny 0.20.2 で、CI と同じ `cargo deny check licenses advisories bans sources` を実行した（2026-10-20。コミット `1d3078e` の木）: **advisories・bans・licenses・sources がすべて通った**。ただし、**`rustls` 0.23.45 の脆弱性の通知 4 件（RUSTSEC-2026-0346〜0349。どれも 2026-10-14 の通知で、直した版は 0.23.46 以上）は、`deny.toml` の `ignore` で除いている**（0.23.46 は公開から 2 週間たっていないため。どれもこのアプリの使い方では当たらないと AI がソースで確かめた。理由は `deny.toml`。**人の確認待ち**。2026-10-28 以降に 0.23.46 以上へ上げて、除外を消す予定。3.14 節）。警告（同じ crate の複数の版）は 38 件で、LIB-21 の前（`2fd05a1`）と同じ（native-tls のときは 40 件だった）。`webpki-root-certs`（CDLA-Permissive-2.0）は `deny.toml` の `targets` のどの依存の木にも入らないので、cargo-deny は見ない。
  - 2026-10-20: `rustls-platform-verifier` 0.7.1（既定の機能なし。ureq 3.4.2 が使う版・機能と同じ）を `genzo-geo` の **dev-dependency** にした（OS による証明書の検証が自己署名の証明書を拒むことを確かめるテスト `tests::os_verifier`。コミット `2ee7461`）。**新しい crate はない**（`Cargo.lock` で増えたのは `genzo-geo` の依存の一覧の 1 行だけ）。
  - `genzo-geo` の `tls.rs`（コミット `8cfeb81` で足し、`8ac4eda` で消した）は、ureq 3.4.2 の `src/tls/native_tls.rs`（MIT OR Apache-2.0）を参考に、同じ流れで書いた（ureq の著作権の表示と許諾の文は 3.14 節）。
  - **ライセンスの表記の要否・方法は、ほかの Rust の crate と同じ扱いで、AI の判断で確定しない**（DIST-05）。
  - 新しく入った 24 個の crate（`created_at` は crates.io の API の値。UTC。「入る対象」は上の 5 つの対象の依存の木）:

| crate | 版 | 公開日 | ライセンス（crates.io） | checksum（`Cargo.lock`） | 入る対象・用途 |
|---|---|---|---|---|---|
| `ureq` | 3.4.2 | 2026-09-13 | MIT OR Apache-2.0 | `9a7ac20be9b7726e0bbdbf974c059676d9acb1cd414961f570a4e8231cacd7fc` | すべて。直接の依存（HTTPS のクライアント。同期の HTTP/1.1） |
| `ureq-proto` | 0.6.4 | 2026-09-16 | MIT OR Apache-2.0 | `f86fd172ccca569e458f61b6bdd6220965a9ef36e672a6852953b51a0e1583be` | すべて（ureq の HTTP/1.1 の解釈） |
| `utf8-zero` | 0.8.1 | 2026-02-28 | MIT OR Apache-2.0 | `b8c0a043c9540bae7c578c88f91dda8bd82e59ae27c21baca69c8b191aaf5a6e` | すべて（ureq の依存） |
| `rustls` | 0.23.45 | 2026-09-14 | Apache-2.0 OR ISC OR MIT | `0d41d731c7d2f962d1ccc364cec258de3c0e93b38c2fb3ba97ac74513048d634` | すべて。直接の依存（TLS）。脆弱性の通知 4 件を `deny.toml` で除いた（上） |
| `rustls-pki-types` | 1.15.1 | 2026-07-23 | MIT OR Apache-2.0 | `2f4925028c7eb5d1fcdaf196971378ed9d2c1c4efc7dc5d011256f76c99c0a96` | すべて（証明書などの型） |
| `rustls-webpki` | 0.103.15 | 2026-08-21 | ISC | `f3c3cf1d8b1e7d4927e2d154c3fcb02979afb9939629c62cd9048d4f07b60ac2` | すべて（証明書の検証の部品） |
| `rustls-platform-verifier` | 0.7.1 | 2026-09-24 | MIT OR Apache-2.0 | `1167586491e2b18b8bfbb293e8180ec17c201c4f076d7cb3070ca964e7598f98` | すべて（証明書の検証を OS に任せる） |
| `ring` | 0.17.14 | 2025-03-11 | Apache-2.0 AND ISC | `a4689e6c2294d81e88dc6261c768b63bc4fcdb852be6d1352498b114f61383b7` | すべて（暗号の実装。C とアセンブリを含む。2 章） |
| `untrusted` | 0.9.0 | 2021-07-13 | ISC | `8ecb6da28b8a351d773b68d5825ac39017e680750f980f3a1a85cd8dd28a47c1` | すべて（ring などの依存） |
| `subtle` | 2.6.1 | 2024-06-24 | BSD-3-Clause | `13c2bddecc57b384dee18652358fb23172facb8a2c51ccc10d74c157bdea3292` | すべて（rustls などの依存） |
| `zeroize` | 1.9.0 | 2026-06-12 | Apache-2.0 OR MIT | `e13c156562582aa81c60cb29407084cdb54c4164760106ab78e6c5b0858cf64e` | すべて（rustls などの依存） |
| `getrandom` | 0.2.17 | 2026-01-11 | MIT OR Apache-2.0 | `ff2abc00be7fca6ebc474524697ae276ad847ad0a6b3faa4bcb027e9a4614ad0` | すべて（ring が使う。依存の木の 0.3.4・0.4.3 と別の版） |
| `rustls-native-certs` | 0.8.4 | 2026-06-01 | Apache-2.0 OR ISC OR MIT | `dab5152771c58876a2146916e53e35057e1a4dfa2b9df0f0305b07f611fdea4d` | Linux だけ（OS の証明書の保管場所を読む） |
| `openssl-probe` | 0.2.1 | 2026-01-20 | MIT OR Apache-2.0 | `7c87def4c32ab89d880effc9e097653c8da5d6ef28e6b539d313baaacfbafcbe` | Linux だけ（証明書のファイルの場所を探す Rust の crate。OpenSSL のライブラリは使わない） |
| `security-framework` | 3.7.0 | 2026-02-20 | MIT OR Apache-2.0 | `b7f4bc775c73d9a02cde8bf7b2ec4c9d12743edf609006c7facc23998404cd1d` | macOS だけ（Security.framework） |
| `security-framework-sys` | 2.17.0 | 2026-02-20 | MIT OR Apache-2.0 | `6ce2691df843ecc5d231c0b14ece2acc3efb62c0a398c7e1d875f3983ce020e3` | macOS だけ |
| `windows-sys` | 0.52.0 | 2023-11-15 | MIT OR Apache-2.0 | `282be5f36a8ce781fad8c8ae18fa3f9beff57ec1b52cb3de0789201425d9a33d` | `aarch64-pc-windows-msvc` だけ（ring が使う。`x86_64-pc-windows-msvc` の木には入らない。依存の木の 0.59.0・0.60.2・0.61.2 と別の版） |
| `schannel` | 0.1.29 | 2026-03-10 | MIT | `91c1b7e4904c873ef0710c1f407dde2e6287de2bebc1bbbf7d430bb7cbffd939` | `Cargo.lock` だけ（`rustls-native-certs` の Windows 用の依存。どの対象の木にも入らない） |
| `webpki-root-certs` | 1.0.9 | 2026-07-18 | CDLA-Permissive-2.0 | `b96554aa2acc8ccdb7e1c9a58a7a68dd5d13bccc69cd124cb09406db612a1c9b` | `Cargo.lock` だけ（`rustls-platform-verifier` の WebAssembly 用の依存。どの対象の木にも入らない） |
| `jni` | 0.22.4 | 2026-03-16 | MIT OR Apache-2.0 | `5efd9a482cf3a427f00d6b35f14332adc7902ce91efb778580e180ff90fa3498` | `Cargo.lock` だけ（Android 用。どの対象の木にも入らない） |
| `jni-macros` | 0.22.4 | 2026-03-16 | MIT OR Apache-2.0 | `a00109accc170f0bdb141fed3e393c565b6f5e072365c3bd58f5b062591560a3` | `Cargo.lock` だけ（Android 用） |
| `rustls-platform-verifier-android` | 0.2.0 | 2026-09-24 | MIT OR Apache-2.0 | `eec689c0bc40ff2458a5977b6619cb718087084a18e02a131c599b62d05e1a5f` | `Cargo.lock` だけ（Android 用） |
| `simd_cesu8` | 1.2.0 | 2026-07-13 | Apache-2.0 OR MIT | `11031e251abf8611c80f460e19dbdeb54a66db918e49c65a7065b46ac7aec520` | `Cargo.lock` だけ（Android 用） |
| `simdutf8` | 0.1.5 | 2024-09-22 | MIT OR Apache-2.0 | `e3a9fe34e3e7a50316060351f37187a3f546bce95496156754b601a5fa71b76e` | `Cargo.lock` だけ（Android 用） |

- **cargo-deny が見るのは、各クレートの `Cargo.toml` の `license` と、ライセンスのファイルだけです。** C のソースを同梱してビルドするクレートについて、同梱したソースの版やファイルのヘッダは確認しません。現在の依存では次の 4 つが該当します。
  - `lcms2-sys`（Little CMS 2）と `libsqlite3-sys`（SQLite）: 2 章の一覧に記録します。
  - `blake3`: BLAKE3 の作者たち自身による C とアセンブリの実装を同梱しています（`c/` ディレクトリ）。crate のライセンスは `Cargo.toml` で CC0-1.0 / Apache-2.0 / Apache-2.0 WITH LLVM-exception の選択です。`c/blake3.c` の先頭にはライセンスの表示がなく（1.8.7 で確認）、C の部分にも crate と同じライセンスが及ぶと考えていますが、要確認です。
  - `ring` 0.17.14（2026-10-20 追加。LIB-21 の HTTPS の暗号の実装。`genzo-geo` の `rustls` が使う）: C のファイル 17（うち `crypto/constant_time_test.c` はテスト用）と、あらかじめ生成したアセンブリ（`pregenerated/` の `.S` 69・NASM の `.asm` 17・Windows 用のオブジェクトのファイル `.o` 17）、手書きのアセンブリ（`.S` 4）を同梱し、`cc` の crate で C のコンパイラを使ってビルドします（数は crates.io の 0.17.14 のソースで数えた。2026-10-20）。crate のライセンスは `Cargo.toml` で Apache-2.0 AND ISC です。`LICENSE` によると、ring の新しいコードは ISC（`LICENSE-other-bits`）、BoringSSL から来たコードは Apache-2.0（`LICENSE-BoringSSL`。一部は ISC。どちらかはファイルの先頭に書いてある）、`third_party/fiat`（fiat-crypto）は Apache-2.0、`src/polyfill/once_cell` は once_cell の MIT・Apache-2.0 です。ファイルごとのヘッダは確かめていません（要確認）。2 章の一覧に記録します。

### 1.1a 画面（`ui/`）の npm の依存（cargo-deny の対象外）

- 画面は React / TypeScript（Vite でビルド）。npm の依存は cargo-deny では確認できないため、次のルールで管理する（2026-10-12、AI 記入）。
  - `ui/package.json` で版を固定し（`save-exact`）、`ui/package-lock.json` をコミットする。CI は `npm ci` で lock のとおりに入れる。
  - 版は、公開から 2 週間以上たったものを選ぶ（公開の直後に乗っ取られた版を避けるため）。`legacy-peer-deps`（npm 10 の依存の解決の不具合の回避。`ui/.npmrc`）。
  - 新しい依存を足すときは、この節の表に追記し、ライセンスを確かめる。
- **配布物（`ui/dist` に入り、アプリに埋め込まれるもの）**: 次の 8 つ（2026-10-12 の `package-lock.json` と、各パッケージの `package.json` の `license` で確認。ファイルのヘッダ・LICENSE の本文は未確認。leaflet は 2026-10-20 に追加し、`LICENSE` の本文も確かめた）。

| パッケージ | 版 | ライセンス（package.json） | 用途 |
|---|---|---|---|
| react / react-dom / scheduler | 19.3.0 / 19.3.0 / 0.28.0 | MIT | 画面の部品 |
| @tanstack/react-virtual / @tanstack/virtual-core | 3.14.13 / 3.17.11 | MIT | グリッド・フィルムストリップの仮想スクロール |
| @tauri-apps/api | 2.11.1 | Apache-2.0 OR MIT | Tauri のコマンド・イベントの呼び出し |
| @tauri-apps/plugin-dialog | 2.7.3 | MIT OR Apache-2.0 | フォルダ・ファイルの選択と確認のダイアログ |
| leaflet | 1.9.4 | BSD-2-Clause | 位置を付けるダイアログの地図（LIB-21 の第 2 段階。2026-10-20 追加。2023-05-18 公開・依存なし。地図を出すときに読む別のファイルにまとまる。`LICENSE` は「BSD 2-Clause License」「Copyright (c) 2010-2023, Volodymyr Agafonkin」「Copyright (c) 2010-2011, CloudMade」。バイナリで再配布するときは、資料などに表示が要ると読める（第 2 項。人が確認する）。`package-lock.json` の integrity `sha512-nxS1yn…FA74PA==` は npm のレジストリの値と一致した） |

- **開発・ビルドだけで使うもの（配布物に入らない）**: Vite 8.3.1（Rolldown・lightningcss など）、@vitejs/plugin-react 6.1.1、TypeScript 6.0.3、vitest 5.0.2、@tauri-apps/cli 2.11.5、@types/leaflet 1.9.22（2026-08-01 公開）と、その依存の @types/geojson 7946.0.16（2025-01-23 公開。どちらも MIT。2026-10-20 に LIB-21 で追加）ほか。ライセンスは MIT が大半で、ほかに Apache-2.0（TypeScript・detect-libc・expect-type）、MPL-2.0（lightningcss。CSS の変換で、出力に lightningcss のコードは入らないと考えるが要確認）、ISC（picocolors）、BSD-3-Clause（source-map-js）。
- 配布物のライセンス表記（DIST-05）に、上の配布物の 8 つを含める必要があると考える（未対応。4 章。要否・方法は AI の判断で確定しない。leaflet の BSD-2-Clause の第 2 項の扱いは 3.14 節）。

### 1.2 この台帳で管理するもの

- C / C++ のライブラリ（LibRaw など）: リンクする版、ビルドの設定、配布物に含めるか
- C のソースを同梱する Rust のクレートのうち、同梱したソースの版を記録しておくべきもの（Little CMS 2、SQLite）
- 同梱データ（カメラ行列、lensfun のデータベース、ICC プロファイルなど）
- 外部の実行ファイル（FFmpeg / ffprobe）
- 移植・参考にするアルゴリズムとコード（RCD デモザイクなど）
- テスト・評価の基準に使う公表データ（ColorChecker の基準値など）
- 開発環境だけで使うもの（配布物に含めないことを確認するため）
- 実行時に使う外部のサービス（利用ポリシー・利用規約のあるもの。2026-10-20 追加: OpenStreetMap のタイルと Nominatim。配布物には含めない。3.14 節）

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
| darktable のレンズ補正（埋め込みのメタデータの解釈。参考） | Sony の ARW に埋め込まれたレンズ補正の値（0x7032・0x7035・0x7037）の意味と、DNG の opcode（WarpRectilinear・FixVignetteRadial）の形・式の確認（DEV-14。決定事項ログ No.121。04 の 2.1 節のステージ 7・9） | darktable のブランチ master（2026-10-13 に取得。コミットは記録していない）の `src/iop/lens.cc`・`src/common/exif.cc`・`src/common/dng_opcode.c`（SHA-256 は 3.9 節） | https://github.com/darktable-org/darktable | GPL-3.0-or-later（3 つのファイルのヘッダの「either version 3 of the License, or (at your option) any later version」で確認） | 該当なし（第三者のファイルは取り込んでいない。自前の Rust のコード `crates/genzo-raw/src/lens.rs`・`crates/genzo-pipeline/src/lens.rs`） | アプリのソースに含まれる（自前のコード） | 値の解釈（節点の位置・尺度・式）が darktable と同じなので、参考にした実装として、darktable の著作権表示（「Copyright (C) 2019-2026 darktable developers」）と出典・SHA-256 を `crates/genzo-pipeline/src/lens.rs` の doc に記載した（2026-10-13）。書き方の妥当性は人・専門家の確認待ち | AI 記入（2026-10-13）。扱いは人・専門家の確認待ち |
| ハイライト復元・ノイズ軽減の手法（公開の手法。参考） | ハイライト復元（DEV-06。04 の 2.1 節のステージ 4）とノイズ軽減（DEV-13。ステージ 6）の考え方: 近傍の色の比による再構成（最小二乗の比）、平方根による分散の安定化（Anscombe 変換の近似）、B3 スプラインの à trous ウェーブレットと non-negative garrote による係数の縮小（決定事項ログ No.126・127） | 該当なし（2026-10-13 の自前の実装。`crates/genzo-pipeline/src/sensor/highlight.rs`・`denoise.rs`、GPU 版は `crates/genzo-gpu/src/shaders/sensor.wgsl`・`denoise.wgsl`） | 文献の手法（3.10 節。**この実装では文献・第三者のコードを取得していない。AI の知識**） | 自前のコードとして、アプリと同じ GPL-3.0-or-later。第三者のコード（darktable・RawTherapee など）は参照・取り込みしていない | 該当なし（自前の Rust・WGSL のコード） | アプリのソースに含まれる（自前のコード） | 不要の見込み（コードを写していないため）。手法の出典は 3.10 節に記す | AI 記入（2026-10-13）。出典の書き方は人の確認待ち |
| テクスチャ・明瞭度・かすみの除去・シャープ・効果の手法（公開の手法。参考） | テクスチャ・明瞭度・かすみの除去（DEV-11。04 の 2.1 節のステージ 12）・シャープ（DEV-12。ステージ 14）・効果（DEV-24。ステージ 16）の考え方: 輝度の対数の領域のぼかしとの差（周波数分解）とアンシャープマスク、暗いチャンネルの事前分布によるかすみの推定、ガイデッドフィルタによる精緻化、超楕円の周辺光量、格子の値の補間による粒子（決定事項ログ No.137〜139） | 該当なし（2026-10-14 の自前の実装。`crates/genzo-pipeline/src/finish/presence.rs`・`sharpen.rs`・`effects.rs`・`frequency.rs`・`crates/genzo-pipeline/src/haze.rs`、GPU 版は `crates/genzo-gpu/src/shaders/detail.wgsl`・`noise.wgsl`） | 文献の手法（3.11 節。**この実装では文献・第三者のコードを取得していない。AI の知識**） | 自前のコードとして、アプリと同じ GPL-3.0-or-later。第三者のコード（darktable・RawTherapee・論文の著者の公開のコードなど）は参照・取り込みしていない | 該当なし（自前の Rust・WGSL のコード） | アプリのソースに含まれる（自前のコード） | 不要の見込み（コードを写していないため）。手法の出典は 3.11 節に記す | AI 記入（2026-10-14）。出典の書き方と、手法の権利（特許など）の有無は人の確認待ち |
| Adobe の DNG の仕様（カメラプロファイル（DCP）の形と色の計算の考え方。参考） | 利用者が取り込むカメラプロファイル（DCP）の読み込みと、ステージ 8 の行列・色相彩度のマップ、ルックテーブルの計算（DEV-17。04 の 2.1 節・2.6 節、決定事項ログ No.143〜146）。**Adobe のプロファイル（DCP のファイル）は同梱しない** | 該当なし（2026-10-14 の自前の実装。`crates/genzo-color/src/dcp.rs`・`cube.rs`、`crates/genzo-pipeline/src/profile.rs`・`finish/look.rs`、GPU 版は `crates/genzo-gpu/src/shaders/profile.wgsl`・`lut3d.wgsl`） | 仕様の文書は**この実装では取得していない**（考え方は AI の知識）。タグの番号は、開発環境の libtiff の `tiff.h`（Ubuntu の `libtiff-dev` 4.5.1+git230720-4ubuntu2.5）と照合した（3.12 節） | 仕様の利用の条件（特許のライセンスなど）は**未確認**（**人・専門家の確認待ち**。AI は判断しない）。第三者のコード（Adobe の DNG SDK・dcamprof・RawTherapee・darktable など）は参照・取り込みしていない | 該当なし（自前の Rust・WGSL のコード） | アプリのソースに含まれる（自前のコード） | 未確認（3.12 節） | AI 記入（2026-10-14）。人・専門家の確認待ち |
| 利用者が取り込むカメラプロファイル・ルック（DCP・.cube） | 現像の色（DEV-17）。利用者が自分で用意したファイルを、カタログのデータのフォルダの `profiles` に写して使う | 利用者による | 利用者による（このアプリは配布しない・ダウンロードしない） | ファイルごとの条件（利用者が確かめる）。画面にファイルの著作権の表示（ProfileCopyright）と埋め込みの方針（ProfileEmbedPolicy）をそのまま出す | 該当なし（実行時に読む） | **配布物に含めない** | — | AI 記入（2026-10-14）。画面の文言は人の確認待ち（implementation_status の人の判断の表 No.159） |
| ColorChecker Classic の基準値 | PoC-4 の色の評価の基準とテスト（`crates/genzo-testkit/src/colorchecker.rs`。2014 年 11 月以降の版と、より前の版の 24 色 × L\*a\*b\*（D50）） | X-Rite の公表値（CGATS のファイル。2014 年 11 月以降の版の測定日は 2015-04-28）。値は colour-science 0.4.6 のデータから写したもの | X-Rite（現在は Calibrite）の公表資料。2026-10-10 の照合には、GitHub 上の第三者のリポジトリにある CGATS ファイルの写しを使った（3.8 節） | 利用条件は未確認（**要確認**） | 該当なし（テストのデータ） | `genzo-testkit` はテストと計測用の crate。開発用の CLI（`genzo`）は計測の記録のために依存するが、配布するアプリに含めるかは未定（含める場合は利用条件の確認が要る） | 出典（X-Rite）を doc に記載 | AI 記入（2026-10-10）。値は写しと一致、一次資料とは未照合（人の確認待ち） |
| カメラ行列（α7 IV / α7C） | カメラ RGB から XYZ への変換（04 の 2.5 節の `render_deps`、2.6 節） | 未作成 | 候補は LibRaw の内蔵の表、DNG の ColorMatrix、カラーチャートからの自作（3.6 節） | 要確認（出典ごとに利用条件を確認する） | アプリのデータファイルとして持ち、ID とハッシュで参照する | アプリに同梱する予定 | 出典による（要確認） | 未着手 |
| Mesa（llvmpipe） | 開発環境（Linux のコンテナ）で wgpu を動かすための、ソフトウェアの Vulkan | 25.2.8（`mesa-vulkan-drivers` 25.2.8-0ubuntu0.24.04.4）。Vulkan のローダーは `libvulkan1` 1.3.275.0 | Ubuntu 24.04 の apt | 主に MIT とされる（未確認。配布しないため優先度は低い） | wgpu が実行時に Vulkan のローダー経由で読み込む。ビルド時のリンクはない | **配布物に含めない** | 不要（配布しない） | 開発環境だけで使う |
| LibRaw の同梱（配布物） | 配布物のワーカーが読み込む LibRaw と、その依存の共有ライブラリ（決定事項ログ No.64） | 配布のビルドで入る版。2026-10-12 の release.yml の実行（run 38276847388）では、macOS は Homebrew の libraw 0.22.2・libomp 23.1.0・jpeg-turbo 3.2.0・little-cms2 2.19.1（ログの元のファイルのパス）。Windows の vcpkg の版はログに出ない（要確認）。実際に入ったファイルと SHA-256 は `crates/genzo-app/binaries/bundled-libs.txt`（配布のビルドが作り、artifact に入る） | Windows: vcpkg。同梱したのは `raw_r.dll`・`lcms2-2.dll`・`z.dll`（ワーカーと DLL の読み込みをたどった結果。jasper・libjpeg-turbo の DLL はたどった先になかった）。macOS: Homebrew。同梱したのは `libraw_r.25.dylib`・`libomp.dylib`・`libjpeg.8.dylib`・`liblcms2.2.dylib` | LibRaw は LGPL-2.1 / CDDL-1.0 の選択（3.1 節）。依存はそれぞれのライセンス（未確認） | 動的リンク（ワーカーだけ。本体は依存しない） | Windows: インストール先のフォルダ。macOS: `.app` の `Contents/Frameworks`（参照を書き換えてアドホック署名） | **2026-10-19 から、LGPL-2.1 の全文・LibRaw の著作権表示・案内（版・同梱したファイル・ソースの入手先・差し替えの方法）・ビルドの手順の写し（vcpkg の port とパッチ、Homebrew の formula。手順のリポジトリのコミットを固定した URL も）を同梱する**（決定事項ログ No.175、3.1 節）。2026-10-12・10-14・10-19（main の 9f0773a）の配布物には入っていない。この内容で LGPL の条件を満たすかは人・専門家の確認待ち | AI 記入（2026-10-12）。配布物は release.yml で作成（2026-10-12。[packaging.md](packaging.md) の 6.2 節） |
| Visual C++ のランタイム | ワーカーと LibRaw の DLL が使う（Windows） | ビルドした Visual Studio の版による（同梱の一覧 `bundled-libs.txt` に元の場所と SHA-256 を書く）。2026-10-12 の実行では、windows-latest の Visual Studio 18 Enterprise の `VC\Redist\MSVC\14.51.36231\x64\Microsoft.VC145.CRT` から | Visual Studio の再頒布用のフォルダ（`VC\Redist\MSVC\<版>\x64\Microsoft.VC*.CRT` など） | Microsoft のライセンス（Visual Studio の `REDIST.TXT` の再頒布可能なファイル。条件は人が確認） | **インストール先に同梱する**（決定事項ログ No.73。本体は静的にリンク）。Windows Update では更新されない | 同梱する（ワーカーと LibRaw の DLL が読み込むもの。2026-10-12 の実行では `VCRUNTIME140.dll`・`VCRUNTIME140_1.dll`・`MSVCP140.dll`） | — | AI 記入（2026-10-12）。利用者の指示で同梱に変更。再頒布の条件は人の確認待ち |
| WebView（Windows: WebView2、macOS: WKWebView） | アプリの画面の表示（Tauri 2。決定事項ログ No.60）。2026-10-13 から、動画の再生（VID-04。`video` 要素。デコードは OS の動画の機能: Windows の Media Foundation・macOS の AVFoundation。Windows の HEVC は OS の「HEVC ビデオ拡張機能」とハードウェアに依存。決定事項ログ No.117） | OS のもの（同梱しない） | Windows 10 / 11 の WebView2 ランタイム（Microsoft）、macOS の WebKit | OS の構成要素。WebView2 のランタイムがない Windows では、インストーラー（NSIS）が Microsoft の配布元から入れる（Tauri の既定の `downloadBootstrapper`。要確認） | 動的に使う（同梱しない） | 同梱しない | 要確認（WebView2 の配布の条件） | AI 記入（2026-10-12） |
| NSIS（Windows のインストーラー） | Windows のインストーラーの作成（`npx tauri build`。SYS-08） | Tauri の CLI がビルドのときに取得する版（未記録） | Tauri の CLI が GitHub の tauri-apps/binary-releases から取得（要確認） | zlib/libpng のライセンスとされる（要確認） | インストーラーの実行ファイルに NSIS のスタブが入る | インストーラー | 要確認 | AI 記入（2026-10-12）。版と取得元は配布のビルドの記録で確かめる |
| WebKitGTK・WebKitWebDriver | 開発環境（Linux のコンテナ）でアプリを動かす・E2E（`e2e/`） | 2.52.6（`libwebkit2gtk-4.1-dev`・`webkit2gtk-driver` 2.52.6-0ubuntu0.24.04.1） | Ubuntu 24.04 の apt | LGPL-2.1 / BSD とされる（要確認） | 開発環境だけ | 配布しない | — | AI 記入（2026-10-12） |
| tauri-driver | E2E の WebDriver の中継（開発環境だけ） | 2.0.6（`cargo install tauri-driver --version 2.0.6 --locked`） | crates.io | Apache-2.0 OR MIT（crates.io の表示） | 開発環境だけ | 配布しない | — | AI 記入（2026-10-12） |
| アプリのアイコン | アプリ・インストーラーのアイコン（`crates/genzo-app/icons/icon.svg` から `tauri icon` で作成） | — | このリポジトリ（自作） | アプリと同じ GPL-3.0-or-later | 同梱 | アプリ・インストーラー | — | AI 記入（2026-10-12） |
| OpenStreetMap の地図のタイル（`tile.openstreetmap.org`） | 位置を付けるダイアログの地図（LIB-21 の第 2 段階。04 の 3.16 節）。利用者が有効にしたときだけ、表示している範囲のタイルを取る | 外部のサービス（OSM Foundation の標準のラスターのタイル） | `https://tile.openstreetmap.org/{z}/{x}/{y}.png`（設定で別の提供元に変えられる） | 地図のデータは ODbL（/copyright の本文の元）。利用はタイルの利用ポリシー（GitHub のソースで読んだ。公開ページは未確認）。タイルの画像のライセンスは要確認 | 実行時に HTTPS で取り、利用者の PC のキャッシュのフォルダ（`map-tiles`）に保存する（`genzo-geo`。フォルダがない・書けないときはメモリーだけ） | **配布物に含めない**（同梱・先読みしない） | 地図の右下に「© OpenStreetMap contributors」（/copyright を開く。文言・位置は人の確認待ち） | AI 記入（2026-10-20）。ポリシー・帰属・ODbL は人の確認待ち（3.14 節） |
| Nominatim（地名の検索。`nominatim.openstreetmap.org`） | 位置を付けるダイアログの地名の検索（LIB-21 の第 2 段階）。**既定では使わない**。設定の「OSM の公開の地名の検索（Nominatim）を使う」を ON にしたときだけ使う（2026-10-20。決定事項ログ No.193）。設定した検索サービスで置き換えられる。利用者が Enter かボタンで検索したときだけ | 外部のサービス（OSM Foundation の Nominatim のサーバー） | `https://nominatim.openstreetmap.org/search`（設定で ON のときだけ。検索サービスの URL を設定したときはそちら） | 結果のデータは ODbL（ポリシーの原文と、応答の `licence`）。利用は Nominatim の利用ポリシー（GitHub のソースで読んだ。公開ページは未確認） | 実行時に HTTPS で問い合わせ、結果はプロセスの中だけにキャッシュする（`genzo-geo`） | **配布物に含めない** | 地図の右下の表示を共用する（検索の結果の一覧には出していない） | AI 記入（2026-10-20）。公開の Nominatim の扱いは利用者の決定（既定 OFF・設定で ON）。ポリシーの順守・ODbL の法的な最終の確認は人の確認待ち（3.14 節） |
| 証明書の検証（Windows: CryptoAPI、macOS: Security.framework、Linux: OS の証明書の保管場所） | オンラインの地図の HTTPS のサーバーの証明書の検証（`rustls-platform-verifier` を通して使う。LIB-21。2026-10-20 に、OS の TLS（native-tls。Windows: SChannel、macOS: Security.framework、Linux: OpenSSL）から置き換えた。TLS そのものは rustls と ring） | OS のもの（同梱しない） | Windows・macOS・Linux の OS | OS の構成要素 | 実行時に OS の機能を呼ぶ・OS の証明書の保管場所を読む（同梱しない）。Linux は検証そのものを rustls-webpki が行う | 同梱しない | — | AI 記入（2026-10-20）。Windows・macOS の実機で未確認 |
| ring（暗号の実装） | オンラインの地図の HTTPS の暗号（`rustls` 0.23.45 の暗号の実装。LIB-21。`genzo-geo`。2026-10-20 追加。それまで Linux の開発環境で動的リンクしていた OpenSSL は、同じ日にやめた（1.1 節）） | 0.17.14（crate。checksum は 1.1 節） | crates.io の `ring`（C とアセンブリを同梱）。上流は https://github.com/briansmith/ring （BoringSSL を元にした部分を含む） | crate の `Cargo.toml` は Apache-2.0 AND ISC。`LICENSE` によると、ring の新しいコードは ISC、BoringSSL から来たコードは Apache-2.0（一部は ISC。ファイルの先頭に書いてある）、`third_party/fiat`（fiat-crypto）は Apache-2.0（1.1 節）。ファイルごとのヘッダは未確認（要確認） | C のファイルとアセンブリを `cc` でビルドして静的にリンクする（C のコンパイラが要る） | アプリの実行ファイル（`genzo-app`）に入る。対応するソースは crates.io の ring 0.17.14 | 要確認。DIST-05 の全体の作業で扱う（未対応。初めての配布の前に要る） | AI 記入（2026-10-20）。FSF の一覧で Apache-2.0 と ISC は GPLv3 と両立するとされる（AI の理解）。GPL-3.0-or-later との両立の法的な最終の確認と表記の方法は人の確認待ち（3.14 節） |

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
  - **配布物に入れる文書**（2026-10-19、決定事項ログ No.175）: 上流の LibRaw（GitHub の `LibRaw/LibRaw` の 0.22.0 のタグ）の `LICENSE.LGPL`（LGPL-2.1 の全文。SHA-256 `eea173a556abac0370461e57e12aab266894ea6be3874c2be05fd87871f75449`。0.21.2・0.21.4・master も同じ内容）と `COPYRIGHT`（SHA-256 `574c88f5dd59414cfc896293a63defbc2d525a418ed413d7ab7dcc43cf79c509`。master と同じ）を、そのまま `crates/genzo-app/licenses/LibRaw/` に置き、ビルドのときに作る案内 `README.txt`（版・同梱したファイル・ソースの入手先・差し替えの方法）・ビルドの手順の写し（`build-recipe/`。vcpkg の port の定義とパッチ、または Homebrew の formula。案内には手順のリポジトリのコミットを固定した URL を書く）と一緒に配布物へ入れる（[packaging.md](packaging.md) の 1.1）。`COPYRIGHT` には、dcraw・DCB・FBDD・X3F・Adobe DNG SDK 由来のコードの表記も含まれる。**この文書と案内で LGPL の条件を満たすかは、人（必要なら専門家）が確認してください。**
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
  - 2026-10-13（VID-04。決定事項ログ No.117・118）: WebView で再生できない動画のために、**利用者が入れた ffmpeg** で再生用のファイル（H.264 ＋ AAC の MP4）を作り、利用者の PC のキャッシュ（データのフォルダの `video-proxies/`）に置くようにしました。エンコーダーは利用者の ffmpeg にあるもの（libx264（GPL-2.0-or-later とされる）・`h264_videotoolbox`（macOS の VideoToolbox）・`h264_mf`（Windows の Media Foundation）・`libopenh264`（Cisco の OpenH264）の順。AAC は ffmpeg の内蔵のエンコーダー）を使い、アプリは ffmpeg も作ったファイルも配布しません。作ったファイルの扱いと H.264 の特許について、確認が要るかは人が判断してください（人の判断の表 No.128。ライセンス・法的な表現は確定していません）。

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

### 3.9 レンズ補正の埋め込みの情報の解釈（darktable を参考にした）

**実装の状況**（2026-10-13、AI 記入。DEV-14、決定事項ログ No.121〜123）
- 利用者の決定（DEV-14）: **ARW / DNG に埋め込まれた補正の情報を使い、ない場合は手動のスライダーで補正する。lensfun などの外部のデータは使わない・同梱しない**。アプリは外部のレンズのデータベース・プロファイルを持たず、ダウンロードもしません。
- 読み取り（`crates/genzo-raw/src/lens.rs`）と補正の計算（`crates/genzo-pipeline/src/lens.rs`、GPU 版は `crates/genzo-gpu/src/shaders/geometry.wgsl`）は自前の Rust・WGSL のコードです。**第三者のファイルは取り込んでいません**（取得したソースはリポジトリの外に置き、リポジトリにはコピーしていない）。
- Sony の値の意味には公開の仕様がないため、darktable の解釈（`_init_coeffs_md_v2`。新しい現像で使われる版 2）に合わせました: 節点の位置 `(i + 0.5) / (n − 1)`、節点の間の線形補間、歪曲 `1 + t · d · 2^−14`、色収差 `1 + a · 2^−21`、周辺減光 `v' = 2^(0.5 − 2^(t · v · 2^−13 − 1))` で画素を割る（下限 1e-4）。**コードは写していません**が、値の解釈と式が同じなので、参考にした実装として著作権表示と出典を `genzo-pipeline/src/lens.rs` の doc に書きました。
- DNG の opcode の形（見出し・面の数・係数）は `dng_opcode.c` で確かめました（WarpRectilinear は面ごとに係数 6 個と中心 2 個）。式は darktable の `lens.cc` の DNG の扱いと同じです。ただし darktable は中心を画像の中心、半径の基準を画像の対角線の半分にしていて、GenzoParis は opcode の光学中心と、中心から最も遠い隅までの距離を使います（DNG の仕様の記述による。**仕様書はこの調査では取得していない。AI の知識**）。
- Sony のタグの場所（RAW の SubIFD。Exiv2 の `Exif.SubImage1.*CorrParams`）は `exif.cc` で確かめました。ExifTool のタグの表は、この調査では取得していません。

**調べたファイル**（2026-10-13、AI の調査。GitHub の公開リポジトリから匿名で取得。**法的な評価（派生物に当たるかなど）はしていません**。記録に使う前に元のページで確かめてください）

| ファイル | 取得元 | 版（取得時点） | ライセンスの表示 | 著作権の表示 | SHA-256 |
|---|---|---|---|---|---|
| `src/iop/lens.cc` | https://raw.githubusercontent.com/darktable-org/darktable/master/src/iop/lens.cc | ブランチ master（2026-10-13。コミットは記録していない） | GPL-3.0-or-later | 「Copyright (C) 2019-2026 darktable developers.」 | `5801e7dd1ef957a8fd243badb3fc28b579a919aa9abcb39c42c9dd2444577886` |
| `src/common/exif.cc` | https://raw.githubusercontent.com/darktable-org/darktable/master/src/common/exif.cc | 同上 | GPL-3.0-or-later | 「Copyright (C) 2009-2026 darktable developers.」 | `c5ecf9fff1234a329fd1e82c9ad6d571ba6e321d922eed01f5a58f6b87c314f5` |
| `src/common/dng_opcode.c` | https://raw.githubusercontent.com/darktable-org/darktable/master/src/common/dng_opcode.c | 同上 | GPL-3.0-or-later | 「Copyright (C) 2011-2023 darktable developers.」 | `61f775f54e25aa1cba4cae6b3a4252adee86a9591ef4e1c9cbc694346a5b5e39` |

- **人の確認が必要な事項**
  1. 値の解釈を参考にした（コードは写していない）実装の、著作権表示の書き方と扱い（RCD デモザイクの 3.5 節と同じ論点）。
  2. 取得したファイルのコミット（記録していない。上の SHA-256 で同じファイルかを確かめられる）。
  3. Sony の値の意味が実機の ARW で正しいか（人の判断の表 No.131）。DNG の opcode の中心と半径の基準（同 No.132）。

### 3.10 ハイライト復元・ノイズ軽減の手法（公開の手法。第三者のコードは参照していない）

**実装の状況**（2026-10-13、AI 記入。DEV-06・DEV-13、決定事項ログ No.126・127）
- ハイライト復元（`crates/genzo-pipeline/src/sensor/highlight.rs`、GPU 版は `crates/genzo-gpu/src/shaders/sensor.wgsl` の `highlight_main`）とノイズ軽減（`crates/genzo-pipeline/src/sensor/denoise.rs`、GPU 版は `crates/genzo-gpu/src/shaders/denoise.wgsl`）は自前の Rust・WGSL のコードです。**第三者のファイル・コードは取得も取り込みもしていません**（darktable の「inpaint opposed」「denoise (wavelets)」・RawTherapee の「color propagation」・dcraw の `-H` の各方式は、この実装のために読んでいません。名前と考え方は AI の知識として知っているもの）。
- 使った考え方（公開の手法。**文献はこの調査では取得していない。出典は AI の知識による**）:
  - 飽和したチャンネルを、近傍の飽和していない画素の色の比で再構成する（ハイライト復元の一般的な考え方）。比は最小二乗の形（`Σ u·c / Σ u²`）で、明るい参照を重く見る重みと、手がかりの少なさによる明るさだけの復元への移り方は、この実装で決めたもの。
  - 分散の安定化: 光のショットノイズに対する平方根の変換（F. J. Anscombe, 1948 の変換の近似）。
  - à trous（穴あき）のウェーブレット変換と B3 スプラインの 5 タップ `[1, 4, 6, 4, 1] / 16`（J.-L. Starck・F. Murtagh らの多重解像度の画像処理の文献で広く使われるもの）。
  - 係数の縮小の non-negative garrote（L. Breiman, 1995。ウェーブレットの縮小への応用は H.-Y. Gao, 1998）。
- 段ごとの雑音の大きさの表（`NR_LUMA_LEVEL_NOISE` など）は、合成の雑音をこのアプリの RCD でデモザイクして計測した値（`denoise/tests.rs`）。

- **人の確認が必要な事項**
  1. 手法の出典の書き方（コードは写していないので著作権表示は要らない見込みだが、論文の参照の要否と書き方）。
  2. 実写での画質（人の判断の表 No.137・138）。

### 3.11 テクスチャ・明瞭度・かすみの除去・シャープ・効果の手法（公開の手法。第三者のコードは参照していない）

**実装の状況**（2026-10-14、AI 記入。DEV-11・DEV-12・DEV-24、決定事項ログ No.137〜140）
- ステージ 12（`crates/genzo-pipeline/src/finish/presence.rs`）・14（`sharpen.rs`）・16 の効果（`effects.rs`）、共通の部品（`frequency.rs`）、段階 B のかすみの推定（`crates/genzo-pipeline/src/haze.rs`）、GPU 版（`crates/genzo-gpu/src/shaders/detail.wgsl`・`noise.wgsl`）は自前の Rust・WGSL のコードです。**第三者のファイル・コードは取得も取り込みもしていません**（論文の著者の公開のコード、darktable の「haze removal」「local contrast」「sharpen」「grain」、RawTherapee の各方式は、この実装のために読んでいません。名前と考え方は AI の知識として知っているもの）。
- 使った考え方（公開の手法。**文献はこの調査では取得していない。出典は AI の知識による**）:
  - かすみの推定: 暗いチャンネルの事前分布（K. He, J. Sun, X. Tang, "Single Image Haze Removal Using Dark Channel Prior", CVPR 2009）。窓の最小で暗いチャンネルを求め、その上位の画素の平均で大気の光を推定し、透過率 `1 − ω·暗いチャンネル` でかすみの式を戻す。窓の大きさ（G の長辺の 1 %）・上位の割合（0.1 %）・ω（0.95）・透過率の下限（0.1）・負の値（かすみを足す）の式は、この実装で決めたもの（仮置き）。
  - 暗いチャンネルの精緻化: ガイデッドフィルタ（K. He, J. Sun, X. Tang, "Guided Image Filtering", ECCV 2010）。半径（窓の 2 倍）と ε（1e−3）はこの実装で決めたもの。
  - テクスチャ・明瞭度・シャープ: ぼかした画像との差を強める（アンシャープマスク。写真の処理で古くから広く使われる考え方）を、輝度の対数の領域で行う。しきい値・上限・中間調の重み・勾配のマスクの形は、この実装で決めたもの。
  - 周辺光量: 超楕円（|u|^q + |v|^q）の距離と smoothstep の重み。粒子: 整数のハッシュ（17b のディザと同じ関数。既存の自前のコード）で作る格子の値の双線形の補間（バリューノイズ）を 2 つの大きさで重ねる。
- **特許などの権利**: これらの手法（特に暗いチャンネルの事前分布とガイデッドフィルタ）に特許などの権利があるか、あるならアプリの配布に関係するかは**調べていません**（**要確認**。AI はこれについて判断しません）。

- **人の確認が必要な事項**
  1. 手法の出典の書き方（コードは写していないので著作権表示は要らない見込みだが、論文の参照の要否と書き方）。
  2. 手法の権利（特許など）の有無と扱い（implementation_status の人の判断の表 No.151）。
  3. 実写での画質（人の判断の表 No.149）。

### 3.12 カメラプロファイル（DCP）とルック（3D LUT）（DNG の仕様の考え方。第三者のコードは参照していない）

**実装の状況**（2026-10-14、AI 記入。DEV-17、決定事項ログ No.143〜146）
- DCP の読み込み（TIFF と同じ形。番号 `0x4352`）・検証・行列と色相彩度のマップの補間・ルックテーブル、.cube の読み込みは、自前の Rust のコード（`crates/genzo-color/src/dcp.rs`・`cube.rs`）と WGSL（`crates/genzo-gpu/src/shaders/profile.wgsl`）です。**第三者のファイル・コードは取得も取り込みもしていません**（Adobe の DNG SDK、dcamprof、RawTherapee・darktable の DCP の読み込み、LUT のライブラリなどは、この実装のために読んでいません）。
- 使った考え方（**DNG の仕様の文書はこの調査では取得していない。AI の知識による**）: カメラの色空間から XYZ への対応（2 つの光源の行列の逆色温度による補間、白の色度の繰り返し、ForwardMatrix、色相彩度のマップとルックテーブルの HSV の表、明度の軸の符号化）。式の書き方・正規化・HSV の表を明度 1 超・色域の外に広げる扱いは、この実装で決めたもの（04 の 2.6 節）。.cube の形（`LUT_3D_SIZE`・`DOMAIN_MIN`・`DOMAIN_MAX`・`TITLE`）も AI の知識による。
- タグの番号の照合: 開発環境の `/usr/include/x86_64-linux-gnu/tiff.h`（Ubuntu 24.04 の `libtiff-dev` 4.5.1+git230720-4ubuntu2.5。SHA-256 `ec0904a51c9392cb6a63dc5e9d563e5adb8af64517264176f370b96b6deb5200`）の `TIFFTAG_*` の値と、読むタグ（UniqueCameraModel 50708、ColorMatrix1・2 50721・50722、CalibrationIlluminant1・2 50778・50779、ProfileCalibrationSignature 50932、ProfileName 50936、ProfileHueSatMapDims 50937、ProfileHueSatMapData1・2 50938・50939、ProfileToneCurve 50940、ProfileEmbedPolicy 50941、ProfileCopyright 50942、ForwardMatrix1・2 50964・50965、ProfileLookTableDims・Data 50981・50982、ProfileHueSatMapEncoding・ProfileLookTableEncoding 51107・51108、BaselineExposureOffset 51109、DefaultBlackRender 51110、使わない 3 つ目の光源の CalibrationIlluminant3 52529・ColorMatrix3 52531・ForwardMatrix3 52532・ProfileHueSatMapData3 52537）が一致することを確かめた（2026-10-14）。libtiff のコードは取り込んでいない（番号の照合だけ）。
- 光源の番号（EXIF の LightSource）と色温度の対応（17 → 2850 K など。`dcp.rs` の `illuminant_temperature`）は AI の知識で、一次資料と照合していない（**要確認**。implementation_status の人の判断の表 No.155）。
- **Adobe のプロファイルは同梱しない**（利用者の指示）。利用者が取り込む DCP・.cube はアプリが配布するものではなく、ファイルの著作権の表示・埋め込みの方針は画面にそのまま出すだけで、このアプリは書き出したファイルに埋め込まない。

- **人の確認が必要な事項**
  1. DNG の仕様の利用の条件（特許のライセンスなど）が、仕様の考え方で自前に実装したこのアプリの配布に関係するか（AI は判断しない。人の判断の表 No.159）。
  2. 手法の出典の書き方（DNG の仕様の参照の要否と書き方）。
  3. 利用者が取り込むプロファイルの利用条件についての画面の文言（人の判断の表 No.159）。
  4. 光源の色温度の表の一次資料での確認（人の判断の表 No.155）。

### 3.13 ローカル補正（マスク）・範囲マスクの手法（一般的な手法。第三者のコードは参照していない）

**実装の状況**（2026-10-14、AI 記入。DEV-18・DEV-19、決定事項ログ No.150〜157）
- マスクの重み（線形・円形のグラデーション、ブラシのラスター、色域・輝度の範囲、部品の合成）と局所の値の面（`crates/genzo-pipeline/src/local/`）、GPU 版（`crates/genzo-gpu/src/shaders/local.wgsl`）、画面（`ui/src/develop/maskLogic.ts`・`MaskLayer.tsx`・`MaskPanel.tsx`）は自前の Rust・WGSL・TypeScript のコードです。**第三者のファイル・コードは取得も取り込みもしていません**（darktable の「drawn and parametric masks」、RawTherapee の「Local Adjustments」、Lightroom Classic の資料などは、この実装のために読んでいません）。**新しい依存（crate・npm のパッケージ）は足していません。**
- 使った考え方（**AI の知識による。文献はこの調査では取得していない**）: smoothstep の段（線形・円形・輝度の範囲の縁）、折れ線からの距離によるブラシの覆い方と「密度まで塗る・消す」の式、OKLab（`genzo-color` の既存の `oklab`。Björn Ottosson, 2020）の色度の距離による色域の選択。式の形・定数（許容量の幅・滑らかさの幅・局所の色温度の強さなど）は、この実装で決めたもの（仮置き）。
- 画面の見本の色の表示（`maskLogic.ts` の `labToCss`）は、OKLab → 線形 sRGB の逆変換に Ottosson の公表の行列の値（小数 10 桁）を使った（`genzo-color` の `oklab.rs` と同じ出典。値は AI の知識により、一次資料と照合していない。**要確認**）。表示の近似で、描画には使わない。
- 画面の構成とキー（O で重ねて表示、[ ] でブラシの大きさ）は、Lightroom Classic の操作に近づけた（AI の知識による。Adobe の資料・コードは参照していない）。

- **人の確認が必要な事項**
  1. 手法の権利（特許など）の有無と扱い（一般的な手法と考えているが、**AI は判断しない**）。
  2. 画面の構成・操作を Lightroom Classic に近づけたことの扱い（**AI は判断しない**）。
  3. 実写での画質（implementation_status の人の判断の表 No.163・164）。

### 3.14 オンラインの地図（OpenStreetMap のタイル・Nominatim）と HTTPS（LIB-21。外部のサービス）

**確認した資料**（2026-10-20（日本時間）、AI 記入。この環境からは公開ページに接続できなかったので、ページの元の GitHub のソースを `raw.githubusercontent.com` から読んだ）
- タイルの利用ポリシー（https://operations.osmfoundation.org/policies/tiles/ ）: `openstreetmap/owg-website` の `policies/tiles.md`（`permalink: /policies/tiles/`。gh-pages・main・master のブランチで同じ中身。SHA-256 `33e26f327a55107dd8a8f078facd27f3538b1621578e182b0815b6a5ec0e4041`）。
- Nominatim の利用ポリシー（https://operations.osmfoundation.org/policies/nominatim/ ）: 同じリポジトリの `policies/nominatim.md`（SHA-256 `f802152c4c878124f51d6660378d2655438fb91d3e4de3cca5a427ec33ae8576`）。
- 著作権とライセンス（https://www.openstreetmap.org/copyright ）: `openstreetmap/openstreetmap-website` の master の `config/locales/en.yml`（ページの本文の元）と `app/views/site/copyright.html.erb`。
- Nominatim の検索の API の資料: `osm-search/Nominatim` の master の `docs/api/Search.md`・`Output.md`（master なので、公開サーバーの版より先の機能が載っている可能性がある）。
- **読めなかったもの**: 公開ページそのもの、Attribution Guidelines（https://wiki.osmfoundation.org/wiki/Licence/Attribution_Guidelines 。取得できなかった）、OSMF Terms of Use（https://wiki.osmfoundation.org/wiki/Terms_of_Use 。両方のポリシーが、OSMF のすべてのサービスにかかるとして参照する。読んでいない）。2026-10-20 にも、公開ページ（operations.osmfoundation.org・wiki.osmfoundation.org）は curl・WebFetch で名前解決できず読めない（WebSearch の抜粋で、公式の Nominatim のページの LLM の条項の文がソースの写しと一致した（一部の確認）。Attribution Guidelines は、抜粋によると OSMF の理事会が 2021-06-25 に採択）。ソースのコミットの SHA と、公開ページへの反映の時期も確かめていない。タイルのサーバーの実際の応答ヘッダーも確かめていない（`tile.openstreetmap.org`・`nominatim.openstreetmap.org` にも接続できなかった）。

**実装の状況**（2026-10-20、AI 記入。決定事項ログ No.182・186〜191・193、04 の 3.16 節）
- 外部のサービスとして実行時に HTTPS で使うだけで、**配布物には何も含めない**（タイルを同梱・先読みしない）。既定で無効で、利用者が有効にしたときだけ通信する。地名の検索の公開の Nominatim は、さらに設定で ON にしたときだけ使う（既定 OFF。利用者の決定。決定事項ログ No.193）。守るようにした規則（User-Agent・キャッシュ・条件付きの要求・1 秒に 1 回（1 つのプロセスの中の間隔。ポリシーの上限はアプリの全利用者の合計）・オートコンプリートの禁止・結果のキャッシュ・提供元の切り替え）は決定事項ログ No.188・189。
- 帰属の表示: 地図の右下に「© OpenStreetMap contributors」（押すと /copyright を既定のブラウザで開く）と「地図の誤りを報告」（/fixthemap）を常に出す。設定でタイルの提供元を変えたときは、その提供元の帰属の表示（設定の `tile_attribution`）を、その前に文字だけで並べる（決定事項ログ No.191）。Leaflet の帰属の表示（Leaflet へのリンクと旗。`attributionControl` の prefix）は出していない（`attributionControl: false`）。これとは別に、Leaflet の BSD-2-Clause の第 2 項は、バイナリで配る場合に、著作権表示・条件・免責を資料などに再掲することを求めていると読める（地図の上の「Leaflet」のリンクは、この再掲には当たらないと考える）。要否と方法は AI の判断で確定せず、人が確かめる（DIST-05。未対応。4 章）。
- TLS: rustls（Rust）と ring（C とアセンブリ。アプリの実行ファイルに静的にリンクされる。1.1 節・2 章）。証明書の検証は OS（Windows の CryptoAPI・macOS の Security.framework・Linux の OS の証明書の保管場所。同梱しない）。初めに使った native-tls（Windows の SChannel・macOS の Security.framework・Linux のシステムの OpenSSL）は、2026-10-20 にやめた（1.1 節）。
- `genzo-geo` の `tls.rs`（native-tls で TLS の接続を組む部品。コミット `8cfeb81` で足し、`8ac4eda` で消した。今の木にはないが、git の履歴には残る）は、ureq 3.4.2 の `src/tls/native_tls.rs`（MIT OR Apache-2.0）を参考に、同じ流れ（構造）で書いた。両方のファイルから `//` で始まる行と空行を除き、行の前後の空白を除いて比べると、`tls.rs` の 117 行のうち、ureq のファイルにある行と同じものが 58 行（記号だけの行（`}`・`};`・`);` など）を除くと 31 行。行の順を保った一致（Python の difflib）では 47 行）。同じ行には、ureq の unversioned の API の trait のシグネチャのほか、処理の本体の行（`needs_tls` の判定、`LazyBuffers` の作成、`transmit_output`・`await_input` の本体の行など）も含まれる（2026-10-20 に、消す前の版の `tls.rs` と ureq 3.4.2 のソースで数え直した。前にこの台帳に書いた「21 行」は、数え方を書いておらず、再現できなかった）。
- ureq の著作権の表示と許諾の文（2026-10-20 追加。旧 `tls.rs` が参考にした ureq 3.4.2 は MIT OR Apache-2.0。ここには crate の `LICENSE-MIT` をそのまま写す。この表示で足りるか・どちらの条件で表示するかの法的な最終の確認は人）:

  ```text
  MIT License

  Copyright (c) 2019 Martin Algesten

  Permission is hereby granted, free of charge, to any person obtaining a copy
  of this software and associated documentation files (the "Software"), to deal
  in the Software without restriction, including without limitation the rights
  to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
  copies of the Software, and to permit persons to whom the Software is
  furnished to do so, subject to the following conditions:

  The above copyright notice and this permission notice shall be included in all
  copies or substantial portions of the Software.

  THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
  OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
  SOFTWARE.
  ```

**人の確認が必要な事項**（**法的な事項は AI の判断で確定しない**）
1. 公開ページ（タイル・Nominatim のポリシー、/copyright）と、読めなかった Attribution Guidelines・OSMF Terms of Use を読み、実装の規則と合うかを確かめる（implementation_status の人の判断の表 No.188）。
2. Nominatim のポリシーの「Usage in LLMs」の節（「LLM がこのサービスを提案するときは、ポリシーをはっきり示して制限を説明すること」「LLM が生成したコードはすべての条項を守ること」「no-code・low-code・vibe-coding のプラットフォームが、公開の API を汎用の地名の検索などとして組み込む・提供する・提案する・自動で生成することはできない」「公開の API は、アプリの開発者がよく知ったうえで意図して使うと決め、順守に直接責任を持つ場合だけ使える」）に照らして、この使い方でよいかを利用者が決める（人の判断の表 No.187）。→ 2026-10-20 に、AI（まとめ役）が原文を示して説明し、利用者が公開の Nominatim を既定 OFF・設定で ON と決めた（決定事項ログ No.193）。法的な最終の確認だけが人に残る。
3. 帰属の表示の文言・位置、ODbL の share-alike が付けた座標（カタログと書き出しの Exif に入る）に及ぶか、検索の結果の一覧の帰属（人の判断の表 No.189）。
4. タイルの画像（地図の描き方）のライセンスの扱い（読んだ資料には、地図のデータの ODbL と資料の CC BY-SA 2.0 は書かれているが、タイルの画像については確かめていない）。
5. 期限の切れたタイルを通信の失敗のときに出すこと（人の判断の表 No.190。2026-10-20 に AI が確認して続けると決めた。決定事項ログ No.188）と、User-Agent（アプリの名前とリポジトリの URL）が OSM の公開の利用統計に載りうること・連絡先（No.193）。
6. `tls.rs`（今の木にはなく、git の履歴にだけ残る）が ureq のコードを参考にしたことについて、履歴ごと main に入れる場合に著作権表示が要るか（AI は判断しない。上の行の数と、流れを ureq に合わせたことを判断の材料にする）。2026-10-20 に、ureq の著作権の表示と許諾の文を上の「実装の状況」に写した。法的な最終の確認だけが人に残る。
7. ring（Apache-2.0 AND ISC。BoringSSL・fiat-crypto のコードを含む）の GPL-3.0-or-later との両立と、2 章の一覧・配布物のライセンスの表記（DIST-05）での書き方（2 章の「ring」「証明書の検証」の行は AI の案）。FSF の一覧で Apache-2.0 と ISC は GPLv3 と両立するとされる（AI の理解。法的な最終の確認は人）。ring と Leaflet の表記は DIST-05 の全体の作業で扱う（未対応。初めての配布の前に要る）。
8. `rustls` 0.23.45 の脆弱性の通知 4 件（RUSTSEC-2026-0346〜0349）を `deny.toml` で除いたこと（このアプリの使い方では当たらないという AI の判断。理由は `deny.toml`）と、0.23.46 以上へ上げる時期（2026-10-28 以降の予定。公開から 2 週間のきまりの例外にして今すぐ上げるかも含む）。→ 2026-10-20 に AI が確かめ、除外を続けて 2026-10-28 に上げる予定（確認を予約済み。人の判断の表 No.194）。
9. タイルのポリシーの推奨（should）「HTTP/2 か HTTP/3 で多重化してダウンロードする」に沿っていないこと（`ureq` 3.4.2 は HTTP/1.1 だけ。上流へ同時に送るのはタイル専用のスレッド 2 本（`genzo-app` の `TILE_THREADS`）までで、接続は keep-alive で使い回す。決定事項ログ No.187）。沿ってよいかは人が決める（人の判断の表 No.187）。

## 4. 今後記入する対象

使うことが決まった時点で、2 章の一覧に追加します。

- lensfun（使う場合。ライブラリは LGPL-3.0、データベースは CC BY-SA 3.0 とされる。03 の 2.2 節）。2026-10-13 の DEV-14 で、利用者の決定により**使わない**ことになった（埋め込みの補正の情報と手動の補正だけ。3.9 節）
- ICC プロファイル（sRGB・Display P3 などのファイルを同梱する場合）
- Exiv2（使う場合。GPL-2.0-or-later とされる。03 の 2.1 節）
- LibRaw が依存するライブラリ（libjpeg、OpenMP のランタイム、lcms2 など）を同梱する場合
- Windows の DirectX のシェーダーコンパイラ（wgpu の Direct3D 12 のバックエンドで、DXC のライブラリを同梱するか。要確認）
- UI の npm の依存の、配布物のライセンス表記への反映（確認の方法は 1.1a 節。2026-10-12）
- OS のランタイム（Visual C++ のランタイムなど）を同梱する場合
- ONNX Runtime（将来の ML 推論）
- 配布物のライセンス表記の自動生成（02 の DIST-05）。Rust のクレートについては、生成するツールを検討する（要検討）
