// 右のパネルのキーワードの欄（LIB-09）と、写真が入っているコレクション（LIB-10）。
//
// - 対象（targets）は、グリッドでは選択中のすべて、ルーペでは表示中の 1 枚（actions.targets）。付いている
//   キーワードを並べ（一部の写真だけに付いているものは薄く「一部」と出す）、× で対象のすべてから外す。
// - 入力欄に「場所 > 京都, 家族」のように書いて Enter で付ける（「,」「、」で複数、「>」で階層。なければ作る）。
//   入力中は、最後の項目の最後の段で既存のキーワードを補完する（↑↓ で選び、Tab・Enter で入れる）。IME の
//   変換中の Enter は確定だけで、付けない（UX-01）。Ctrl / ⌘ + K でこの欄に移る。
// - 一覧は対象・キーワードの変更（organizeVersion）で読み直す。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import type { CollectionInfo, KeywordHit, VariantId, VariantKeywords } from "../api/types";
import { Section } from "../components/Section";
import { comboLabel } from "../keyboard";
import { actions, useApp } from "../state/app";
import { addKeywordText, KEYWORD_INPUT_ID, removeFromCollection, setKeyword } from "../state/organize";
import { collectionPathText } from "./collectionLogic";
import {
  applyCompletion,
  completionQuery,
  keywordPathText,
  parseKeywordInput,
  sortedVariantKeywords,
} from "./keywordLogic";

/** 入力から補完の候補を探すまでの時間（ms。仮置き）。 */
const COMPLETE_DELAY_MS = 150;
/** 補完の候補の数（仮置き）。 */
const COMPLETE_LIMIT = 8;

export function KeywordEditor(props: { targets: VariantId[]; disabled?: boolean }) {
  const { targets } = props;
  const organizeVersion = useApp((s) => s.organizeVersion);
  const key = targets.join(",");
  const [summary, setSummary] = useState<{ key: string; data: VariantKeywords } | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    if (targets.length === 0) {
      setSummary(null);
      return;
    }
    core
      .variantKeywords(targets)
      .then((data) => {
        if (!alive) return;
        setSummary({ key, data });
        setError(null);
      })
      .catch((e: unknown) => alive && setError(actions.describeError(e)));
    return () => {
      alive = false;
    };
    // targets は key で比べる。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, organizeVersion]);
  const data = summary?.key === key ? summary.data : null;
  const list = data ? sortedVariantKeywords(data) : [];
  const multi = targets.length > 1;

  return (
    <Section id="lib.meta.keywords" title={data && data.keywords.length > 0 ? `キーワード（${data.keywords.length}）` : "キーワード"}>
      {error && <p className="error-text">キーワードを読めません: {error}</p>}
      {list.length > 0 && (
        <ul className="lib-keyword-chips" aria-label="付いているキーワード">
          {list.map((k) => {
            const partial = data !== null && k.count < data.variants;
            return (
              <li key={k.keyword_id} className={partial ? "partial" : undefined}>
                <span className="name" title={keywordPathText(k.path)}>
                  {k.name}
                  {partial && <span className="note">（{k.count.toLocaleString("ja-JP")} 枚）</span>}
                </span>
                <button
                  type="button"
                  className="remove"
                  aria-label={`${k.name} を外す`}
                  title={multi ? "選択中のすべてから外す" : "外す"}
                  disabled={props.disabled}
                  onClick={() => void setKeyword(targets, k.keyword_id, false)}
                >
                  ×
                </button>
              </li>
            );
          })}
        </ul>
      )}
      <KeywordInput targets={targets} disabled={props.disabled} />
      <p className="hint lib-meta-note">
        「,」で区切って複数、「&gt;」で階層（例: 場所 &gt; 京都, 家族）。{multi ? `選択中の ${targets.length.toLocaleString("ja-JP")} 枚に付けます。` : ""}
        {comboLabel("mod+k")} でこの欄へ。
      </p>
    </Section>
  );
}

