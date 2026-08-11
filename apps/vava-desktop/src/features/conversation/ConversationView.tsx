import type { DesktopMessage } from "../../lib/ipc";
import { useSessionStore } from "../../stores/session";

/**
 * The conversation panel: renders the active session's transcript.
 *
 * D3 renders the persisted transcript read-only. D4 streams live deltas
 * into the same view; D5 replaces the tool rows below with proper tool
 * cards.
 */
export function ConversationView() {
  const { activeSession, error } = useSessionStore();

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
      {activeSession.messages.length === 0 && (
        <div className="conversation-empty">
          This session has no messages yet. Prompting arrives in the next
          milestone.
        </div>
      )}
      {activeSession.messages.map((message, index) => (
        <MessageRow key={index} message={message} />
      ))}
    </div>
  );
}

function MessageRow({ message }: { message: DesktopMessage }) {
  switch (message.type) {
    case "user":
      return (
        <div className="message message-user">
          <div className="message-label">You</div>
          <div className="message-content">{message.content}</div>
        </div>
      );
    case "assistant":
      return (
        <div className="message message-assistant">
          <div className="message-label">Assistant</div>
          {message.reasoning_content && (
            <details className="reasoning">
              <summary>Reasoning</summary>
              <pre>{message.reasoning_content}</pre>
            </details>
          )}
          {message.content !== "" && (
            <div className="message-content">{message.content}</div>
          )}
          {message.tool_calls && message.tool_calls.length > 0 && (
            <div className="tool-calls">
              {message.tool_calls.map((call) => (
                <div key={call.id} className="tool-call">
                  <span className="tool-call-name">{call.name}</span>
                  <span className="tool-call-args">
                    {JSON.stringify(call.arguments)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      );
    case "tool":
      return (
        <div className="message message-tool">
          <div
            className={
              message.is_error
                ? "tool-result-label tool-result-error"
                : "tool-result-label"
            }
          >
            {message.is_error ? "✕" : "✓"} {message.tool_name}
          </div>
          <pre className="tool-result-content">{message.content}</pre>
        </div>
      );
  }
}
