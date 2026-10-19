-- GenzoParis のカタログ（catalog.db）のスキーマ 版 13: 位置情報の後付け（LIB-21。ジオタグ付け）。
-- docs/04_architecture.md の 3.1 節・3.16 節。
--
-- - asset_location: 後から付けた位置（asset と 1 対 1。付けた asset だけが行を持つ）。元のファイルには書かない
--   （カタログの中だけ）。asset 単位なので、仮想コピーや RAW と JPEG の組も同じ位置になる。
--   asset.gps_lat・gps_lon はカメラ（ファイル）の記録の値のままにする（取り込み・ファイルの読み直しのたびに
--   ファイルの値で上書きするため、付けた位置はそこに書かない）。使う位置は、付けた位置があればそれ、なければ
--   カメラの位置。
-- - location_edit・location_edit_item: 位置の変更の履歴（元に戻すため）。変更のたびに 1 行と、変えた asset
--   ごとに変える前の付けた位置（なければ NULL）を記録する。新しい順に一定の数だけ残す（genzo-catalog の
--   location.rs の MAX_LOCATION_EDITS）。asset を消すと、その行も消える。
-- - 公開ビュー photos_v1 に、末尾の列 location_lat・location_lon・location_source を足す（使う位置と、その
--   出どころ。既存の列の名前・意味・値の形は変えない。gps_lat・gps_lon はカメラの値のまま。
--   docs/catalog_schema.md の「公開ビュー」の約束）。緯度と経度は同じ条件で選ぶ（別々の出どころの緯度と
--   経度を混ぜない）。カメラの位置が片方だけの asset は、位置なし（3 列とも NULL）として扱う。
--   マイグレーションの間は公開ビューを外している（schema.rs の apply_migrations）ので、ここで新しい定義で
--   作れば、元の定義では作り直されない。
--
-- 既存の asset には行を作らない（付けた位置がないのと同じ）。asset の表は変えない。

CREATE TABLE asset_location (
    asset_id INTEGER PRIMARY KEY REFERENCES asset(id) ON DELETE CASCADE,
    lat      REAL NOT NULL CHECK (lat BETWEEN -90.0 AND 90.0),     -- 緯度（度。北が正）
    lon      REAL NOT NULL CHECK (lon BETWEEN -180.0 AND 180.0),   -- 経度（度。東が正）
    set_at   TEXT NOT NULL                                         -- 付けた日時（UTC）
) STRICT;

CREATE TABLE location_edit (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL CHECK (kind IN ('set', 'clear')),
    lat         REAL,                      -- 付けた位置（set。clear は NULL）
    lon         REAL,
    asset_count INTEGER NOT NULL CHECK (asset_count >= 0),  -- 変えた asset の数
    created_at  TEXT NOT NULL,             -- 変更した日時（UTC）
    CHECK ((lat IS NULL) = (lon IS NULL)),
    CHECK ((kind = 'set') = (lat IS NOT NULL))
) STRICT;

CREATE TABLE location_edit_item (
    edit_id     INTEGER NOT NULL REFERENCES location_edit(id) ON DELETE CASCADE,
    asset_id    INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    prev_lat    REAL,                      -- 変える前の付けた位置（なければ NULL）
    prev_lon    REAL,
    prev_set_at TEXT,                      -- 変える前の付けた日時（なければ NULL）
    PRIMARY KEY (edit_id, asset_id),
    CHECK ((prev_lat IS NULL) = (prev_lon IS NULL)),
    CHECK ((prev_lat IS NULL) = (prev_set_at IS NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX location_edit_item_asset ON location_edit_item(asset_id);

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
    ip.copyright        AS copyright,
    a.camera_serial     AS camera_serial,
    CASE WHEN al.asset_id IS NOT NULL THEN al.lat
         WHEN a.gps_lat IS NOT NULL AND a.gps_lon IS NOT NULL THEN a.gps_lat END AS location_lat,
    CASE WHEN al.asset_id IS NOT NULL THEN al.lon
         WHEN a.gps_lat IS NOT NULL AND a.gps_lon IS NOT NULL THEN a.gps_lon END AS location_lon,
    CASE WHEN al.asset_id IS NOT NULL THEN 'assigned'
         WHEN a.gps_lat IS NOT NULL AND a.gps_lon IS NOT NULL THEN 'camera' END AS location_source
FROM variant AS v
JOIN asset AS a ON a.id = v.asset_id
LEFT JOIN file AS f ON f.asset_id = a.id AND f.role = 'primary'
LEFT JOIN folder AS fo ON fo.id = f.folder_id
LEFT JOIN volume AS vo ON vo.id = fo.volume_id
LEFT JOIN video_meta AS vm ON vm.asset_id = a.id
LEFT JOIN asset_iptc AS ip ON ip.asset_id = a.id
LEFT JOIN asset_location AS al ON al.asset_id = a.id;
