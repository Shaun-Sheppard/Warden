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
    <span className="avatar" style={{ width: size, height: size, fontSize: Math.round(size * 0.4), background: avatarColor(name) }} aria-hidden>
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
