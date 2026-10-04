import { relayAuthorization } from "@/shared/auth/session";
import { relayHttpBaseUrl } from "@/shared/lib/relay-url";

const INVITE_REQUEST_TIMEOUT_MS = 15_000;

export type BrowserInviteClaim = {
  status: "joined" | "already_member";
  communityId: string;
  host: string;
  role: string;
};

export async function claimInviteInBrowser(
  code: string,
  policyReceipt?: string,
): Promise<BrowserInviteClaim> {
  const url = `${relayHttpBaseUrl().replace(/\/+$/, "")}/api/invites/claim`;
  const body = JSON.stringify({
    code,
    policy_receipt: policyReceipt,
  });
  // Token-auth relays: the signed-in account claims the invite (Bearer).
  // Key relays: a NIP-07 key must sign, so a reload cannot orphan the
  // membership row created for a throwaway page key.
  const authorization = await relayAuthorization(url, "POST", {
    body,
    requireNip07: true,
    requireSession: true,
  });
  const response = await fetch(url, {
    method: "POST",
    headers: {
      Authorization: authorization,
      "Content-Type": "application/json",
    },
    body,
    signal: AbortSignal.timeout(INVITE_REQUEST_TIMEOUT_MS),
  });
  const json = (await response.json().catch(() => ({}))) as Record<
    string,
    unknown
  >;
  if (!response.ok) {
    const message =
      typeof json.error === "string" ? json.error : `HTTP ${response.status}`;
    throw new Error(message);
  }

  return {
    status: json.status as BrowserInviteClaim["status"],
    communityId: String(json.community_id),
    host: String(json.host),
    role: String(json.role),
  };
}
