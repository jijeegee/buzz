import * as React from "react";
import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { type AuthDevice, listAuthDevices } from "@/shared/api/tokenAuth";
import { deviceRobotTag } from "@/shared/lib/deviceRobot";

export const AUTH_DEVICES_QUERY_KEY = ["auth-devices"] as const;

/** Window event fired after the device list changes (e.g. a rename). */
export const AUTH_DEVICES_CHANGED_EVENT = "buzz:auth-devices-changed";

export function notifyAuthDevicesChanged() {
  window.dispatchEvent(new Event(AUTH_DEVICES_CHANGED_EVENT));
}

/**
 * After a successful rename: write the new name into the cached device list
 * so every "Running on …" tooltip shows it at once, then refetch. Works even
 * when no `useAuthDeviceLabels` is mounted to hear `notifyAuthDevicesChanged`.
 */
export function applyAuthDeviceRename(
  queryClient: QueryClient,
  deviceId: string,
  name: string,
) {
  queryClient.setQueryData<AuthDevice[]>(AUTH_DEVICES_QUERY_KEY, (devices) =>
    devices?.map((device) =>
      device.id === deviceId ? { ...device, name } : device,
    ),
  );
  void queryClient.invalidateQueries({ queryKey: AUTH_DEVICES_QUERY_KEY });
  notifyAuthDevicesChanged();
}

/** What an agent's robot tooltip says about one of the account's devices. */
export type AuthDeviceLabel = {
  name: string;
  /** The device this app is signed in on. */
  current: boolean;
};

/**
 * Devices of the signed-in account keyed by robot tag, so an agent's owner
 * can see which named computer the agent runs on, and whether it is this one.
 * Empty when not signed in with Google (the list call fails and is not
 * retried).
 */
export function useAuthDeviceLabels(
  enabled = true,
): Map<string, AuthDeviceLabel> {
  const queryClient = useQueryClient();
  const query = useQuery({
    enabled,
    queryKey: AUTH_DEVICES_QUERY_KEY,
    queryFn: async () => {
      try {
        return await listAuthDevices();
      } catch {
        return [];
      }
    },
    staleTime: 5 * 60_000,
  });
  React.useEffect(() => {
    const refresh = () =>
      void queryClient.invalidateQueries({ queryKey: AUTH_DEVICES_QUERY_KEY });
    window.addEventListener(AUTH_DEVICES_CHANGED_EVENT, refresh);
    return () =>
      window.removeEventListener(AUTH_DEVICES_CHANGED_EVENT, refresh);
  }, [queryClient]);
  return React.useMemo(() => {
    const labels = new Map<string, AuthDeviceLabel>();
    for (const device of query.data ?? []) {
      const tag = deviceRobotTag(device.id);
      if (tag) {
        labels.set(tag, {
          name: device.name,
          current: device.current === true,
        });
      }
    }
    return labels;
  }, [query.data]);
}
