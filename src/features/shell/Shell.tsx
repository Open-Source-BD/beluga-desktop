import { useCallback, useEffect, useRef, useState } from "react";
import Composer, { type ComposerControl, type SendIntent } from "../chat/Composer";
import EmptyChat from "../chat/EmptyChat";
import ErrorNotice from "../chat/ErrorNotice";
import { toUiError } from "../chat/errors";
import { useChatCreation } from "../chat/useChatCreation";
import ModelPicker from "../models/ModelPicker";
import { chooseModel } from "../models/defaultModel";
import { readLastModel, useModels } from "../models/useModels";
import Gauge from "../gauge/Gauge";
import LowAllowance from "../gauge/LowAllowance";
import { formatChatUsage } from "../gauge/allowance";
import { useAllowance } from "../gauge/useAllowance";
import Thread from "../chat/Thread";
import { useChat } from "../chat/useChat";
import { resolveSlash } from "../commands/resolveSlash";
import { commands } from "../commands/registry";
import { useCommandActions } from "../commands/useCommandActions";
import { useSlashMenu } from "../commands/useSlashMenu";
import { useComposerMenus } from "../menu/useComposerMenus";
import { useFolder } from "../mentions/useFolder";
import { useMentionMenu } from "../mentions/useMentionMenu";
import ActiveSkills from "../skills/ActiveSkills";
import { useChatState } from "../skills/useChatState";
import { useSkills } from "../skills/useSkills";
import {
  activateSkill,
  deleteChat,
  listChats,
  renameChat,
  type AttachmentInput,
  type ChatSummary,
} from "../../lib/ipc";
import Settings from "../settings/Settings";
import Sidebar from "./Sidebar";
import "./shell.css";
import "../chat/chat.css";

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
    (id: string) => {
      refreshChats();
      refreshAllowance();
      void refreshActive(id);
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
    // Rejecting keeps the draft in the message box.
    const id = await ensureChat();
    if (intent?.skill) {
      try {
        await activateSkill(id, intent.skill, modelId);
      } catch (e) {
        actions.showAlert(toUiError(e).message);
        // A new chat swaps the message box for a fresh one: put the draft back.
        if (original !== null) control.current?.setText(original);
        throw e;
      }
      await refreshActive(id, modelId);
    }
    actions.clearAlert();
    chat.send(text, attachments, modelId, id);
  };

  const select = (id: string | null) => {
    setShowSettings(false);
    if (models) {
      const own = chats.find((c) => c.id === id)?.model_id;
      setModelId(chooseModel(models, [own, readLastModel()]));
    }
    setSelectedId(id);
    setStartError(null);
  };

  const pickModel = () => setPickerOpen(true);

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
      openSkills: () => onOpenSkills?.(),
      openSettings: () => setShowSettings(true),
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

  const usingFreeModel = models?.find((m) => m.id === modelId)?.is_free ?? false;
  const blocked = usingFreeModel && allowance.status?.free_daily?.remaining === 0;

  const composer = (
    <>
    {blocked && models && <LowAllowance models={models} onPick={chooseFromPicker} />}
    {actions.panels}
    <ActiveSkills skills={activeSkills.skills} onRemove={activeSkills.remove} />
    <Composer
      controlRef={control}
      menu={menus.menu}
      fieldAria={menus.fieldAria}
      onKeyIntercept={menus.onKeyIntercept}
      onTextChange={menus.onTextChange}
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
            onChange={chooseFromPicker}
            onRetry={() => loadModels(true)}
            open={pickerOpen}
            onOpenChange={setPickerOpen}
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
            if (id === selectedId) select(null);
            refreshChats();
          }, () => {})
        }
        onOpenSettings={() => setShowSettings(true)}
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
      {showSettings ? (
        <Settings
          onBack={() => setShowSettings(false)}
          onSignedOut={() => onSignInAgain?.()}
        />
      ) : (
        <section className="main">
          <header className="head">
            <span className="ttl">{selected?.title ?? "New chat"}</span>
            <div className="usage" data-slot="usage">
              {selectedId && allowance.usage && (
                <>
                  {`This chat: ${formatChatUsage(allowance.usage)}`}
                </>
              )}
            </div>
          </header>
          {selectedId === null ? (
            <EmptyChat>
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
                onRetry={() => chat.retry(modelId)}
                onSignInAgain={() => onSignInAgain?.()}
                onPickModel={pickModel}
              />
              {composer}
            </>
          )}
        </section>
      )}
    </div>
  );
}
