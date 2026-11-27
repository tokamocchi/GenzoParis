-- GenzoParis のカタログ（catalog.db）のスキーマ 版 15: GPX のログからの位置付け（LIB-21 の拡張）。
--
-- - location_edit.source: 位置の変更の出どころ。NULL は、選んだ写真にまとめて同じ位置を付けた・消した変更
--   （版 14 までのすべての変更）。'track' は、GPX のログと撮影日時を照らし合わせて、写真ごとに違う位置を付けた
--   変更（genzo-catalog の location.rs の edit_locations_each）。'track' の行の kind は 'set' で、lat・lon には
--   代表として、付けた位置のうち最初の 1 つを入れる（写真ごとの位置は asset_location にある。変える前の位置は
--   location_edit_item に、ほかの変更と同じ形で記録する）。
--
-- ALTER TABLE ... ADD COLUMN で足す（表を作り直さない。既存の行は NULL。CHECK は既存の行で確かめられる）。
-- ほかの表・公開ビューは変えない。

ALTER TABLE location_edit ADD COLUMN source TEXT
    CHECK (source IS NULL OR source IN ('track'));
