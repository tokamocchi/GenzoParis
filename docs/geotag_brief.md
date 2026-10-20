# 位置情報の後付け（ジオタグ付け）の実装の依頼書

> **2026-10-20 に実装しました**（LIB-21。第 1 段階・第 2 段階とも。ブランチ `claude/inspiring-turing-jl69rk` の PR）。設計は [04 の 3.16 節](04_architecture.md)、決めたことは [決定事項ログ](decision_log.md) の No.182〜192（AI の仮決定を含む）、残りの確認は [implementation_status](implementation_status.md) の人の判断の表 No.187〜197 です。下の本文は依頼の時点のままです（4.1 節の「原文は未確認」は、実装の前に元の GitHub のソースで読みました。公開ページは未確認のままです）。

2026-10-20 の会話のまとめです。クラウドのセッションで実装するための依頼書で、**設計はまだ確定していません**（下の「決めること」）。

## 1. 背景と要望

- 利用者は α7 IV で撮っている。α7 IV に GPS は付いておらず、スマホとの位置情報連動（Creators' App）を使わないと、写真に位置情報が入らない。実際に **位置情報が入っていない写真が多い**。
- 要望: **地図で検索して、後から写真に位置情報を付けられるようにしたい**。
- 利用者の決定:
  - 「すべて実装して」: 下の第 1 段階と第 2 段階の両方を作る。
  - 地図は **無料がいいので OpenStreetMap を使う**（タイルと地名の検索）。

## 2. 今の状況（コードで確認したこと）

### 2.1 書き出しの Exif（すでに対応済み）

- 現像して書き出すと、元のファイルから読んだ撮影情報を Exif として書く（JPEG・PNG・TIFF）。組み立ては自前のコード（`crates/genzo-media/src/exif_write.rs`）。
- 書くもの: メーカー・機種・レンズ名、シャッター速度・絞り値・ISO・焦点距離、撮影日時（ミリ秒・タイムゾーン。補正していれば補正後の値）、GPS（書き出しの設定の `remove_gps` がオフのとき）、ASCII の文字だけでできている説明のメタデータ（キャプション・作成者・著作権表示。日本語は XMP に書く）。
- 書かないもの: 露出補正・ホワイトバランス・測光モード・フラッシュ・シリアル番号・メーカーノートなど（元の Exif を丸ごとは写さない）。Orientation は常に 1、Software は「GenzoParis」。
- 書き出しで Exif を作るところ: `crates/genzo-api/src/export.rs` の 636 行目付近。

  ```rust
  let exif = ExifData::from_photo_metadata(&loaded.metadata)
      .with_capture_time(&capture)
      .with_descriptive(descriptive)
      .with_keywords(keywords);
  ```

  GPS は `loaded.metadata`（元のファイルから読んだ値）の `gps` が使われる。**後から付けた位置を使うには、ここを `with_capture_time` と同じ形で置き換える必要がある**（例: `with_gps` を足す）。
- テスト: `exif_write.rs` の単体テストはタグを細かく確かめている。API の書き出しのテスト（`crates/genzo-api/tests/export.rs`）が Exif で確かめているのは GPS だけ。

### 2.2 GPS の扱い

- カタログの `asset` の表の `gps_lat`・`gps_lon`（REAL。`crates/genzo-catalog/src/sql/catalog_0001_initial.sql` の 49・50 行目）。公開ビュー `photos_v1` にも `gps_lat`・`gps_lon` の列がある。
- RAW の撮影情報（GPS を含む）は LibRaw で読む（`crates/genzo-raw/src/decode.rs` の冒頭の表）。
- UI は表示とコピーだけ（`ui/src/library/metadataLogic.ts` の `formatGps`・`gpsDecimal`、`MetadataPanel.tsx`）。**位置を編集する機能はない**。GPX のログで付ける機能もない（`gpx`・`geotag` はコードのどこにも出てこない）。
- 機能の一覧（`docs/01_feature_inventory.md` の 71 行目）に **LIB-18 地図表示**（「GPS 情報を地図上に表示し、地図から検索する」。`later`。課題「地図タイルの提供元と利用規約」）がある。今回の機能は、これと関係する新しい項目として扱うのがよさそう（番号は実装のときに決める）。

