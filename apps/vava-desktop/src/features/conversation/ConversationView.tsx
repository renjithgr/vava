import { useMemo } from "react";
import { useSessionStore } from "../../stores/session";
import { ToolCard } from "./ToolCard";
import { normalizeMessages } from "./normalize";

/**
 * The conversation panel: the active session's transcript plus the live
 * streaming assistant message (D4), rendered with tool cards (D5).
 *
 * Subscriptions are narrow — only the streaming bubble re-renders per text
 * delta; the persisted message list re-renders on message boundaries.
 */
export function ConversationView() {
  const activeSession = useSessionStore((state) => state.activeSession);
  const streaming = useSessionStore((state) => state.streaming);
  const error = useSessionStore((state) => state.error);
  const toolResults = useSessionStore((state) => state.toolResults);

  // Merge assistant tool_calls with their results (live `toolResults` map
  // first, then the transcript's separate tool messages).
  const items = useMemo(
    () => normalizeMessages(activeSession?.messages ?? [], toolResults),
    [activeSession, toolResults],
  );

  if (error) {
    return (
      <div className="conversation">
        <div className="conversation-empty">{error}</div>
      </div>
    );
  }
  if (!activeSession) {
    return (
      <div className="conversation">
        <div className="conversation-empty">
          Select or create a session to see the conversation.
        </div>
      </div>
    );
  }

  return (
    <div className="conversation">
      {items.length === 0 && !streaming && (
        <div className="conversation-empty">
          This session has no messages yet. Ask vava something below.
        </div>
      )}
      {items.map((item, index) => {
        switch (item.kind) {
          case "user":
            return (
              <div key={index} className="message message-user">
                <div className="message-label">You</div>
                <div className="message-content">{item.content}</div>
              </div>
            );
          case "assistant":
            return (
              <div key={index} className="message message-assistant">
                <div className="message-label">Assistant</div>
                {item.reasoning && (
                  <details className="reasoning">
                    <summary>Reasoning</summary>
                    <pre>{item.reasoning}</pre>
                  </details>
                )}
                {item.content !== "" && (
                  <div className="message-content">{item.content}</div>
                )}
              </div>
            );
          case "tool":
            return (
              <ToolCard
                key={index}
                tool={item.tool}
                args={item.args}
                result={item.result}
                running={item.running}
              />
            );
        }
      })}
      {streaming && <StreamingMessage />}
    </div>
  );
}

/** The live partial assistant message while a turn streams. */
function StreamingMessage() {
  const streaming = useSessionStore((state) => state.streaming);
  if (!streaming) return null;

  return (
    <div className="message message-assistant message-streaming">
      <div className="message-label">Assistant</div>
      {streaming.reasoning !== "" && (
        <details className="reasoning">
          <summary>Reasoning</summary>
          <pre>{streaming.reasoning}</pre>
        </details>
      )}
      {streaming.content !== "" && (
        <div className="message-content">
          {streaming.content}
          <span className="caret" />
        </div>
      )}
      {streaming.toolCalls.map((call) => (
        <ToolCard
          key={call.callId}
          tool={call.tool}
          args={call.input}
          result={call.result}
          running={call.result === null}
        />
      ))}
      {streaming.content === "" &&
        streaming.reasoning === "" &&
        streaming.toolCalls.length === 0 && (
          <div className="message-content">
            <span className="caret" />
          </div>
        )}
    </div>
  );
}
