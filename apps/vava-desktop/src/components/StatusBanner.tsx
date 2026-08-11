import type { ReactNode } from "react";

/**
 * A slim status banner for user-facing messages (errors from commands,
 * informational notes). Errors are concise — never raw backtraces.
 */
export function StatusBanner({
  kind,
  children,
}: {
  kind: "error" | "info";
  children: ReactNode;
}) {
  return <div className={`banner banner-${kind}`}>{children}</div>;
}
