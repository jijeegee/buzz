export type JoinAgentsAfterCreateDeps = {
  /** `useAttachDefaultAi().attachDefaultAi` — never rejects. */
  attachDefaultAi: (channelId: string) => Promise<void>;
  /** `useApplyTemplate().applyAgents` — best-effort, swallows its own errors. */
  applyAgents: (
    templateId: string | undefined,
    channelId: string,
  ) => void | Promise<void>;
};

export type JoinAgentsAfterCreateInput = {
  channelId: string;
  templateId?: string;
  addDefaultAi?: boolean;
};

/**
 * The one post-creation agent join sequence shared by every channel creation
 * point (AppShell channel/forum, project home, project channel): the starred
 * default AI joins first, then the template's personas. Both steps rewrite
 * the channel's replaceable membership event, so they must never run
 * concurrently; and template application dedupes against members, so the
 * default AI's persona is not instantiated twice. Callers fire this after
 * navigation (`void joinAgentsAfterCreate(...)`); each step reports its own
 * failures, so the returned promise never rejects for those.
 */
export async function joinAgentsAfterCreate(
  deps: JoinAgentsAfterCreateDeps,
  input: JoinAgentsAfterCreateInput,
): Promise<void> {
  if (input.addDefaultAi) {
    await deps.attachDefaultAi(input.channelId);
  }
  await deps.applyAgents(input.templateId, input.channelId);
}
