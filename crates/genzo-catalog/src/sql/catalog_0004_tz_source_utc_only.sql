-- GenzoParis のカタログ（catalog.db）のスキーマ 版 4: 撮影日時の出どころ（tz_source）に
-- 'utc_only'（UTC の印 Z だけが記録されていた。動画の creation_time など）を足す
-- （implementation_status の No.32 の (a)。genzo-model の TzSource::UtcOnly）。
--
-- CHECK 制約は ALTER TABLE で変えられないため、SQLite の説明書の ALTER TABLE の
-- 「12 の手順」で asset の表を作り直す（マイグレーションは外部キーの確認を止めて 1 つの
-- トランザクションで適用し、コミットの前に foreign_key_check で確かめる。schema.rs）。
-- asset を参照する表（file・variant・video_meta・asset_text）の REFERENCES asset(id) は
-- 名前で参照しているので、作り直した後もそのまま使える。asset に付いたトリガー・ビューはない。
--
-- 削除した asset の id を再利用しないよう（外部のキャッシュが id で参照するため）、
-- AUTOINCREMENT の連番（sqlite_sequence）を引き継ぐ。

CREATE TEMP TABLE migration_0004_seq AS
    SELECT seq FROM sqlite_sequence WHERE name = 'asset';

CREATE TABLE asset_new (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    kind              TEXT NOT NULL CHECK (kind IN ('photo', 'video')),
    captured_at_raw   TEXT,
    captured_offset   TEXT,
    tz_source         TEXT NOT NULL DEFAULT 'user_default'
                      CHECK (tz_source IN ('exif', 'utc_only', 'user_default', 'user_set')),
    tz_assumed        TEXT,
    time_correction_s INTEGER NOT NULL DEFAULT 0,
    captured_at_utc   TEXT,                    -- 補正後の UTC（固定長。不明なら NULL）
    camera            TEXT,
    lens              TEXT,
    iso               INTEGER,
    aperture          REAL,
    shutter           REAL,
    focal             REAL,
    width             INTEGER,
    height            INTEGER,
    orientation       INTEGER NOT NULL DEFAULT 1 CHECK (orientation BETWEEN 1 AND 8),
    gps_lat           REAL,
    gps_lon           REAL,
    caption           TEXT,
    created_at        TEXT NOT NULL            -- 登録した日時（UTC）
) STRICT;

INSERT INTO asset_new(
    id, kind, captured_at_raw, captured_offset, tz_source, tz_assumed, time_correction_s,
    captured_at_utc, camera, lens, iso, aperture, shutter, focal, width, height, orientation,
    gps_lat, gps_lon, caption, created_at)
SELECT
    id, kind, captured_at_raw, captured_offset, tz_source, tz_assumed, time_correction_s,
    captured_at_utc, camera, lens, iso, aperture, shutter, focal, width, height, orientation,
    gps_lat, gps_lon, caption, created_at
FROM asset;

DROP TABLE asset;
ALTER TABLE asset_new RENAME TO asset;

CREATE INDEX asset_captured_at ON asset(captured_at_utc);
CREATE INDEX asset_camera ON asset(camera);
CREATE INDEX asset_lens ON asset(lens);

DELETE FROM sqlite_sequence WHERE name = 'asset';
INSERT INTO sqlite_sequence(name, seq)
    SELECT 'asset', max(
        coalesce((SELECT max(seq) FROM temp.migration_0004_seq), 0),
        coalesce((SELECT max(id) FROM asset), 0));
DROP TABLE temp.migration_0004_seq;

-- 版 3 までに取り込んだ、UTC の印（Z）だけの撮影日時（ファイルの記録の文字列が Z で終わり、
-- 元のオフセットを +00:00 として記録したもの）を utc_only にする。UTC（captured_at_utc）は
-- 変わらない。表示のオフセット（tz_assumed）は +00:00 のままにし、カタログを開いたコア
-- （genzo-api）が既定のタイムゾーンで推定し直す（UTC_ONLY_SCHEMA_VERSION の doc）。
-- 利用者がタイムゾーンを指定したもの（user_set）は、指定した結果を変えないよう、そのままにする。
UPDATE asset
SET tz_source = 'utc_only', captured_offset = 'Z'
WHERE tz_source = 'exif'
  AND captured_offset = '+00:00'
  AND rtrim(captured_at_raw, ' ' || char(9, 10, 13)) LIKE '%Z';

-- utc_only にした asset があれば、撮影日時の推定し直しが済んでいない印を付ける（この移行と同じ
-- トランザクション。コアが推定し直しを終える前にアプリが終わっても、次に開いたときに推定し直す。
-- キーは genzo-catalog の STATE_CAPTURE_TIME_RERESOLVE、値は 'pending'）。
INSERT INTO app_state(key, value)
    SELECT 'capture_time_reresolve', 'pending'
    WHERE EXISTS (SELECT 1 FROM asset WHERE tz_source = 'utc_only')
    ON CONFLICT(key) DO UPDATE SET value = excluded.value;