### 2.3 手本になる既存の機能

- **撮影日時の一括修正（LIB-16）**: 元の値を変えずに補正の値を別に持ち、修正の履歴を残して元に戻せる。
  - スキーマ: `crates/genzo-catalog/src/sql/catalog_0011_capture_time_edits.sql`（`capture_time_edit`・`capture_time_edit_item` の表。変える前の状態を asset ごとに記録。公開ビューは末尾に列を足すだけで、既存の列は変えない約束）。
  - コード: `crates/genzo-catalog/src/capture.rs`、`crates/genzo-model/src/capture_time.rs`、`crates/genzo-api/src/capture_time.rs`、UI の `ui/src/library/captureTimeLogic.ts`。
- **説明のメタデータの編集（LIB-15）**: `ui/src/library/MetadataEditor.tsx`・`metadataLogic.ts`、`catalog_0006_asset_iptc.sql`。
- カタログのマイグレーションは今 `catalog_0012_volumes_and_missing.sql` まで。足すなら `catalog_0013_...`。

### 2.4 ネットワークまわり

- 今のアプリは基本的に外部のネットワークにつながない。`crates/genzo-app/tauri.conf.json` の CSP は、`img-src` が `'self' data: blob: genzo: http://genzo.localhost`、`connect-src` が `ipc: http://ipc.localhost genzo: http://genzo.localhost` で、外部の origin を許していない。
- `Cargo.lock` に `reqwest` 0.13.5 と `hyper` が入っている（何かの依存として。直接の依存にしている crate はない）。TLS の機能が有効かは未確認。
- UI の依存は `react`・`react-dom`・`@tanstack/react-virtual`・`@tauri-apps/*` だけ（`ui/package.json`）。地図のライブラリはまだない。

## 3. 作るもの

### 第 1 段階: 座標の貼り付けで付ける（ネットにつながない）

1. 後から付けた位置を写真ごとにカタログに保存する。**RAW などの元のファイルは書き換えない**。
2. カメラが記録した位置と、後から付けた位置を区別して持つ。付けた位置は消せる（元に戻せる）。
3. 複数の写真を選んで、まとめて同じ位置を付けられる。
4. 座標を貼り付けて付けられる。例: `35.681236, 139.767125`（Google マップで場所を右クリックするとコピーできる形式）。
5. 書き出しの Exif・XMP、メタデータのパネルの表示、`remove_gps` の設定など、GPS を使っている既存の箇所が、付けた位置を正しく使う。

### 第 2 段階: アプリの中の地図と地名の検索（OpenStreetMap）

6. アプリの中の地図（OSM のタイル）で、クリックやピンの移動で位置を付けられる。
7. 地名を検索して場所を探せる（OSM の Nominatim）。
8. 今の写真の位置を地図に表示する。

### のちの候補（今回は作らない）

- GPX のログから、撮影日時と照らし合わせて位置を付ける（撮影日時の補正 LIB-16 と組み合わせられる）。
- LIB-18 の地図の画面（写真を地図の上に並べて、地図から探す）。

## 4. 守ること・決めること

### 4.1 OpenStreetMap の利用ポリシー（**原文は未確認。実装の前に必ず読むこと**）

- タイル: https://operations.osmfoundation.org/policies/tiles/
- Nominatim: https://operations.osmfoundation.org/policies/nominatim/

会話の中で挙げた要点（AI の知識によるもので、原文では確かめていない）:

- 帰属の表示（「© OpenStreetMap contributors」）を地図に出す。
- アプリを識別できる User-Agent（または Referer）を送る。
- Nominatim は 1 秒に 1 回まで。入力の途中で候補を出す使い方（オートコンプリート）は禁止。結果はキャッシュする。
- タイルの大量の取得・先読みは禁止。キャッシュの指示（HTTP ヘッダー）に従う。
- 配布するアプリで大勢が使う前提にはなっていないので、利用の量によっては別のタイルの提供元（OSM のデータを使う別のサーバー）に切り替えられるようにしておくとよい。

