-- GenzoParis のカタログ（catalog.db）のスキーマ 版 12: 外付けドライブの識別（FILE-03）と、見つからない写真・
-- フォルダの印（FILE-04）。docs/04_architecture.md の 3.1 節・3.3 節・3.14 節。
--
-- - volume.fs_type: ボリュームのファイルシステムの種類（ext4・apfs・NTFS・exFAT など。表示用。分からなければ
--   NULL）。
-- - volume.last_seen_at: ボリュームがつながっていることを最後に確かめた日時（UTC。つながっていないボリュームの
--   表示用。既存の行は NULL）。
-- - folder.status・folder.status_at: ボリュームはつながっているのにフォルダが見つからない（アプリの外で移動・
--   名前の変更・削除された）印と、最後に記録した日時（genzo-model の FolderStatus）。既存の行は 'ok'。
-- - 索引 file_not_ok: 状態が ok でないファイルだけの部分索引（見つからない写真の一覧とフォルダごとの数を、
--   ファイルの表を全部読まずに求める。条件に `status <> 'ok'` を含む問い合わせで使われる）。
--
-- ボリュームの ID（volume.uuid）の形は genzo-api が決める（OS のボリューム ID）。版 11 までの、パスの先頭から
-- 作った ID（`path:` で始まる）は、この移行では変えない（OS に問い合わせる必要があるため）。genzo-api が
-- カタログを開いたときに、いま見えるフォルダから順に OS のボリューム ID のボリュームへ移す（3.14 節）。
--
-- ALTER TABLE ... ADD COLUMN は表を作り直さないので、folder の id の連番（AUTOINCREMENT）も変わらない。

ALTER TABLE volume ADD COLUMN fs_type TEXT;
ALTER TABLE volume ADD COLUMN last_seen_at TEXT;

ALTER TABLE folder ADD COLUMN status TEXT NOT NULL DEFAULT 'ok' CHECK (status IN ('ok', 'missing'));
ALTER TABLE folder ADD COLUMN status_at TEXT;

CREATE INDEX file_not_ok ON file(folder_id, status) WHERE status <> 'ok';
