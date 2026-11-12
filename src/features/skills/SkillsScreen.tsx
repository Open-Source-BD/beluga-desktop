import { useCallback, useEffect, useMemo, useState } from "react";
import { APP_NAME } from "../../app/brand";
import { listPacks, setPackOn, setSkillOn, type PackView, type SkillView } from "../../lib/ipc";
import { toUiError } from "../chat/errors";
import AddSkill from "./AddSkill";
import MorePacks from "./MorePacks";
import PackCard from "./PackCard";
import SkillDetail from "./SkillDetail";
import "./skills.css";

interface Props {
  onBack: () => void;
  /** Which skills exist or are on changed: refresh the `/` menu. */
  onChanged: () => void;
  /** Activates the skill in the current chat (creating one if needed) and returns to it. */
  onUse: (skillId: string) => Promise<void>;
  /** Attaches one of the skill's files to the current chat and returns to it. */
  onAttach: (skillId: string, file: string) => Promise<void>;
}

type Tab = "installed" | "more";

const isAvailable = (p: PackView, s: SkillView) => p.on && s.on && !s.broken;

function withAvailability(packs: PackView[]): PackView[] {
  return packs.map((p) => ({ ...p, skills: p.skills.map((s) => ({ ...s, available: isAvailable(p, s) })) }));
}

export default function SkillsScreen({ onBack, onChanged, onUse, onAttach }: Props) {
  const [packs, setPacks] = useState<PackView[] | null>(null);
  const [tab, setTab] = useState<Tab>("installed");
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<{ skill: SkillView; pack: PackView } | null>(null);
  const [adding, setAdding] = useState(false);

  const load = useCallback(async () => {
    try {
      setPacks(withAvailability(await listPacks()));
    } catch (e) {
      setError(toUiError(e).message);
      setPacks((p) => p ?? []);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const patch = (packId: string, f: (p: PackView) => PackView) =>
    setPacks((all) => all && withAvailability(all.map((p) => (p.id === packId ? f(p) : p))));

  const toggleSkill = async (pack: PackView, skill: SkillView, on: boolean) => {
    setError(null);
    const set = (v: boolean) =>
      patch(pack.id, (p) => ({ ...p, skills: p.skills.map((s) => (s.id === skill.id ? { ...s, on: v } : s)) }));
    set(on);
    try {
      await setSkillOn(skill.id, on);
      onChanged();
    } catch (e) {
      set(!on);
      setError(toUiError(e).message);
    }
  };

  const togglePack = async (pack: PackView, on: boolean) => {
    setError(null);
    patch(pack.id, (p) => ({ ...p, on }));
    try {
      await setPackOn(pack.id, on);
      onChanged();
    } catch (e) {
      patch(pack.id, (p) => ({ ...p, on: !on }));
      setError(toUiError(e).message);
    }
  };

  const total = useMemo(
    () => (packs ?? []).reduce((n, p) => n + p.skills.filter((s) => s.available).length, 0),
    [packs],
  );

  const q = query.trim().toLowerCase();
  const visible = (packs ?? [])
    .map((p) => ({
      pack: p,
      skills: q
        ? p.skills.filter((s) => `${s.name} ${s.description}`.toLowerCase().includes(q))
        : p.skills,
    }))
    .filter((v) => !q || v.skills.length > 0);

  const afterChange = () => {
    void load();
    onChanged();
  };

  if (open) {
    return (
      <section className="main skills-view">
        <header className="head">
          <span className="ttl">Skills</span>
        </header>
        <div className="skills-body">
          <div className="col">
            <SkillDetail
              id={open.skill.id}
              removable={open.pack.origin === "yours"}
              onBack={() => setOpen(null)}
              onUse={onUse}
              onAttach={onAttach}
              onRemoved={() => {
                setOpen(null);
                afterChange();
              }}
            />
          </div>
        </div>
      </section>
    );
  }

  return (
    <section className="main skills-view">
      <header className="head">
        <button type="button" className="btn" onClick={onBack}>
          Back to chat
        </button>
        <span className="ttl">Skills</span>
      </header>
      <div className="skills-body">
        <div className="col">
          <h2>Skills</h2>
          <p className="lede">
            {`Skills are instructions ${APP_NAME} follows for a kind of task. ${total} are on. Fewer is better on small free models.`}
          </p>
          <div className="tabs">
            <div role="tablist" className="tablist">
              <button type="button" role="tab" aria-selected={tab === "installed"} onClick={() => setTab("installed")}>
                Installed
              </button>
              <button type="button" role="tab" aria-selected={tab === "more"} onClick={() => setTab("more")}>
                More packs
              </button>
            </div>
            <button type="button" className="btn danger add" onClick={() => setAdding(true)}>
              Add a skill
            </button>
          </div>
          {error && (
            <p className="skills-error" role="alert">
              {error}
            </p>
          )}
          {tab === "installed" ? (
            <>
              <input
                type="search"
                className="skills-search"
                aria-label="Search skills"
                placeholder="Search skills"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
              {packs && visible.length === 0 && q && <p className="empty">No skills match.</p>}
              {visible.map(({ pack, skills }) => (
                <PackCard
                  key={pack.id}
                  pack={pack}
                  skills={skills}
                  searching={q !== ""}
                  onTogglePack={(on) => void togglePack(pack, on)}
                  onToggleSkill={(s, on) => void toggleSkill(pack, s, on)}
                  onOpen={(skill) => setOpen({ skill, pack })}
                />
              ))}
            </>
          ) : (
            <MorePacks onChanged={afterChange} />
          )}
        </div>
      </div>
      {adding && (
        <AddSkill
          onClose={(added) => {
            setAdding(false);
            if (added) afterChange();
          }}
        />
      )}
    </section>
  );
}
