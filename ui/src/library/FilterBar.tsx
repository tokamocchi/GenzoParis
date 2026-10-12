// グリッドの上の絞り込み（LIB-07）と並べ替え（LIB-08）。
//
// - 条件は appStore.filter（actions.setFilter・replaceFilter で変え、変えるたびに検索し直す）。
// - テキストは、IME の変換中は検索しない。変換の確定（compositionend）・入力から 300 ms 後・Enter で反映する。
// - 撮影日は、カタログの既定のタイムゾーン（CoreSettings の default_utc_offset_minutes）の日付とみなし、
//   UTC の境界にする（開始日はその日の 0 時を含む、終了日は翌日の 0 時を含まない。filterLogic.ts の冒頭）。
// - カメラ・レンズ・動画のコーデックの候補は core.facets()。カタログが変わったら読み直す。読めなければ
//   候補なしで動く（コーデックの欄は候補があるときだけ出す）。
// - 長辺の画素数・動画の長さ・フレームレートの範囲（VID-03）は「詳しい条件」の中に置く（入力は全角の数字も
//   読む。フォーカスを外したとき・Enter・入力から 600 ms 後に反映する）。

import { useEffect, useId, useRef, useState } from "react";

import * as core from "../api/core";
import type { AssetKind, Facets, FacetValue, SearchFilter, SearchSort, SortKey } from "../api/types";
import { COLOR_LABEL_NAMES } from "../format";
import { actions, appStore, useApp } from "../state/app";
import {
  activeConditionCount,
  choiceToList,
  clearConditions,
  EMPTY_FACETS,
  facetOptions,
  FLAG_OPTIONS,
  type FlagOption,
  flagOption,
  flagsFor,
  flipDirection,
  formatRangeValue,
  formatUtcOffset,
  isDateRangeReversed,
  isNumericRangeReversed,
  KIND_OPTIONS,
  LABEL_CHOICES,
  localDateToUtc,
  MULTIPLE,
  normalizeText,
  NUMERIC_RANGES,
  type NumericRange,
  numericRangeCount,
  parseRangeInput,
  RATING_OPTIONS,
  ratingOption,
  ratingPatch,
  singleChoice,
  SORT_OPTIONS,
  sortDirectionLabel,
  toggleLabel,
  utcToLocalDate,
} from "./filterLogic";
import "./library.css";

/** テキストの入力から検索までの時間（ms）。 */
const TEXT_DELAY_MS = 300;
/** 日付の入力から検索までの時間（ms。年の数字を打っている途中で検索しないように。仮置き）。 */
const DATE_DELAY_MS = 600;
/** 数値（長辺の画素数・動画の長さ・fps）の入力から検索までの時間（ms。仮置き）。 */
const NUMBER_DELAY_MS = 600;
/** 評価などの変更（detailsVersion）の後に、カメラ・レンズの候補を読み直すまでの時間（ms。仮置き）。 */
const FACETS_DELAY_MS = 1000;

