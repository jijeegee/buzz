import * as React from "react";
import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { relayClient } from "@/shared/api/relayClient";
import {
  type AuthDevice,
  listAuthDevices,
  onTokenAuthChanged,
} from "@/shared/api/tokenAuth";
import { deviceRobotTag } from "@/shared/lib/deviceRobot";

export const AUTH_DEVICES_QUERY_KEY = ["auth-devices"] as const;

/** Window event fired after the device list changes (e.g. a rename). */
export const AUTH_DEVICES_CHANGED_EVENT = "kuzz:auth-devices-changed";

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

/** Waits between failed reads; the last one repeats until a read succeeds. */
const AUTH_DEVICES_RETRY_DELAYS_MS = [1_000, 5_000, 30_000, 60_000, 300_000];

export function authDevicesRetryDelay(failureCount: number) {
  return AUTH_DEVICES_RETRY_DELAYS_MS[
    Math.min(failureCount, AUTH_DEVICES_RETRY_DELAYS_MS.length - 1)
  ];
}

/**
 * The signed-in account's devices in the relay's order (oldest first), shared
 * by every caller. The command waits for a session restore in progress, and
 * a failure stays a failure, not a cached empty list ("no devices" would hide
 * every other computer's agents): it retries with growing waits, starting
 * over from the shortest with every new read. `useAuthDevicesLiveUpdates`
 * reads it at once when the session or the relay is back.
 */
export function useAuthDevicesQuery(enabled = true) {
  return useQuery({
    enabled,
    queryKey: AUTH_DEVICES_QUERY_KEY,
    queryFn: listAuthDevices,
    retry: true,
    retryDelay: authDevicesRetryDelay,
    staleTime: 5 * 60_000,
  });
}

/**
 * Read the device list again when the sign-in state changes or the relay
 * reconnects, so a list that failed during launch or an outage comes back at
 * once. Mount once near the app root.
 */
export function useAuthDevicesLiveUpdates(): void {
  const queryClient = useQueryClient();
  React.useEffect(() => {
    // Cancel first: with no data yet, a refetch would only join the retry
    // that is sitting out its wait.
    const refresh = () =>
      void queryClient
        .cancelQueries({ queryKey: AUTH_DEVICES_QUERY_KEY })
        .then(() =>
          queryClient.invalidateQueries({ queryKey: AUTH_DEVICES_QUERY_KEY }),
        );
    let disposed = false;
    let unlisten: (() => void) | undefined;
    onTokenAuthChanged(refresh)
      .then((fn) => {
        if (disposed) {
          fn();
          return;
        }
        unlisten = fn;
        // The restore may have finished before this listener was attached.
        const state = queryClient.getQueryState(AUTH_DEVICES_QUERY_KEY);
        if (state && state.fetchFailureCount > 0) refresh();
      })
      .catch(() => {
        // No sign-in events here: reconnects still refresh the list.
      });
    const unsubscribeReconnect = relayClient.subscribeToReconnects(refresh);
    return () => {
      disposed = true;
      unlisten?.();
      unsubscribeReconnect();
    };
  }, [queryClient]);
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
 * Empty when not signed in with Google (the list call fails).
 */
export function useAuthDeviceLabels(
  enabled = true,
): Map<string, AuthDeviceLabel> {
  const queryClient = useQueryClient();
  const query = useAuthDevicesQuery(enabled);
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
