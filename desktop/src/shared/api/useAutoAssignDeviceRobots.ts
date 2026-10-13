import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { useAuthDevicesQuery } from "@/shared/api/useAuthDeviceNames";
import {
  fetchOwnerDevices,
  ownerDevicesQueryKey,
  publishDeviceRobot,
} from "@/shared/api/useOwnerDevices";
import { nextDeviceRobotAssignments } from "@/shared/lib/deviceRobot";
import { normalizePubkey } from "@/shared/lib/pubkey";

/**
 * Give this device, when it has no stored robot yet, the next free slot of
 * the palette (see `nextDeviceRobotAssignments`) and store it as the device's
 * kind:30181 robot, so its look never changes afterwards. Each app assigns
 * only its own device: old sign-ins still listed on the account must not take
 * the first, most distinct paints, so devices get them in the order they
 * start using the app. Runs only once both the device list and the stored
 * robots have loaded, so a still-loading record set can never look like "no
 * robots yet". Mount once near the app root.
 */
export function useAutoAssignDeviceRobots(
  pubkey: string | null | undefined,
  publish: typeof publishDeviceRobot = publishDeviceRobot,
): void {
  const queryClient = useQueryClient();
  const owner = normalizePubkey(pubkey ?? "");
  const devices = useAuthDevicesQuery(owner.length > 0);
  const records = useQuery({
    enabled: owner.length > 0,
    queryKey: ownerDevicesQueryKey(owner),
    queryFn: () => fetchOwnerDevices(owner),
    staleTime: 5 * 60_000,
  });
  // Assignment is cosmetic. Try once per owner/device during this mount;
  // rollback or a stale refetch must not create an automatic publish loop.
  // A later app launch or an explicit picker action can try again.
  const attempted = React.useRef(new Set<string>());

  React.useEffect(() => {
    if (!owner || !devices.isSuccess || !records.isSuccess) return;
    const assignments = nextDeviceRobotAssignments(
      devices.data
        .filter((device) => device.current)
        .map((device) => device.id),
      records.data.robotOverrides,
    );
    for (const [deviceId, override] of assignments) {
      const key = `${owner}:${deviceId}`;
      if (attempted.current.has(key)) continue;
      attempted.current.add(key);
      void publish(queryClient, {
        ownerPubkey: owner,
        deviceId,
        override,
      }).catch((error) => {
        console.error("Failed to store a device robot", error);
      });
    }
  }, [
    owner,
    devices.isSuccess,
    devices.data,
    records.isSuccess,
    records.data,
    queryClient,
    publish,
  ]);
}
