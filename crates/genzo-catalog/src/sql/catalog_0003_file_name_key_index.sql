-- GenzoParis のカタログ（catalog.db）のスキーマ 版 3: ファイル名（比較用のキー）だけの索引。
--
-- 書き出しの原本の照合（04 の 6.4 節）で、上書きの設定のときに、書き出すファイルと同じ名前の
-- カタログのファイルをすべてのフォルダから探す（genzo-catalog の Catalog::files_named）。
-- 版 2 までは file(folder_id, name_key) の UNIQUE 制約の索引しかなく、名前だけでは使えないため、
-- 問い合わせのたびに file の表全体を走査していた（implementation_status の No.11・27 の (b)）。

CREATE INDEX file_name_key ON file(name_key);
