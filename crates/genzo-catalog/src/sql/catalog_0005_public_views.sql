-- GenzoParis のカタログ（catalog.db）のスキーマ 版 5: 外部から SQL で読むための公開ビュー（ORG-07）。
--
-- カタログの表の構造は docs/catalog_schema.md に書く。表は機能の追加（v1 の各パッケージ）で変わりうるため、
-- 外部のツール（SQLite の CLI・DB Browser for SQLite・表計算ソフトなど）から読むときの安定した入口として、
-- 名前の末尾に版（_v1）を付けたビューを置く。
--
-- 公開ビューの約束（docs/catalog_schema.md の「公開ビュー」）:
-- - 列の名前・意味・値の形を変えない。列を減らさない。変える必要があれば、別の名前（_v2）のビューを足す。
-- - 列を足すことはある（`SELECT *` ではなく、列の名前を指定して読むことをすすめる）。
-- - 行の並び順は決めない（ORDER BY を指定して読む）。
--
-- マイグレーションの間は、公開ビューを外してから各版の SQL を実行し、終わったら同じ定義で作り直す
-- （schema.rs の apply_migrations。SQLite の「12 の手順」で表を作り直すと、その表を参照するビューがあれば
-- ALTER TABLE ... RENAME が失敗するため）。

-- 写真・動画の一覧（グリッドに並ぶ単位。1 行 = 1 つの variant。仮想コピーも 1 行）。
-- 元ファイルの場所は、主なファイル（role = 'primary'）の「ボリュームのマウント先 ＋ フォルダ ＋ ファイル名」。
CREATE VIEW photos_v1 AS
SELECT
    v.id                AS variant_id,
    v.asset_id          AS asset_id,
    a.kind              AS kind,
    CASE v.is_master WHEN 1 THEN 0 ELSE 1 END AS is_virtual_copy,
    v.name              AS copy_name,
    v.rating            AS rating,
    v.flag              AS flag,
    v.color_label       AS color_label,
    a.captured_at_utc   AS captured_at_utc,
    strftime('%Y-%m-%dT%H:%M:%f', a.captured_at_utc, a.tz_assumed) AS captured_at_local,
    a.tz_assumed        AS utc_offset,
    a.camera            AS camera,
    a.lens              AS lens,
    a.iso               AS iso,
    a.aperture          AS aperture,
    a.shutter           AS shutter_s,
    a.focal             AS focal_mm,
    a.width             AS width,
    a.height            AS height,
    a.orientation       AS orientation,
    a.gps_lat           AS gps_lat,
    a.gps_lon           AS gps_lon,
    a.caption           AS caption,
    f.name              AS file_name,
    fo.rel_path         AS folder_path,
    vo.label            AS volume_label,
    vo.last_mount_path  AS volume_mount_path,
    f.size              AS file_size,
    f.status            AS file_status,
    vm.duration_s       AS duration_s,
    vm.fps              AS fps,
    vm.codec            AS video_codec,
    v.process_version   AS process_version,
    v.develop_json      AS develop_json,
    a.created_at        AS imported_at,
    v.updated_at        AS updated_at
FROM variant AS v
JOIN asset AS a ON a.id = v.asset_id
LEFT JOIN file AS f ON f.asset_id = a.id AND f.role = 'primary'
LEFT JOIN folder AS fo ON fo.id = f.folder_id
LEFT JOIN volume AS vo ON vo.id = fo.volume_id
LEFT JOIN video_meta AS vm ON vm.asset_id = a.id;

-- 写真に付けたキーワード（1 行 = 1 つの variant と 1 つのキーワードの組）。keyword_path は最上位からの
-- 階層を「 > 」でつないだもの（「場所 > 京都」。01 の LIB-09 の書き方）。親子の循環があっても止まるよう、
-- 階層は 64 段までたどる。
CREATE VIEW photo_keywords_v1 AS
WITH RECURSIVE keyword_path(id, path, depth) AS (
    SELECT id, name, 1 FROM keyword WHERE parent_id IS NULL
    UNION ALL
    SELECT k.id, p.path || ' > ' || k.name, p.depth + 1
    FROM keyword AS k
    JOIN keyword_path AS p ON k.parent_id = p.id
    WHERE p.depth < 64
)
SELECT
    vk.variant_id       AS variant_id,
    k.id                AS keyword_id,
    k.name              AS keyword,
    kp.path             AS keyword_path
FROM variant_keyword AS vk
JOIN keyword AS k ON k.id = vk.keyword_id
JOIN keyword_path AS kp ON kp.id = k.id;