export function FilterBar() {
  const filter = useApp((s) => s.filter);
  const sort = useApp((s) => s.sort);
  const offset = useApp((s) => s.catalog?.core_settings.default_utc_offset_minutes ?? 0);
  const { facets, failed } = useFacets();
  const active = activeConditionCount(filter);

  const rating = ratingOption(filter);
  const flag = flagOption(filter.flags) ?? "other";

  return (
    <div className="lib-filterbar" role="search" aria-label="絞り込みと並べ替え">
      <TextSearch value={filter.text ?? null} />

      <select
        value={rating}
        aria-label="評価"
        onChange={(e) => actions.setFilter(ratingPatch(Number(e.target.value)))}
      >
        {rating === -1 && (
          <option value={-1} disabled>
            評価: その他の条件
          </option>
        )}
        {RATING_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>

      <select
        value={flag}
        aria-label="フラグ"
        onChange={(e) => actions.setFilter({ flags: flagsFor(e.target.value as FlagOption) })}
      >
        {flag === "other" && (
          <option value="other" disabled>
            フラグ: その他の条件
          </option>
        )}
        {FLAG_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>

      <span className="lib-labels" role="group" aria-label="カラーラベル（複数選べる）">
        {LABEL_CHOICES.map((l) => {
          const on = filter.color_labels?.includes(l) ?? false;
          const name = l ? `ラベル: ${COLOR_LABEL_NAMES[l]}` : "ラベルなし";
          return (
            <button
              key={l ?? "none"}
              type="button"
              className={`lib-label-btn${on ? " on" : ""}`}
              aria-pressed={on}
              aria-label={name}
              title={name}
              onClick={() => actions.setFilter({ color_labels: toggleLabel(filter.color_labels, l) })}
            >
              {l ? <span className={`label-dot ${l}`} /> : "なし"}
            </button>
          );
        })}
      </span>

      <select
        value={filter.kind ?? ""}
        aria-label="種類"
        onChange={(e) => actions.setFilter({ kind: e.target.value === "" ? null : (e.target.value as AssetKind) })}
      >
        {KIND_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>

      <FacetSelect
        label="カメラ"
        values={facets.cameras}
        selected={filter.cameras}
        failed={failed}
        onChange={(v) => actions.setFilter({ cameras: v })}
      />
      <FacetSelect
        label="レンズ"
        values={facets.lenses}
        selected={filter.lenses}
        failed={failed}
        onChange={(v) => actions.setFilter({ lenses: v })}
      />

      {(facets.codecs.length > 0 || (filter.codecs?.length ?? 0) > 0) && (
        <FacetSelect
          label="コーデック"
          title="動画のコーデック（指定すると写真は除く。VID-03）"
          values={facets.codecs}
          selected={filter.codecs}
          failed={failed}
          onChange={(v) => actions.setFilter({ codecs: v })}
        />
      )}

      <MoreConditions filter={filter} />

      <DateRange filter={filter} offset={offset} />

      <span className="spacer" />

      <SortControl sort={sort} />

      {active > 0 && (
        <span className="lib-filter-count" title={`有効な条件: ${active} 個`}>
          {active}
        </span>
      )}
      <button
        type="button"
        className="btn small"
        disabled={active === 0}
        onClick={() => actions.replaceFilter(clearConditions(filter))}
        title="絞り込みの条件をすべて解除する（フォルダの選択は残す）"
      >
        条件を解除
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// テキスト
// ---------------------------------------------------------------------------

function TextSearch({ value }: { value: string | null }) {
  const [text, setText] = useState(value ?? "");
  const composing = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const cancel = () => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  };
  const commit = (s: string) => {
    cancel();
    const t = normalizeText(s);
    if (t !== (appStore.get().filter.text ?? null)) actions.setFilter({ text: t });
  };
  const schedule = (s: string) => {
    cancel();
    timer.current = setTimeout(() => commit(s), TEXT_DELAY_MS);
  };

  // 外から変わったとき（条件の解除など）は、入力欄を合わせる。
  useEffect(() => {
    setText((cur) => (normalizeText(cur) === value ? cur : (value ?? "")));
  }, [value]);
  useEffect(() => cancel, []);

  return (
    <input
      type="search"
      className="lib-filter-search"
      placeholder="ファイル名・キャプション"
      aria-label="テキストで検索（ファイル名・キャプション）"
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        if (!composing.current && !(e.nativeEvent as InputEvent).isComposing) schedule(e.target.value);
      }}
      onCompositionStart={() => {
        composing.current = true;
        cancel();
      }}
      onCompositionEnd={(e) => {
        composing.current = false;
        schedule(e.currentTarget.value);
      }}
      onKeyDown={(e) => {
        // IME の変換を確定する Enter（変換中・keyCode 229）では検索しない。
        if (composing.current || e.nativeEvent.isComposing || e.keyCode === 229) return;
        if (e.key === "Enter") commit(e.currentTarget.value);
        else if (e.key === "Escape") {
          setText("");
          commit("");
        }
      }}
    />
  );
}

// ---------------------------------------------------------------------------
// カメラ・レンズ
// ---------------------------------------------------------------------------

/** 絞り込みの候補（取り込み・削除の後はすぐに、評価などの変更の後は落ち着いてから読み直す）。 */
function useFacets(): { facets: Facets; failed: boolean } {
  const foldersVersion = useApp((s) => s.foldersVersion);
  const detailsVersion = useApp((s) => s.detailsVersion);
  const [state, setState] = useState<{ facets: Facets; failed: boolean }>({ facets: EMPTY_FACETS, failed: false });
  const lastFolders = useRef<number | null>(null);
  useEffect(() => {
    const immediate = lastFolders.current !== foldersVersion;
    let alive = true;
    const t = setTimeout(
      () => {
        // 要求を始めたときに覚える（取り消した実行では覚えない）。
        lastFolders.current = foldersVersion;
        core
          .facets()
          .then((facets) => {
            if (alive) setState({ facets, failed: false });
          })
          .catch((e: unknown) => {
            // 候補がなくても絞り込みは使える（前に読めた候補は残す）。
            if (alive) setState((prev) => ({ facets: prev.facets, failed: true }));
            console.warn("絞り込みの候補を読めません", e);
          });
      },
      immediate ? 0 : FACETS_DELAY_MS,
    );
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [foldersVersion, detailsVersion]);
  return state;
}

function FacetSelect(props: {
  label: string;
  title?: string;
  values: FacetValue[];
  selected: string[] | null | undefined;
  failed: boolean;
  onChange: (v: string[] | null) => void;
}) {
  const options = facetOptions(props.values, props.selected);
  const value = singleChoice(props.selected);
  const empty = options.length === 0 && value === "";
  return (
    <select
      value={value}
      aria-label={props.label}
      disabled={empty}
      title={
        empty ? (props.failed ? `${props.label}の候補を読めません` : `${props.label}の候補がありません`) : props.title
      }
      onChange={(e) => props.onChange(choiceToList(e.target.value))}
    >
      <option value="">{props.label}: すべて</option>
      {value === MULTIPLE && (
        <option value={MULTIPLE} disabled>
          {props.label}: 複数
        </option>
      )}
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.count > 0 ? `${o.value}（${o.count.toLocaleString("ja-JP")}）` : o.value}
        </option>
      ))}
    </select>
  );
}

