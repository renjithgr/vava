import { useState, type ReactNode } from "react";
import { countLines, parseBashOutput, simpleDiff, stringArg } from "../../lib/tools";
import type { ToolResultInfo } from "../../lib/tools";

export type { ToolResultInfo };

type Status = "ok" | "error" | "running";

/**
 * A tool card (Phase 13): communicates what happened, where, and whether it
 * succeeded — without overwhelming the conversation with full payloads.
 * Outputs are collapsed by default and expand on demand.
 */
export function ToolCard({
  tool,
  args,
  result,
  running,
}: {
  tool: string;
  args: unknown;
  result: ToolResultInfo | null;
  running: boolean;
}) {
  const status: Status = running ? "running" : result?.isError ? "error" : "ok";
  switch (tool) {
    case "read":
      return <ReadCard args={args} result={result} status={status} />;
    case "write":
      return <WriteCard args={args} result={result} status={status} />;
    case "edit":
      return <EditCard args={args} result={result} status={status} />;
    case "bash":
      return <BashCard args={args} result={result} status={status} />;
    default:
      return <GenericCard tool={tool} args={args} result={result} status={status} />;
  }
}

function CardShell({
  status,
  title,
  body,
  meta,
  children,
}: {
  status: Status;
  title: string;
  body?: ReactNode;
  meta?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className={`tool-card tool-card-${status}`}>
      <div className="tool-card-header">
        <span className="tool-card-status" title={status}>
          {status === "ok" ? "✓" : status === "error" ? "✕" : "●"}
        </span>
        <span className="tool-card-title">{title}</span>
        {meta && <span className="tool-card-meta">{meta}</span>}
      </div>
      {body && <div className="tool-card-body">{body}</div>}
      {children}
    </div>
  );
}

function Expandable({ label, children }: { label: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="tool-card-expand">
      <button className="tool-card-toggle" onClick={() => setOpen((v) => !v)}>
        {open ? "Hide" : "Show"} {label}
        <span className="tool-card-caret">{open ? "▴" : "▾"}</span>
      </button>
      {open && <div className="tool-card-output">{children}</div>}
    </div>
  );
}

function Output({ content }: { content: string }) {
  const lines = countLines(content);
  return (
    <Expandable label={lines > 0 ? `output (${lines} line${lines === 1 ? "" : "s"})` : "output"}>
      <pre>{content}</pre>
    </Expandable>
  );
}

function ReadCard({
  args,
  result,
  status,
}: {
  args: unknown;
  result: ToolResultInfo | null;
  status: Status;
}) {
  const path = stringArg(args, "path");
  return (
    <CardShell status={status} title="read" body={path ?? <span className="dim">unknown path</span>}>
      {result && <Output content={result.content} />}
      {status === "running" && <div className="tool-card-running">working…</div>}
    </CardShell>
  );
}

function WriteCard({
  args,
  result,
  status,
}: {
  args: unknown;
  result: ToolResultInfo | null;
  status: Status;
}) {
  const path = stringArg(args, "path");
  const bytes = result?.content.match(/wrote (\d+) bytes/)?.[1];
  return (
    <CardShell
      status={status}
      title="write"
      body={path ?? <span className="dim">unknown path</span>}
      meta={bytes ? `${bytes} bytes` : undefined}
    >
      {result && <Output content={result.content} />}
      {status === "running" && <div className="tool-card-running">working…</div>}
    </CardShell>
  );
}

function EditCard({
  args,
  result,
  status,
}: {
  args: unknown;
  result: ToolResultInfo | null;
  status: Status;
}) {
  const path = stringArg(args, "path");
  const oldText = stringArg(args, "old_text") ?? "";
  const newText = stringArg(args, "new_text") ?? "";
  const diff = simpleDiff(oldText, newText);
  const meta =
    diff.added.length > 0 || diff.removed.length > 0 ? (
      <span className="tool-card-counts">
        <span className="added">+{diff.added.length}</span>
        <span className="removed">−{diff.removed.length}</span>
      </span>
    ) : undefined;
  return (
    <CardShell status={status} title="edit" body={path ?? <span className="dim">unknown path</span>} meta={meta}>
      {status === "running" && <div className="tool-card-running">working…</div>}
      {(diff.added.length > 0 || diff.removed.length > 0) && (
        <Expandable label="diff">
          <pre className="diff">
            {diff.removed.map((line, i) => (
              <div key={`r${i}`} className="diff-line diff-removed">
                − {line}
              </div>
            ))}
            {diff.added.map((line, i) => (
              <div key={`a${i}`} className="diff-line diff-added">
                + {line}
              </div>
            ))}
          </pre>
        </Expandable>
      )}
      {result && <Output content={result.content} />}
    </CardShell>
  );
}

function BashCard({
  args,
  result,
  status,
}: {
  args: unknown;
  result: ToolResultInfo | null;
  status: Status;
}) {
  const command = stringArg(args, "command");
  const parsed = result ? parseBashOutput(result.content) : null;
  const meta =
    parsed && (parsed.exitCode !== null || parsed.duration) ? (
      <>
        {parsed.timedOut
          ? "timeout"
          : parsed.exitCode !== null
            ? `exit ${parsed.exitCode}`
            : "killed"}
        {parsed.duration ? ` · ${parsed.duration}` : ""}
      </>
    ) : undefined;
  return (
    <CardShell
      status={status}
      title="bash"
      body={<code className="tool-card-command">{command ?? "unknown command"}</code>}
      meta={meta}
    >
      {status === "running" && <div className="tool-card-running">working…</div>}
      {result &&
        (parsed?.matches ? (
          <Expandable label="output">
            {parsed.stdout !== "" && (
              <>
                <div className="tool-card-stream">stdout</div>
                <pre>{parsed.stdout}</pre>
              </>
            )}
            {parsed.stderr !== "" && (
              <>
                <div className="tool-card-stream">stderr</div>
                <pre className="tool-card-stream-error">{parsed.stderr}</pre>
              </>
            )}
            {parsed.stdout === "" && parsed.stderr === "" && <pre>(no output)</pre>}
          </Expandable>
        ) : (
          <Output content={result.content} />
        ))}
    </CardShell>
  );
}

function GenericCard({
  tool,
  args,
  result,
  status,
}: {
  tool: string;
  args: unknown;
  result: ToolResultInfo | null;
  status: Status;
}) {
  return (
    <CardShell status={status} title={tool}>
      {args !== null && args !== undefined && (
        <pre className="tool-card-args">{JSON.stringify(args, null, 2)}</pre>
      )}
      {result && <Output content={result.content} />}
      {status === "running" && <div className="tool-card-running">working…</div>}
    </CardShell>
  );
}
