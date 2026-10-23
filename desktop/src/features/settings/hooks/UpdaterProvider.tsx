import { createContext, useContext, type ReactNode } from "react";
import { RequiredUpdateScreen } from "../RequiredUpdateScreen";
import { useUpdater } from "./use-updater";

type UpdaterContextValue = ReturnType<typeof useUpdater>;

const UpdaterContext = createContext<UpdaterContextValue | null>(null);

export function UpdaterProvider({ children }: { children: ReactNode }) {
  const updater = useUpdater();
  return (
    <UpdaterContext value={updater}>
      {children}
      <RequiredUpdateScreen />
    </UpdaterContext>
  );
}

export function useUpdaterContext(): UpdaterContextValue {
  const ctx = useContext(UpdaterContext);
  if (!ctx) {
    throw new Error("useUpdaterContext must be used within an UpdaterProvider");
  }
  return ctx;
}
