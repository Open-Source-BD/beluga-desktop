import { useCallback, useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import Composer, { type ComposerControl, type SendIntent } from "../chat/Composer";
import EmptyChat from "../chat/EmptyChat";
import ErrorNotice from "../chat/ErrorNotice";
import { toUiError } from "../chat/errors";
import ModelPicker from "../models/ModelPicker";
import { chooseModel } from "../models/defaultModel";
import Gauge from "../gauge/Gauge";
import LowAllowance from "../gauge/LowAllowance";
import { formatChatUsage } from "../gauge/allowance";
import { useAllowance } from "../gauge/useAllowance";
import Thread from "../chat/Thread";
import { useChat } from "../chat/useChat";
import { useCommandActions } from "../commands/useCommandActions";
import { useSlashMenu } from "../commands/useSlashMenu";
import ActiveSkills from "../skills/ActiveSkills";
import { useChatState } from "../skills/useChatState";
import { useSkills } from "../skills/useSkills";
import {
  activateSkill,
  createChat,
  deleteChat,
  listChats,
  listModels,
  renameChat,
  type AttachmentInput,
  type ChatSummary,
  type ModelInfo,
  type UiError,
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

function readLastModel(): string | null {
  try {
    return localStorage.getItem("lastModel");
  } catch {
    return null;
  }
}

export default function Shell({ onSignInAgain, onOpenSkills }: Props) {
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [modelsFailed, setModelsFailed] = useState(false);
  const [modelId, setModelId] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [startError, setStartError] = useState<UiError | null>(null);
  const [showSettings, setShowSettings] = useState(false);

  const refreshChats = useCallback(() => {
    listChats().then(setChats, () => {});
  }, []);
  useEffect(refreshChats, [refreshChats]);

  const loadModels = useCallback((force: boolean) => {
    setModels(null);
    setModelsFailed(false);
    listModels(force).then(
      (list) => {
        setModels(list);
        const id = chooseModel(list, [readLastModel()]);
        setModelId((cur) => (cur && list.some((m) => m.id === cur) ? cur : id));
        if (id === null) setPickerOpen(true);
      },
      () => {
        setModels([]);
        setModelsFailed(true);
        setPickerOpen(true);
      },
    );
  }, []);
  useEffect(() => loadModels(false), [loadModels]);

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
  const chat = useChat(selectedId, onTurnEnd);
  const selected = chats.find((c) => c.id === selectedId);
  const hasMessages = chat.messages.length > 0 || chat.pendingUser !== null;

  const handleSend = async (
    text: string,
    attachments: AttachmentInput[],
    intent?: SendIntent,
  ) => {
    if (!modelId || creating) return;
    let id = selectedId;
    if (!id) {
      setCreating(true);
      setStartError(null);
      try {
        id = (await createChat(modelId)).id;
        flushSync(() => setSelectedId(id));
        refreshChats();
      } catch (e) {
        // Rejecting keeps the draft in the message box.
        setStartError(toUiError(e));
        throw e;
      } finally {
        setCreating(false);
      }
    }
    if (intent?.skill) {
      try {
        await activateSkill(id, intent.skill, modelId);
      } catch (e) {
        actions.showAlert(toUiError(e).message);
        throw e;
      }
      await refreshActive(id, modelId);
    }
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
  const slash = useSlashMenu({ skills: skillList.rows, ctx: actions.ctx, control });

  const chooseFromPicker = (id: string) => {
    setModelId(id);
    try {
      localStorage.setItem("lastModel", id);
    } catch {
      // Remembering the choice is a convenience; ignore storage failures.
    }
  };

  const usingFreeModel = models?.find((m) => m.id === modelId)?.is_free ?? false;
  const blocked = usingFreeModel && allowance.status?.free_daily?.remaining === 0;

  const composer = (
    <>
    {blocked && models && <LowAllowance models={models} onPick={chooseFromPicker} />}
    {actions.panels}
    <ActiveSkills skills={activeSkills.skills} onRemove={activeSkills.remove} />
    <Composer
      controlRef={control}
      menu={slash.menu}
      fieldAria={slash.fieldAria}
      onKeyIntercept={slash.onKeyIntercept}
      onTextChange={slash.onTextChange}
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
