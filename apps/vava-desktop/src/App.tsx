import { useEffect } from "react";
import { LauncherScreen } from "./features/launcher/LauncherScreen";
import { RepositoryScreen } from "./features/repository/RepositoryScreen";
import { useRepositoryStore } from "./stores/repository";

export default function App() {
  const active = useRepositoryStore((state) => state.active);
  const bootstrap = useRepositoryStore((state) => state.bootstrap);

  // On mount, re-sync with the Rust state (recents; an already-open
  // repository after a reload) so the UI never diverges from the backend.
  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  return active ? <RepositoryScreen /> : <LauncherScreen />;
}
