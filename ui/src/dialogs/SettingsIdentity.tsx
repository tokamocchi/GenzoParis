// 設定の「カタログ」の「内容による同一性（重複の検出）」（ORG-03・IMP-04）: 内容のハッシュ（ファイル全体の
// BLAKE3）の記録の状況、今すぐ計算するボタン、内容が同じファイルの組の一覧（押すとその写真を選ぶ）。
//
// バックグラウンドで計算するか・ネットワークのボリュームも読むか・取り込みの既定は、SettingsCatalog の
// 「既定の値」のフォームで保存する（このコンポーネントは読むだけと、計算の実行）。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import type { ContentHashStatus, DuplicateGroupList } from "../api/types";
import { actions } from "../state/app";
import { revealAndSelect } from "../state/stacks";
import { waitJobUntilClosed } from "./catalogActions";
import {
  contentFileLine,
  describeHashReport,
  describeHashStatus,
  duplicateGroupTitle,
  duplicateListNote,
} from "./identityLogic";
import { Busy } from "./parts";
import { notifyResult } from "./resultLog";

/** 一覧で読む組の上限（仮置き: 100。genzo-api の DUPLICATE_GROUPS_MAX は 1000）。 */
const GROUPS_SHOWN = 100;

export function SettingsIdentity(props: { includeNetwork: boolean }) {
  const [status, setStatus] = useState<ContentHashStatus | null>(null);
  const [groups, setGroups] = useState<DuplicateGroupList | null>(null);
  const [running, setRunning] = useState(false);
  const [loading, setLoading] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const reload = () =>
    core
      .contentHashStatus()
      .then((s) => {
        if (mounted.current) setStatus(s);
      })
      .catch(() => undefined);

  useEffect(() => {
    void reload();
  }, []);

  const compute = async () => {
    setRunning(true);
    try {
      const jobId = await core.computeContentHashes(props.includeNetwork);
      actions.trackJob(jobId, "hash_files");
      const info = await waitJobUntilClosed(jobId);
      if (info === "closed") return;
      const r = info.result;
      if (r && r.type === "hash_files") notifyResult(describeHashReport(r));
      else if (info.state === "cancelled") actions.notify("info", "内容のハッシュの計算を取り消しました");
    } catch (e) {
      actions.reportError(e, "内容のハッシュを計算できません");
    } finally {
      if (mounted.current) {
        setRunning(false);
        void reload();
      }
    }
  };

  const showGroups = async () => {
    setLoading(true);
    try {
      const list = await core.duplicateGroups(GROUPS_SHOWN);
      if (mounted.current) setGroups(list);
    } catch (e) {
      actions.reportError(e, "内容が同じファイルを読めません");
    } finally {
      if (mounted.current) setLoading(false);
    }
  };

  const note = groups ? duplicateListNote(groups) : null;
  return (
    <div className="dlg-section">
      <h3>内容による同一性（重複の検出）</h3>
      <p className="hint" style={{ marginTop: 0 }}>
        ファイルの内容のハッシュ（ファイル全体の BLAKE3）をカタログに記録し、場所や名前によらず同じ写真を見分けます（取り込みの重複の検出、アプリの外で移したファイルの確認に使います）。元のファイルは読むだけで、変更しません。
      </p>
      {status ? (
        describeHashStatus(status).map((line) => (
          <div key={line} className="dlg-kv-line">
            {line}
          </div>
        ))
      ) : (
        <Busy>状況を読んでいます…</Busy>
      )}
      <div className="dlg-row">
        <button className="btn" onClick={() => void compute()} disabled={running}>
          今すぐ計算する
        </button>
        <button className="btn" onClick={() => void showGroups()} disabled={loading}>
          内容が同じファイルを表示
        </button>
        {running && <Busy>計算しています…</Busy>}
      </div>
      {groups &&
        (groups.groups.length === 0 ? (
          <p className="hint">内容が同じファイルはありません（記録済みのファイルの中で）。</p>
        ) : (
          <div className="dlg-list">
            {note && <div className="hint">{note}</div>}
            {groups.groups.map((g, i) => (
              <div key={g.full_hash} className="dlg-list-item column">
                <div className="dlg-grow">{duplicateGroupTitle(i, g.files.length, g.full_hash)}</div>
                {g.files.map((f) => (
                  <div key={f.file_id} className="dlg-row">
                    <div className="dlg-grow selectable" title={f.path ?? undefined}>
                      {contentFileLine(f)}
                    </div>
                    <button
                      className="btn small ghost"
                      onClick={() => {
                        actions.closeDialog();
                        void revealAndSelect(f.variant_id);
                      }}
                      title="この写真をグリッドで選ぶ"
                    >
                      表示
                    </button>
                  </div>
                ))}
              </div>
            ))}
          </div>
        ))}
    </div>
  );
}
