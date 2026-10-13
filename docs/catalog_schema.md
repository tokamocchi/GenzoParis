# カタログのスキーマ（catalog.db・thumbs.db）

GenzoParis のカタログ（SQLite のデータベース）の構造と、外部のツールから SQL で読む方法です（01 の ORG-07、02 の COMPAT-04）。カタログは特別な形式ではなく、普通の SQLite 3 のファイルなので、SQLite の CLI・DB Browser for SQLite・Python などで読めます。

- この文書は、実際のスキーマ（`crates/genzo-catalog/src/sql/` のマイグレーション）と食い違わないことをテストで確かめています（`crates/genzo-catalog/src/tests/schema_doc.rs`。すべての表・ビューとその列・型、索引・トリガーがこの文書に載っているか、スキーマの版が合っているか）。
- 外部から読むときは、まず **公開ビュー**（3 章。`photos_v1`・`photo_keywords_v1`・`photo_collections_v1`・`photo_stacks_v1`・`photo_files_v1`）を使ってください。表の構造は機能の追加で変わりますが、公開ビューの列は変えない約束です。
- **カタログには書き込まないでください**（読み取り専用で開く。2 章）。アプリは、ファイルの状態・検索の索引・現像設定のハッシュ・ファイル操作の記録などの整合を保ちながら書き込んでいます。外部から書き換えると、アプリが誤動作したり、写真の情報を失ったりする可能性があります。

## 1. ファイルと版

| ファイル | 場所 | 内容 |
|---|---|---|
| `catalog.db` | 利用者が選んだ場所（既定はピクチャの `GenzoParis/catalog.db`） | カタログの本体（この文書の 3 章・4 章）。名前は任意 |
| `catalog.db-wal`・`catalog.db-shm` | カタログと同じフォルダ | SQLite の WAL（先行書き込みログ）と共有メモリ。アプリが開いている間にでき、**最近の変更は `-wal` の中にある**（2.2 節） |
| `catalog.db.lock` | カタログと同じフォルダ | アプリ・CLI が同時に同じカタログを開かないためのロックファイル（中身は使わない） |
| `<カタログ名>-data/thumbs.db` | データのフォルダ（既定はカタログの隣の `<カタログ名>-data`） | サムネイル（L0）と標準プレビュー（L1）の索引（5 章）。**作り直せるキャッシュ**で、カタログのバックアップには含めない |
| `<カタログ名>-data/previews/` | 同上（設定で変えられる） | 標準プレビューの JPEG（`previews/ab/cd/<キー>.jpg`） |
| `<カタログ名>-data/backups/` | 同上（設定で変えられる） | カタログの自動・手動のバックアップと、スキーマの移行の前のバックアップ（`catalog.v<版>-before-migration-<時刻>.db`） |

スキーマの版は `PRAGMA user_version`、GenzoParis のファイルであることの印は `PRAGMA application_id` に入っています。

- catalog.db のスキーマの版: **12**（`application_id` は 1197097812 = 16 進数の 0x475A4354 = ASCII の `GZCT`）
- thumbs.db のスキーマの版: **2**（`application_id` は 1197102152 = 16 進数の 0x475A5448 = ASCII の `GZTH`）

アプリは、古い版のカタログを開くと、自動でバックアップを作ってから新しい版に移行します。新しい版のカタログは、古いアプリでは開けません（壊さないため）。

| 版 | 変更 | マイグレーション |
|---|---|---|
| 1 | 初期スキーマ | `catalog_0001_initial.sql` |
| 2 | 外部キーの子の列の索引（`variant_history_pos`・`stack_top_variant`） | `catalog_0002_fk_child_indexes.sql` |
| 3 | ファイル名（比較用のキー）の索引（`file_name_key`） | `catalog_0003_file_name_key_index.sql` |
| 4 | 撮影日時の出どころ（`asset.tz_source`）に `utc_only` を足す（`asset` の作り直し） | `catalog_0004_tz_source_utc_only.sql` |
| 5 | 外部から読むための公開ビュー（`photos_v1`・`photo_keywords_v1`） | `catalog_0005_public_views.sql` |
| 6 | 説明のメタデータ（タイトル・作成者・著作権など。`asset_iptc`）と、`photos_v1` の列 `title`・`creator`・`copyright` | `catalog_0006_asset_iptc.sql` |
| 7 | キーワードの書き出しの印（`keyword.include_on_export`）、コレクションの名前の比較用のキー（`collection.name_key`）と重複の禁止（`collection_unique_name`）、公開ビュー `photo_collections_v1` | `catalog_0007_collections.sql`（`name_key` の値と索引は Rust の処理。6 章） |
| 8 | フォルダ・キーワード・コレクションの ID を使い回さない（`folder`・`keyword`・`collection` を `AUTOINCREMENT` で作り直す。列・索引は変えない） | `catalog_0008_stable_ids.sql`（連番の初めの値は Rust の処理。6 章） |
| 9 | スタック（LIB-12）を使い始める（`stack` を `AUTOINCREMENT` で作り直し、折りたたみの状態 `collapsed` を足す。トリガー `stack_member_after_delete`、公開ビュー `photo_stacks_v1`） | `catalog_0009_stacks.sql`（既存の行の位置の付け直しなどは Rust の処理。6 章） |
| 10 | 内容による同一性（ORG-03・IMP-04・IMP-06）: クイックハッシュ・全体のハッシュの索引（`file_quick_hash`・`file_full_hash`）、公開ビュー `photo_files_v1`。既存のファイルの全体のハッシュは移行では計算しない（アプリがバックグラウンドで計算する） | `catalog_0010_content_identity.sql` |
| 11 | 撮影日時の一括修正（LIB-16）: カメラの本体のシリアル番号（`asset.camera_serial`）と UTC の計算に使ったオフセット（`asset.tz_basis`）の列、修正の履歴（`capture_time_edit`・`capture_time_edit_item`）、`photos_v1` の列 `camera_serial`。既存の行の 2 つの列は NULL のまま（撮影日時は変わらない） | `catalog_0011_capture_time_edits.sql` |
| 12 | 外付けドライブの識別（FILE-03）・見つからない写真（FILE-04）: ボリュームのファイルシステムの種類（`volume.fs_type`）と最後につながった日時（`volume.last_seen_at`）、フォルダの状態（`folder.status`・`folder.status_at`）、状態が `ok` でないファイルの部分索引（`file_not_ok`）。版 11 までのパスから作ったボリュームの ID（`path:`）は移行では変えない（アプリが開いたときに、見えるフォルダから OS のボリューム ID へ移す。2.4 節） | `catalog_0012_volumes_and_missing.sql` |

## 2. 読み方

### 2.1 読み取り専用で開く

SQLite の CLI（`sqlite3`）:

```sh
# 読み取り専用で開く（-readonly）。Windows ではパスを "C:\Users\...\catalog.db" のように書く。
sqlite3 -readonly ~/Pictures/GenzoParis/catalog.db
sqlite> PRAGMA user_version;            -- スキーマの版（12）
sqlite> .mode box
sqlite> SELECT variant_id, file_name, captured_at_local, rating
   ...> FROM photos_v1 WHERE rating >= 4 ORDER BY captured_at_utc;
```

Python（標準ライブラリの `sqlite3`。URI の `mode=ro` で読み取り専用）:

```python
import sqlite3
from pathlib import Path

path = Path.home() / "Pictures" / "GenzoParis" / "catalog.db"
con = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
for row in con.execute(
    "SELECT file_name, captured_at_local, camera FROM photos_v1 "
    "WHERE kind = 'photo' ORDER BY captured_at_utc"
):
    print(row)
```

DB Browser for SQLite では、「データベースを開く」の代わりに「読み取り専用でデータベースを開く」を使います。

検証用の CLI（`genzo`）でも構造を表示できます（`genzo catalog schema`。README の CLI の節）。アプリでは「設定」→「カタログ」の「カタログの構造」で見られます。

