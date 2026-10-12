// 進捗の棒（total が 0 なら進み具合が分からない表示）。

export function Progress(props: { done: number; total: number }) {
  if (props.total <= 0) {
    return (
      <div className="progress indeterminate">
        <div />
      </div>
    );
  }
  const pct = Math.max(0, Math.min(100, (props.done / props.total) * 100));
  return (
    <div className="progress" role="progressbar" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
      <div style={{ width: `${pct}%` }} />
    </div>
  );
}
