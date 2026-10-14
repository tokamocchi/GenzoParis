-- GenzoParis のカタログ（catalog.db）のスキーマ 版 10: 内容による同一性（ORG-03）・重複の検出（IMP-04）・
-- RAW と JPEG のペア（IMP-06）。docs/04_architecture.md の 3.3 節・3.11 節。
--
-- 列（file.quick_hash・file.full_hash・file.role）は版 1 からある。この版で変えるもの:
--
-- - 索引 file_quick_hash: クイックハッシュ（大きさ ＋ 先頭と末尾の 64 KB）が同じファイルを探す。取り込みで、
--   別の場所にある同じ内容のファイル（重複の候補）を探すのに使う（候補は全体のハッシュで確定する）。
--   無圧縮の RAW のように大きさがそろうファイルが多くても、候補だけを読む。
-- - 索引 file_full_hash（全体のハッシュのある行だけの部分索引）: 全体のハッシュ（内容）でファイルを探す・
--   重複の組をまとめる。バックグラウンドのジョブが全体のハッシュを順に計算して記録する（genzo-api の
--   identity.rs）。既存の行の全体のハッシュは、この移行では計算しない（ファイルを読むため。移行の後に
--   アプリがバックグラウンドで計算する）。
-- - 公開ビュー photo_files_v1（ORG-07。docs/catalog_schema.md の 3 章）: 1 行 = 1 つのファイル。RAW と
--   同時記録の JPEG（role = 'sidecar_jpeg'）も 1 行ずつ出し、全体のハッシュも読めるようにする。

CREATE INDEX file_quick_hash ON file(quick_hash);
CREATE INDEX file_full_hash ON file(full_hash) WHERE full_hash IS NOT NULL;

-- 元ファイルの一覧（1 行 = 1 つのファイル）。master_variant_id は、ファイルの属する asset のマスターの variant
-- （photos_v1 の variant_id と結べる）。
CREATE VIEW photo_files_v1 AS
SELECT
    f.id                AS file_id,
    f.asset_id          AS asset_id,
    mv.id               AS master_variant_id,
    f.role              AS role,
    f.name              AS file_name,
    fo.rel_path         AS folder_path,
    vo.label            AS volume_label,
    vo.last_mount_path  AS volume_mount_path,
    f.size              AS file_size,
    f.status            AS file_status,
    f.full_hash         AS full_hash
FROM file AS f
JOIN folder AS fo ON fo.id = f.folder_id
JOIN volume AS vo ON vo.id = fo.volume_id
LEFT JOIN variant AS mv ON mv.asset_id = f.asset_id AND mv.is_master = 1;
