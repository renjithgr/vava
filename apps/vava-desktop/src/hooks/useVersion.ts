import { useEffect, useState } from "react";
import { ipc } from "../lib/ipc";

/**
 * Loads the backend version once on mount.
 *
 * D1 only: later milestones replace this with repository/session state.
 */
export function useVersion() {
  const [version, setVersion] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc
      .getVersion()
      .then((value) => {
        if (!cancelled) setVersion(value);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return { version, error };
}
