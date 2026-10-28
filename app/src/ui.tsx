import { useEffect, useState, type KeyboardEvent, type ReactNode } from "react";
import { api } from "./api";
import { avatarColor, initials } from "./util";

/**
 * "Check now", with visible feedback: a check often finishes in a fraction
 * of a second, so the button holds "Checking…" briefly and then confirms.
 */
export function CheckButton({ checking, disabled = false, label = "Check now" }: { checking: boolean; disabled?: boolean; label?: string }) {
  const [phase, setPhase] = useState<"idle" | "asked" | "done">("idle");
  const [since, setSince] = useState(0);
  useEffect(() => {
    if (phase === "asked" && !checking) {
      const wait = Math.max(0, 700 - (Date.now() - since));
      const timer = setTimeout(() => setPhase("done"), wait);
      return () => clearTimeout(timer);
    }
    if (phase === "done") {
      const timer = setTimeout(() => setPhase("idle"), 2000);
      return () => clearTimeout(timer);
    }
  }, [phase, checking, since]);
  const busy = phase === "asked" || (checking && phase !== "done");
  return (
    <button className="btn" disabled={disabled || busy} aria-live="polite"
      onClick={() => { setSince(Date.now()); setPhase("asked"); api.checkNow(); }}>
      {busy ? <><span className="spinner" aria-hidden /> Checking…</> : phase === "done" ? "✓ Checked" : label}
    </button>
  );
}

export function Avatar({ name, size = 22 }: { name: string; size?: number }) {
  return (
    <span className="avatar" style={{ width: size, height: size, fontSize: Math.round(size * 0.38), "--hue": avatarColor(name) } as React.CSSProperties} aria-hidden>
      {initials(name)}
    </span>
  );
}

export function Switch({ on, onChange, label }: { on: boolean; onChange: (on: boolean) => void; label: string }) {
  return <button type="button" role="switch" aria-checked={on} aria-label={label} className={`switch ${on ? "on" : ""}`} onClick={() => onChange(!on)} />;
}

export function Segmented<T extends string | number>({ options, value, onChange, label }: {
  options: { value: T; label: string }[]; value: T; onChange: (value: T) => void; label: string;
}) {
  return (
    <div className="seg" role="group" aria-label={label}>
      {options.map((o) => (
        <button type="button" key={String(o.value)} className={o.value === value ? "on" : ""} aria-pressed={o.value === value} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function SwitchRow({ title, help, on, onChange, className = "" }: {
  title: string; help?: ReactNode; on: boolean; onChange: (on: boolean) => void; className?: string;
}) {
  return (
    <div className={`switch-row ${className}`}>
      <div className="stack">
        <span className="label">{title}</span>
        {help && <span className="help">{help}</span>}
      </div>
      <Switch on={on} onChange={onChange} label={title} />
    </div>
  );
}

/** A list of names edited as removable tags, with suggestions to pick from. */
export function TagInput({ values, onChange, placeholder, suggestions, people = false, id }: {
  values: string[]; onChange: (values: string[]) => void; placeholder: string; suggestions: string[]; people?: boolean; id: string;
}) {
  const [text, setText] = useState("");
  const add = (raw: string) => {
    const name = raw.trim();
    if (!name) return;
    // Prefer the exact spelling Azure DevOps uses, since names must match.
    const known = suggestions.find((s) => s.toLowerCase() === name.toLowerCase()) ?? name;
    if (!values.some((v) => v.toLowerCase() === known.toLowerCase())) onChange([...values, known]);
    setText("");
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" || e.key === ",") {
      e.preventDefault();
      add(text);
    } else if (e.key === "Backspace" && !text && values.length) {
      onChange(values.slice(0, -1));
    }
  };
  const remaining = suggestions.filter((s) => !values.includes(s));
  return (
    <div className="tags">
      {values.map((v) => (
        <span className="tag" key={v} style={people ? { paddingLeft: 3 } : undefined}>
          {people && <Avatar name={v} size={16} />}
          {v}
          <button type="button" aria-label={`Remove ${v}`} onClick={() => onChange(values.filter((x) => x !== v))}>×</button>
        </span>
      ))}
      <input
        value={text} placeholder={placeholder} list={id} aria-label={placeholder}
        onChange={(e) => (remaining.includes(e.target.value) ? add(e.target.value) : setText(e.target.value))}
        onKeyDown={onKeyDown} onBlur={() => add(text)}
      />
      <datalist id={id}>{remaining.map((s) => <option key={s} value={s} />)}</datalist>
    </div>
  );
}

/** A page's title, a line under it, and its tools on the right. Drags the window. */
export function PageHeader({ title, sub, crumbs, children }: {
  title: ReactNode; sub?: ReactNode; crumbs?: ReactNode; children?: ReactNode;
}) {
  return (
    <header className="topbar" data-tauri-drag-region>
      <div className="page-title" data-tauri-drag-region>
        {crumbs && <div className="crumbs">{crumbs}</div>}
        <h1 data-tauri-drag-region>{title}</h1>
        {sub && <div className="page-sub" data-tauri-drag-region>{sub}</div>}
      </div>
      {children && <div className="tools">{children}</div>}
    </header>
  );
}

export function SearchBox({ value, onChange, placeholder, className = "" }: {
  value: string; onChange: (value: string) => void; placeholder: string; className?: string;
}) {
  return (
    <label className={`search-box ${className}`}>
      <svg viewBox="0 0 24 24" aria-hidden><circle cx="11" cy="11" r="7" /><path d="M20 20l-3.5-3.5" /></svg>
      <input type="search" value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} aria-label={placeholder} />
    </label>
  );
}

/** The design's square tick box, as a button so it works inside clickable cards. */
export function Checkbox({ on, onChange, label, small = false }: { on: boolean; onChange: (on: boolean) => void; label: string; small?: boolean }) {
  return (
    <button type="button" role="checkbox" aria-checked={on} aria-label={label} title={label} className={`checkbox ${small ? "small" : ""} ${on ? "on" : ""}`}
      onClick={(e) => { e.stopPropagation(); onChange(!on); }} onKeyDown={(e) => e.stopPropagation()}>
      {on && <svg viewBox="0 0 24 24" aria-hidden><path d="M5 12.5l4.5 4.5L19 7.5" /></svg>}
    </button>
  );
}

const NOTICE_ICON = {
  ok: <path d="M5 12.5l4.5 4.5L19 7.5" />,
  critical: <path d="M7 7l10 10M17 7L7 17" />,
  major: <path d="M12 6v7M12 17.5v.5" />,
  info: <path d="M12 11v6M12 7v.5" />,
} as const;

/** A notice at the top of a page: a coloured icon, a title, a line of detail and its actions. */
export function Notice({ tone, title, children, actions, onDismiss, role }: {
  tone: keyof typeof NOTICE_ICON; title: ReactNode; children?: ReactNode; actions?: ReactNode;
  onDismiss?: () => void; role?: "alert" | "status";
}) {
  return (
    <div className={`notice notice-${tone}`} role={role ?? (tone === "critical" ? "alert" : "status")}>
      <span className="notice-icon" aria-hidden><svg viewBox="0 0 24 24">{NOTICE_ICON[tone]}</svg></span>
      <div className="notice-text">
        <b>{title}</b>
        {children && <span className="selectable">{children}</span>}
      </div>
      {actions && <div className="notice-actions">{actions}</div>}
      {onDismiss && <button className="notice-close" onClick={onDismiss} aria-label="Dismiss">×</button>}
    </div>
  );
}
