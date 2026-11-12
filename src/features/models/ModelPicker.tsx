import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import type { ModelInfo } from "../../lib/ipc";
import { formatPrice } from "./defaultModel";
import "./models.css";

interface Props {
  /** null while the list is loading. */
  models: ModelInfo[] | null;
  loadFailed: boolean;
  value: string | null;
  onChange: (id: string) => void;
  onRetry: () => void;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** A project chat: only models that can use tools are listed. */
  toolsOnly?: boolean;
}

interface RowProps {
  m: ModelInfo;
  index: number;
  selected: boolean;
  active: boolean;
  onPick: (id: string) => void;
  onHover: (index: number) => void;
}

const Row = memo(function Row({ m, index, selected, active, onPick, onHover }: RowProps) {
  return (
    <div
      role="option"
      id={`model-opt-${index}`}
      aria-selected={selected}
      className={`model-row${active ? " active" : ""}`}
      onMouseMove={() => onHover(index)}
      onClick={() => onPick(m.id)}
    >
      <span className="model-name">{m.name}</span>
      <span className={`model-price${m.is_free ? " free" : ""}`}>{formatPrice(m)}</span>
    </div>
  );
});

function Chevron() {
  return (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" aria-hidden="true" focusable="false">
      <path d="m6 9 6 6 6-6" />
    </svg>
  );
}

export default function ModelPicker({ models, loadFailed, value, onChange, onRetry, open, onOpenChange, toolsOnly = false }: Props) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const chip = useRef<HTMLButtonElement>(null);
  const search = useRef<HTMLInputElement>(null);

  // Free group first, then paid; this is also the keyboard order.
  const { free, paid, flat } = useMemo(() => {
    const q = query.trim().toLowerCase();
    const hit = (models ?? []).filter(
      (m) =>
        (!toolsOnly || m.supports_tools) &&
        (!q || m.name.toLowerCase().includes(q) || m.id.toLowerCase().includes(q)),
    );
    const free = hit.filter((m) => m.is_free);
    const paid = hit.filter((m) => !m.is_free);
    return { free, paid, flat: [...free, ...paid] };
  }, [models, query, toolsOnly]);

  useEffect(() => {
    if (open) {
      setQuery("");
      const listed = (models ?? []).filter((m) => !toolsOnly || m.supports_tools);
      const i = listed.filter((m) => m.is_free).concat(listed.filter((m) => !m.is_free)).findIndex((m) => m.id === value);
      setActive(Math.max(i, 0));
      search.current?.focus();
    }
    // Re-seed only when the popover opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => setActive(0), [query]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) onOpenChange(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open, onOpenChange]);

  useEffect(() => {
    if (open) document.getElementById(`model-opt-${active}`)?.scrollIntoView?.({ block: "nearest" });
  }, [open, active]);

  const pick = useCallback(
    (id: string) => {
      onChange(id);
      onOpenChange(false);
      chip.current?.focus();
    },
    [onChange, onOpenChange],
  );

  const close = () => {
    onOpenChange(false);
    chip.current?.focus();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, flat.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const m = flat[active];
      if (m) pick(m.id);
    }
  };

  const loading = models === null;
  const current = models?.find((m) => m.id === value);

  const group = (label: string, items: ModelInfo[], offset: number) =>
    items.length > 0 && (
      <div role="group" aria-label={label}>
        <div className="model-group" aria-hidden="true">{label}</div>
        {items.map((m, i) => (
          <Row
            key={m.id}
            m={m}
            index={offset + i}
            selected={m.id === value}
            active={offset + i === active}
            onPick={pick}
            onHover={setActive}
          />
        ))}
      </div>
    );

  return (
    <div className="model-picker" ref={root} onKeyDown={open ? onKeyDown : undefined}>
      <button
        ref={chip}
        type="button"
        className="chip"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={loading}
        onClick={() => onOpenChange(!open)}
      >
        {loading ? (
          "Loading models…"
        ) : current ? (
          <>
            {current.name}
            {current.is_free && <span className="free">Free</span>}
          </>
        ) : (
          "Choose a model"
        )}
        <Chevron />
      </button>
      {open && !loading && (
        <div className="model-pop">
          {loadFailed ? (
            <div className="model-fail">
              <p role="alert">Could not load models.</p>
              <button type="button" className="btn" onClick={onRetry}>
                Try again
              </button>
            </div>
          ) : (
            <>
              <input
                ref={search}
                className="model-search"
                type="text"
                role="combobox"
                aria-expanded="true"
                aria-controls="model-listbox"
                aria-activedescendant={flat.length ? `model-opt-${active}` : undefined}
                aria-label="Search models"
                placeholder="Search models"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
              <div role="listbox" id="model-listbox" aria-label="Models" className="model-list">
                {group("Free", free, 0)}
                {group("Paid", paid, free.length)}
                {flat.length === 0 && <p className="model-empty">No models match.</p>}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
