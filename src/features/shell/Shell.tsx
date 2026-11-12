import { useCallback, useEffect, useRef, useState, type DragEvent } from "react";
import Composer, { type ComposerControl, type SendIntent } from "../chat/Composer";
import EmptyChat from "../chat/EmptyChat";
import ConfirmEdit from "../chat/ConfirmEdit";
import ErrorNotice from "../chat/ErrorNotice";
import Fullness from "../chat/Fullness";
import Queued from "../chat/Queued";
import { useFullness } from "../chat/useFullness";
import { toUiError } from "../chat/errors";
import { useChatCreation } from "../chat/useChatCreation";
import { discardDraft } from "../chat/drafts";
import ModelPicker from "../models/ModelPicker";
import { chooseModel } from "../models/defaultModel";
import { readLastModel, useModels } from "../models/useModels";
import Gauge from "../gauge/Gauge";
import LowAllowance from "../gauge/LowAllowance";
import { formatChatUsage } from "../gauge/allowance";
import { useAllowance } from "../gauge/useAllowance";
import Thread from "../chat/Thread";
import { useChat, type ChatStatus } from "../chat/useChat";
import { resolveSlash } from "../commands/resolveSlash";
import { commands } from "../commands/registry";
import { useCommandActions } from "../commands/useCommandActions";
import { useSlashMenu } from "../commands/useSlashMenu";
import { useComposerMenus } from "../menu/useComposerMenus";
import { useFolder } from "../mentions/useFolder";
import { useMentionMenu } from "../mentions/useMentionMenu";
import ActiveSkills from "../skills/ActiveSkills";
import { useChatState } from "../skills/useChatState";
import SkillsScreen from "../skills/SkillsScreen";
import { useSkills } from "../skills/useSkills";
import {
  activateSkill,
  deleteChat,
  listChats,
  renameChat,
  skillFile,
  rowsAfter,
  takeUserMessage,
  type AttachmentInput,
  type ChatSummary,
  type StoredMessage,
} from "../../lib/ipc";
import Settings from "../settings/Settings";
import { readSendWithModifier } from "../settings/sendKey";
import Sidebar from "./Sidebar";
import "./shell.css";
import "../chat/chat.css";

/** Suggested first messages for an empty chat; each only fills the box with its skill. */
const STARTERS = [
  {
    label: "Plan a feature before writing it",
    hint: "asks you questions, then writes a step-by-step plan",
    skill: "superpowers:brainstorming",
  },
  {
    label: "Find and fix a bug",
    hint: "reproduces it first, then fixes the cause",
    skill: "superpowers:systematic-debugging",
  },
  {
    label: "Review my recent changes",
    hint: "checks for mistakes and security problems",
    skill: "agent-skills:code-review-and-quality",
  },
];

/** A message waiting for the running reply to finish. */
interface Queue {
  /** What is sent, after a typed /skill-id was split off. */
  text: string;
  /** What the user typed, for the line above the box. */
  shown: string;
  attachments: AttachmentInput[];
  intent?: SendIntent;
  /** The model of the chat it was queued in. */
  modelId: string;
  /** Why the last attempt to send it failed. */
  error?: string;
}

interface Props {
  onSignInAgain?: () => void;
  /** Opens the Skills screen. */
  onOpenSkills?: () => void;
}

