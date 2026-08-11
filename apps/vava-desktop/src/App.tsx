import { useVersion } from "./hooks/useVersion";

/**
 * D1 shell: proves the React ↔ Rust IPC round trip.
 *
 * Later milestones replace this welcome screen with the repository picker
 * (D2) and the three-area conversation layout (D6).
 */
export default function App() {
  const { version, error } = useVersion();

  return (
    <main className="shell">
      <header className="titlebar">
        <span className="brand">Vava</span>
      </header>
      <section className="welcome">
        <h1 className="welcome-title">Vava</h1>
        <p className="welcome-tagline">Coding agent for your repositories.</p>
        <p className="status" role="status">
          {error ? (
            <span className="status-error">
              Rust bridge error: {error}
            </span>
          ) : version ? (
            <span className="status-ok">
              Rust backend connected · v{version}
            </span>
          ) : (
            <span className="status-pending">Connecting to Rust backend…</span>
          )}
        </p>
      </section>
    </main>
  );
}
