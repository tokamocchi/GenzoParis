-- GenzoParis のカタログ（catalog.db）のスキーマ 版 1（初期スキーマ）。
--
-- docs/04_architecture.md の 3.1 節（ER 図）・3.2 節（インデックス）・3.5 節（制約）・
-- 3.6 節（テキスト検索）・6.4 節（file_op）に対応する。
-- 型の取り違えを防ぐため、通常のテーブルは STRICT にする（SQLite 3.37 以降）。
-- 外部キャッシュ（サムネイル DB・プレビュー）から参照する ID（asset・variant・file）と、
-- 履歴の順序に使う ID（history_entry）は、削除後に同じ値が再利用されないよう AUTOINCREMENT にする。

-- ボリューム（ドライブ）。場所は「ボリューム ＋ 相対パス」で管理する（3.3 節・FILE-03）。
CREATE TABLE volume (
    id              INTEGER PRIMARY KEY,
    uuid            TEXT NOT NULL UNIQUE,      -- OS のボリューム ID
    label           TEXT,
    last_mount_path TEXT                       -- 最後に確認したマウント先（ドライブ文字など）
) STRICT;

-- フォルダ。ボリュームのルートは rel_path = '' で、親を持たない。
CREATE TABLE folder (
    id           INTEGER PRIMARY KEY,
    volume_id    INTEGER NOT NULL REFERENCES volume(id),
    parent_id    INTEGER REFERENCES folder(id),
    rel_path     TEXT NOT NULL,                -- 表示用（'/' 区切り）
    rel_path_key TEXT NOT NULL,                -- 比較用（NFC ＋ 小文字化。3.5 節）
    UNIQUE (volume_id, rel_path_key),
    CHECK ((parent_id IS NULL) = (rel_path = ''))
) STRICT;
CREATE INDEX folder_parent ON folder(parent_id);