// ---------------------------------------------------------------------------
// 詳しい条件（長辺の画素数・動画の長さ・フレームレート。VID-03）
// ---------------------------------------------------------------------------

function MoreConditions({ filter }: { filter: SearchFilter }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  const panelId = useId();
  const count = numericRangeCount(filter);

  // 外を押したら閉じる。
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [open]);

  return (
    <span
      className="lib-filter-more"
      ref={ref}
      onKeyDown={(e) => {
        // Escape は閉じるだけにする（ルーペから戻るなどの全体のキーに渡さない）。
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        className={`btn small${count > 0 ? " active" : ""}`}
        aria-expanded={open}
        aria-controls={panelId}
        title="長辺の画素数・動画の長さ・フレームレートで絞り込む"
        onClick={() => setOpen((o) => !o)}
      >
        詳しい条件{count > 0 ? `（${count}）` : ""} ▾
      </button>
      {open && (
        <div id={panelId} className="lib-filter-pop" role="group" aria-label="詳しい条件">
          {NUMERIC_RANGES.map((r) => (
            <NumericRangeRow key={r.id} range={r} filter={filter} />
          ))}
          <p className="hint">
            下限・上限ともその値を含みます。動画の長さ・フレームレートを指定すると、写真と、その値の分からない動画は
            除きます。長辺の画素数は写真にも動画にも効きます。
          </p>
        </div>
      )}
    </span>
  );
}

function NumericRangeRow({ range, filter }: { range: NumericRange; filter: SearchFilter }) {
  const reversed = isNumericRangeReversed(filter, range);
  // 確定の時点の条件と比べる（入力の遅延の間に条件が変わっていてもよいように）。
  const commit = (key: NumericRange["min"] | NumericRange["max"], v: number | null) => {
    if (v !== (appStore.get().filter[key] ?? null)) actions.setFilter({ [key]: v });
  };
  return (
    <div
      className={`lib-filter-range${reversed ? " reversed" : ""}`}
      title={reversed ? "下限が上限より大きいため、該当する写真はありません" : undefined}
    >
      <span className="label">{range.label}</span>
      <NumberInput
        value={filter[range.min] ?? null}
        range={range}
        label={`${range.label}の下限（${range.unit}）`}
        placeholder="下限"
        onCommit={(v) => commit(range.min, v)}
      />
      <span className="label">〜</span>
      <NumberInput
        value={filter[range.max] ?? null}
        range={range}
        label={`${range.label}の上限（${range.unit}）`}
        placeholder="上限"
        onCommit={(v) => commit(range.max, v)}
      />
      <span className="label">{range.unit}</span>
    </div>
  );
}

/**
 * 数値の入力欄（全角の数字も読む。IME の変換中は反映しない）。読めない値では条件を変えず、欄に印を付ける。
 * 欄が消えるとき（「詳しい条件」を閉じたとき）も、編集中の値を反映する。
 */
function NumberInput(props: {
  value: number | null;
  range: NumericRange;
  label: string;
  placeholder: string;
  onCommit: (v: number | null) => void;
}) {
  const { integer, limit } = props.range;
  const [text, setText] = useState(() => formatRangeValue(props.value));
  const [invalid, setInvalid] = useState(false);
  const dirty = useRef(false);
  const composing = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const latest = useRef({ text, onCommit: props.onCommit });
  latest.current = { text, onCommit: props.onCommit };

  // 外から変わったとき（条件の解除など）は、編集していなければ合わせる。
  useEffect(() => {
    if (dirty.current) return;
    setText(formatRangeValue(props.value));
    setInvalid(false);
  }, [props.value]);

  const cancel = () => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  };
  /** 入力を反映する（`unmounting` のときは表示を変えない）。 */
  const flush = (unmounting = false) => {
    cancel();
    if (!dirty.current) return;
    dirty.current = false;
    const p = parseRangeInput(latest.current.text, integer, limit);
    if (!p.ok) {
      if (!unmounting) setInvalid(true);
      return;
    }
    latest.current.onCommit(p.value);
    if (!unmounting) {
      setInvalid(false);
      // 表示を整える（"３８４０" → "3840"）。
      setText(formatRangeValue(p.value));
    }
  };
  const flushRef = useRef(flush);
  flushRef.current = flush;
  useEffect(() => () => flushRef.current(true), []);

  return (
    <input
      type="text"
      inputMode={integer ? "numeric" : "decimal"}
      className={`lib-filter-number${invalid ? " invalid" : ""}`}
      value={text}
      placeholder={props.placeholder}
      aria-label={props.label}
      aria-invalid={invalid || undefined}
      title={invalid ? (integer ? "0 以上の整数を入力してください" : "0 以上の数を入力してください") : props.label}
      onChange={(e) => {
        dirty.current = true;
        setText(e.target.value);
        cancel();
        if (!composing.current && !(e.nativeEvent as InputEvent).isComposing) {
          timer.current = setTimeout(() => flushRef.current(), NUMBER_DELAY_MS);
        }
      }}
      onCompositionStart={() => {
        composing.current = true;
        cancel();
      }}
      onCompositionEnd={() => {
        composing.current = false;
        cancel();
        timer.current = setTimeout(() => flushRef.current(), NUMBER_DELAY_MS);
      }}
      onBlur={() => flush()}
      onKeyDown={(e) => {
        // IME の変換を確定する Enter では反映しない。
        if (composing.current || e.nativeEvent.isComposing || e.keyCode === 229) return;
        if (e.key === "Enter") flush();
      }}
    />
  );
}

