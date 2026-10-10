import * as React from "react";

import {
  EMPTY_OWNER_DEVICES,
  type OwnerDevices,
} from "@/shared/api/ownerDevices";
import { useOwnerDevices } from "@/shared/api/useOwnerDevices";

const OwnerDevicesContext =
  React.createContext<OwnerDevices>(EMPTY_OWNER_DEVICES);

/**
 * Shares the signed-in user's device records with surfaces that render many
 * small pieces (mention chips, composer decorations) without a query observer
 * per piece. Outside the provider every agent keeps the default robot.
 */
export function OwnerDevicesProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const ownerDevices = useOwnerDevices();
  return (
    <OwnerDevicesContext.Provider value={ownerDevices}>
      {children}
    </OwnerDevicesContext.Provider>
  );
}

export function useOwnerDevicesContext(): OwnerDevices {
  return React.useContext(OwnerDevicesContext);
}
