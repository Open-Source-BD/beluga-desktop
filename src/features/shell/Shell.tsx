import { useCallback, useEffect, useState } from "react";
import Composer from "../chat/Composer";
import EmptyChat from "../chat/EmptyChat";
import ErrorNotice from "../chat/ErrorNotice";
import { toUiError } from "../chat/errors";
import Thread from "../chat/Thread";
import { useChat } from "../chat/useChat";
import {
  createChat,
  deleteChat,
  listChats,
  renameChat,
  type ChatSummary,
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
  // Task 11 adds the setter with the model picker.
  const [modelId] = useState<string | null>(readLastModel);
  const [creating, setCreating] = useState(false);
  const [startError, setStartError] = useState<UiError | null>(null);

  const refreshChats = useCallback(() => {
    listChats().then(setChats, () => {});
  }, []);
  useEffect(refreshChats, [refreshChats]);

  const chat = useChat(selectedId, refreshChats);
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
    setSelectedId(id);
    setStartError(null);
  };

  // Task 11 opens the model picker here.
  const pickModel = () => {};

  const composer = (
    <Composer
      onSend={handleSend}
      onStop={chat.stop}
      streaming={chat.status === "streaming"}
      disabled={creating}
      noModel={modelId === null}
      hasMessages={hasMessages}
      slots={{ model: null }}
    />
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
        footer={null}
      />
      <section className="main">
        <header className="head">
          <span className="ttl">{selected?.title ?? "New chat"}</span>
          <div className="usage" data-slot="usage" />
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