// ---------------------------------------------------------------------------
// 撮影日
// ---------------------------------------------------------------------------

function DateRange({ filter, offset }: { filter: SearchFilter; offset: number }) {
  const from = utcToLocalDate(filter.captured_from, offset, false);
  const to = utcToLocalDate(filter.captured_until, offset, true);
  const reversed = isDateRangeReversed(from, to);
  const tz = formatUtcOffset(offset);
  // 確定の時点の条件と比べる（入力の遅延の間に条件が変わっていてもよいように）。
  const commit = (key: "captured_from" | "captured_until", date: string) => {
    const v = localDateToUtc(date, offset, key === "captured_until");
    if (v !== (appStore.get().filter[key] ?? null)) actions.setFilter({ [key]: v });
  };
  return (
    <span
      className={`lib-filter-group lib-filter-dates${reversed ? " reversed" : ""}`}
      title={`撮影日（既定のタイムゾーン ${tz} の日付。終了日はその日の終わりまでを含む）${
        reversed ? "\n開始日が終了日より後のため、該当する写真はありません" : ""
      }`}
    >
      <span className="label">撮影日</span>
      <DateInput value={from} label="撮影日の開始" onCommit={(d) => commit("captured_from", d)} />
      <span className="label">〜</span>
      <DateInput value={to} label="撮影日の終了（その日を含む）" onCommit={(d) => commit("captured_until", d)} />
    </span>
  );
}

