-- GenzoParis のカタログ（catalog.db）のスキーマ 版 6: 説明のメタデータ（LIB-15。IPTC Core の主要な項目）。
--
-- タイトル・作成者と連絡先・著作権・撮影場所などを、元のファイルには書かずカタログに保存する（キャプションは
-- MVP からの asset.caption のまま）。asset と 1 対 1 で、どれかの項目に値がある asset だけが行を持つ
-- （すべての値を消したら行を消す。50 万件の多くは値を持たないため）。値は genzo-model の
-- normalize_metadata_value で整えたもの（空の文字列は入れない。NULL が「なし」）。
-- 仮想コピーも同じ値（キャプションと同じく asset に付ける。決定事項ログ No.78）。
--
-- 列の名前は genzo-model の MetadataField::as_str と同じ。
CREATE TABLE asset_iptc (
    asset_id            INTEGER PRIMARY KEY REFERENCES asset(id) ON DELETE CASCADE,
    title               TEXT,
    headline            TEXT,
    creator             TEXT,
    creator_job_title   TEXT,
    creator_address     TEXT,
    creator_city        TEXT,
    creator_region      TEXT,
    creator_postal_code TEXT,
    creator_country     TEXT,
    creator_phone       TEXT,
    creator_email       TEXT,
    creator_url         TEXT,
    copyright           TEXT,
    copyright_status    TEXT CHECK (copyright_status IN ('copyrighted', 'public_domain')),
    usage_terms         TEXT,
    copyright_url       TEXT,
    sublocation         TEXT,
    city                TEXT,
    province_state      TEXT,
    country             TEXT,
    country_code        TEXT,
    credit              TEXT,
    source              TEXT,
    instructions        TEXT
) STRICT;

-- 公開ビュー photos_v1 に、タイトル・作成者・著作権表示の列を末尾に足す（版 5 の列の名前・意味・値の形は
-- 変えない。docs/catalog_schema.md の「公開ビュー」の約束）。マイグレーションの間は公開ビューを外している
-- （schema.rs の apply_migrations）ので、ここで新しい定義で作れば、元の定義では作り直されない。
DROP VIEW IF EXISTS photos_v1;
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
    v.updated_at        AS updated_at,
    ip.title            AS title,
    ip.creator          AS creator,
    ip.copyright        AS copyright
FROM variant AS v
JOIN asset AS a ON a.id = v.asset_id
LEFT JOIN file AS f ON f.asset_id = a.id AND f.role = 'primary'
LEFT JOIN folder AS fo ON fo.id = f.folder_id
LEFT JOIN volume AS vo ON vo.id = fo.volume_id
LEFT JOIN video_meta AS vm ON vm.asset_id = a.id
LEFT JOIN asset_iptc AS ip ON ip.asset_id = a.id;
