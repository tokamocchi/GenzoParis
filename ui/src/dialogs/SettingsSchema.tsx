// 設定の「カタログ」の「カタログの構造」（ORG-07）: カタログは普通の SQLite のファイルで、外部のツールから
// SQL で直接読めることの案内と、構造（公開ビューの列・すべての SQL）の表示。文書は docs/catalog_schema.md。

import { useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { CatalogSchema, ErrorInfo } from "../api/types";
import { describeSchema, publicViewTables, readOnlyCommand, schemaSql } from "./logsLogic";
import { Busy, ErrorBox } from "./parts";

export function SettingsSchema(props: { catalogPath: string; schemaVersion: number | null }) {
  const [schema, setSchema] = useState<CatalogSchema | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      setSchema(await core.catalogSchema());
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="dlg-section">
      <h3>カタログの構造（SQL で直接読む）</h3>
      <p className="hint" style={{ marginTop: 0 }}>
        カタログは普通の SQLite のファイルなので、SQLite のツール（sqlite3・DB Browser for SQLite など）で読めます。
        書き換えると不整合の原因になるので、必ず読み取り専用で開いてください。外部から読むときは、列の名前・意味を変えない約束の
        公開ビュー（photos_v1・photo_keywords_v1・photo_collections_v1）を使ってください。構造の説明はソースの docs/catalog_schema.md にあります。
        {props.schemaVersion !== null && `今のスキーマの版は ${props.schemaVersion} です。`}
      </p>
      <div className="dlg-path box" title="読み取り専用で開く例">
        {readOnlyCommand(props.catalogPath)}
      </div>
      <div className="dlg-row" style={{ marginTop: 6 }}>
        <button className="btn" onClick={() => void load()} disabled={loading}>
          {schema ? "構造を読み直す" : "構造を表示"}
        </button>
        {loading && <Busy>読んでいます…</Busy>}
        {schema && <span className="hint">{describeSchema(schema)}</span>}
      </div>
      {schema && (
        <>
          {publicViewTables(schema).map((t) => (
            <div key={t.name} style={{ marginTop: 6 }}>
              <div style={{ fontWeight: 600 }}>{t.name}（公開ビュー）</div>
              <div className="hint">{t.columns.map((c) => c.name).join("、")}</div>
            </div>
          ))}
          <details style={{ marginTop: 6 }}>
            <summary>すべての表・索引・ビュー・トリガーの SQL</summary>
            <pre className="dlg-pre">{schemaSql(schema)}</pre>
          </details>
        </>
      )}
      <ErrorBox error={error} title="構造を読めません" />
    </div>
  );
}