### 2.2 アプリが起動している間の注意（WAL）

- カタログは WAL モードです。アプリが開いている間も、別の接続から読めます（読む側はアプリを待たせず、アプリが書いても読んでいる途中の結果は変わりません）。
- 最近の変更は `catalog.db-wal` に入っていて、`catalog.db` だけをコピーすると最近の変更が欠けることがあります。コピーするときは、アプリのバックアップ（「設定」→「バックアップ」）を使うか、SQLite の機能でコピーしてください（例: `sqlite3 -readonly catalog.db ".backup copy.db"`）。
- 読み取り専用の接続でも、`-wal`・`-shm` のファイルが既にあるか、作れる必要があります（SQLite 3.22 以降。読み取り専用のフォルダに置いたカタログを、アプリを閉じた後に読むときは、URI に `immutable=1` を付ける方法もありますが、その間にほかのプログラムが書き込まないことが前提です）。
- 長い読み取りのトランザクションを開いたままにすると、WAL のチェックポイント（`-wal` の内容を本体に戻す処理）が進まず、`-wal` が大きくなります。読み終えたら接続を閉じてください。
- 書き込みはしないでください（冒頭）。アプリと同時に書くと、アプリの書き込みが待たされたり失敗したりします。

### 2.3 時刻の形式

| 列 | 形式 | 例 |
|---|---|---|
| `*_at`（`created_at`・`updated_at`・`status_at`・`captured_at_utc` など）、`app_state` の日時 | UTC の ISO 8601（RFC 3339）。ミリ秒まで、末尾は `Z`。桁数が一定なので、文字列の比較で時刻順になる | `2024-05-01T03:00:00.000Z` |
| `asset.captured_at_raw` | ファイルに記録されていた元の文字列（EXIF の `2024:05:01 12:00:00`、動画の `2024-05-01T03:00:00.000000Z` など） | `2024:05:01 12:00:00` |
| `asset.captured_offset`・`asset.tz_assumed`・`asset.tz_basis` | UTC からのオフセット（`±HH:MM`）。`captured_offset` は UTC の印だけのとき `Z` | `+09:00` |
| `file.mtime` | ファイルの更新日時。UNIX 時刻（1970-01-01T00:00:00Z）からのナノ秒の整数 | `1714532400000000000` |
| `photos_v1.captured_at_local` | 補正後の UTC を表示のオフセット（`tz_assumed`）で表した現地時刻（オフセットなし。秒の小数 3 桁） | `2024-05-01T12:00:00.000` |

撮影日時（`asset`）は、元の値と推定に使った情報を分けて持ちます（04 の 3.1 節）。利用者の修正（時計のずれの補正・タイムゾーンの指定。LIB-16）は `time_correction_s`・`tz_source = 'user_set'`・`tz_assumed`・`tz_basis` に入り、元の値（`captured_at_raw`・`captured_offset`）は変えません。UTC は `captured_at_raw` の時刻を `tz_basis`（NULL なら `tz_assumed`。UTC の印だけの値で `tz_basis` が NULL なら UTC）のオフセットで読み、`time_correction_s` を足したものです。表示する撮影日時は `captured_at_utc`（補正後の UTC）を `tz_assumed` のオフセットで表したものです（`photos_v1.captured_at_local`）。撮影日時が分からない写真は `captured_at_utc` が NULL です（並べ替えでは最後に並べる）。SQLite の日時の関数で現地時刻を求めるには `strftime('%Y-%m-%dT%H:%M:%f', captured_at_utc, tz_assumed)` と書きます（`tz_assumed` の `±HH:MM` を時刻の修飾子として足す。アプリが同梱する SQLite と、SQLite 3.45 で動くことを確かめています。古い SQLite で使うときは、結果を確かめてください）。

### 2.4 ID の関係

```text
volume ─< folder ─< file >─ asset ─< variant ─< history_entry
                                 │           ├─< snapshot
                                 │           ├─< variant_keyword >─ keyword（階層）
                                 │           ├─< collection_member >─ collection（階層）
                                 │           └─  stack_member >─ stack
                                 ├─  video_meta（動画だけ）
                                 └─  asset_text（検索用の文字列）─ asset_fts（全文検索の索引）
```

- **asset** は写真・動画の実体（1 回の撮影）です。RAW と JPEG のペアは 1 つの asset に 2 つの **file**（`role` が `primary` と `sidecar_jpeg`）として属します。
- **variant** は画面のグリッドに並ぶ単位で、asset ごとにマスター（`is_master = 1`）が 1 つと、仮想コピー（`is_master = 0`）が 0 個以上あります。評価・フラグ・カラーラベル・現像設定・キーワードは variant に付きます。キャプションと説明のメタデータ（タイトル・作成者・著作権など。`asset_iptc`）は asset に付きます（仮想コピーも同じ値）。
- 画面・CLI の「写真の ID」は variant の ID（`variant.id`）です。
- `asset.id`・`variant.id`・`file.id`・`history_entry.id`・`snapshot.id`・`file_op.id` は、削除した後も同じ値を使い回しません（`AUTOINCREMENT`。サムネイル DB などの外部のキャッシュが ID で参照するため）。`folder.id`・`keyword.id`・`collection.id` も、版 8 から使い回しません（スマートコレクションの条件や画面のターゲットコレクションが ID で参照し、削除したものを参照する条件が、後から作ったものに付け替わらないようにするため。版 7 までに削除したものも、条件が参照している ID は使いません）。`stack.id` も版 9 から使い回しません（画面・CLI がスタックを ID で指すため）。`volume` の ID は使い回すことがあります。
- 外部キーは、asset を消すと file・variant・履歴・キーワードの付与などが消える（`ON DELETE CASCADE`）ように定義しています。キーワード・コレクションそのものは残ります。外部キーの確認は接続ごとの設定（`PRAGMA foreign_keys`）で、アプリはすべての接続で有効にしています。
- 元ファイルの絶対パスは「ボリュームのマウント先（`volume.last_mount_path`）＋ フォルダ（`folder.rel_path`。区切りは `/`）＋ ファイル名（`file.name`）」です。版 12 から（FILE-03）、ボリュームは OS のボリューム ID で識別し（`volume.uuid` が `uuid:<ファイルシステムの UUID>`（Linux・macOS）・`winvol:<ファイルシステム>:<シリアル番号>`（Windows）・`net:<共有の名前>`（ネットワークのボリューム）の形）、ドライブ文字やマウント先が変わると、アプリが `last_mount_path` を新しいマウント先にします。ボリューム ID を取れないファイルシステム（コンテナの中・一時的なファイルシステムなど）と版 11 までに登録したものは、パスの先頭のマウント先から作った `path:/`・`path:C:\` の形です。**つながっていないボリュームの `last_mount_path` には、別のドライブがつながっていることがあります**（外部のツールでそのパスを開くと、別のファイルを読みうる）。

### 2.5 文字列の正規化と JSON

- `*_key` の列（`folder.rel_path_key`・`file.name_key`）は比較用で、Unicode の NFC に正規化して小文字にしたものです（Windows と macOS の既定のファイルシステムに合わせて、大文字・小文字を区別しない）。表示には元の列（`rel_path`・`name`）を使います。`keyword.name_key`・`collection.name_key` は検索・名前の重複の確かめ用で、NFKC に正規化して小文字にしたものです（全角・半角、大文字・小文字の違う同じ名前を、同じ親の下に作らない）。
- `collection.smart_rule_json` はスマートコレクションの条件の JSON（genzo-model の `SmartRule`。版付き。04 の 3.9 節）です: `{"version": 1, "match": "all", "rules": [{"type": "rating", "op": "gte", "value": 3}, {"type": "group", "match": "any", "rules": [...]}]}`。条件の種類は `rating`・`flag`・`color_label`・`keyword`・`capture_time`・`camera`・`lens`・`kind`・`text`・`folder`・`collection`・`group`（入れ子）です。
- `asset_text.text_norm` は、ファイル名・キャプションと、説明のメタデータのうち検索の対象の項目（タイトル・見出し・作成者・著作権表示・撮影場所の 4 項目・クレジット・提供元。連絡先・URL・著作権の状態・使用条件・指示は含めない）を NFKC に正規化して小文字にし、改行でつないだ検索用の文字列です。3 文字以上の語は全文検索の索引（`asset_fts`。FTS5 の trigram）で、1〜2 文字の語は `text_norm` への `LIKE` で探します（04 の 3.6 節）。
- `variant.develop_json`・`history_entry.develop_json`・`snapshot.develop_json` は現像設定の JSON（genzo-model の `DevelopSettings`。04 の 2.5 節）です。**NULL は「その処理バージョン（`process_version`）の既定の設定」**です（未調整の写真の容量を抑えるため）。JSON の項目は処理バージョンとともに増えます。
- `file_op.payload_json` はファイル操作の対象（`{"entries": [{"file_id", "asset_id", "from": {"folder_id", "name", "absolute_path"}, "to": …}]}`）です。版 12 から、別のボリュームへの移動（コピーして照合し、元をゴミ箱へ移す）の項目には `"method": "copy"` が付き、フォルダの移動・名前の変更は `"entries": []` と `"folder": {"folder_id", "from_rel", "from_path", "to_volume", "to_rel", "to_path"}` です。
- 真偽値は 0 / 1 の整数です（`variant.is_master`・`stack.collapsed`・`photos_v1.is_virtual_copy`・`photo_stacks_v1.is_top`）。
- **スタック**（LIB-12。版 9 から）: 1 つの variant は高々 1 つのスタックに入ります。スタックの中の順は `stack_member` の `(position, variant_id)` で、一番上の写真（`stack.top_variant_id`）は常に最初です。写真が 1 枚以下になったスタックはトリガー（`stack_member_after_delete`）が消すので、スタックは常に 2 枚以上です。仮想コピーは、作ると元の写真と同じスタックに入ります（04 の 3.10 節）。

### 2.6 例

```sql
-- 評価 4 以上の写真を撮影日時の順に（仮想コピーを除く）。
SELECT variant_id, file_name, captured_at_local, camera, lens, rating
FROM photos_v1
WHERE kind = 'photo' AND is_virtual_copy = 0 AND rating >= 4
ORDER BY captured_at_utc;

