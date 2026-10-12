// ジョブの進捗（SYS-02・UX-04）: 上の帯に、実行中のジョブ（「取り込み 120/500」など）を出す。押すと一覧を
// 開き、それぞれの進捗と取り消しのボタンを出す。終わったジョブは state/app.ts が少し後に一覧から消す。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import { Progress } from "../components/Progress";
import { type JobView, actions, useApp } from "../state/app";
import "./dialogs.css";
import { JOB_KIND_LABEL, JOB_STATE_LABEL, formatCount, formatJobProgress, sortJobs } from "./dialogFormat";

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
        {job.total > 0 ? `${formatCount(Math.min(job.done, job.total))} / ${formatCount(job.total)}` : running ? "準備中…" : ""}
        {job.message ? `\n${job.message}` : ""}
      </div>
    </div>
  );
}

export function JobsIndicator() {
  const jobs = useApp((s) => s.jobs);
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

  useEffect(() => {
    if (list.length === 0 && open) setOpen(false);
  }, [list.length, open]);

  if (list.length === 0) return null;
  const head = running[0] ?? list[0];
  return (
    <div className="jobs-indicator" ref={ref}>
      <button
        className="btn ghost small jobs-button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        title="バックグラウンドの処理（押すと一覧と取り消し）"
      >
        {running.length > 0 && <span className="spinner" aria-hidden="true" />}
        <span className="text">
          {running.length > 0 ? formatJobProgress(head) : `${JOB_KIND_LABEL[head.kind]}: ${JOB_STATE_LABEL[head.state]}`}
        </span>
        {running.length > 1 && <span className="muted">ほか {running.length - 1} 件</span>}
      </button>
      {open && (
        <div className="jobs-popover" role="dialog" aria-label="バックグラウンドの処理">
          {list.map((j) => (
            <JobRow key={j.jobId} job={j} />
          ))}
        </div>
      )}
    </div>
  );
}
