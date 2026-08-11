/**
 * Convert a Tauri invoke rejection into a user-facing message.
 *
 * Commands return `DesktopError`, which serializes as `{ kind, message }`.
 * The rejection arrives as a JSON string (or, on some Tauri versions, an
 * already-parsed object); both shapes are handled here.
 */
export function toMessage(error: unknown): string {
  if (typeof error === "string") {
    try {
      const parsed = JSON.parse(error) as { message?: unknown };
      if (parsed && typeof parsed.message === "string") return parsed.message;
    } catch {
      // Not JSON — fall through to the raw string.
    }
    return error;
  }
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  return String(error);
}
