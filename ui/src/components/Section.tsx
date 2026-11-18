// 折りたためるパネルの区切り（開閉の状態はブラウザに保存する）。

import { type ReactNode, useState } from "react";

import { Icon } from "./Icon";

function load(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(`genzo.section.${key}`);
    return v === null ? fallback : v === "1";
  } catch {
    return fallback;
  }
}

export function Section(props: {
  id: string;
  title: ReactNode;
  children: ReactNode;
  defaultOpen?: boolean;
  right?: ReactNode;
}) {
  const [open, setOpen] = useState(() => load(props.id, props.defaultOpen ?? true));
  const toggle = () => {
    const v = !open;
    setOpen(v);
    try {
      localStorage.setItem(`genzo.section.${props.id}`, v ? "1" : "0");
    } catch {
      // 保存できなくても開閉はできる。
    }
  };
  return (
    <div className="section">
      <div className="section-head" onClick={toggle}>
        <span className="title">{props.title}</span>
        {props.right && (
          <span className="side" onClick={(e) => e.stopPropagation()}>
            {props.right}
          </span>
        )}
        <span className="chev">
          <Icon name={open ? "chevronDown" : "chevronRight"} />
        </span>
      </div>
      {open && <div className="section-body">{props.children}</div>}
    </div>
  );
}
