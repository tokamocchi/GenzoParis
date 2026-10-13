-- GenzoParis のカタログ（catalog.db）のスキーマ 版 11: 撮影日時の一括修正（LIB-16）。docs/04_architecture.md の
-- 3.1 節・3.13 節。
--
-- - asset.camera_serial: カメラの本体のシリアル番号（Exif の BodySerialNumber。LibRaw の shootinginfo.BodySerial）。
--   機材（カメラの機種・シリアル番号・写真か動画か）ごとに時計のずれを補正するために、機種と組で使う。既存の
--   行は NULL のまま（ファイルを読み直したときに入る）。
-- - asset.tz_basis: UTC の計算に使ったオフセット（"+09:00" 形式）。表示のオフセット（tz_assumed）と違う場合
--   （撮影した瞬間を変えずに、別のタイムゾーンの時刻で表示する）と、UTC の印（Z）だけの値を現地時刻として
--   読む場合（DJI などで現地時刻が UTC として書かれていた）だけ持つ。NULL なら従来どおり tz_assumed（UTC の印
--   だけの値は UTC）で計算する（genzo-model の CaptureTime::basis）。既存の行はすべて NULL で、値は変わらない。
-- - capture_time_edit・capture_time_edit_item: 撮影日時の修正の履歴（元に戻すため）。修正のたびに 1 行と、
--   変えた asset ごとに変える前の修正の状態（tz_source・tz_assumed・tz_basis・time_correction_s）を記録する。
--   元の値（captured_at_raw・captured_offset）は修正で変えないので記録しない。新しい順に一定の数だけ残す
--   （genzo-catalog の capture.rs の MAX_CAPTURE_TIME_EDITS）。asset を消すと、その行も消える。
-- - 公開ビュー photos_v1 に、末尾の列 camera_serial を足す（既存の列の名前・意味・値の形は変えない。
--   docs/catalog_schema.md の「公開ビュー」の約束）。マイグレーションの間は公開ビューを外している
--   （schema.rs の apply_migrations）ので、ここで新しい定義で作れば、元の定義では作り直されない。
--
-- ALTER TABLE ... ADD COLUMN は表を作り直さないので、asset の id の連番（AUTOINCREMENT）も変わらない。

ALTER TABLE asset ADD COLUMN camera_serial TEXT;
ALTER TABLE asset ADD COLUMN tz_basis TEXT;

CREATE TABLE capture_time_edit (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL
                CHECK (kind IN ('shift', 'sync', 'set_zone', 'clear_zone', 'reset')),
    shift_s     INTEGER,                   -- ずらした秒数（shift・sync）
    zone        TEXT,                      -- 指定したオフセット（set_zone。"+09:00" 形式）
    zone_mode   TEXT CHECK (zone_mode IS NULL OR zone_mode IN ('keep_local', 'keep_instant')),
    asset_count INTEGER NOT NULL CHECK (asset_count >= 0),  -- 変えた asset の数
    created_at  TEXT NOT NULL              -- 修正した日時（UTC）
) STRICT;

CREATE TABLE capture_time_edit_item (
    edit_id           INTEGER NOT NULL REFERENCES capture_time_edit(id) ON DELETE CASCADE,
    asset_id          INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    prev_tz_source    TEXT NOT NULL
                      CHECK (prev_tz_source IN ('exif', 'utc_only', 'user_default', 'user_set')),
    prev_tz_assumed   TEXT,
    prev_tz_basis     TEXT,
    prev_correction_s INTEGER NOT NULL,
    PRIMARY KEY (edit_id, asset_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX capture_time_edit_item_asset ON capture_time_edit_item(asset_id);

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
    a.camera_serial     AS camera_serial
FROM variant AS v
JOIN asset AS a ON a.id = v.asset_id
LEFT JOIN file AS f ON f.asset_id = a.id AND f.role = 'primary'
LEFT JOIN folder AS fo ON fo.id = f.folder_id
LEFT JOIN volume AS vo ON vo.id = fo.volume_id
LEFT JOIN video_meta AS vm ON vm.asset_id = a.id
LEFT JOIN asset_iptc AS ip ON ip.asset_id = a.id;
