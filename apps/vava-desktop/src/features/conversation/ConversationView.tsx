import type { DesktopMessage } from "../../lib/ipc";
import { useSessionStore } from "../../stores/session";

/**
 * The conversation panel: the active session's transcript plus the live
 * streaming assistant message (D4).
 *
 * Subscriptions are narrow — only the streaming bubble re-renders per text
 * delta; the persisted message list re-renders on message boundaries.
 */
export function ConversationView() {
  const activeSession = useSessionStore((state) => state.activeSession);
  const streaming = useSessionStore((state) => state.streaming);
  const error = useSessionStore((state) => state.error);
  const toolResults = useSessionStore((state) => state.toolResults);

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
      {activeSession.messages.length === 0 && !streaming && (
        <div className="conversation-empty">
          This session has no messages yet. Ask vava something below.
        </div>
      )}
      {activeSession.messages.map((message, index) => (
        <MessageRow key={index} message={message} toolResults={toolResults} />
      ))}
      {streaming && <StreamingMessage />}
    </div>
  );
}

function MessageRow({
  message,
  toolResults,
}: {
  message: DesktopMessage;
  toolResults: Record<string, { content: string; isError: boolean }>;
}) {
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
                  {toolResults[call.id] && (
                    <pre
                      className={
                        toolResults[call.id].isError
                          ? "tool-call-result tool-call-result-error"
                          : "tool-call-result"
                      }
                    >
                      {toolResults[call.id].content}
                    </pre>
                  )}
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
        <div key={call.callId} className="tool-call">
          <span className="tool-call-name">{call.tool}</span>
          {call.input !== null && (
            <span className="tool-call-args">{JSON.stringify(call.input)}</span>
          )}
          {call.result && (
            <pre
              className={
                call.result.isError
                  ? "tool-call-result tool-call-result-error"
                  : "tool-call-result"
              }
            >
              {call.result.content}
            </pre>
          )}
          {!call.result && <span className="tool-call-working">working…</span>}
        </div>
      ))}
      {streaming.content === "" && streaming.reasoning === "" && streaming.toolCalls.length === 0 && (
        <div className="message-content">
          <span className="caret" />
        </div>
      )}
    </div>
  );
}