/**
 * 日付の入力欄。年月日を打っている途中（validity.badInput）の値で条件を消さないよう、値を React で
 * 持たずに（非制御）、条件が外から変わったときだけ書き換える。
 */
function DateInput(props: { value: string; label: string; onCommit: (date: string) => void }) {
  const ref = useRef<HTMLInputElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const commitRef = useRef(props.onCommit);
  commitRef.current = props.onCommit;

  useEffect(() => {
    const el = ref.current;
    if (!el || el.value === props.value) return;
    if (document.activeElement === el && el.validity.badInput) return;
    el.value = props.value;
  }, [props.value]);
  useEffect(
    () => () => {
      if (timer.current !== null) clearTimeout(timer.current);
    },
    [],
  );

  const flush = () => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
    const el = ref.current;
    if (el && !el.validity.badInput) commitRef.current(el.value);
  };

  return (
    <input
      ref={ref}
      type="date"
      // 未指定（WebKit は今日の日付を薄く出す）と、指定した日付を見分けられるようにする。
      className={props.value === "" ? "empty" : undefined}
      defaultValue={props.value}
      aria-label={props.label}
      min="0001-01-01"
      max="9999-12-31"
      onChange={(e) => {
        if (e.target.validity.badInput) return;
        if (timer.current !== null) clearTimeout(timer.current);
        timer.current = setTimeout(flush, DATE_DELAY_MS);
      }}
      onBlur={() => {
        if (timer.current !== null) flush();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") flush();
      }}
    />
  );
}

// ---------------------------------------------------------------------------
// 並べ替え
// ---------------------------------------------------------------------------

function SortControl({ sort }: { sort: SearchSort }) {
  const dir = sortDirectionLabel(sort.direction);
  return (
    <span className="lib-filter-group">
      <span className="label">並べ替え</span>
      <select
        value={sort.key}
        aria-label="並べ替えの項目"
        title={sort.key === "capture_time" ? "撮影日時のない写真は、どちらの向きでも最後に並びます" : undefined}
        onChange={(e) => actions.setSort({ ...sort, key: e.target.value as SortKey })}
      >
        {SORT_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
      <button
        type="button"
        className="btn small"
        aria-label={`並べ替えの向き: ${dir}（押すと逆にする）`}
        title={`${dir}（押すと逆にする）`}
        onClick={() => actions.setSort(flipDirection(sort))}
      >
        {sort.direction === "ascending" ? "↑ 昇順" : "↓ 降順"}
      </button>
    </span>
  );
}
