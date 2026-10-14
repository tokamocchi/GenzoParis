// カメラプロファイル・ルック（DEV-17）の一覧のダイアログ（現像のパネルの「管理…」）。
//
// - 取り込んだもの（カメラプロファイル（DCP）・ルック（3D LUT の .cube））を種類ごとに並べ、選んだものの詳細
//   （中身の説明・使わない情報・著作権の表示・埋め込みの方針・元のファイル・内容の SHA-256）を出す。
// - 取り込み（OS のファイルの選択。複数可。「同じ名前のものを置き換える」）と、一覧から除く（確認してから）。
// - 著作権の表示・埋め込みの方針はファイルにあるものをそのまま出す（このアプリは判断しない）。Adobe のプロファイル
//   などは同梱しない。

import { useEffect, useState } from "react";

import type { ProfileEntry, ProfileKind } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions } from "../state/app";
import { importProfileFiles, reloadProfiles, removeProfileEntry, useProfiles } from "../state/profiles";
import { Busy } from "./parts";

const KIND_LABEL: Record<ProfileKind, string> = {
  camera: "カメラプロファイル",
  look: "ルック",
};

function key(e: { kind: ProfileKind; id: string }): string {
  return `${e.kind}:${e.id}`;
}

function Details(props: { entry: ProfileEntry }) {
  const e = props.entry;
  return (
    <dl className="dlg-kv" aria-label="詳細">
      <dt>種類</dt>
      <dd>{KIND_LABEL[e.kind]}</dd>
      <dt>ID</dt>
      <dd>{e.id}</dd>
      <dt>名前</dt>
      <dd>{e.name}</dd>
      {e.kind === "camera" && (
        <>
          <dt>機種</dt>
          <dd>
            {e.camera_model ?? "—"}
            {e.matches_camera === true ? "（この写真の機種用）" : e.matches_camera === false ? "（ほかの機種用）" : ""}
          </dd>
        </>
      )}
      <dt>内容</dt>
      <dd>{e.usable ? e.description : "読めません（取り込み直してください）"}</dd>
      {e.notes.length > 0 && (
        <>
          <dt>注意</dt>
          <dd>
            {e.notes.map((n, i) => (
              <div key={i}>{n}</div>
            ))}
          </dd>
        </>
      )}
      <dt>著作権の表示</dt>
      <dd>{e.copyright ?? "—"}</dd>
      {e.kind === "camera" && (
        <>
          <dt>埋め込みの方針</dt>
          <dd>{e.embed_policy ?? "—"}</dd>
        </>
      )}
      <dt>元のファイル</dt>
      <dd>{e.original_name}</dd>
      <dt>大きさ</dt>
      <dd>{e.size.toLocaleString("ja-JP")} バイト</dd>
      <dt>取り込んだ日時</dt>
      <dd>{e.imported_at || "—"}</dd>
      <dt>SHA-256</dt>
      <dd className="dlg-path">{e.sha256}</dd>
    </dl>
  );
}

export function ProfileBrowserDialog() {
  const entries = useProfiles((s) => s.entries);
  const busy = useProfiles((s) => s.busy);
  const error = useProfiles((s) => s.error);
  const [selected, setSelected] = useState<string | null>(null);
  const [replace, setReplace] = useState(false);
  const [confirming, setConfirming] = useState(false);

  useEffect(() => {
    void reloadProfiles();
  }, []);

  const list = entries ?? [];
  const current = list.find((e) => key(e) === selected) ?? null;
  const groups: ProfileKind[] = ["camera", "look"];

  return (
    <Dialog
      title="カメラプロファイル・ルック"
      width={720}
      onClose={actions.closeDialog}
      busy={busy && entries === null}
      actions={
        <>
          <label className="dlg-check" style={{ margin: 0 }}>
            <input type="checkbox" checked={replace} onChange={(e) => setReplace(e.target.checked)} />
            同じ名前のものを置き換える
          </label>
          <span className="dlg-grow" />
          <button
            className="btn"
            disabled={busy}
            onClick={() =>
              void importProfileFiles(replace).then((added) => {
                if (added.length > 0) setSelected(key(added[0]));
              })
            }
          >
            取り込む…
          </button>
          <button className="btn primary" onClick={actions.closeDialog}>
            閉じる
          </button>
        </>
      }
    >
      <p className="hint">
        利用者が用意したカメラプロファイル（.dcp）とルック（3D LUT の .cube）を取り込んで使います（元のファイルは変えず、カタログのデータのフォルダに写します）。このアプリにはプロファイルは付属していません。ファイルの利用条件は、提供元の条件を確かめてください。
      </p>
      {error && <div className="dlg-error">一覧を読めません: {error}</div>}
      {entries === null && !error && <Busy>一覧を読んでいます…</Busy>}
      {entries !== null && list.length === 0 && <p className="hint">取り込んだものはありません（「取り込む…」で取り込めます）</p>}
      {groups.map((g) => {
        const items = list.filter((e) => e.kind === g);
        if (items.length === 0) return null;
        return (
          <div key={g} className="dlg-section">
            <h3>{KIND_LABEL[g]}</h3>
            <div className="dlg-list" role="listbox" aria-label={KIND_LABEL[g]}>
              {items.map((e) => (
                <button
                  key={key(e)}
                  type="button"
                  role="option"
                  aria-selected={key(e) === selected}
                  className={`dlg-list-item selectable${key(e) === selected ? " selected" : ""}`}
                  onClick={() => {
                    setSelected(key(e));
                    setConfirming(false);
                  }}
                >
                  <span className="dlg-grow">{e.name === e.id ? e.name : `${e.name}（${e.id}）`}</span>
                  {e.kind === "camera" && <span className="muted">{e.camera_model ?? "機種の記録なし"}</span>}
                  {!e.usable && <span className="muted">読めません</span>}
                </button>
              ))}
            </div>
          </div>
        );
      })}
      {current && (
        <div className="dlg-section">
          <h3>詳細</h3>
          <Details entry={current} />
          {confirming ? (
            <div className="dlg-confirm">
              {`「${current.id}」を一覧から除きます。使っている写真は、${
                current.kind === "camera" ? "標準の色" : "ルックなし"
              }で表示し、「見つからない」と知らせます（設定は残るので、取り込み直すと元に戻ります）。`}
              <div className="dlg-row end">
                <button className="btn" onClick={() => setConfirming(false)}>
                  やめる
                </button>
                <button
                  className="btn danger"
                  onClick={() =>
                    void removeProfileEntry(current.kind, current.id).then((ok) => {
                      if (ok) setSelected(null);
                      setConfirming(false);
                    })
                  }
                >
                  除く
                </button>
              </div>
            </div>
          ) : (
            <div className="dlg-row end">
              <button className="btn" onClick={() => setConfirming(true)}>
                一覧から除く…
              </button>
            </div>
          )}
        </div>
      )}
    </Dialog>
  );
}