export default function Shell({ onSignInAgain, onOpenSkills }: Props) {
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const { models, modelsFailed, modelId, setModelId, pickerOpen, setPickerOpen, loadModels, chooseFromPicker } =
    useModels();
  const [showSettings, setShowSettings] = useState(false);
  const [showSkills, setShowSkills] = useState(false);
  const [sendWithModifier, setSendWithModifier] = useState(readSendWithModifier);
  const [dropping, setDropping] = useState(false);
  /** "Retry with another model…" opened the picker: the next choice also answers again. */
  const regenerateOnPick = useRef(false);
  const [pendingFile, setPendingFile] = useState<AttachmentInput | null>(null);

  const [queued, setQueued] = useState<Record<string, Queue>>({});
  const queuedRef = useRef(queued);
  queuedRef.current = queued;
  const [confirmEdit, setConfirmEdit] = useState<{ message: StoredMessage; count: number } | null>(null);
  const [turn, setTurn] = useState(0);
  const [draftChars, setDraftChars] = useState(0);
  const flushing = useRef(new Set<string>());
  const deleted = useRef(new Set<string>());
  const flushRef = useRef<(id: string) => void>(() => {});

  const refreshChats = useCallback(() => {
    listChats().then(setChats, () => {});
  }, []);
  useEffect(refreshChats, [refreshChats]);

  const allowance = useAllowance(selectedId);
  const refreshAllowance = allowance.refresh;
  const skillList = useSkills();
  const activeSkills = useChatState(selectedId, modelId);
  const refreshActive = activeSkills.refresh;
  const onTurnEnd = useCallback(
    (id: string, status: ChatStatus) => {
      refreshChats();
      refreshAllowance();
      void refreshActive(id);
      setTurn((n) => n + 1);
      // A queued message goes out when the reply is done, not when it failed or was stopped.
      if (status === "idle") flushRef.current(id);
    },
    [refreshChats, refreshAllowance, refreshActive],
  );
  const { creating, startError, setStartError, ensureChat } = useChatCreation({
    selectedId,
    modelId,
    setSelectedId,
    refreshChats,
  });
  const chat = useChat(selectedId, onTurnEnd);
  const selected = chats.find((c) => c.id === selectedId);
  const hasMessages = chat.messages.length > 0 || chat.pendingUser !== null;

  const handleSend = async (
    text: string,
    attachments: AttachmentInput[],
    intent?: SendIntent,
  ) => {
    const original = intent ? null : text;
    if (!intent) {
      const r = resolveSlash(text, skillList.rows);
      if (r.kind === "command") {
        commands.find((c) => c.name === r.name)?.run(actions.ctx, r.rest);
        return;
      }
      if (r.kind === "skill") {
        intent = { skill: r.id };
        text = r.rest || "Start.";
      }
    }
    if (!modelId || creating) return;
    if (selectedId && chat.status === "streaming") {
      // A reply is running: hold this one (replacing any earlier) until it is done.
      setQueued((all) => ({
        ...all,
        [selectedId]: { text, shown: original ?? text, attachments, intent, modelId },
      }));
      actions.clearAlert();
      return;
    }
    // Rejecting keeps the draft in the message box.
    const id = await ensureChat();
    try {
      await deliver(id, modelId, text, attachments, intent, actions.showAlert);
      actions.clearAlert();
    } catch (e) {
      // A new chat swaps the message box for a fresh one: put the draft back.
      if (original !== null) control.current?.setText(original);
      throw e;
    }
  };

  /**
   * Activates the intent's skill in chat `id`, then sends on `model`. A refused skill
   * throws (and shows its message through `onError`); true when the send was accepted.
   */
  const deliver = async (
    id: string,
    model: string,
    text: string,
    attachments: AttachmentInput[],
    intent: SendIntent | undefined,
    onError: (message: string) => void,
  ): Promise<boolean> => {
    if (intent?.skill) {
      try {
        await activateSkill(id, intent.skill, model);
      } catch (e) {
        onError(toUiError(e).message);
        throw e;
      }
      await refreshActive(id, model);
    }
    return chat.send(text, attachments, model, id);
  };

  flushRef.current = (id) => {
    const q = queuedRef.current[id];
    if (!q || flushing.current.has(id)) return;
    if (deleted.current.has(id)) {
      setQueued(({ [id]: _gone, ...rest }) => rest);
      return;
    }
    flushing.current.add(id);
    // It stays queued until the send is accepted: a failure or a busy chat keeps it shown.
    deliver(id, q.modelId, q.text, q.attachments, q.intent, (error) =>
      setQueued((all) => (all[id] ? { ...all, [id]: { ...all[id], error } } : all)),
    )
      .then(
        (accepted) => {
          if (accepted) setQueued(({ [id]: _gone, ...rest }) => rest);
        },
        () => {},
      )
      .finally(() => flushing.current.delete(id));
  };

  /** Brings a user message back into the box and removes it, and what followed, from the chat. */
  const takeForEdit = async (message: StoredMessage) => {
    if (!selectedId) return;
    try {
      const taken = await takeUserMessage(selectedId, message.id);
      const lead = activeSkills.skills.find((s) => taken.text.startsWith(`/${s.id} `));
      const text = lead ? taken.text.slice(lead.id.length + 2) : taken.text;
      await chat.reload();
      setTurn((n) => n + 1);
      control.current?.setText(text);
      taken.attachments.forEach((a) => control.current?.addAttachment(a));
    } catch (e) {
      actions.showAlert(toUiError(e).message);
    }
  };
  const editMessage = async (message: StoredMessage) => {
    if (!selectedId || chat.status === "streaming") return;
    const latest = chat.messages.filter((m) => m.kind === "message" && m.role === "user").pop();
    if (latest?.id === message.id) return takeForEdit(message);
    try {
      setConfirmEdit({ message, count: await rowsAfter(selectedId, message.id) });
    } catch (e) {
      actions.showAlert(toUiError(e).message);
    }
  };

  const select = (id: string | null) => {
    setConfirmEdit(null);
    setShowSettings(false);
    setShowSkills(false);
    if (models) {
      const own = chats.find((c) => c.id === id)?.model_id;
      setModelId(chooseModel(models, [own, readLastModel()]));
    }
    setSelectedId(id);
    setStartError(null);
  };

  const pickModel = () => setPickerOpen(true);
  const retryWithAnotherModel = () => {
    regenerateOnPick.current = true;
    setPickerOpen(true);
  };
  const chooseModelFor = (id: string) => {
    chooseFromPicker(id);
    if (regenerateOnPick.current) {
      regenerateOnPick.current = false;
      chat.regenerate(id);
    }
  };
  const changePickerOpen = (open: boolean) => {
    if (!open) regenerateOnPick.current = false;
    setPickerOpen(open);
  };

  // Up arrow in an empty box: edit the latest user message.
  const editLatest = () => {
    if (chat.status === "streaming" || chat.pendingUser !== null) return;
    const latest = chat.messages.filter((m) => m.kind === "message" && m.role === "user").pop();
    if (latest) void editMessage(latest);
  };

  // Cmd/Ctrl+N starts a new chat. The key is the one the layout produces when that is a
  // Latin letter (Dvorak keeps its own N); on other layouts the physical N position stands in.
  const selectRef = useRef(select);
  selectRef.current = select;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.shiftKey || e.altKey || e.isComposing) return;
      const isN = /^[a-z]$/i.test(e.key) ? e.key.toLowerCase() === "n" : e.code === "KeyN";
      if (!isN || document.querySelector('[role="dialog"]')) return;
      e.preventDefault();
      selectRef.current(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const dragHasFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");
  // A dialog open over the conversation takes no drops.
  const modalOpen = (e: DragEvent) => e.currentTarget.querySelector('[role="dialog"]') !== null;
  const drag = {
    onDragOver: (e: DragEvent) => {
      if (!dragHasFiles(e) || modalOpen(e)) return;
      e.preventDefault();
      setDropping(true);
    },
    onDragLeave: (e: DragEvent) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false);
    },
    onDrop: (e: DragEvent) => {
      if (!dragHasFiles(e)) return;
      e.preventDefault();
      setDropping(false);
      if (modalOpen(e)) return;
      void control.current?.addFiles(Array.from(e.dataTransfer.files));
    },
  };

  const startWith = (skillId: string) => () => {
    control.current?.setText(`/${skillId} `);
    control.current?.focus();
  };
  // A suggestion needs its skill to be usable now (on, in a pack that is on, not broken).
  const starters = STARTERS.filter((s) => skillList.rows.some((r) => r.id === s.skill)).map((s) => ({
    label: s.label,
    hint: s.hint,
    onPick: startWith(s.skill),
  }));

  const control = useRef<ComposerControl>(null);
  const actions = useCommandActions({
    chatId: selectedId,
    title: selected?.title,
    modelId,
    status: chat.status,
    compact: chat.compact,
    keyStatus: allowance.status,
    usage: allowance.usage,
    monthSpend: allowance.monthSpend,
    handlers: {
      newChat: () => select(null),
      openModel: pickModel,
      openSkills: () => {
        setShowSettings(false);
        setShowSkills(true);
        onOpenSkills?.();
      },
      openSettings: () => {
        setShowSkills(false);
        setShowSettings(true);
      },
    },
  });
  const folder = useFolder({
    chatId: selectedId,
    ensureChat,
    refresh: refreshActive,
    onError: actions.showAlert,
  });
  const slash = useSlashMenu({ skills: skillList.rows, ctx: actions.ctx, control });
  const mention = useMentionMenu({
    chatId: selectedId,
    folder: activeSkills.folder,
    openFolder: folder.open,
    control,
  });
  const menus = useComposerMenus(slash, mention);
  const fullness = useFullness(
    selectedId,
    modelId,
    activeSkills.skills.map((s) => s.id).join(","),
    turn,
    draftChars,
  );
  const queuedHere = selectedId ? queued[selectedId] : undefined;

  // A file picked on the Skills screen lands in the message box once it is back.
  useEffect(() => {
    // Stays pending until the message box exists; a refusal shows in the box itself.
    if (!pendingFile || showSkills || !control.current) return;
    control.current.addAttachment(pendingFile);
    setPendingFile(null);
  }, [pendingFile, showSkills, selectedId]);

  const useSkillHere = async (skillId: string) => {
    if (!modelId) throw new Error("Pick a model first.");
    const id = await ensureChat();
    await activateSkill(id, skillId, modelId);
    await refreshActive(id, modelId);
    setShowSkills(false);
  };
  const attachSkillFile = async (skillId: string, file: string) => {
    const attachment = await skillFile(skillId, file);
    await ensureChat();
    setPendingFile(attachment);
    setShowSkills(false);
  };

  const usingFreeModel = models?.find((m) => m.id === modelId)?.is_free ?? false;
  const blocked = usingFreeModel && allowance.status?.free_daily?.remaining === 0;

  const composer = (
    <>
    {blocked && models && <LowAllowance models={models} onPick={chooseFromPicker} />}
    {actions.panels}
    <ActiveSkills skills={activeSkills.skills} onRemove={activeSkills.remove} />
    {queuedHere && selectedId && (
      <Queued
        text={queuedHere.shown}
        error={queuedHere.error}
        onRemove={() => setQueued(({ [selectedId]: _gone, ...rest }) => rest)}
      />
    )}
    <Composer
      controlRef={control}
      draftId={selectedId}
      sendWithModifier={sendWithModifier}
      onEmptyUp={editLatest}
      menu={menus.menu}
      fieldAria={menus.fieldAria}
      onKeyIntercept={menus.onKeyIntercept}
      onTextChange={(t, c) => {
        setDraftChars(t.length);
        menus.onTextChange(t, c);
      }}
      onSend={handleSend}
      onStop={chat.stop}
      streaming={chat.status === "streaming"}
      disabled={creating || models === null || blocked}
      noModel={models !== null && modelId === null}
      hasMessages={hasMessages}
      acceptsImages={models?.find((m) => m.id === modelId)?.accepts_images ?? true}
      slots={{
        model: (
          <ModelPicker
            models={models}
            loadFailed={modelsFailed}
            value={modelId}
            onChange={chooseModelFor}
            onRetry={() => loadModels(true)}
            open={pickerOpen}
            onOpenChange={changePickerOpen}
          />
        ),
      }}
    />
    </>
  );

  return (
    <div className="shell" data-testid="shell">
      <Sidebar
        chats={chats}
        selectedId={selectedId}
        onSelect={select}
        onNew={() => select(null)}
        onRename={(id, title) => renameChat(id, title).then(refreshChats, () => {})}
        onDelete={(id) =>
          deleteChat(id).then(() => {
            deleted.current.add(id);
            discardDraft(id);
            setQueued(({ [id]: _gone, ...rest }) => rest);
            if (id === selectedId) select(null);
            refreshChats();
          }, () => {})
        }
        onOpenSettings={() => {
          setShowSkills(false);
          setShowSettings(true);
        }}
        onOpenSkills={() => {
          setShowSettings(false);
          setShowSkills(true);
        }}
        skillsOn={skillList.rows.length}
        skillsActive={showSkills}
        folder={activeSkills.folder}
        onOpenFolder={() => void folder.open()}
        onCloseFolder={() => void folder.close()}
        footer={
          allowance.status ? (
            <Gauge
              status={allowance.status}
              monthSpend={allowance.monthSpend}
              usingFreeModel={usingFreeModel}
              refreshFailed={allowance.failed}
            />
          ) : allowance.failed ? (
            <div className="gauge"><small>Could not refresh.</small></div>
          ) : null
        }
      />
      {showSkills ? (
        <SkillsScreen
          onBack={() => setShowSkills(false)}
          onChanged={skillList.refresh}
          onUse={useSkillHere}
          onAttach={attachSkillFile}
        />
      ) : showSettings ? (
        <Settings
          onBack={() => setShowSettings(false)}
          onSignedOut={() => onSignInAgain?.()}
          onSendKeyChange={setSendWithModifier}
        />
      ) : (
        <section className={dropping ? "main dropping" : "main"} {...drag}>
          <header className="head">
            <span className="ttl">{selected?.title ?? "New chat"}</span>
            {selectedId && fullness !== null && <Fullness percent={fullness} />}
            <div className="usage" data-slot="usage">
              {selectedId && allowance.usage && (
                <>
                  {`This chat: ${formatChatUsage(allowance.usage)}`}
                </>
              )}
            </div>
          </header>
          {selectedId === null ? (
            <EmptyChat starters={starters}>
              {startError && (
                <div className="col">
                  <ErrorNotice
                    error={startError}
                    onSignInAgain={() => onSignInAgain?.()}
                    onPickModel={pickModel}
                    onRetry={() => setStartError(null)}
                  />
                </div>
              )}
              {composer}
            </EmptyChat>
          ) : (
            <>
              <Thread
                messages={chat.messages}
                pendingUser={chat.pendingUser}
                streamingText={chat.streamingText}
                status={chat.status}
                error={chat.error}
                notSent={chat.unsent !== null}
                onEdit={(m) => void editMessage(m)}
                onRetryReply={() => chat.regenerate(modelId)}
                onRetryWithModel={retryWithAnotherModel}
                onRetry={() => chat.retry(modelId)}
                onSignInAgain={() => onSignInAgain?.()}
                onPickModel={pickModel}
              />
              {composer}
            </>
          )}
          {confirmEdit && (
            <ConfirmEdit
              count={confirmEdit.count}
              onCancel={() => setConfirmEdit(null)}
              onConfirm={() => {
                const { message } = confirmEdit;
                setConfirmEdit(null);
                void takeForEdit(message);
              }}
            />
          )}
        </section>
      )}
    </div>
  );
}