function KeywordInput(props: { targets: VariantId[]; disabled?: boolean }) {
  const [text, setText] = useState("");
  const [hits, setHits] = useState<KeywordHit[]>([]);
  const [active, setActive] = useState(-1);
  const [busy, setBusy] = useState(false);
  const composing = useRef(false);
  const seq = useRef(0);

  useEffect(() => {
    const q = completionQuery(text);
    const my = ++seq.current;
    if (q === "") {
      setHits([]);
      return;
    }
    const t = setTimeout(() => {
      core
        .findKeywords(q, COMPLETE_LIMIT)
        .then((h) => {
          if (seq.current === my) {
            setHits(h);
            setActive(-1);
          }
        })
        .catch(() => seq.current === my && setHits([]));
    }, COMPLETE_DELAY_MS);
    return () => clearTimeout(t);
  }, [text]);

  const submit = async () => {
    if (parseKeywordInput(text).length === 0 || props.targets.length === 0) return;
    setBusy(true);
    const n = await addKeywordText(props.targets, text);
    setBusy(false);
    if (n !== null) {
      setText("");
      setHits([]);
    }
  };
  const complete = (h: KeywordHit) => {
    setText(applyCompletion(text, h.path));
    setHits([]);
    setActive(-1);
  };

  return (
    <div className="lib-keyword-input">
      <input
        id={KEYWORD_INPUT_ID}
        type="text"
        value={text}
        disabled={props.disabled || busy || props.targets.length === 0}
        placeholder="キーワードを入れて Enter"
        aria-label="写真に付けるキーワード"
        aria-autocomplete="list"
        aria-expanded={hits.length > 0}
        onChange={(e) => setText(e.target.value)}
        onCompositionStart={() => {
          composing.current = true;
        }}
        onCompositionEnd={() => {
          composing.current = false;
        }}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing || composing.current || e.keyCode === 229) return;
          if (e.key === "ArrowDown" && hits.length > 0) {
            e.preventDefault();
            setActive((a) => (a + 1) % hits.length);
          } else if (e.key === "ArrowUp" && hits.length > 0) {
            e.preventDefault();
            setActive((a) => (a <= 0 ? hits.length - 1 : a - 1));
          } else if (e.key === "Tab" && hits.length > 0) {
            e.preventDefault();
            complete(hits[Math.max(0, active)]);
          } else if (e.key === "Enter") {
            e.preventDefault();
            if (active >= 0 && hits[active]) complete(hits[active]);
            else void submit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            if (hits.length > 0) setHits([]);
            else {
              setText("");
              (e.target as HTMLInputElement).blur();
            }
          }
        }}
        onBlur={() => setTimeout(() => setHits([]), 150)}
      />
      {hits.length > 0 && (
        <ul className="lib-keyword-hits" role="listbox" aria-label="キーワードの候補">
          {hits.map((h, i) => (
            <li
              key={h.keyword_id}
              role="option"
              aria-selected={i === active}
              className={i === active ? "active" : undefined}
              onMouseDown={(e) => {
                e.preventDefault();
                complete(h);
              }}
            >
              {keywordPathText(h.path)}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** 写真（1 枚）が入っている手動のコレクション。クリックで表示、× で外す。 */
export function PhotoCollections(props: { variantId: VariantId }) {
  const organizeVersion = useApp((s) => s.organizeVersion);
  const [state, setState] = useState<{ id: VariantId; ids: number[]; all: CollectionInfo[] } | null>(null);
  useEffect(() => {
    let alive = true;
    Promise.all([core.variantCollections(props.variantId), core.collections(false)])
      .then(([ids, all]) => alive && setState({ id: props.variantId, ids, all }))
      .catch(() => alive && setState(null));
    return () => {
      alive = false;
    };
  }, [props.variantId, organizeVersion]);
  if (!state || state.id !== props.variantId || state.ids.length === 0) return null;
  return (
    <Section id="lib.meta.collections" title={`コレクション（${state.ids.length}）`}>
      <ul className="lib-keyword-chips" aria-label="入っているコレクション">
        {state.ids.map((id) => {
          const c = state.all.find((x) => x.collection_id === id);
          if (!c) return null;
          return (
            <li key={id}>
              <span
                className="name link"
                title={`${collectionPathText(state.all, id)}（クリックで表示）`}
                onClick={() => actions.showCollection(id)}
              >
                {c.name}
              </span>
              <button
                type="button"
                className="remove"
                aria-label={`${c.name} から外す`}
                title="このコレクションから外す（写真は削除しない）"
                onClick={() => void removeFromCollection(id, [props.variantId], c.name)}
              >
                ×
              </button>
            </li>
          );
        })}
      </ul>
    </Section>
  );
}
