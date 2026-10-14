-- GenzoParis のカタログ（catalog.db）のスキーマ 版 7: 階層キーワード・コレクション・スマートコレクション
-- （LIB-09・LIB-10・LIB-11。docs/04_architecture.md の 3.9 節）。
--
-- 表（keyword・variant_keyword・collection・collection_member）は版 1 からある。この版で足すもの:
--
-- - keyword.include_on_export: 書き出すファイルのメタデータ（XMP の dc:subject）にこのキーワードを書くか
--   （1 = 書く。既定）。0 にしたキーワードは、写真に付いていても、子のキーワードの親としても書かない
--   （子のキーワードはそれぞれの値に従う。決定事項ログ No.81）。
-- - collection.name_key: 名前の比較用（NFKC ＋ 小文字化。keyword.name_key と同じ。genzo-catalog の
--   text::search_key）。同じ親（コレクションセット）の下で同じ名前は 1 つだけ（索引 collection_unique_name）。
--   既存の行の値と索引は、この SQL の後の Rust の処理（schema.rs の migrate_collection_names）で作る
--   （NFKC は SQL では求められないため。既存の行に同じ名前があれば、後のものの名前に「 (2)」などを足す）。
-- - 公開ビュー photo_collections_v1: 手動のコレクションに入れた写真（ORG-07。docs/catalog_schema.md の 3 章）。
--
-- ALTER TABLE ... ADD COLUMN で足す（表を作り直さない。CHECK は既存の行で確かめられる。SQLite 3.37 以降）。

ALTER TABLE keyword ADD COLUMN include_on_export INTEGER NOT NULL DEFAULT 1
    CHECK (include_on_export IN (0, 1));

ALTER TABLE collection ADD COLUMN name_key TEXT NOT NULL DEFAULT '';

-- 手動のコレクションに入れた写真（1 行 = 1 つの variant と 1 つのコレクションの組）。collection_path は
-- 最上位のコレクションセットからの階層を「 > 」でつないだもの（「旅行 > 2024 京都」）。親子の循環があっても
-- 止まるよう、階層は 64 段までたどる（photo_keywords_v1 と同じ）。
CREATE VIEW photo_collections_v1 AS
WITH RECURSIVE collection_path(id, path, depth) AS (
    SELECT id, name, 1 FROM collection WHERE parent_id IS NULL
    UNION ALL
    SELECT c.id, p.path || ' > ' || c.name, p.depth + 1
    FROM collection AS c
    JOIN collection_path AS p ON c.parent_id = p.id
    WHERE p.depth < 64
)
SELECT
    cm.variant_id       AS variant_id,
    c.id                AS collection_id,
    c.name              AS collection,
    cp.path             AS collection_path
FROM collection_member AS cm
JOIN collection AS c ON c.id = cm.collection_id
JOIN collection_path AS cp ON cp.id = c.id;