-- 実体（写真・動画）。撮影日時は元の値と推定に使った情報を分けて持つ（3.1 節。レビュー R-18）。
CREATE TABLE asset (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    kind              TEXT NOT NULL CHECK (kind IN ('photo', 'video')),
    captured_at_raw   TEXT,
    captured_offset   TEXT,
    tz_source         TEXT NOT NULL DEFAULT 'user_default'
                      CHECK (tz_source IN ('exif', 'user_default', 'user_set')),
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
CREATE INDEX asset_captured_at ON asset(captured_at_utc);
CREATE INDEX asset_camera ON asset(camera);
CREATE INDEX asset_lens ON asset(lens);

-- ファイル。RAW と JPEG のペアは同じ asset に属する（JPEG は role = 'sidecar_jpeg'）。
CREATE TABLE file (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    asset_id      INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    folder_id     INTEGER NOT NULL REFERENCES folder(id),
    name          TEXT NOT NULL,               -- 表示用
    name_key      TEXT NOT NULL,               -- 比較用（NFC ＋ 小文字化。3.5 節）
    role          TEXT NOT NULL CHECK (role IN ('primary', 'sidecar_jpeg')),
    size          INTEGER NOT NULL CHECK (size >= 0),
    mtime         INTEGER NOT NULL,            -- UNIX 時刻からのナノ秒
    quick_hash    TEXT NOT NULL,
    full_hash     TEXT,                        -- 必要になったときに計算（3.3 節）
    revision      INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    status        TEXT NOT NULL DEFAULT 'ok' CHECK (status IN ('ok', 'missing', 'error')),
    status_reason TEXT,                        -- missing / error の理由（6.3 節）
    status_at     TEXT,                        -- 状態を最後に記録した日時
    -- 同じファイルの二重登録を防ぐ（登録の繰り返しは冪等）。file(folder_id) の索引を兼ねる（3.2 節）。
    UNIQUE (folder_id, name_key)
) STRICT;
CREATE INDEX file_asset ON file(asset_id);
-- asset ごとに主となるファイルは 1 つだけ。
CREATE UNIQUE INDEX file_one_primary ON file(asset_id) WHERE role = 'primary';

-- 現像のバリエーション（マスター ＋ 仮想コピー）。グリッドに表示する単位。
-- develop_json が NULL のときは「process_version の既定の現像設定」を表す（未調整の写真の容量を抑えるため）。
CREATE TABLE variant (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    asset_id        INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    is_master       INTEGER NOT NULL CHECK (is_master IN (0, 1)),
    name            TEXT,                      -- 仮想コピーの名前（マスターは NULL）
    rating          INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
    flag            INTEGER NOT NULL DEFAULT 0 CHECK (flag IN (-1, 0, 1)),
    color_label     TEXT CHECK (color_label IN ('red', 'yellow', 'green', 'blue', 'purple')),
    develop_json    TEXT,
    develop_hash    TEXT NOT NULL,
    process_version INTEGER NOT NULL CHECK (process_version >= 1),
    history_pos     INTEGER REFERENCES history_entry(id) ON DELETE SET NULL, -- 現在の履歴の位置（DEV-27）
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
) STRICT;
-- asset ごとにマスターは 1 つだけ（部分インデックスによる UNIQUE。3.5 節）。
CREATE UNIQUE INDEX variant_one_master ON variant(asset_id) WHERE is_master = 1;
CREATE INDEX variant_asset ON variant(asset_id);
CREATE INDEX variant_rating ON variant(rating);

-- 現像の履歴（DEV-27）。develop_json が NULL のときは process_version の既定の設定。
CREATE TABLE history_entry (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    variant_id      INTEGER NOT NULL REFERENCES variant(id) ON DELETE CASCADE,
    created_at      TEXT NOT NULL,
    label           TEXT NOT NULL,
    develop_json    TEXT,
    process_version INTEGER NOT NULL CHECK (process_version >= 1)
) STRICT;
CREATE INDEX history_entry_variant ON history_entry(variant_id, id);

-- スナップショット（DEV-28）。
CREATE TABLE snapshot (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    variant_id      INTEGER NOT NULL REFERENCES variant(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    develop_json    TEXT,
    process_version INTEGER NOT NULL CHECK (process_version >= 1),
    created_at      TEXT NOT NULL
) STRICT;
CREATE INDEX snapshot_variant ON snapshot(variant_id);

-- 階層キーワード（LIB-09）。同じ親の下で同じ名前（正規化後）は 1 つだけ。
CREATE TABLE keyword (
    id        INTEGER PRIMARY KEY,
    parent_id INTEGER REFERENCES keyword(id) ON DELETE CASCADE,
    name      TEXT NOT NULL,
    name_key  TEXT NOT NULL                    -- 検索用（NFKC ＋ 小文字化）
) STRICT;
CREATE UNIQUE INDEX keyword_unique_name ON keyword(ifnull(parent_id, 0), name_key);
CREATE INDEX keyword_parent ON keyword(parent_id);
CREATE INDEX keyword_name_key ON keyword(name_key);

CREATE TABLE variant_keyword (
    variant_id INTEGER NOT NULL REFERENCES variant(id) ON DELETE CASCADE,
    keyword_id INTEGER NOT NULL REFERENCES keyword(id) ON DELETE CASCADE,
    PRIMARY KEY (variant_id, keyword_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX variant_keyword_keyword ON variant_keyword(keyword_id, variant_id);

-- コレクション（LIB-10・LIB-11。v1）。
CREATE TABLE collection (
    id              INTEGER PRIMARY KEY,
    parent_id       INTEGER REFERENCES collection(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('manual', 'smart', 'set')),
    smart_rule_json TEXT
) STRICT;
CREATE INDEX collection_parent ON collection(parent_id);

CREATE TABLE collection_member (
    collection_id INTEGER NOT NULL REFERENCES collection(id) ON DELETE CASCADE,
    variant_id    INTEGER NOT NULL REFERENCES variant(id) ON DELETE CASCADE,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (collection_id, variant_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX collection_member_variant ON collection_member(variant_id);

-- スタック（LIB-12。v1）。variant は高々 1 つのスタックに属する。
CREATE TABLE stack (
    id             INTEGER PRIMARY KEY,
    top_variant_id INTEGER REFERENCES variant(id) ON DELETE SET NULL
) STRICT;

CREATE TABLE stack_member (
    stack_id   INTEGER NOT NULL REFERENCES stack(id) ON DELETE CASCADE,
    variant_id INTEGER NOT NULL UNIQUE REFERENCES variant(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    PRIMARY KEY (stack_id, variant_id)
) STRICT;

-- 動画に固有の情報（VID-03）。
CREATE TABLE video_meta (
    asset_id        INTEGER PRIMARY KEY REFERENCES asset(id) ON DELETE CASCADE,
    duration_s      REAL,
    fps             REAL,
    codec           TEXT,
    bit_depth       INTEGER,
    color_transfer  TEXT,
    color_primaries TEXT
) STRICT;

-- テキスト検索の対象（ファイル名とキャプション）を NFKC ＋ 小文字化した文字列（3.6 節）。
-- 1〜2 文字の語は、この列への LIKE で探す。
CREATE TABLE asset_text (
    asset_id  INTEGER PRIMARY KEY REFERENCES asset(id) ON DELETE CASCADE,
    text_norm TEXT NOT NULL
) STRICT;

-- 3 文字以上の語の索引（FTS5 の trigram。asset_text を内容とする外部内容テーブル）。
-- 入力は既に小文字化しているため case_sensitive 1 とする（二重の変換を避ける）。
CREATE VIRTUAL TABLE asset_fts USING fts5(
    text_norm,
    content = 'asset_text',
    content_rowid = 'asset_id',
    tokenize = 'trigram case_sensitive 1'
);

-- asset_text の変更を FTS の索引に反映する（FTS5 の説明書の外部内容テーブルの例と同じ形）。
-- asset の削除による ON DELETE CASCADE でも、このトリガーが動く。
CREATE TRIGGER asset_text_after_insert AFTER INSERT ON asset_text BEGIN
    INSERT INTO asset_fts(rowid, text_norm) VALUES (new.asset_id, new.text_norm);
END;
CREATE TRIGGER asset_text_after_delete AFTER DELETE ON asset_text BEGIN
    INSERT INTO asset_fts(asset_fts, rowid, text_norm) VALUES ('delete', old.asset_id, old.text_norm);
END;
CREATE TRIGGER asset_text_after_update AFTER UPDATE ON asset_text BEGIN
    INSERT INTO asset_fts(asset_fts, rowid, text_norm) VALUES ('delete', old.asset_id, old.text_norm);
    INSERT INTO asset_fts(rowid, text_norm) VALUES (new.asset_id, new.text_norm);
END;

-- ファイル操作の記録（移動・リネーム・ゴミ箱への移動。6.4 節・DATA-07）。
CREATE TABLE file_op (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    kind         TEXT NOT NULL CHECK (kind IN ('move', 'rename', 'trash')),
    state        TEXT NOT NULL CHECK (state IN ('planned', 'executing', 'done', 'failed')),
    payload_json TEXT NOT NULL,
    error        TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
) STRICT;
-- 起動時に、終わっていない記録だけを速く探す。
CREATE INDEX file_op_unfinished ON file_op(id) WHERE state IN ('planned', 'executing');

-- アプリの状態（前回正常に終了したかの印など。DATA-05）。
CREATE TABLE app_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT, WITHOUT ROWID;

-- 設定（キー・値）。
CREATE TABLE setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT, WITHOUT ROWID;
