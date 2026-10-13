// スマートコレクションの条件の編集（LIB-11）: 名前・場所（コレクションセット）・条件（すべて / いずれか、
// 入れ子のグループ）と、一致する写真の数の見込み。
//
// - 条件の種類: 評価・フラグ・カラーラベル・キーワード（下の階層を含む）・撮影日（期間・直近の日数・不明）・
//   カメラ・レンズ・種類（写真 / 動画）・テキスト・フォルダ・コレクション。
// - 撮影日は、カタログの既定のタイムゾーンの日付として入れ、UTC の境界にして保存する（絞り込みと同じ。
//   開始日の 0 時を含み、終了日の翌日の 0 時を含まない）。
// - 一致する数は、条件を変えてから少し待って core.count で数える（表示中の検索結果は変えない。50 万件では
//   1 回 0.1〜0.4 秒ほど）。検証で問題がある間は数えない。
// - 保存のときにコアも検証する（循環する参照など）。失敗はダイアログの中に出す（閉じない）。

import { useEffect, useMemo, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type {
  CollectionId,
  CollectionInfo,
  ErrorInfo,
  Facets,
  FolderInfo,
  KeywordInfo,
  MatchMode,
  SmartCondition,
  SmartRule,
} from "../api/types";
import { Dialog } from "../components/Dialog";
import { COLOR_LABEL_NAMES } from "../format";
import { collectionPathText, parentChoices, validateCollectionName } from "../library/collectionLogic";
import { localDateToUtc, utcToLocalDate } from "../library/filterLogic";
import { keywordPath, keywordPathText } from "../library/keywordLogic";
import {
  appendTo,
  CONDITION_TYPES,
  type ConditionType,
  defaultCondition,
  emptyRule,
  FLAG_CHOICES,
  KIND_CHOICES,
  MATCH_CHOICES,
  MAX_SMART_DAYS,
  MAX_SMART_DEPTH,
  OPS,
  removeAt,
  replaceAt,
  ruleProblems,
  setMatch,
  withOp,
} from "../library/smartRuleLogic";
import { actions, useApp } from "../state/app";
import { ErrorBox } from "./parts";

/** 条件を変えてから数えるまでの時間（ms。仮置き）。 */
const COUNT_DELAY_MS = 400;

/** 選択肢の材料（開いたときに読む）。 */
interface Sources {
  collections: CollectionInfo[];
  keywords: KeywordInfo[];
  folders: FolderInfo[];
  facets: Facets;
}

async function loadSources(): Promise<Sources> {
  const [collections, keywords, folders, facets] = await Promise.all([
    core.collections(false),
    core.keywordTree(),
    core.folders(),
    core.facets().catch(() => ({ cameras: [], lenses: [], codecs: [] }) as Facets),
  ]);
  return { collections, keywords, folders, facets };
}

export function SmartCollectionDialog(props: { collectionId?: CollectionId; parentId?: CollectionId | null }) {
  const editing = props.collectionId !== undefined;
  const offset = useApp((s) => s.catalog?.core_settings.default_utc_offset_minutes ?? 0);
  const [src, setSrc] = useState<Sources | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [rule, setRule] = useState<SmartRule | null>(editing ? null : emptyRule());
  const [name, setName] = useState("");
  const [originalName, setOriginalName] = useState("");
  const [parent, setParent] = useState<string>(props.parentId == null ? "root" : String(props.parentId));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [count, setCount] = useState<{ n: number | null; failed: boolean }>({ n: null, failed: false });

  useEffect(() => {
    let alive = true;
    loadSources()
      .then((s) => {
        if (!alive) return;
        setSrc(s);
        if (editing) {
          const c = s.collections.find((x) => x.collection_id === props.collectionId);
          if (!c) {
            setLoadError("スマートコレクションが見つかりません（削除された可能性があります）");
            return;
          }
          if (c.rule_error || !c.rule) {
            setLoadError(`条件を読めないため編集できません: ${c.rule_error ?? "条件がありません"}`);
            return;
          }
          setRule(c.rule);
          setName(c.name);
          setOriginalName(c.name);
        }
      })
      .catch((e: unknown) => alive && setLoadError(actions.describeError(e)));
    return () => {
      alive = false;
    };
  }, [editing, props.collectionId]);

  const problems = useMemo(() => (rule ? ruleProblems(rule) : []), [rule]);
  const ruleKey = rule ? JSON.stringify(rule) : "";
  useEffect(() => {
    if (!rule || problems.length > 0) {
      setCount({ n: null, failed: false });
      return;
    }
    let alive = true;
    const t = setTimeout(() => {
      core
        .count({ smart_rule: rule })
        .then((n) => alive && setCount({ n, failed: false }))
        .catch(() => alive && setCount({ n: null, failed: true }));
    }, COUNT_DELAY_MS);
    return () => {
      alive = false;
      clearTimeout(t);
    };
    // 条件は JSON の文字列で比べる。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ruleKey, problems.length]);

  const nameProblem = validateCollectionName(name);
  const disabled = busy || !rule || !src || problems.length > 0 || nameProblem !== null;
  const save = async () => {
    if (disabled || !rule) return;
    setBusy(true);
    setError(null);
    try {
      if (editing) {
        if (name.trim() !== originalName) await core.renameCollection(props.collectionId!, name.trim());
        await core.setSmartRule(props.collectionId!, rule);
      } else {
        const id = await core.createCollection(parent === "root" ? null : Number(parent), name.trim(), "smart", rule);
        actions.showCollection(id);
      }
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={editing ? "スマートコレクションを編集" : "スマートコレクションを作る"}
      width={760}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <span className="smart-count" aria-live="polite">
            {problems.length > 0
              ? ""
              : count.failed
                ? "数えられません"
                : count.n === null
                  ? "数えています…"
                  : `一致する写真: ${count.n.toLocaleString("ja-JP")} 枚`}
          </span>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void save()} disabled={disabled}>
            {editing ? "保存" : "作る"}
          </button>
        </>
      }
    >
      {loadError && <p className="error-text">{loadError}</p>}
      <div className="field">
        <label htmlFor="smart-name">名前</label>
        <input id="smart-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      {!editing && src && (
        <div className="field">
          <label htmlFor="smart-parent">場所</label>
          <select id="smart-parent" value={parent} onChange={(e) => setParent(e.target.value)}>
            {parentChoices(src.collections).map((c) => (
              <option key={c.id ?? "root"} value={c.id === null ? "root" : String(c.id)}>
                {c.label}
              </option>
            ))}
          </select>
        </div>
      )}
      {rule && src && (
        <div className="smart-editor">
          <GroupEditor
            path={[]}
            depth={1}
            match={rule.match}
            rules={rule.rules}
            onRule={setRule}
            rule={rule}
            src={src}
            offset={offset}
            self={props.collectionId ?? null}
          />
        </div>
      )}
      {problems.length > 0 && (
        <ul className="smart-problems" role="alert">
          {problems.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      )}
      <ErrorBox error={error} />
    </Dialog>
  );
}

interface EditorProps {
  rule: SmartRule;
  onRule: (r: SmartRule) => void;
  src: Sources;
  offset: number;
  /** 編集中のスマートコレクション（コレクションの条件の選択肢から除く）。 */
  self: CollectionId | null;
}

function GroupEditor(
  props: EditorProps & { path: number[]; depth: number; match: MatchMode; rules: SmartCondition[] },
) {
  const { rule, onRule, path, depth } = props;
  return (
    <div className={`smart-group${depth > 1 ? " nested" : ""}`}>
      <div className="smart-group-head">
        <select
          value={props.match}
          aria-label="すべて / いずれか"
          onChange={(e) => onRule(setMatch(rule, path, e.target.value as MatchMode))}
        >
          {MATCH_CHOICES.map((m) => (
            <option key={m.value} value={m.value}>
              {m.label}
            </option>
          ))}
        </select>
        <span className="spacer" />
        <button type="button" className="btn small" onClick={() => onRule(appendTo(rule, path, defaultCondition("rating")))}>
          ＋ 条件
        </button>
        <button
          type="button"
          className="btn small"
          disabled={depth >= MAX_SMART_DEPTH}
          title={depth >= MAX_SMART_DEPTH ? `入れ子は ${MAX_SMART_DEPTH} 段までです` : "条件のグループ（入れ子）を足す"}
          onClick={() => onRule(appendTo(rule, path, defaultCondition("group")))}
        >
          ＋ グループ
        </button>
      </div>
      {props.rules.length === 0 && (
        <p className="hint">{props.match === "all" ? "条件がないので、すべての写真が一致します。" : "条件がないので、どの写真も一致しません。"}</p>
      )}
      {props.rules.map((c, i) => (
        <ConditionRow key={i} {...props} path={[...path, i]} cond={c} />
      ))}
    </div>
  );
}

function ConditionRow(props: EditorProps & { path: number[]; depth: number; cond: SmartCondition }) {
  const { rule, onRule, path, cond } = props;
  const set = (c: SmartCondition) => onRule(replaceAt(rule, path, c));
  const remove = (
    <button type="button" className="btn ghost small" aria-label="この条件を消す" title="この条件を消す" onClick={() => onRule(removeAt(rule, path))}>
      −
    </button>
  );
  const typeSelect = (
    <select
      value={cond.type}
      aria-label="条件の種類"
      onChange={(e) => set(defaultCondition(e.target.value as ConditionType))}
    >
      {CONDITION_TYPES.map((t) => (
        <option key={t.type} value={t.type} disabled={t.type === "group" && props.depth >= MAX_SMART_DEPTH}>
          {t.label}
        </option>
      ))}
    </select>
  );
  if (cond.type === "group") {
    return (
      <div className="smart-row group">
        <div className="smart-row-line">
          {typeSelect}
          <span className="spacer" />
          {remove}
        </div>
        <GroupEditor {...props} path={path} depth={props.depth + 1} match={cond.match} rules={cond.rules} />
      </div>
    );
  }
  const ops = OPS[cond.type];
  return (
    <div className="smart-row">
      <div className="smart-row-line">
        {typeSelect}
        {ops && "op" in cond && (
          <select value={cond.op} aria-label="条件" onChange={(e) => set(withOp(cond, e.target.value))}>
            {ops.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        )}
        <ValueEditor {...props} cond={cond} set={set} />
        <span className="spacer" />
        {remove}
      </div>
    </div>
  );
}

function ValueEditor(props: EditorProps & { cond: SmartCondition; set: (c: SmartCondition) => void }) {
  const { cond, set, src, offset } = props;
  switch (cond.type) {
    case "rating":
      return (
        <select value={cond.value} aria-label="評価" onChange={(e) => set({ ...cond, value: Number(e.target.value) as typeof cond.value })}>
          {[0, 1, 2, 3, 4, 5].map((n) => (
            <option key={n} value={n}>
              {n === 0 ? "★0（評価なし）" : `★${n}`}
            </option>
          ))}
        </select>
      );
    case "flag":
      return (
        <select value={cond.value} aria-label="フラグ" onChange={(e) => set({ ...cond, value: Number(e.target.value) as typeof cond.value })}>
          {FLAG_CHOICES.map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </select>
      );
    case "color_label":
      return (
        <select
          value={cond.value ?? ""}
          aria-label="カラーラベル"
          onChange={(e) => set({ ...cond, value: e.target.value === "" ? null : (e.target.value as typeof cond.value) })}
        >
          {(Object.keys(COLOR_LABEL_NAMES) as (keyof typeof COLOR_LABEL_NAMES)[]).map((l) => (
            <option key={l} value={l}>
              {COLOR_LABEL_NAMES[l]}
            </option>
          ))}
          <option value="">ラベルなし</option>
        </select>
      );
    case "keyword": {
      if (cond.op !== "has" && cond.op !== "not_has") return null;
      const options = src.keywords
        .map((k) => ({ id: k.keyword_id, label: keywordPathText(keywordPath(src.keywords, k.keyword_id)) }))
        .sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
      const known = options.some((o) => o.id === cond.keyword_id);
      return (
        <select value={cond.keyword_id ?? 0} aria-label="キーワード" onChange={(e) => set({ ...cond, keyword_id: Number(e.target.value) })}>
          <option value={0}>（キーワードを選ぶ）</option>
          {!known && cond.keyword_id ? <option value={cond.keyword_id}>（削除されたキーワード）</option> : null}
          {options.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
      );
    }
    case "capture_time":
      if (cond.op === "in_last_days") {
        return (
          <>
            <input
              type="number"
              className="smart-number"
              min={1}
              max={MAX_SMART_DAYS}
              value={cond.days ?? ""}
              aria-label="直近の日数"
              onChange={(e) => set({ ...cond, days: e.target.value === "" ? undefined : Math.trunc(Number(e.target.value)) })}
            />
            <span>日</span>
          </>
        );
      }
      if (cond.op === "range") {
        const from = utcToLocalDate(cond.from, offset, false);
        const to = utcToLocalDate(cond.until, offset, true);
        return (
          <>
            <input
              type="date"
              value={from}
              aria-label="開始日"
              onChange={(e) => set({ ...cond, from: localDateToUtc(e.target.value, offset, false) ?? undefined })}
            />
            <span>〜</span>
            <input
              type="date"
              value={to}
              aria-label="終了日（この日を含む）"
              onChange={(e) => set({ ...cond, until: localDateToUtc(e.target.value, offset, true) ?? undefined })}
            />
          </>
        );
      }
      return null;
    case "camera":
    case "lens": {
      if (cond.op === "unknown") return null;
      const values = (cond.type === "camera" ? src.facets.cameras : src.facets.lenses).map((f) => f.value);
      const listId = `smart-${cond.type}-values`;
      return (
        <>
          <input
            type="text"
            list={listId}
            value={cond.value}
            aria-label={cond.type === "camera" ? "カメラ" : "レンズ"}
            onChange={(e) => set({ ...cond, value: e.target.value })}
          />
          <datalist id={listId}>
            {values.map((v) => (
              <option key={v} value={v} />
            ))}
          </datalist>
        </>
      );
    }
    case "kind":
      return (
        <select value={cond.value} aria-label="種類" onChange={(e) => set({ ...cond, value: e.target.value as typeof cond.value })}>
          {KIND_CHOICES.map((k) => (
            <option key={k.value} value={k.value}>
              {k.label}
            </option>
          ))}
        </select>
      );
    case "text":
      return (
        <input
          type="text"
          value={cond.value}
          aria-label="テキスト（ファイル名・キャプション・タイトルなど）"
          placeholder="ファイル名・キャプション・タイトルなど"
          onChange={(e) => set({ ...cond, value: e.target.value })}
        />
      );
    case "folder": {
      const known = src.folders.some((f) => f.folder_id === cond.folder_id);
      return (
        <>
          <select value={cond.folder_id} aria-label="フォルダ" onChange={(e) => set({ ...cond, folder_id: Number(e.target.value) })}>
            <option value={0}>（フォルダを選ぶ）</option>
            {!known && cond.folder_id ? <option value={cond.folder_id}>（削除されたフォルダ）</option> : null}
            {src.folders.map((f) => (
              <option key={f.folder_id} value={f.folder_id}>
                {f.path ?? f.rel_path}
              </option>
            ))}
          </select>
          <label className="smart-inline-check">
            <input
              type="checkbox"
              checked={cond.include_subfolders}
              onChange={(e) => set({ ...cond, include_subfolders: e.target.checked })}
            />
            サブフォルダを含む
          </label>
        </>
      );
    }
    case "collection": {
      const options = src.collections
        .filter((c) => c.collection_id !== props.self)
        .map((c) => ({ id: c.collection_id, label: collectionPathText(src.collections, c.collection_id) }))
        .sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
      const known = options.some((o) => o.id === cond.collection_id);
      return (
        <select
          value={cond.collection_id}
          aria-label="コレクション"
          onChange={(e) => set({ ...cond, collection_id: Number(e.target.value) })}
        >
          <option value={0}>（コレクションを選ぶ）</option>
          {!known && cond.collection_id ? <option value={cond.collection_id}>（削除されたコレクション）</option> : null}
          {options.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
      );
    }
    default:
      return null;
  }
}
