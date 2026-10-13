-- GenzoParis のカタログ（catalog.db）のスキーマ 版 8: フォルダ・キーワード・コレクションの ID を、削除した後に
-- 使い回さない（AUTOINCREMENT。docs/catalog_schema.md の 2.4 節）。
--
-- 版 7 までは folder・keyword・collection の id が INTEGER PRIMARY KEY（AUTOINCREMENT なし）で、最大の id の
-- 行を削除すると、次に作る行が同じ id になった。スマートコレクションの条件（keyword_id・collection_id・
-- folder_id。genzo-model の SmartRule）と画面のターゲットコレクションは、削除した後も古い id を持つので、
-- 後から作ったキーワード・コレクションに黙って付け替わっていた（削除したものを参照する条件は「どの写真にも
-- 一致しない（not_has・not_in はすべてに一致する）」という約束が守られない）。フォルダは今の版では削除しないが、
-- 条件から参照するので同じにする。
--
-- CHECK・AUTOINCREMENT は ALTER TABLE で変えられないため、SQLite の説明書の ALTER TABLE の「12 の手順」で
-- 3 つの表を作り直す（マイグレーションは外部キーの確認を止めて 1 つのトランザクションで適用し、コミットの前に
-- foreign_key_check で確かめる。ビューは前に外して後で作り直す。schema.rs）。これらの表を参照する表
-- （file・variant_keyword・collection_member と自分の parent_id）の REFERENCES は名前で参照しているので、
-- 作り直した後もそのまま使える。列の名前・順・型・制約・既定値と索引は版 7 と同じ。
--
-- 連番（sqlite_sequence）の初めの値は、残っている行の最大の id。スマートコレクションの条件が、それより大きい
-- （削除した）id を参照していれば、SQL の後の Rust の処理（schema.rs の migrate_id_sequences）でその値まで
-- 上げる（JSON の中は SQL で確実には読めないため）。

-- フォルダ。
CREATE TABLE folder_new (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    volume_id    INTEGER NOT NULL REFERENCES volume(id),
    parent_id    INTEGER REFERENCES folder(id),
    rel_path     TEXT NOT NULL,                -- 表示用（'/' 区切り）
    rel_path_key TEXT NOT NULL,                -- 比較用（NFC ＋ 小文字化。3.5 節）
    UNIQUE (volume_id, rel_path_key),
    CHECK ((parent_id IS NULL) = (rel_path = ''))
) STRICT;

INSERT INTO folder_new(id, volume_id, parent_id, rel_path, rel_path_key)
SELECT id, volume_id, parent_id, rel_path, rel_path_key FROM folder;

DROP TABLE folder;
ALTER TABLE folder_new RENAME TO folder;

CREATE INDEX folder_parent ON folder(parent_id);

-- 階層キーワード（LIB-09）。
CREATE TABLE keyword_new (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id         INTEGER REFERENCES keyword(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    name_key          TEXT NOT NULL,           -- 検索用（NFKC ＋ 小文字化）
    include_on_export INTEGER NOT NULL DEFAULT 1
                      CHECK (include_on_export IN (0, 1))
) STRICT;

INSERT INTO keyword_new(id, parent_id, name, name_key, include_on_export)
SELECT id, parent_id, name, name_key, include_on_export FROM keyword;

DROP TABLE keyword;
ALTER TABLE keyword_new RENAME TO keyword;

CREATE UNIQUE INDEX keyword_unique_name ON keyword(ifnull(parent_id, 0), name_key);
CREATE INDEX keyword_parent ON keyword(parent_id);
CREATE INDEX keyword_name_key ON keyword(name_key);

-- コレクション（LIB-10・LIB-11）。name_key の既定値 '' は版 7 の ALTER TABLE ... ADD COLUMN と同じ（アプリは
-- 必ず値を入れる）。
CREATE TABLE collection_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id       INTEGER REFERENCES collection(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('manual', 'smart', 'set')),
    smart_rule_json TEXT,
    name_key        TEXT NOT NULL DEFAULT ''   -- 名前の比較用（NFKC ＋ 小文字化）
) STRICT;

INSERT INTO collection_new(id, parent_id, name, kind, smart_rule_json, name_key)
SELECT id, parent_id, name, kind, smart_rule_json, name_key FROM collection;

DROP TABLE collection;
ALTER TABLE collection_new RENAME TO collection;

CREATE INDEX collection_parent ON collection(parent_id);
CREATE UNIQUE INDEX collection_unique_name ON collection(ifnull(parent_id, 0), name_key);

-- 連番。作り直した表へ id を指定して入れたので SQLite が行を作っているが、名前の付け替えに頼らず、
-- ここで決めた値にする（asset の作り直しの版 4 と同じ）。
DELETE FROM sqlite_sequence
WHERE name IN ('folder', 'folder_new', 'keyword', 'keyword_new', 'collection', 'collection_new');
INSERT INTO sqlite_sequence(name, seq) SELECT 'folder', coalesce(max(id), 0) FROM folder;
INSERT INTO sqlite_sequence(name, seq) SELECT 'keyword', coalesce(max(id), 0) FROM keyword;
INSERT INTO sqlite_sequence(name, seq) SELECT 'collection', coalesce(max(id), 0) FROM collection;