-- 元ファイルの絶対パス（Linux・macOS の例。Windows ではマウント先が "C:\" の形）。
SELECT variant_id,
       rtrim(volume_mount_path, '/\') || '/' ||
       CASE folder_path WHEN '' THEN '' ELSE folder_path || '/' END || file_name AS path
FROM photos_v1;

-- キーワード「場所 > 京都」とその下のキーワードが付いた写真。
SELECT DISTINCT p.variant_id, p.file_name
FROM photos_v1 AS p
JOIN photo_keywords_v1 AS k ON k.variant_id = p.variant_id
WHERE k.keyword_path = '場所 > 京都' OR k.keyword_path LIKE '場所 > 京都 > %';

-- コレクションセット「旅行」の中の手動のコレクションに入れた写真（スマートコレクションの結果は、条件を
-- アプリが検索のたびに SQL にするため、ビューにはない）。
SELECT DISTINCT p.variant_id, p.file_name, c.collection_path
FROM photos_v1 AS p
JOIN photo_collections_v1 AS c ON c.variant_id = p.variant_id
WHERE c.collection_path LIKE '旅行 > %';

-- スタックごとの写真の数と一番上の写真のファイル名（版 9 から）。
SELECT s.stack_id, s.stack_size, p.file_name
FROM photo_stacks_v1 AS s
JOIN photos_v1 AS p ON p.variant_id = s.variant_id
WHERE s.is_top = 1
ORDER BY p.captured_at_utc;

-- 月ごと・カメラごとの枚数（表示のオフセットでの月）。
SELECT substr(captured_at_local, 1, 7) AS month, camera, count(*) AS photos
FROM photos_v1
WHERE kind = 'photo' AND is_virtual_copy = 0 AND captured_at_local IS NOT NULL
GROUP BY month, camera ORDER BY month;

-- 見つからない元ファイル（ドライブが外れている・アプリの外で移動・削除した）。
SELECT variant_id, volume_mount_path, folder_path, file_name FROM photos_v1 WHERE file_status = 'missing';

-- RAW と同時記録の JPEG を 1 枚にまとめた写真の、JPEG のファイル（版 10 から）。
SELECT master_variant_id, folder_path, file_name FROM photo_files_v1 WHERE role = 'sidecar_jpeg';

-- 内容（ファイル全体の BLAKE3）が同じファイルの組（計算済みのファイルの中で。版 10 から）。
SELECT full_hash, count(*) AS files, group_concat(folder_path || '/' || file_name, ' | ') AS paths
FROM photo_files_v1 WHERE full_hash IS NOT NULL
GROUP BY full_hash HAVING count(*) > 1;
```

## 3. 公開ビュー

外部から読むための安定した入口です（スキーマの版 5 から。版 6・版 11 で `photos_v1` に列を足した。版 7 で `photo_collections_v1`、版 9 で `photo_stacks_v1`、版 10 で `photo_files_v1` を足した）。

- **約束**: 名前の末尾の版（`_v1`）が同じ間は、列の名前・意味・値の形を変えず、列を減らしません。変える必要があれば、別の名前（`_v2`）のビューを足します。列を足すことはあるので、`SELECT *` ではなく列の名前を指定して読んでください。行の並び順は決めていないので、`ORDER BY` を指定してください。
- ビューは読むためのものです（書き込めません）。
- アプリの機能の追加（v1）で表が変わっても、ビューの列は同じ意味のまま保ちます（スキーマの移行の間は、ビューを外してから表を変え、同じ定義で作り直す。6 章）。

<!-- schema:catalog -->

### `photos_v1`

写真・動画の一覧です。1 行が 1 つの variant（グリッドの 1 枚。仮想コピーも 1 行）です。元ファイルの情報は主なファイル（`file.role = 'primary'`）のものです。

| 列 | 型 | 意味 |
|---|---|---|
| `variant_id` | INTEGER | variant の ID（画面・CLI の写真の ID） |
| `asset_id` | INTEGER | asset の ID（同じ撮影の仮想コピーは同じ値） |
| `kind` | TEXT | `photo`（写真）/ `video`（動画） |
| `is_virtual_copy` | （式） | 仮想コピーなら 1、マスターなら 0 |
| `copy_name` | TEXT | 仮想コピーの名前（マスターは NULL） |
| `rating` | INTEGER | 評価（0〜5。0 は評価なし） |
| `flag` | INTEGER | フラグ（1 = 採用、0 = なし、-1 = 除外） |
| `color_label` | TEXT | カラーラベル（`red`・`yellow`・`green`・`blue`・`purple`。なければ NULL） |
| `captured_at_utc` | TEXT | 撮影日時（補正後の UTC。2.3 節。不明なら NULL） |
| `captured_at_local` | （式） | 撮影日時の現地時刻（`captured_at_utc` を `utc_offset` で表したもの。オフセットは付けない。不明なら NULL） |
| `utc_offset` | TEXT | 表示のオフセット（`asset.tz_assumed`。`+09:00` の形） |
| `camera` | TEXT | カメラ（メーカー名と機種名。機種名がメーカー名で始まれば機種名だけ） |
| `lens` | TEXT | レンズ |
| `iso` | INTEGER | ISO 感度 |
| `aperture` | REAL | 絞り値（F 値） |
| `shutter_s` | REAL | シャッター速度（秒） |
| `focal_mm` | REAL | 焦点距離（mm） |
| `width` | INTEGER | 画像の幅（画素。向き（`orientation`）を適用する前） |
| `height` | INTEGER | 画像の高さ（画素。向きを適用する前） |
| `orientation` | INTEGER | 向き（EXIF の Orientation の 1〜8） |
| `gps_lat` | REAL | 緯度（度。北が正） |
| `gps_lon` | REAL | 経度（度。東が正） |
| `caption` | TEXT | キャプション（asset に付く。仮想コピーも同じ値） |
| `file_name` | TEXT | 元ファイルの名前 |
| `folder_path` | TEXT | 元ファイルのフォルダ（ボリュームの中の相対パス。区切りは `/`。ボリュームの最上位は空文字列） |
| `volume_label` | TEXT | ボリュームの表示名 |
| `volume_mount_path` | TEXT | ボリュームの最後に確認したマウント先（2.4 節） |
| `file_size` | INTEGER | 元ファイルの大きさ（バイト） |
| `file_status` | TEXT | 元ファイルの状態（`ok` / `missing`（見つからない）/ `error`（読めない・壊れている可能性）） |
| `duration_s` | REAL | 動画の長さ（秒。写真は NULL） |
| `fps` | REAL | 動画のフレームレート（写真は NULL） |
| `video_codec` | TEXT | 動画のコーデック（`h264`・`hevc` など。写真は NULL） |
| `process_version` | INTEGER | 現像設定の処理バージョン |
| `develop_json` | TEXT | 現像設定の JSON（NULL は処理バージョンの既定の設定。2.5 節） |
| `imported_at` | TEXT | 取り込んだ日時（UTC） |
| `updated_at` | TEXT | variant を最後に変えた日時（UTC） |
| `title` | TEXT | タイトル（`asset_iptc.title`。版 6 から。なければ NULL） |
| `creator` | TEXT | 作成者（`asset_iptc.creator`。版 6 から） |
| `copyright` | TEXT | 著作権表示（`asset_iptc.copyright`。版 6 から） |
| `camera_serial` | TEXT | カメラの本体のシリアル番号（`asset.camera_serial`。版 11 から。記録しないカメラ・版 11 より前に取り込んだものは NULL） |

そのほかの説明のメタデータ（見出し・連絡先・撮影場所など）は、表 `asset_iptc`（4 章）を `asset_id` で結んで読みます（表の構造は変わることがあります）。

### `photo_keywords_v1`

写真に付けたキーワードです。1 行が「1 つの variant と 1 つのキーワード」の組です。

| 列 | 型 | 意味 |
|---|---|---|
| `variant_id` | INTEGER | variant の ID |
| `keyword_id` | INTEGER | キーワードの ID |
| `keyword` | TEXT | キーワードの名前（階層の最後） |
| `keyword_path` | TEXT | 最上位からの階層を「 > 」でつないだもの（`場所 > 京都`）。親子が循環した壊れたキーワードは出ない（階層は 64 段までたどる） |

### `photo_collections_v1`

手動のコレクションに入れた写真です（版 7 から）。1 行が「1 つの variant と 1 つのコレクション」の組です。スマートコレクション（条件で決まる）とコレクションセット（入れ物）の行はありません。

| 列 | 型 | 意味 |
|---|---|---|
| `variant_id` | INTEGER | variant の ID |
| `collection_id` | INTEGER | コレクションの ID |
| `collection` | TEXT | コレクションの名前 |
| `collection_path` | TEXT | 最上位のコレクションセットからの階層を「 > 」でつないだもの（`旅行 > 京都 2024`）。親子が循環した壊れたコレクションは出ない（階層は 64 段までたどる） |

### `photo_stacks_v1`

スタック（LIB-12）に入っている写真です（版 9 から）。1 行が 1 つの variant です。スタックに入っていない写真の行はありません。

| 列 | 型 | 意味 |
|---|---|---|
| `variant_id` | INTEGER | variant の ID |
| `stack_id` | INTEGER | スタックの ID |
| `position` | （式） | スタックの中の順（1 から。1 が一番上） |
| `is_top` | （式） | 一番上の写真なら 1、そうでなければ 0 |
| `stack_size` | （式） | スタックの写真の数（2 以上） |

### `photo_files_v1`

元ファイルの一覧です（版 10 から）。1 行が 1 つのファイルです。RAW と同時記録の JPEG を 1 枚の写真として扱っている場合（IMP-06）は、RAW（`role = 'primary'`）と JPEG（`role = 'sidecar_jpeg'`）が別の行で、同じ `asset_id`・`master_variant_id` を持ちます。`photos_v1` の元ファイルの列は主なファイルだけなので、付属の JPEG と内容のハッシュはこちらで読みます。

| 列 | 型 | 意味 |
|---|---|---|
| `file_id` | INTEGER | ファイルの ID |
| `asset_id` | INTEGER | asset の ID |
| `master_variant_id` | INTEGER | asset のマスターの variant の ID（`photos_v1` の `variant_id` と結べる） |
| `role` | TEXT | `primary`（主なファイル）/ `sidecar_jpeg`（RAW と同時記録の JPEG） |
| `file_name` | TEXT | ファイルの名前 |
| `folder_path` | TEXT | フォルダ（ボリュームの中の相対パス。区切りは `/`。ボリュームの最上位は空文字列） |
| `volume_label` | TEXT | ボリュームの表示名 |
| `volume_mount_path` | TEXT | ボリュームの最後に確認したマウント先（2.4 節） |
| `file_size` | INTEGER | 大きさ（バイト） |
| `file_status` | TEXT | 状態（`ok` / `missing`（見つからない）/ `error`（読めない）） |
| `full_hash` | TEXT | ファイル全体の BLAKE3（16 進数の小文字 64 文字。`b3sum` と同じ値）。まだ計算していなければ NULL（アプリがバックグラウンドで計算する。ORG-03）。同じ値のファイルは同じ内容 |

## 4. カタログの表（catalog.db）

通常の表は `STRICT`（宣言した型以外の値を入れない）です。列の表の「NULL」は、NULL を入れられるかです（「不可」は `NOT NULL` か主キー）。

### `volume`

ボリューム（ドライブ）。ファイルの場所は「ボリューム ＋ ボリュームの中の相対パス」で持ちます（04 の 3.3 節）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー |
| `uuid` | TEXT | 不可 | | ボリュームの識別子（`UNIQUE`。OS のボリューム ID（`uuid:`・`winvol:`・`net:`）か、パスの先頭から作った `path:/`・`path:C:\` の形。2.4 節） |
| `label` | TEXT | 可 | | 表示名（ボリュームのラベル） |
| `last_mount_path` | TEXT | 可 | | 最後に確認したマウント先（`/`・`/media/me/SD`・`E:\` など） |
| `fs_type` | TEXT | 可 | | ファイルシステムの種類（`ext4`・`apfs`・`NTFS`・`exFAT` など。表示用。版 12 から） |
| `last_seen_at` | TEXT | 可 | | つながっていることを最後に確かめた日時（UTC。版 12 から） |

### `folder`

フォルダ。ボリュームの最上位は `rel_path = ''` で、親を持ちません（`CHECK ((parent_id IS NULL) = (rel_path = ''))`）。同じボリュームで比較用のキーが同じフォルダは 1 つだけ（`UNIQUE (volume_id, rel_path_key)`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`。版 8 から） |
| `volume_id` | INTEGER | 不可 | | ボリューム（→ `volume.id`） |
| `parent_id` | INTEGER | 可 | | 親のフォルダ（→ `folder.id`。最上位は NULL） |
| `rel_path` | TEXT | 不可 | | ボリュームの中の相対パス（表示用。区切りは `/`） |
| `rel_path_key` | TEXT | 不可 | | 比較用のキー（NFC ＋ 小文字。2.5 節） |
| `status` | TEXT | 不可 | `'ok'` | `ok` / `missing`（ボリュームはつながっているのに、フォルダが見つからない）（`CHECK`。版 12 から。アプリのバックグラウンドの確認で付く） |
| `status_at` | TEXT | 可 | | 状態を最後に記録した日時（UTC。版 12 から） |

### `asset`

写真・動画の実体。撮影日時は元の値と推定に使った情報を分けて持ちます（04 の 3.1 節。2.3 節）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`） |
| `kind` | TEXT | 不可 | | `photo` / `video`（`CHECK`） |
| `captured_at_raw` | TEXT | 可 | | ファイルに記録されていた日時の元の文字列 |
| `captured_offset` | TEXT | 可 | | ファイルに記録されていたオフセット（`+09:00`。UTC の印だけなら `Z`。なければ NULL） |
| `tz_source` | TEXT | 不可 | `'user_default'` | UTC の推定に使ったオフセットの出どころ: `exif`（ファイルの記録）/ `utc_only`（UTC の印だけ。表示のオフセットは既定のタイムゾーン）/ `user_default`（記録がなく既定のタイムゾーン）/ `user_set`（利用者が指定）（`CHECK`） |
| `tz_assumed` | TEXT | 可 | | 表示に使うオフセット（`+09:00`。`tz_basis` がなければ UTC の推定にも使う。日時がなければ NULL） |
| `time_correction_s` | INTEGER | 不可 | `0` | 時計のずれの補正（秒。UTC に加える） |
| `captured_at_utc` | TEXT | 可 | | 補正後の UTC（2.3 節の形式。不明なら NULL） |
| `camera` | TEXT | 可 | | カメラ（`photos_v1.camera` と同じ） |
| `lens` | TEXT | 可 | | レンズ |
| `iso` | INTEGER | 可 | | ISO 感度 |
| `aperture` | REAL | 可 | | 絞り値（F 値） |
| `shutter` | REAL | 可 | | シャッター速度（秒） |
| `focal` | REAL | 可 | | 焦点距離（mm） |
| `width` | INTEGER | 可 | | 幅（画素。向きを適用する前） |
| `height` | INTEGER | 可 | | 高さ（画素。向きを適用する前） |
| `orientation` | INTEGER | 不可 | `1` | 向き（EXIF の 1〜8。`CHECK`） |
| `gps_lat` | REAL | 可 | | 緯度（度） |
| `gps_lon` | REAL | 可 | | 経度（度） |
| `caption` | TEXT | 可 | | キャプション |
| `created_at` | TEXT | 不可 | | 登録した日時（UTC） |
| `camera_serial` | TEXT | 可 | | カメラの本体のシリアル番号（Exif の BodySerialNumber。機材ごとの時刻の補正に機種と組で使う。版 11 から） |
| `tz_basis` | TEXT | 可 | | UTC の計算に使ったオフセット（`+09:00`）。表示のオフセット（`tz_assumed`）と違う場合（撮影した瞬間を変えずに別のタイムゾーンで表示する）と、UTC の印だけの値を現地時刻として読む場合だけ持つ。NULL なら `tz_assumed`（UTC の印だけの値は UTC）で計算する（版 11 から。2.3 節） |

### `file`

ファイル。同じフォルダで比較用の名前が同じファイルは 1 つだけ（`UNIQUE (folder_id, name_key)`。登録の繰り返しで増えない）、asset ごとに主なファイルは 1 つだけ（`file_one_primary`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`） |
| `asset_id` | INTEGER | 不可 | | asset（→ `asset.id`。asset を消すと消える） |
| `folder_id` | INTEGER | 不可 | | フォルダ（→ `folder.id`） |
| `name` | TEXT | 不可 | | ファイル名（表示用） |
| `name_key` | TEXT | 不可 | | 比較用のキー（NFC ＋ 小文字） |
| `role` | TEXT | 不可 | | `primary`（主なファイル）/ `sidecar_jpeg`（RAW と組の JPEG）（`CHECK`） |
| `size` | INTEGER | 不可 | | 大きさ（バイト。0 以上） |
| `mtime` | INTEGER | 不可 | | 更新日時（UNIX 時刻からのナノ秒） |
| `quick_hash` | TEXT | 不可 | | クイックハッシュ（用途の文字列 ＋ ファイルの大きさ ＋ 先頭と末尾の 64 KB の BLAKE3。16 進数。変化の検知用で、`b3sum` の値とは違う） |
| `full_hash` | TEXT | 可 | | ファイル全体の BLAKE3（16 進数。`b3sum` と同じ値）。アプリがバックグラウンドで順に計算し、取り込みの重複の確認でも計算する（版 10 から。ORG-03・IMP-04）。内容の変化を検知すると消え（NULL）、記録した後に別の値で上書きしない |
| `revision` | INTEGER | 不可 | `1` | 内容の変化を検知するたびに 1 増える（1 以上） |
| `status` | TEXT | 不可 | `'ok'` | `ok` / `missing`（見つからない）/ `error`（読めない）（`CHECK`） |
| `status_reason` | TEXT | 可 | | `missing`・`error` の理由 |
| `status_at` | TEXT | 可 | | 状態を最後に記録した日時（UTC） |

### `variant`

現像のバリエーション（マスター ＋ 仮想コピー）。グリッドに並ぶ単位です。asset ごとにマスターは 1 つだけ（`variant_one_master`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`。写真の ID） |
| `asset_id` | INTEGER | 不可 | | asset（→ `asset.id`。asset を消すと消える） |
| `is_master` | INTEGER | 不可 | | マスターなら 1、仮想コピーなら 0（`CHECK`） |
| `name` | TEXT | 可 | | 仮想コピーの名前（マスターは NULL） |
| `rating` | INTEGER | 不可 | `0` | 評価（0〜5。`CHECK`） |
| `flag` | INTEGER | 不可 | `0` | フラグ（-1 / 0 / 1。`CHECK`） |
| `color_label` | TEXT | 可 | | カラーラベル（`red`・`yellow`・`green`・`blue`・`purple`。`CHECK`） |
| `develop_json` | TEXT | 可 | | 現像設定の JSON（NULL は処理バージョンの既定。2.5 節） |
| `develop_hash` | TEXT | 不可 | | 現像設定のハッシュ（丸めて正規化した JSON の SHA-256。16 進数 64 文字） |
| `process_version` | INTEGER | 不可 | | 処理バージョン（1 以上） |
| `history_pos` | INTEGER | 可 | | 今の履歴の位置（→ `history_entry.id`。履歴を消すと NULL） |
| `created_at` | TEXT | 不可 | | 作った日時（UTC） |
| `updated_at` | TEXT | 不可 | | 最後に変えた日時（UTC） |

### `history_entry`

現像の履歴（DEV-27）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`。履歴の順序） |
| `variant_id` | INTEGER | 不可 | | variant（→ `variant.id`。variant を消すと消える） |
| `created_at` | TEXT | 不可 | | 記録した日時（UTC） |
| `label` | TEXT | 不可 | | 操作の名前（`読み込み`・`露光量` など） |
| `develop_json` | TEXT | 可 | | その時点の現像設定（NULL は既定） |
| `process_version` | INTEGER | 不可 | | 処理バージョン |

### `snapshot`

スナップショット（DEV-28。v1 で使う）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`） |
| `variant_id` | INTEGER | 不可 | | variant（→ `variant.id`。variant を消すと消える） |
| `name` | TEXT | 不可 | | 名前 |
| `develop_json` | TEXT | 可 | | 現像設定（NULL は既定） |
| `process_version` | INTEGER | 不可 | | 処理バージョン |
| `created_at` | TEXT | 不可 | | 作った日時（UTC） |

### `keyword`

階層キーワード（LIB-09）。同じ親の下で同じ名前（検索用のキー）は 1 つだけ（`keyword_unique_name`）。キーワードを消すと、子のキーワードと付与も消えます。名前は 200 文字までで、「>」「＞」「|」「,」「，」「、」と制御文字を含みません（アプリが確かめる規則で、SQL の制約ではありません。決定事項ログ No.85）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`。版 8 から） |
| `parent_id` | INTEGER | 可 | | 親のキーワード（→ `keyword.id`。最上位は NULL） |
| `name` | TEXT | 不可 | | 名前（表示用） |
| `name_key` | TEXT | 不可 | | 検索用のキー（NFKC ＋ 小文字） |
| `include_on_export` | INTEGER | 不可 | `1` | 書き出すファイルのメタデータ（XMP の `dc:subject`）に書くか（1 = 書く、0 = 書かない。版 7 から。0 のキーワードは、写真に付いていても、子のキーワードの親としても書かない）（`CHECK`） |

### `variant_keyword`

variant に付けたキーワード（`WITHOUT ROWID`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `variant_id` | INTEGER | 不可 | | variant（→ `variant.id`。主キーの 1 番目） |
| `keyword_id` | INTEGER | 不可 | | キーワード（→ `keyword.id`。主キーの 2 番目） |

### `collection`

コレクション（LIB-10・LIB-11）。入れ子はコレクションセット（`kind = 'set'`）の中だけです。同じ親の下で同じ名前（比較用のキー。種類によらない）は 1 つだけ（`collection_unique_name`。版 7 から）。コレクションを消すと、セットの中のコレクションとメンバーの記録も消えます（写真は消えません）。名前は 200 文字までで、制御文字を含みません（アプリが確かめる規則）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`。版 8 から） |
| `parent_id` | INTEGER | 可 | | 親（コレクションセット。→ `collection.id`） |
| `name` | TEXT | 不可 | | 名前 |
| `kind` | TEXT | 不可 | | `manual`（通常）/ `smart`（スマートコレクション）/ `set`（コレクションセット）（`CHECK`） |
| `smart_rule_json` | TEXT | 可 | | スマートコレクションの条件（JSON。2.5 節。スマートコレクションだけ） |
| `name_key` | TEXT | 不可 | `''` | 名前の比較用のキー（NFKC ＋ 小文字。版 7 から） |

### `collection_member`

コレクションに入れた variant（`WITHOUT ROWID`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `collection_id` | INTEGER | 不可 | | コレクション（→ `collection.id`。主キーの 1 番目） |
| `variant_id` | INTEGER | 不可 | | variant（→ `variant.id`。主キーの 2 番目） |
| `sort_order` | INTEGER | 不可 | `0` | 入れた順（同じ操作で入れたものは渡した順） |

### `stack`

スタック（LIB-12。版 9 から使う。2.5 節）。写真が 1 枚以下になると消えます（トリガー `stack_member_after_delete`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`。版 9 から） |
| `top_variant_id` | INTEGER | 可 | | 一番上の variant（→ `variant.id`。スタックの中の順の最初。消すとトリガーが次の写真にする） |
| `collapsed` | INTEGER | 不可 | `1` | 折りたたんでいるか（1 = 折りたたむ。グリッドに 1 枚だけ出す。版 9 から） |

### `stack_member`

スタックの中の variant。variant は高々 1 つのスタックに属します（`variant_id` の `UNIQUE`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `stack_id` | INTEGER | 不可 | | スタック（→ `stack.id`。主キーの 1 番目） |
| `variant_id` | INTEGER | 不可 | | variant（→ `variant.id`。主キーの 2 番目。`UNIQUE`） |
| `position` | INTEGER | 不可 | | スタックの中の位置（`(position, variant_id)` の順。アプリは 0 から付け直す） |

### `video_meta`

動画に固有の情報（VID-03）。動画の asset だけに 1 行あります。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `asset_id` | INTEGER | 不可 | | asset（→ `asset.id`。主キー） |
| `duration_s` | REAL | 可 | | 長さ（秒） |
| `fps` | REAL | 可 | | フレームレート |
| `codec` | TEXT | 可 | | コーデック（ffprobe の名前。`h264`・`hevc` など） |
| `bit_depth` | INTEGER | 可 | | ビット深度 |
| `color_transfer` | TEXT | 可 | | 伝達関数（ffprobe の名前。`arib-std-b67` など） |
| `color_primaries` | TEXT | 可 | | 原色（ffprobe の名前。`bt2020` など） |

### `asset_iptc`

説明のメタデータ（01 の LIB-15。IPTC Core の主要な項目。版 6 から）。元のファイルには書かず、カタログの中だけで編集します（書き出したファイルには XMP と Exif で入れる）。asset と 1 対 1 で、**どれかの項目に値がある asset だけが行を持ちます**（すべての値を消すと行も消す）。値は前後の空白・制御文字を除いて整えたもので、空の文字列は入れません（NULL が「なし」）。1 つの値は 2,000 文字まで。キャプションは `asset.caption` にあります。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `asset_id` | INTEGER | 不可 | | asset（→ `asset.id`。主キー。asset を消すと消える） |
| `title` | TEXT | 可 | | タイトル（XMP の `dc:title`） |
| `headline` | TEXT | 可 | | 見出し（`photoshop:Headline`） |
| `creator` | TEXT | 可 | | 作成者（`dc:creator`・Exif の Artist） |
| `creator_job_title` | TEXT | 可 | | 作成者の肩書き（`photoshop:AuthorsPosition`） |
| `creator_address` | TEXT | 可 | | 作成者の住所（`Iptc4xmpCore:CreatorContactInfo` の `CiAdrExtadr`。複数行） |
| `creator_city` | TEXT | 可 | | 作成者の市区町村（`CiAdrCity`） |
| `creator_region` | TEXT | 可 | | 作成者の都道府県・州（`CiAdrRegion`） |
| `creator_postal_code` | TEXT | 可 | | 作成者の郵便番号（`CiAdrPcode`） |
| `creator_country` | TEXT | 可 | | 作成者の国（`CiAdrCtry`） |
| `creator_phone` | TEXT | 可 | | 作成者の電話（`CiTelWork`） |
| `creator_email` | TEXT | 可 | | 作成者のメール（`CiEmailWork`） |
| `creator_url` | TEXT | 可 | | 作成者の Web サイト（`CiUrlWork`） |
| `copyright` | TEXT | 可 | | 著作権表示（`dc:rights`・Exif の Copyright） |
| `copyright_status` | TEXT | 可 | | 著作権の状態: `copyrighted`（著作権あり）/ `public_domain`（パブリックドメイン）/ NULL（不明）（`CHECK`。XMP の `xmpRights:Marked`） |
| `usage_terms` | TEXT | 可 | | 使用条件（`xmpRights:UsageTerms`。複数行） |
| `copyright_url` | TEXT | 可 | | 著作権情報の URL（`xmpRights:WebStatement`） |
| `sublocation` | TEXT | 可 | | 撮影場所の詳細（`Iptc4xmpCore:Location`） |
| `city` | TEXT | 可 | | 撮影場所の市区町村（`photoshop:City`） |
| `province_state` | TEXT | 可 | | 撮影場所の都道府県・州（`photoshop:State`） |
| `country` | TEXT | 可 | | 撮影場所の国（`photoshop:Country`） |
| `country_code` | TEXT | 可 | | 撮影場所の国コード（`Iptc4xmpCore:CountryCode`。値は確かめない） |
| `credit` | TEXT | 可 | | クレジット（`photoshop:Credit`） |
| `source` | TEXT | 可 | | 提供元（`photoshop:Source`） |
| `instructions` | TEXT | 可 | | 指示（`photoshop:Instructions`。複数行） |

### `asset_text`

テキスト検索の対象の文字列（ファイル名・キャプションと、説明のメタデータのうち検索の対象の項目。2.5 節）。asset ごとに 1 行。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `asset_id` | INTEGER | 不可 | | asset（→ `asset.id`。主キー） |
| `text_norm` | TEXT | 不可 | | NFKC ＋ 小文字にした文字列 |

### `asset_fts`

3 文字以上の語の全文検索の索引（FTS5 の仮想表。`tokenize = 'trigram case_sensitive 1'`、内容は `asset_text`。rowid は `asset_text.asset_id`）。`asset_text` のトリガー（`asset_text_after_insert`・`asset_text_after_delete`・`asset_text_after_update`）で更新します。例: `SELECT rowid FROM asset_fts WHERE asset_fts MATCH '"清水寺"'`（検索語は小文字にしてから渡す）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `text_norm` | — | 可 | | 索引の対象の文字列 |

FTS5 の中身を持つ表（`asset_fts_data`・`asset_fts_idx`・`asset_fts_docsize`・`asset_fts_config`）と、SQLite の内部の表（`sqlite_sequence`。`AUTOINCREMENT` の連番）は直接読まないでください。

### `file_op`

ファイル操作の記録（移動・名前の変更・ゴミ箱への移動。04 の 6.4 節、DATA-07）。終わっていない記録は、次に開いたときに確定します。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`） |
| `kind` | TEXT | 不可 | | `move` / `rename` / `trash`（`CHECK`。版 12 から、フォルダの移動・名前の変更も `move` / `rename`） |
| `state` | TEXT | 不可 | | `planned`（予定）/ `executing`（実行中）/ `done`（完了）/ `failed`（失敗）（`CHECK`） |
| `payload_json` | TEXT | 不可 | | 対象のファイルと移動先（2.5 節） |
| `error` | TEXT | 可 | | 失敗の理由 |
| `created_at` | TEXT | 不可 | | 記録した日時（UTC） |
| `updated_at` | TEXT | 不可 | | 最後に状態を変えた日時（UTC） |

### `capture_time_edit`

撮影日時の修正の履歴（LIB-16。版 11 から）。1 行が 1 回の修正（ずらす・基準の写真に合わせる・タイムゾーンの指定と取り消し・元に戻す）です。アプリは新しい順に 20 回分を残し（`capture.rs` の `MAX_CAPTURE_TIME_EDITS`）、最後の修正から順に取り消せます。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `id` | INTEGER | 不可 | | 主キー（`AUTOINCREMENT`） |
| `kind` | TEXT | 不可 | | `shift`（ずらす）/ `sync`（基準の写真の正しい時刻に合わせる）/ `set_zone`（タイムゾーンの指定）/ `clear_zone`（指定の取り消し）/ `reset`（ファイルの値に戻す）（`CHECK`） |
| `shift_s` | INTEGER | 可 | | ずらした秒数（`shift`・`sync`） |
| `zone` | TEXT | 可 | | 指定したオフセット（`set_zone`。`+09:00` の形） |
| `zone_mode` | TEXT | 可 | | `keep_local`（記録の時刻をそのタイムゾーンの時刻として読む）/ `keep_instant`（撮影した瞬間はそのままで、そのタイムゾーンで表示する）（`set_zone`。`CHECK`） |
| `asset_count` | INTEGER | 不可 | | 変えた asset の数（`CHECK`） |
| `created_at` | TEXT | 不可 | | 修正した日時（UTC） |

### `capture_time_edit_item`

修正で変えた asset ごとの、変える前の修正の状態（版 11 から。`WITHOUT ROWID`）。元の値（`captured_at_raw`・`captured_offset`）は修正で変えないので記録しません。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `edit_id` | INTEGER | 不可 | | `capture_time_edit.id`（主キーの 1 つ目。修正の行を消すと消える） |
| `asset_id` | INTEGER | 不可 | | `asset.id`（主キーの 2 つ目。asset を消すと消える） |
| `prev_tz_source` | TEXT | 不可 | | 変える前の `tz_source`（`CHECK`） |
| `prev_tz_assumed` | TEXT | 可 | | 変える前の `tz_assumed` |
| `prev_tz_basis` | TEXT | 可 | | 変える前の `tz_basis` |
| `prev_correction_s` | INTEGER | 不可 | | 変える前の `time_correction_s` |

### `app_state`

アプリの状態（キー・値。`WITHOUT ROWID`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `key` | TEXT | 不可 | | キー（主キー） |
| `value` | TEXT | 不可 | | 値 |

| キー | 値 |
|---|---|
| `shutdown` | 前回の終了の印（`in_use` = 開いている・正常に終了しなかった、`clean` = 正常に終了した。DATA-05） |
| `last_opened_at`・`last_closed_at` | 最後に開いた・正常に閉じた日時（UTC） |
| `catalog_id` | カタログの ID（16 進数 32 文字。バックアップ・復元では変わらない。バックアップのファイル名に先頭 16 文字を入れる） |
| `cache_generation` | キャッシュの世代（16 進数 32 文字。作ったとき・復元したときに変わる。thumbs.db の `cache_meta` と比べる） |
| `capture_time_reresolve` | 撮影日時の推定し直しが済んでいない印（`pending`。04 の 3.4 節） |

### `setting`

カタログに保存する設定（キー・値。`WITHOUT ROWID`）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `key` | TEXT | 不可 | | キー（主キー） |
| `value` | TEXT | 不可 | | 値（文字列） |

| キー | 値 |
|---|---|
| `api.preview_cache_dir` | 標準プレビューのキャッシュの場所（パス） |
| `api.preview_cache_bytes` | 標準プレビューのキャッシュの上限（バイト。整数の文字列） |
| `api.default_export_color_space` | 書き出しの既定の色空間（JSON の文字列。`"srgb"` など） |
| `api.default_utc_offset_minutes` | 既定のタイムゾーンのオフセット（分。整数の文字列。540 = +09:00） |
| `api.import_skip_duplicates` | 取り込みの既定: 重複と思われるもの（内容が同じファイルが登録済みのもの）を取り込まない（`true` / `false`。なければ `true`。IMP-04） |
| `api.import_separate_raw_jpeg` | 取り込みの既定: RAW と同時記録の JPEG を別々の写真として取り込む（`true` / `false`。なければ `false`。IMP-06） |
| `api.hash_in_background` | 内容のハッシュ（`file.full_hash`）をバックグラウンドで計算する（`true` / `false`。なければ `true`。ORG-03） |
| `api.hash_network_volumes` | バックグラウンドの計算で、ネットワークのボリュームのファイルも読む（`true` / `false`。なければ `false`） |

### 索引

| 索引 | 表（列） | 用途 |
|---|---|---|
| `folder_parent` | `folder(parent_id)` | 子のフォルダ |
| `asset_captured_at` | `asset(captured_at_utc)` | 撮影日時の順・範囲 |
| `asset_camera` | `asset(camera)` | カメラでの絞り込み |
| `asset_lens` | `asset(lens)` | レンズでの絞り込み |
| `file_asset` | `file(asset_id)` | asset のファイル |
| `file_one_primary` | `file(asset_id) WHERE role = 'primary'`（`UNIQUE`） | asset ごとに主なファイルは 1 つ |
| `file_name_key` | `file(name_key)` | 名前でファイルを探す（書き出しの原本の照合。版 3） |
| `file_quick_hash` | `file(quick_hash)` | クイックハッシュが同じファイル（取り込みでの重複の候補。版 10） |
| `file_full_hash` | `file(full_hash) WHERE full_hash IS NOT NULL` | 内容（全体のハッシュ）でファイルを探す・重複の組をまとめる（版 10） |
| `file_not_ok` | `file(folder_id, status) WHERE status <> 'ok'` | 見つからない・読めないファイルの一覧と、フォルダごとの数（版 12。問い合わせの条件に `status <> 'ok'` を含めると使われる） |
| `variant_one_master` | `variant(asset_id) WHERE is_master = 1`（`UNIQUE`） | asset ごとにマスターは 1 つ |
| `variant_asset` | `variant(asset_id)` | asset の variant |
| `variant_rating` | `variant(rating)` | 評価での絞り込み |
| `variant_history_pos` | `variant(history_pos)` | 履歴を消すときの参照の確認（版 2） |
| `history_entry_variant` | `history_entry(variant_id, id)` | variant の履歴の順 |
| `snapshot_variant` | `snapshot(variant_id)` | variant のスナップショット |
| `keyword_unique_name` | `keyword(ifnull(parent_id, 0), name_key)`（`UNIQUE`） | 同じ親の下で同じ名前は 1 つ |
| `keyword_parent` | `keyword(parent_id)` | 子のキーワード |
| `keyword_name_key` | `keyword(name_key)` | 名前でキーワードを探す |
| `variant_keyword_keyword` | `variant_keyword(keyword_id, variant_id)` | キーワードの付いた variant |
| `collection_parent` | `collection(parent_id)` | 子のコレクション |
| `collection_unique_name` | `collection(ifnull(parent_id, 0), name_key)`（`UNIQUE`） | 同じ親の下で同じ名前は 1 つ（版 7） |
| `collection_member_variant` | `collection_member(variant_id)` | variant の入ったコレクション |
| `stack_top_variant` | `stack(top_variant_id)` | variant を消すときの参照の確認（版 2） |
| `file_op_unfinished` | `file_op(id) WHERE state IN ('planned', 'executing')` | 起動時に終わっていない記録を探す |
| `capture_time_edit_item_asset` | `capture_time_edit_item(asset_id)` | asset を消すときの参照の確認（版 11） |

`UNIQUE` の制約と主キーには、SQLite が自動で索引（`sqlite_autoindex_*`）を作ります。

### トリガー

| トリガー | 表 | 内容 |
|---|---|---|
| `asset_text_after_insert` | `asset_text` | 追加した文字列を `asset_fts` に入れる |
| `asset_text_after_delete` | `asset_text` | 消した文字列を `asset_fts` から除く（asset の削除の `ON DELETE CASCADE` でも動く） |
| `asset_text_after_update` | `asset_text` | 変えた文字列を `asset_fts` に入れ直す |
| `stack_member_after_delete` | `stack_member` | スタックの写真が抜けたら、一番上の写真が抜けたときは残りの最初の写真を一番上にし、1 枚以下になったスタックを消す（写真の削除の `ON DELETE CASCADE` でも動く。版 9） |

<!-- /schema -->

## 5. サムネイル DB の表（thumbs.db）

失っても作り直せるキャッシュです（カタログのバックアップに含めない）。カタログとは別のファイルなので外部キーはなく、`variant_id` はカタログの `variant.id` を指します（カタログにない variant の行は、アプリが回収します）。

<!-- schema:thumbs -->

### `thumb`

サムネイル（L0。長辺 320 px の JPEG）。variant ごとに 1 行。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `variant_id` | INTEGER | 不可 | | variant の ID（主キー） |
| `cache_key` | TEXT | 不可 | | キャッシュキー（16 進数。処理バージョン・現像設定などから求める。04 の 4.1 節） |
| `jpeg` | BLOB | 不可 | | JPEG（Display P3 の ICC を埋め込む） |
| `updated_at` | TEXT | 不可 | | 作った日時（UTC） |

### `preview`

標準プレビュー（L1）のファイル（`previews/ab/cd/<キー>.jpg`）の索引（`WITHOUT ROWID`）。最後に使った順に回収します。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `cache_key` | TEXT | 不可 | | キャッシュキー（16 進数の小文字 64 文字。主キー。ファイル名） |
| `variant_id` | INTEGER | 可 | | variant の ID（参考） |
| `size` | INTEGER | 不可 | | ファイルの大きさ（バイト） |
| `last_used_at` | TEXT | 不可 | | 最後に使った日時（UTC） |
| `use_seq` | INTEGER | 不可 | | 使った順の番号 |

索引: `preview_use_seq`（`preview(use_seq)`。古いものから回収する）。

### `cache_meta`

キャッシュの持ち主など（キー・値。`WITHOUT ROWID`）。キー `catalog_generation` は、キャッシュを作ったカタログの世代（カタログの `app_state` の `cache_generation`）で、違えばアプリはキャッシュを捨てます。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `key` | TEXT | 不可 | | キー（主キー） |
| `value` | TEXT | 不可 | | 値 |

### `regen_pending`

現像設定を保存した後、現像結果から L0 / L1 を作り直す前の variant（次の起動で作り直す）。

| 列 | 型 | NULL | 既定値 | 意味 |
|---|---|---|---|---|
| `variant_id` | INTEGER | 不可 | | variant の ID（主キー） |
| `marked_at` | TEXT | 不可 | | 印を付けた日時（UTC） |

<!-- /schema -->

## 6. スキーマを変えるとき（開発者向け）

- 変更は番号付きのマイグレーション（`crates/genzo-catalog/src/sql/catalog_NNNN_*.sql` と `schema.rs` の `CATALOG_MIGRATIONS`）で行います。既存のマイグレーションは書き換えず、新しいファイルを足して `CATALOG_SCHEMA_VERSION` を上げ、古い版のカタログからの移行のテストを書きます（04 の 3.4 節）。
- **この文書も更新します**（1 章の版と版の履歴、表・ビューの節）。文書に載っていない表・列・索引・トリガー、型の違う列、版の食い違いがあると、テスト（`schema_doc.rs`）が失敗します。
- 公開ビュー: マイグレーションの間は、すべてのビューを外してから各版の SQL を実行し、終わったら同じ定義で作り直します（`schema.rs` の `apply_migrations`）。SQLite の「12 の手順」で表を作り直しても、ビューのことは考えなくて構いません。ビューが参照する列を変える・消すときは、そのマイグレーションで `DROP VIEW IF EXISTS` してから新しい定義で `CREATE VIEW` します（列の名前と意味は保つ。3 章の約束）。作り直せないビューがあると、その版の移行は失敗して戻ります。
- 新しい機能の表を外部から読む入口が必要になったら、公開ビュー（`<名前>_v1`）を足し、`PUBLIC_VIEWS` とこの文書に加えます（版 7 でコレクションの `photo_collections_v1`、版 9 でスタックの `photo_stacks_v1` を足した）。
- SQL だけで書けない手順（NFKC の正規化など）は、マイグレーションの `post`（Rust の関数。SQL の後、同じトランザクションで動く）に書きます。版 7 の `collection.name_key` の値と `collection_unique_name` の索引は `schema.rs` の `migrate_collection_names` が作ります（既存の行に同じ名前があれば、後の行の名前に「 (2)」などを足してから索引を作る）。
- 版 8 で `folder`・`keyword`・`collection` を `AUTOINCREMENT` で作り直したとき、連番（`sqlite_sequence`）の初めの値は、残っている行の最大の ID と、スマートコレクションの条件（`smart_rule_json`。読めない版のものも含む）が参照する ID の最大のうち大きいほうにしました（`schema.rs` の `migrate_id_sequences`。版 7 までに削除したものを参照する条件が、移行の後に作るものに付け替わらないように）。ID で参照される表を足すときは、最初から `AUTOINCREMENT` にしてください。
- 版 9 で `stack` を `AUTOINCREMENT` で作り直したとき（列 `collapsed` も足した）、既存の行（版 8 まではアプリが作らないが、外部のツールで作られたものがありうる）は、SQL の後の Rust の処理（`schema.rs` の `migrate_stacks`）で整えました: 写真が 1 枚以下のスタックを消し、残りは一番上の写真（メンバーでなければ位置の最初のもの）を先頭に `(position, variant_id)` の順で位置を 0 から付け直す。マイグレーションの間は外部キーの確認を止めているので `ON DELETE CASCADE` が働かず、スタックを消す前にメンバーの行を消しています。
