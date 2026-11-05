/** This key is an agent paired to one of the account's computers. */
export function isPairedAgent(
  devices:
    | readonly { agents?: readonly { pubkey: string }[] | null }[]
    | undefined,
  agentPubkey: string,
) {
  const agent = agentPubkey.toLowerCase();
  return !!devices?.some((device) =>
    device.agents?.some(({ pubkey }) => pubkey.toLowerCase() === agent),
  );
}
