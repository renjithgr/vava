import { useState } from "react";
import { useSessionStore } from "../stores/session";

/**
 * The prompt editor (Phase 16): multiline input, Cmd/Ctrl+Enter to send,
 * and the Send → Stop toggle while a turn is running (Phase 12).
 */
export function PromptEditor() {
  const { running, send, stop } = useSessionStore();
  const [input, setInput] = useState("");

  const submit = () => {
    const text = input.trim();
    if (!text || running) return;
    setInput("");
    void send(text);
  };

  return (
    <div className="prompt-bar">
      <textarea
        className="prompt-input"
        value={input}
        onChange={(event) => setInput(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            submit();
          }
        }}
        placeholder="Ask vava…  (⌘/Ctrl+Enter to send)"
        rows={3}
        spellCheck={false}
      />
      <button
        className={running ? "danger" : "primary"}
        onClick={running ? () => void stop() : submit}
        disabled={!running && input.trim() === ""}
      >
        {running ? "Stop" : "Send"}
      </button>
    </div>
  );
}