### 4.2 プライバシー

- 外部に送るのは、地図の表示範囲（タイルの要求）と検索語。送り先は OSM Foundation のサーバー。
- **オンラインの地図は、利用者が明示的に有効にするまで使わない**（設定と、初めて使うときの確認）。無効のときやオフラインのときも、第 1 段階（座標の貼り付け）は使える。

### 4.3 決めること（設計のとき）

- **ネットワークの構成**（いちばん大きい決めごと）:
  - 案 A: Rust（genzo-app）がタイルと Nominatim を HTTP で取り、タイルはディスクにキャッシュして、カスタムプロトコル（既存の `genzo:` と同じしくみ）で WebView に渡す。User-Agent を確実に設定でき、CSP は外部の origin を許さないままにできる。そのかわり HTTP クライアントと TLS の依存が増える（`deny.toml` のライセンスの検査を通るか確認が要る）。
  - 案 B: WebView から直接タイルを読む（CSP の `img-src` に OSM を足す）。作る量は少ないが、WebView から識別できる User-Agent を送れるか（Tauri のウィンドウの `userAgent` の設定など）と、キャッシュの扱いを確かめる必要がある。
  - 会話の中では **案 A を有力** と考えていた（ポリシーの順守とプライバシーの管理がしやすいため）。未確定。
- 地図の JS ライブラリ: OSM の標準のタイルはラスターなので Leaflet（1.9.4。BSD-2-Clause）が候補。自前で簡単なタイル表示を作る案もある。
- 保存の形: 撮影日時の補正と同じく、履歴の表を持って元に戻せるようにするか。公開ビュー `photos_v1` の `gps_lat`・`gps_lon` の意味（付けた位置を優先するか、列を足すか）。公開ビューは既存の列の意味を変えない約束（`docs/catalog_schema.md`）なので、末尾に列を足す形が安全。
- 座標の貼り付けで受け付ける形式（10 進の度、度分秒、`N/S/E/W` の付いた形など）と、範囲の検査（緯度 −90〜90、経度 −180〜180）。
- 動画にも位置を付けられるようにするか。

## 5. 守るべきリポジトリのきまり

- コード・コメント・資料・PR は日本語。周りのコードの書き方（コメントの密度・命名）に合わせる。
- 資料の更新の習慣: `docs/01_feature_inventory.md`（機能の一覧）、`docs/decision_log.md`（決定事項ログ）、`docs/implementation_status.md`（人の判断の表・進捗）、`docs/04_architecture.md`、`docs/catalog_schema.md`、`docs/third_party.md`（第三者の台帳）。AI が決めたことは「AI の仮決定（人の確認待ち）」と書く習慣がある。
- 新しく入れるパッケージは信頼と実績のあるものだけ。版は正確に固定し、公開から 2 週間以上たったものにする。`cargo deny` の検査を通す。
- 確認のコマンド（README の「開発」）: `cargo test --workspace`、`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、UI は `npm ci`・`npm test`・`npm run build`（`ui/` で）。

## 6. ブランチについて

- この依頼書を書いた時点のローカルの HEAD は `fix/copy-import-progress-total`（`ab3436d`。PR が開いている）で、`origin/ccr-2719c0aa-2mo58c` より 40 コミット先にある。GitHub の既定のブランチは `main`。
- この依頼書は `feat/geotag` のブランチ（`ab3436d` から作った）に置いた。実装はこのブランチから始めると、上の PR の変更も含んだ最新のコードになる。
- 手元のチェックアウトには、比較明・比較暗合成（DEV-34）の設計の未コミットの変更（`README.md` と `docs/` の 4 ファイル、`docs/composite_design.md`）があり、`docs/decision_log.md` の No.175〜178 と `docs/implementation_status.md` の No.183〜186 を使う予定になっている。**今回の機能の決定事項ログ・人の判断の表の番号は、それと重ならないようにする**。
