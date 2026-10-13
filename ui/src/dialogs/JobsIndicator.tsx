// ジョブの進捗（SYS-02・UX-04）: 上の帯に、実行中のジョブ（「取り込み 120/500」など）を出す。押すと一覧を
// 開き、それぞれの進捗と取り消しのボタンを出す。終わったジョブは state/app.ts が少し後に一覧から消す。
//
// 書き出し・削除の結果の詳細（resultLog.ts）も、ここの「結果」から見直し・コピーできる（通知は少したつと
// 消え、長い詳細は短くして出すため）。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import { Progress } from "../components/Progress";
import { type JobView, actions, useApp } from "../state/app";
import "./dialogs.css";
import {
  JOB_KIND_LABEL,
  JOB_STATE_LABEL,
  formatCount,
  formatJobAmount,
  formatJobProgress,
  formatLocalDateTime,
  sortJobs,
} from "./dialogFormat";
import { type ResultEntry, clearResults, removeResult, useResults } from "./resultLog";

function JobRow(props: { job: JobView }) {
  const { job } = props;
  const [cancelling, setCancelling] = useState(false);
  const running = job.state === "running";
  const cancel = async () => {
    setCancelling(true);
    try {
      await core.cancelJob(job.jobId);
    } catch (e) {
      setCancelling(false);
      actions.reportError(e, "取り消せません");
    }
  };
  return (
    <div className="jobs-item">
      <div className="head">
        <span className="label" title={job.label}>
          {job.label}
        </span>
        {running ? (
          <button className="btn small" onClick={() => void cancel()} disabled={cancelling}>
            {cancelling ? "取り消し中…" : "取り消す"}
          </button>
        ) : (
          <span className={`state ${job.state}`}>{JOB_STATE_LABEL[job.state]}</span>
        )}
      </div>
      {running && <Progress done={job.done} total={job.total} />}
      <div className="message">
        {job.total > 0 ? formatJobAmount(job) : running ? "準備中…" : ""}
        {job.message ? `\n${job.message}` : ""}
      </div>
    </div>
  );
}

/** 結果の詳細を写す（クリップボードに書けなければ、文字を選んで写すよう案内する）。 */
async function copyText(text: string): Promise<void> {
  try {
    if (typeof navigator === "undefined" || !navigator.clipboard) throw new Error("クリップボードを使えません");
    await navigator.clipboard.writeText(text);
    actions.notify("info", "結果の詳細をコピーしました");
  } catch {
    actions.notify("warning", "コピーできません。詳細の文字を選んでコピーしてください");
  }
}

function ResultRow(props: { entry: ResultEntry }) {
  const { entry } = props;
  const when = formatLocalDateTime(new Date(entry.at).toISOString()).slice(11);
  return (
    <div className="jobs-item">
      <div className="head">
        <span className={`label result-${entry.level}`} title={entry.message}>
          {entry.message}
        </span>
        <span className="state">{when}</span>
      </div>
      <pre className="result-detail">{entry.detail}</pre>
      <div className="dlg-row end">
        <button className="btn small" onClick={() => void copyText(`${entry.message}\n${entry.detail}`)}>
          コピー
        </button>
        <button className="btn small ghost" onClick={() => removeResult(entry.id)}>
          消す
        </button>
      </div>
    </div>
  );
}

export function JobsIndicator() {
  const jobs = useApp((s) => s.jobs);
  const results = useResults();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const list = sortJobs(jobs);
  const running = list.filter((j) => j.state === "running");

  // 一覧の外を押すか Esc で閉じる。
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  const empty = list.length === 0 && results.length === 0;
  useEffect(() => {
    if (empty && open) setOpen(false);
  }, [empty, open]);

  if (empty) return null;
  const head = running[0] ?? list[0];
  let text: string;
  if (running.length > 0) text = formatJobProgress(running[0]);
  else if (head) text = `${JOB_KIND_LABEL[head.kind]}: ${JOB_STATE_LABEL[head.state]}`;
  else text = `結果（${formatCount(results.length)}）`;
  return (
    <div className="jobs-indicator" ref={ref}>
      <button
        className="btn ghost small jobs-button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        title="バックグラウンドの処理と結果（押すと一覧・取り消し・結果の詳細）"
      >
        {running.length > 0 && <span className="spinner" aria-hidden="true" />}
        <span className="text">{text}</span>
        {running.length > 1 && <span className="muted">ほか {running.length - 1} 件</span>}
      </button>
      {open && (
        <div className="jobs-popover" role="dialog" aria-label="バックグラウンドの処理と結果">
          {list.map((j) => (
            <JobRow key={j.jobId} job={j} />
          ))}
          {results.length > 0 && (
            <>
              <div className="jobs-section">
                <span>結果（新しい順）</span>
                <button className="btn small ghost" onClick={clearResults}>
                  すべて消す
                </button>
              </div>
              {results.map((r) => (
                <ResultRow key={r.id} entry={r} />
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}
