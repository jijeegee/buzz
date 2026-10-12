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
 * Give each of the signed-in user's devices that has no stored robot the next
 * free slot of the palette (see `nextDeviceRobotAssignments`) and store it as
 * the device's kind:30181 robot, so its look never changes afterwards. Runs
 * only once both the device list and the stored robots have loaded, so a
 * still-loading record set can never look like "no robots yet". Mount once
 * near the app root.
 */
export function useAutoAssignDeviceRobots(
  pubkey: string | null | undefined,
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
  const inFlight = React.useRef(new Set<string>());

  React.useEffect(() => {
    if (!owner || !devices.isSuccess || !records.isSuccess) return;
    const assignments = nextDeviceRobotAssignments(
      devices.data.map((device) => device.id),
      records.data.robotOverrides,
    );
    for (const [deviceId, override] of assignments) {
      if (inFlight.current.has(deviceId)) continue;
      inFlight.current.add(deviceId);
      void publishDeviceRobot(queryClient, {
        ownerPubkey: owner,
        deviceId,
        override,
      })
        .catch((error) => {
          console.error("Failed to store a device robot", error);
        })
        .finally(() => {
          inFlight.current.delete(deviceId);
        });
    }
  }, [
    owner,
    devices.isSuccess,
    devices.data,
    records.isSuccess,
    records.data,
    queryClient,
  ]);
}
