import { useCallback, useEffect, useState } from "react";
import Composer from "../chat/Composer";
import EmptyChat from "../chat/EmptyChat";
import ErrorNotice from "../chat/ErrorNotice";
import { toUiError } from "../chat/errors";
import ModelPicker from "../models/ModelPicker";
import { chooseModel } from "../models/defaultModel";
import Gauge from "../gauge/Gauge";
import LowAllowance from "../gauge/LowAllowance";
import { formatMoney } from "../gauge/allowance";
import { useAllowance } from "../gauge/useAllowance";
import Thread from "../chat/Thread";
import { useChat } from "../chat/useChat";
import {
  createChat,
  deleteChat,
  listChats,
  listModels,
  renameChat,
  type ChatSummary,
  type ModelInfo,
  type UiError,
} from "../../lib/ipc";
import Sidebar from "./Sidebar";
import "./shell.css";
import "../chat/chat.css";

interface Props {
  onOpenSettings?: () => void;
  onSignInAgain?: () => void;
}

function readLastModel(): string | null {
  try {
    return localStorage.getItem("lastModel");
  } catch {
    return null;
  }
}

export default function Shell({ onOpenSettings, onSignInAgain }: Props) {
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [modelsFailed, setModelsFailed] = useState(false);
  const [modelId, setModelId] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [startError, setStartError] = useState<UiError | null>(null);

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
  const onTurnEnd = useCallback(() => {
    refreshChats();
    refreshAllowance();
  }, [refreshChats, refreshAllowance]);
  const chat = useChat(selectedId, onTurnEnd);
  const selected = chats.find((c) => c.id === selectedId);
  const hasMessages = chat.messages.length > 0 || chat.pendingUser !== null;

  const handleSend = async (text: string) => {
    if (!modelId || creating) return;
    let id = selectedId;
    if (!id) {
      setCreating(true);
      setStartError(null);
      try {
        id = (await createChat(modelId)).id;
        setSelectedId(id);
        refreshChats();
      } catch (e) {
        setStartError(toUiError(e));
        return;
      } finally {
        setCreating(false);
      }
    }
    chat.send(text, [], modelId, id);
  };

  const select = (id: string | null) => {
    if (models) {
      const own = chats.find((c) => c.id === id)?.model_id;
      setModelId(chooseModel(models, [own, readLastModel()]));
    }
    setSelectedId(id);
    setStartError(null);
  };

  const pickModel = () => setPickerOpen(true);

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
    <Composer
      onSend={handleSend}
      onStop={chat.stop}
      streaming={chat.status === "streaming"}
      disabled={creating || models === null || blocked}
      noModel={models !== null && modelId === null}
      hasMessages={hasMessages}
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
        onOpenSettings={() => onOpenSettings?.()}
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
      <section className="main">
        <header className="head">
          <span className="ttl">{selected?.title ?? "New chat"}</span>
          <div className="usage" data-slot="usage">
            {selectedId && allowance.usage && (
              <>
                {`This chat: ${allowance.usage.requests} ${allowance.usage.requests === 1 ? "request" : "requests"}, ${formatMoney(allowance.usage.cost_usd)}`}
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
    </div>
  );
}
