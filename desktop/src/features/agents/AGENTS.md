# Agent Configuration — Contributor Rules

Scope: `desktop/src/features/agents/` (config surfaces, shared config renderer,
and the agent config core). Read this before changing how harness / provider /
model / effort configuration is modeled, rendered, persisted, or applied.

Plan of record: `Buzz/Harness-Provider-Model.md` in Morgan's Obsidian vault
(PR sequence, decisions log). PRs: #2140 (rename), #2148 (flag reduction),
#2156 (honest model states), #2158 (Agent Config Core).

## The one rule

**Harness capability facts have exactly one source: the Rust runtime catalog.**
`KnownAcpRuntime` (`desktop/src-tauri/src/managed_agents/discovery/runtime_metadata.rs`)
declares each harness's model/provider/effort env keys and capabilities. Spawn
applies them; `AcpRuntimeCatalogEntry` exposes them over IPC; and
`lib/agentConfigCore.ts` projects them into field descriptors. The frontend
never maintains a rival copy of this table. Setup guidance follows the same
rule: `requires_external_cli` is derived from `KnownAcpRuntime` and projected
to the UI rather than inferred from a runtime ID in a component. So is
billing: `subscription_provider` (`anthropic` for Claude Code, `openai` for
Codex; `Some` iff `login_hint` is) names the provider a harness's CLI login
pays for, and Settings › Models files that harness's sign-in row under the
provider by reading `AcpRuntimeCatalogEntry.subscriptionProvider` — never by
comparing runtime ids.

**Second metadata source: command-keyed execution policy.**
`harness_max_parallelism` (`managed_agents/parallelism.rs`) maps the harness's
static command string to a spawn-time cap (`OPENCLAW_MAX_PARALLELISM = 5` for
OpenClaw). This cap is not a `KnownAcpRuntime` field because it applies to
preset harnesses (like OpenClaw) that are not in the builtin catalog. It is
projected onto `AcpRuntimeCatalogEntry.max_parallelism` by all four
catalog-producing constructors (builtin discovery, preset catalog, custom
discovery, custom-save response) using the **static definition command**, not
the resolved `entry.command` (which may be `null` for unavailable entries).
The frontend reads `maxParallelism` from the catalog entry and never keeps a
separate constant.

If you need a new capability fact (a new env key, a native option, a "supports
X" flag): add it to `KnownAcpRuntime` first, expose it on
`AcpRuntimeCatalogEntry`, then project it through the core. Do not shortcut
with a TypeScript lookup table or an id comparison in a component.

## Rules

1. **No hardcoded harness-ID checks in render code.** `runtime.id === "claude"`
   belongs in `deriveAgentConfigFieldModel` (once, with a named reason), never
   in a component. Components ask the field model what exists
   (`hasRenderableAgentConfigField`, `getRenderableEffortField`).
2. **Effort reads/writes go through the descriptor.** Use the effort
   descriptor's `currentPersistence` key — never a raw
   `BUZZ_AGENT_THINKING_EFFORT` literal in UI code. `currentPersistence` is
   where the value lives *today*; `targetApplication` is how the harness
   *should* receive it. They intentionally differ until PR 2.7 migrates
   Goose/Claude — do not "fix" one to match the other without doing the
   migration work.
3. **Field absence has a named reason, not a boolean.** A harness whose
   catalog entry publishes `effortThoughtLevel` (Claude Code `effort`, Codex
   and Hermes `reasoning_effort` — the adapter advertises effort as an ACP
   `thought_level` option) gets a `deferredUntilNativeOptionsAvailable` effort
   descriptor whose option id comes from that catalog fact, never from a
   runtime-id comparison; its per-agent write control is `EffortPickerField`
   (rule 14). A harness without it is omitted as `unsupportedByHarness`. New
   absences get new named reasons in `AgentConfigOmission` / `render` — never
   a `showX` prop.
4. **The clearing policy is the named types.** `onContextChange:
   "resetDependentValues"` (user changed harness/provider → dependent values
   reset everywhere) vs `onCatalogMismatch: "explainOnly" | "onboardingCleanup"`
   (an async catalog miss never silently erases saved state outside
   onboarding's named cleanup). Do not add mutation booleans like
   `clearInvalidModel`; extend the policy types.
5. **"Metadata unknown" ≠ "harness lacks the capability".** Passing
   `runtime: undefined` to the core means fields won't render. Surfaces must
   gate on the runtime catalog query settling (loading/error states) rather
   than letting fields silently vanish — see `AgentDefaultsEditor` /
   `DefaultConfigStep` for the pattern.
6. **One canonical behavior, disclosure presets for visibility.** Behavior
   flags were deliberately killed in #2148 (`CANONICAL_CONFIG_BEHAVIORS`).
   Surface differences are expressed via the `disclosure` preset, not new
   boolean props.  **Exception:** `onboarding-essential` hides happy-path
   helper copy (provider/model descriptions) but a non-null model-discovery
   status always bypasses the preset and renders the status line — enforced
   via `shouldShowModelStatusMessage()` (`AgentConfigFields.tsx`).
   Additionally, a successful discovery response that yields no usable options
   (`supportsSwitching:false` or empty model list) synthesizes a warning status
   via `synthesizeEmptyDiscoveryStatus()` and is intentionally **not cached**
   so that closing → reopening the dialog re-runs discovery after the user
   installs or signs into the CLI (`isCacheableDiscoveryResponse()`).
7. **Onboarding setup detects readiness; it does not select defaults.** The
   setup page derives visible and ready harnesses from the runtime catalog and
   only offers install or sign-in actions. The following defaults page is the
   sole onboarding surface that chooses `preferred_runtime`. Its complete draft
   lives in machine-onboarding session state, so Back performs no write and
   restores even incomplete edits when the user returns. Skip abandons that
   draft and advances with zero config writes. Next is the only persistence
   boundary: it consumes the shared renderer's `onValidityChange` signal,
   disables editing while awaiting `set_global_agent_config`, advances only on
   success, and leaves the draft in place with a retryable inline error on
   failure. A harness selection alone does not enable Next when the harness
   requires provider/model/credential config (e.g. buzz-agent with no
   provider). Baked build env and runtime-file config satisfy the gate. Drafts
   intentionally do not survive an app restart.
   `onboarding-agent-defaults.spec.ts` is the acceptance gate for anything
   touching this flow or the shared renderer.
8. **Omit the Model control only after a confirmed successful empty
   discovery on an optional-model harness.** When the field model marks model
   as `acpNative` (Claude Code / Codex), `shouldRenderModelControl` hides the
   picker while discovery is in flight and after IPC resolves with no usable
   options (`modelDiscoverySuccessfulEmpty` / `isSuccessfulEmptyDiscovery`).
   A thrown or unavailable discovery keeps the control so #2246 failure UI can
   render, and must not heal/clear persisted model or effort. Full disclosure
   still shows the control when Custom model is available. Required-model
   harnesses always keep the field. Gate: `defaults hides model when optional
   harness has empty discovery` (and the failed-discovery counterpart) in
   `onboarding-agent-defaults.spec.ts`.
   **An optional-model harness never requires a model in Customize either.**
   The persona Create/Edit "Customize for this agent" section requires the
   provider+model pair only for provider-selection harnesses (Buzz Agent /
   Goose — the ones the backend `required_normalized_fields` gate refuses to
   spawn without it). For every other harness an empty model is the
   "harness default" choice: Save stays enabled, the picker keeps a blank
   `HARNESS_DEFAULT_MODEL_OPTION` row above the discovered models so the user
   can return to it, and spawn omits `BUZZ_ACP_MODEL` so the harness runs on
   its own configured model. Gate: `agentAiConfigurationPolicy.test.mjs`.
9. **The defaults modal is progressively disclosed.** An unset global config
   starts on the Buzz Agent-first deployment fallback and carries that visible
   harness into the next saved edit. The `progressive-defaults` disclosure
   preset therefore begins at Provider for Buzz Agent, then reveals Model,
   Effort, and Advanced only after a provider is configured. Harnesses whose
   runtime metadata has no provider field skip that gate. Reveals animate their
   height through Motion and become immediate when reduced motion is requested.
   Once the Advanced toggle is visible, its expanded state is exclusively
   user-controlled: provider, harness, and required-env changes must never
   open it automatically in defaults, create, or edit flows. In Create mode,
   `Run on` belongs in Advanced directly after **Who can send instructions**;
   keep it out of the basic create fields. The defaults summary follows
   preferred-harness changes saved while the dialog is open, and its configured
   state includes required credentials as well as provider/model values. If no
   available harness can resolve, Create starts in Customize and lets unavailable
   catalog entries be selected only to expose their setup guidance; submission
   remains blocked.
   Advanced-only required credentials and incomplete remote **Run on** setup
   mark the collapsed Advanced toggle without opening it, and block incomplete
   saves.
   Runtime-file credentials satisfy Global Defaults just as they do Create and
   Edit. In Edit,
   selecting Custom command keeps its required command field beside the harness
   picker rather than hiding it in Advanced.
10. **Catalog visibility is community-scoped relay state, never a global
    definition field.** `AgentDefinition.shared` is only the active
    relay+owner projection returned to the UI. Durable heads and pending
    publications live in the scoped retention database, and explicit share
    toggles await relay acceptance before the UI claims that an agent was
    published or removed. A queued update must stay visibly queued, and the
    catalog itself must render only relay-confirmed publications — never an
    optimistic local persona.
11. **Shared agent access names the consequence where it is selected.** The
   shared respond-to field shows a persistent warning whenever `anyone` **or**
   `allowlist` is selected — both hand the host's access to someone other than
   the owner, so both disclose it and only the audience phrase differs. This
   covers persona-backed create and edit surfaces. Keep that disclosure in
   the shared field instead of adding surface-specific flags. It renders
   directly below the selector for `anyone` but *after* the people picker for
   `allowlist`, so it never sits between the user and the selection they came
   to make. The copy leads with the audience ("Anyone can use this agent to
   access…") so it reads as a warning rather than an explanation, and stays one
   sentence — don't split the mechanism into a second sentence. Both the machine
   and the stakes it names come from `lib/agentAccessWarning.ts`, keyed on an
   optional `runLocation`: instance surfaces resolve it from
   `ManagedAgent.backend` via `runLocationForBackend`, and the create flow from
   `WhereToRunDraft.runOn` via `runLocationForRunOn`. `AgentDialog` is the one
   place that resolves it for dialog surfaces and publishes it through
   `ui/AgentRunLocationContext.tsx`; the field reads that context and lets an
   explicit `runLocation` prop win. Do **not** thread the value as a prop
   through `AgentDefinitionDialog` / `AgentInstanceEditDialog` — neither uses
   the value itself, and the shared context keeps the dialog boundary stable.
   Surfaces rendered outside `AgentDialog` (e.g. `EditRespondToDialog`) pass the
   prop directly. Local names "your
   computer, including files, accounts, and connected tools"; remote names "the
   server it runs on, including any accounts and tools available there" —
   deliberately *not* the owner's files, which aren't theirs to describe on a
   host they don't own. **For a persona-linked deployed agent, the profile Edit
   dialog seeds access from the exact clicked instance and saves access through
   `update_managed_agent`; persona behavior remains the definition default, but
   must never bypass the instance command's stop, persist, publish, and restart
   boundary.** An unknown location falls back to the local wording — never hedge
   with "computer or server". A remote host requires an
   installed `buzz-backend-*` provider, and without one `WhereToRunSection`
   never renders, so "server" would name a concept the owner has never been
   shown; when it *is* remote they picked that host from the selector
   themselves. Never synthesize a run location a surface doesn't have. Don't
   expose `respond-to`, `allowlist`, Nostr, or harness jargon in primary UI
   copy. **The owner-only-access build capability is backend-independent.** When
   `getAgentAccessOwnerOnly()` is true, every managed agent's access control is
   locked to owner-only, including provider-backed agents. A provider backend
   does not prove remote execution and must never create a policy carve-out.
12. **Shared instructions must be reviewable byte-for-byte.** Agent definitions
   execute their `system_prompt` verbatim, so catalog and snapshot review
   surfaces render the literal prompt, never the chat Markdown projection
   (which can conceal spoilers, link destinations, and image sources). Reject
   Unicode default-ignorable, bidirectional-formatting, and non-layout control
   characters at both the untrusted catalog parser and the Rust persistence /
   import boundary. Do not silently strip them: rejection keeps the reviewed
   string identical to the executed string. New sharing paths must reuse the
   same validation before they persist or activate a definition.
13. **Profile runtime sections render only reported agent data.** Missing
   runtime, model, status, command, MCP, advanced, or diagnostics values stay
   absent in every build mode. Do not fill profile or agent-panel gaps with
   development/staging examples, preview controls, or synthetic configuration;
   those values can be mistaken for the viewed agent's real configuration.
   Configuration rows show the effective value regardless of whether it came
   from an explicit choice, global default, config file, or runtime override.
   Do not add provenance lines, shadowed/struck-through values, pre-start
   placeholders, or whole-section dimming; use an em dash for an unknown value.
   Info, activity, agent-configuration, and model-setting rows use the same bare
   16px leading-icon treatment as agent management actions. Keep semantic icons
   visible in profile variants and do not wrap them in background shapes. An
   owned agent profile is entry-point invariant: opening the same deployed
   agent from Agents, a DM, or a channel must expose the same actions, tabs,
   fields, and profile-wide activity selection. Caller context may control the
   panel shell or return navigation, but must not filter or replace profile
   content. Explicit public-key targets are always exact, including stopped,
   archived, and relay-only identities. Only explicit persona navigation may
   select a representative or offer persona Start; a relay persona link cannot
   borrow a local sibling's management controls. See
   [the identity contract](../../../../docs/agent-profile-identity.md).
   Availability dots read relay presence, never a saved deployment
   receipt or runtime status. Failed/disconnected reads are unknown; lifecycle
   actions retain their separate routing. Current exact-key Online/Away presence
   suppresses Start for an inactive local record without granting Stop authority;
   list/profile/member startup guards must not interpret Offline as proof of safe
   startup. Deletion also consumes that same exact-key availability reader:
   unknown requests shutdown when a channel exists, request failure retains the
   record, and only established Offline keeps the intentional no-request path.
   Unqueried persona siblings are unknown. No presence state grants deletion or
   Stop authority; native local stop-before-remove remains independent. See
   [the availability contract](../../../../docs/agent-availability.md).
   The shared cloud marker means “Not managed on this device” only
   after ownership and successful local inventory are known. It does not imply
   hosting location, availability, or permission. Keep all identity surfaces on
   the shared provenance context, without per-row directory subscriptions. See
   [the provenance contract](../../../../docs/agent-management-provenance.md).
14. **Thinking effort is the model choice's companion: three persisted
   columns, one launch projection, one write control, one read-only
   display.** Any surface that selects a model renders effort through
   `ui/ModelEffortFields.tsx` — the model control as `children`, the shared
   `EffortPickerField` (`ui/EffortPickerField.tsx`) beneath it. Today that is
   `AgentDefinitionDialog` (Create **and** Edit, in "Use defaults" as well as
   "Customize"), `EditAgentProviderModelFields` inside `AgentInstanceEditDialog`
   (a required `effort` prop), and `AgentConfigFields` (Global AI Defaults,
   settings card, onboarding) for the default model. Do not add a model picker
   without routing it through `ModelEffortFields`, and do not render the picker
   outside it. The picker is **Save-gated, not direct-write**: fully controlled
   by its parent (`value`/`onChange`), owning no mutation, so a Cancel or a
   failed Save never writes effort.

   **Three columns, one order.** Effort lives on three records, each with the
   same `effort_level: Option<String>` shape (serde default,
   `skip_serializing_if = "Option::is_none"`, so pre-field stores, event bytes
   and `persona_content_hash` are byte-identical while unset):
   `ManagedAgentRecord.effort_level` (the per-instance override),
   `AgentDefinition.effort_level` (the definition default — a persona field,
   published in the kind:30175 content and hashed, so an edit badges linked
   instances for restart), and `GlobalAgentConfig.effort_level` (the global
   default). `config_bridge::effort::effort_launch_projection` resolves them with
   the env tiers in one CLEAR order, mirrored by the reader's
   `build_thinking_field` (`InheritedConfigTiers.persona_effort` /
   `global_effort`, origins `PersonaDefault` / `GlobalDefault`):

   ```text
   record native > column > record legacy
     > persona native > persona column > persona legacy
     > global native > global column > definition(native) > baked(native)
   ```

   Every tier has the same shape — native env key above its structured column —
   so a Goose/buzz-agent knob set in Global AI Defaults is never outranked by a
   global column that dialog could not show for that harness (and
   `resetConfigForHarnessChange` clears the column on a harness switch anyway).
   The env tiers need the runtime's native key; the three columns do not, so
   for the ACP thought-level harnesses (Claude Code, Codex, Hermes — no native
   knob) the columns are the only authorities and the resolved value rides the
   retained transport key `BUZZ_ACP_EFFORT_LEVEL`. Goose and buzz-agent keep
   their native env knob (`GOOSE_THINKING_EFFORT` / `BUZZ_AGENT_THINKING_EFFORT`)
   and its `EffortSelectField`; the catalog's `effort_thought_level` is `None`
   for them, so the column picker hides.

   **Where each write goes.** Create embeds the pick in `create_persona`
   (`CreatePersonaRequest.effort_level`, trimmed, blank = unset) — the
   definition default — and deliberately never on `create_managed_agent`
   (`buildInstanceInputForDefinition` seeds no effort), so the instance
   inherits live and a later definition edit propagates on restart like
   model/provider. Edit of a definition sends `UpdatePersonaRequest.effort_level`
   tri-state (absent = keep, `null` = clear, string = set; a harness switch
   clears, a hidden picker on an unchanged harness sends nothing). The instance
   dialog embeds `effortLevel` tri-state in the locked `update_managed_agent`
   (PR #4625), atomic with any access-policy restart; its sentinel reads
   "Template default" only when the linked template actually sets one
   (`effortSentinelLabel`). Global AI Defaults writes
   `GlobalAgentConfig.effort_level` through `set_global_agent_config`. Every
   boundary that can write a level validates it with
   `validate_effort_level_text` (no blank, no control/NUL characters, ≤ 64
   chars): `apply_persona_effort_level`, `create_persona`,
   `validate_inbound_persona_definition` (an unsafe inbound effort rejects the
   event, like an unsafe name), the community-catalog parser (rejects the
   entry), `validate_global_config`, and the instance column's two writers
   (`apply_picker_effort_level`, `normalize_create_effort_level`) — the value
   is emitted verbatim into the child's env and bypasses the env-var sanitizer.

   **Gating** lives in the pure helper `ui/effortPicker.ts`
   (`effortPickerState`): visible iff (no `backend` **or** a local one) **and**
   a vocabulary — a `thought_level` `effortConfigId` discovered from the running
   session, or the prospective runtime's catalog `effortThoughtLevel.fallbackValues`
   (one Rust catalog fact, `KnownAcpRuntime` / `PresetHarness`
   `effort_thought_level`: option id plus the safe `low | medium | high`
   subset). `backend` is absent on the definition and global surfaces: there is
   no instance, and their default applies to every instance. The definition
   dialog derives "was the picker offered" from the same `effortPickerState(...)
   .visible`, never from a second gate. Discovered options win; fallback-sourced
   options carry the "Options may vary by model." hint, and a saved value
   outside the fallback stays selectable under its raw name. Never reuse
   `effort_normalization` / `effort_canonical_values` for the picker (those
   filter at spawn and drive the Goose auto-clear). **The picker never shows a
   value other than what the next spawn launches**: the instance dialog seeds
   from the config surface's `normalized.thinkingEffort` (which now includes the
   persona and global columns), and after an in-dialog runtime switch from the
   stored column `ManagedAgent.effortLevel`, diffing an explicit pick against
   that so it is never collapsed as "unchanged" against a stale effective value.

   **Remote semantics.** The definition and global defaults apply to every
   instance, remote ones included: the deploy path builds its launch block from
   the same `effort_launch_projection` (`launch.env`, not a separate
   `policy_env` authority), so the Run-on never gates the definition or global
   picker. Only the **instance** column write is local-only
   (`ensure_effort_change_supported`, `normalize_create_effort_level`): a
   remote instance launches its definition's or the global default and offers
   no per-instance override. Showing that inherited value read-only in the
   remote instance dialog is deferred.

   **Deliberate exclusions.** Team adopt (`teams/adopt/apply.rs`), the team
   catalog projection (`team_catalog.rs`), and agent snapshot export/import
   (`agent_snapshot.rs`, `personas/snapshot/import.rs`) keep their portable
   schemas: copies start at the adapter default. Community-catalog copies DO
   carry the publisher's effort (`CatalogAgentProjection.effort_level` →
   `publicationToPersona` → the Add payload) because a copy's fresh local id is
   never patched by a later inbound event.

   The read-only display is the `thinkingEffort` normalized field rendered by
   `AgentConfigPanel` via `NormalizedRow`, which shows both facts —
   `field.value` (the effort the next spawn launches) and, when a running ACP
   session differs, `field.overriddenValue` struck through. No component owns
   "configured vs current" logic; the reader's tier order feeds both facts. Do
   not add a second effort write path or restate the two-facts logic in a
   component.

   **Cut invariant — live mid-conversation effort machinery was deliberately
   removed.** Effort is spawn-scoped only: the worker holds one `startup_effort`
   read from `BUZZ_ACP_EFFORT_LEVEL` and applies it once at session creation
   (`apply_startup_effort` in `buzz-acp/src/pool.rs`); there is no pool-level
   effort authority, no live effort switching, and no effort-ack frame. Do not
   reintroduce a live effort-switch RPC, a pool effort field, or a
   mid-conversation effort control without a plan ruling. The archived live-effort
   machinery lives on `archive/claude-config-gaps-live-effort` for reference only.

15. **The persona `description` is public display metadata.** It is optional,
   capped at 280 characters, and validated through the shared visible-text
   policy (`validate_agent_description_text` in `definition_validation.rs`)
   on the raw authored bytes at create/update, snapshot import, publication,
   inbound sync, and the untrusted catalog parser — rejected, never stripped.
   It is deliberately EXCLUDED from `persona_content_hash`
   (`description_change_does_not_change_content_hash`), so a description-only
   edit never flips the restart badge on linked instances. Only the AUTHORED
   description exists — there is deliberately no derived/generated fallback;
   a blank description publishes an empty kind:0 `about`, exactly as before
   the field existed. Agent and team snapshots carry the authored description
   in the member profile's `about` and validate it before import. The trim/empty
   resolution exists twice and must stay in
   sync (port changes in the same PR): `lib/agentDescription.ts`
   (`effectiveAgentDescription`) feeds display surfaces, and its Rust twin
   (`managed_agents/agent_description.rs`, `effective_agent_description` /
   `record_effective_description`) feeds the publish path, where
   `profile_needs_sync` compares `about` (None == empty) so description edits
   reconcile instead of being clobbered. Persona-linked instances do not own a
   second description copy; snapshot export materializes the definition value
   only into the portable snapshot, and a dangling link resolves no description
   rather than reviving stale instance metadata. The agents-page card face shows the
   authored description as its second line, falling back to the model label
   when none exists (`UnifiedAgentsSection.tsx` composes it;
   `AgentIdentityCard` takes a presentational `subtitle`). The community catalog
   shows the same authored description before consent: a clamped two-line list
   subtitle for scanning and the full safely wrapped value in persona detail.
   The dialog field
   lives in `ui/AgentDescriptionField.tsx` (`AgentIdentityFields`), not
   inline in the over-1000-line dialogs.

16. **Owner-only builds constrain managed runtimes, not relay-agent mentions.**
    The compiled owner-only capability applies when Desktop starts or deploys a
    managed agent. Independently operated relay agents with NIP-OA ownership
    remain eligible in every build when their verified owner's signed
    `respond_to` policy admits the viewer and relay membership includes the
    target channel at publication. Owned nonmembers may be offered for preparation
    and Invite; this is not permission to publish. Final authorization refreshes
    the exact destination and retains captured selected identities across uploads
    and edits. Denial preserves the draft, never silently removes a selected key.
    See `docs/remote-mention-routing.md`. Marked builds require that verified owner coordinate but do
    not require it to equal the viewer; OSS builds retain compatibility with
    self-authored legacy directory records. Keep native discovery and send-time
    revalidation fail closed on invalid ownership or managed policy evidence,
    and on missing membership or directory evidence; do not add a cross-owner
    clamp to either mention path. Local `agents-data-changed` events
    refresh only local persona/team/managed-agent caches; they must never
    invalidate the remote relay directory.

17. **Databricks model discovery has one shared catalog authority.** Desktop and ACP call the shared `buzz-agent` discovery library; Desktop passes the effective merged `DATABRICKS_MODEL_FILTER` explicitly, and the library applies it to raw workspace endpoint IDs and Unity Catalog model-service FQNs after the additive union. A successful filtered-empty catalog is authoritative: it stays empty, disables switching, and never falls through to configured or known-model fallback. Verified provider-qualified exact records in `scripts/model-capabilities.json` take precedence for UC FQNs: `data_workflow_tools.goose.goose-claude-opus-5-5` uses Anthropic Messages, adaptive thinking, and `output_config.effort` (default medium). This does not confer capabilities on other namespaces or similarly named services. Uncurated UC FQNs do not inherit family effort capabilities: Claude service components select Anthropic Messages and expose no effort choices, while GPT-5-or-newer service components select OpenAI Responses with neutral fallback effort capabilities; other uncurated FQNs use MLflow Chat Completions. Catalog/schema components never infer routing. Keep exact-record precedence and fallback rules identical in the Rust and TypeScript capability interpreters and shared corpus. Effort inheritance, persistence, and clearing semantics are unchanged. Global Defaults preserves the discovered model ID as the selected value while its closed trigger renders the provider-scoped display label; do not force the raw persisted ID over that label.

18. **ACP transport is persona-owned before deployment.** Select `acp_command` in the persona create/edit form beside the harness. Deployment inherits that value; linked instances do not expose a competing post-deploy override. Legacy definitions without the field use `buzz-acp`; definition-less agents retain their stored command. Switching a linked definition back to stock resets the instance transport on the next spawn. Shared persona events and restart snapshots carry the field so edits apply on the next spawn.

19. **ACP command selection is convention-based.** The editor always offers
    stock `buzz-acp` and installed executable `buzz-*-acp` aliases discovered
    from normal executable search directories. It does not offer arbitrary
    command entry. A persisted value outside that set remains visible as an
    unavailable compatibility option but is not editable; selecting a conventional
    option replaces it. Discovery returns the path produced by the same resolver
    used at spawn, so a duplicate alias must never advertise one executable and
    later launch another. Keep these transitions in the pure
    `ui/acpCommandPicker.ts` helper and preserve persisted values across loading,
    failed discovery, and late candidate arrival. ACP-only selections must mark
    the form dirty, including catalog-update and embedded discard protection.
    Catalog and portable agent/team snapshots carry only stock or conventional
    aliases (ASCII letters, digits, hyphens, and underscores in the middle).
    Foreign artifacts with other command values are rejected; exports omit
    legacy machine-local commands. Owner-native and owner-device synchronization
    retain custom-command compatibility and are not an execution sandbox.
    Shared persona heads redact nonportable commands and emit explicit stock
    for resets; owner replay of a redacted head preserves only a nonportable
    local override. That local path is not synchronized through catalog heads.

20. **The default AI is one starred record, chosen in Rust; it is the
    channel routing agent.**
    `ManagedAgentRecord.is_default_ai` (`#[serde(default,
    skip_serializing_if = "std::ops::Not::not")]`) marks at most one managed
    agent per desktop. The single-selection rule lives in exactly one place,
    the pure `set_default_ai` in `managed_agents/types.rs`, which unstars every
    other record in the same pass and rejects key-less definition records
    (empty `pubkey`). Its only caller is the `set_channel_routing` command
    (`commands/channel_routing.rs`, rule 23), which moves the star and saves
    the routing mode as one user action; there is no standalone star command.
    The frontend only reads `ManagedAgent.isDefaultAi` (`lib/defaultAi.ts`:
    `findDefaultAi`, `isDefaultAiEligible`) and never keeps a rival "current
    default" copy or picks a default on its own. There is exactly one edit
    location: the routing agent picker in the Agents page **Channel routing**
    card (`ui/routing/ChannelRoutingCard.tsx` ->
    `ChannelRoutingAgentPicker`, `data-testid="agents-channel-routing"`),
    rendered by `AgentsView` directly under the page header for every viewer
    of `/agents` (no owner/bot gate). The profile Runtime tab carries **no**
    Default AI row (it was removed; `profile/ui/UserProfileDefaultAiWiring.test.mjs`
    pins that no profile source writes the star), and the header settings menu
    (`UserProfileAgentActions.tsx`) must not get one either: `UserProfilePanel`
    renders that slot only when `!isBot`, so for agents it never mounts.
    Settings › Agents › Conversations shows a read-only
    `ChannelRoutingSummaryRow` ("Channel routing: Host (Honey)") with a
    "Change on the Agents page" link, never a control.
    **The star's spawn consequence is its routing role**, and the role comes
    only from `channel_routing::routing_role_for(record, mode, owner)`: local
    spawn (`runtime.rs`, after the `descriptor.env` loop beside the session
    policy), the prospective restart snapshot (summary builder), and remote
    deploy (`agents_deploy.rs` `policy_env`) all call it, then
    `managed_agents/routing_env.rs` turns the role into env. Dispatcher ->
    `BUZZ_ACP_DISPATCHER=true` (clap `bool` spelling, same as
    `BUZZ_ACP_LAZY_POOL`); any other role -> the key is removed. The desktop
    never sets `BUZZ_ACP_DISPATCHER_CONFIG` (the harness default gate,
    channel owner/admin + the agent's owner with no AI authors, is what runs),
    and both keys are reserved env keys so a saved value can neither promote
    an ordinary agent to a router nor hand it a custom gate. The dispatcher
    base prompt is compiled into the harness; the desktop injects nothing
    beyond the flag. `SpawnConfigSnapshot.routing_role` captures the same
    value, so moving the star or switching the mode on a running agent raises
    the restart badge (`routing_role` entry, rendered plain) and gives the
    transition plan the agent's *running* role.

21. **The default AI joins channels you create through one attach path.**
    Every create form (channel/forum dialog and browser via
    `sidebar/lib/useCreateChannelForm`, project home via
    `projects/ui/useCreateProjectFormSettings`, project channels via the same
    channel dialog) renders the shared `ui/AddDefaultAiRow` ("Add your
    default AI") **only while the saved channel routing mode joins new
    channels** — `lib/defaultAi.ts` `routingJoinsNewChannels(mode)`: Host or
    Lead; Off, Smart routing, and an unknown (loading/failed) mode hide the
    row, because there the star is only remembered (rule 23). While shown it
    is a live switch when `useDefaultAi()` returns a starred agent, otherwise
    the same row disabled and off with `NO_DEFAULT_AI_HINT` (never hidden for
    a missing star — that made the setting look like a no-op).
    The switch is seeded from the desktop-local
    preference `lib/defaultAiPreferences.ts` (localStorage
    `buzz-default-ai-auto-join`, default on, broken JSON reads as the default;
    the global "Join new channels I create" checkbox lives under the routing
    agent picker in the Agents page Channel routing card; Settings › Agents
    no longer carries it). It is **not** a
    `GlobalAgentConfig` field — saving that config restarts every local agent.
    The submitted `addDefaultAi` is `resolveAddDefaultAi(star, switch,
    mode)`: `false` whenever no default AI exists or the mode does not join.
    The four creation points (`app/AppShell.tsx` channel + forum handlers,
    `projects/useCreateProject.ts`, `projects/useAddProjectChannel.ts`) all
    run the one post-create sequence `lib/joinAgentsAfterCreate.ts` after the
    channel id is known and navigation is done: the default AI joins first,
    then `useApplyTemplate.applyAgents`. (For a project home the
    form-requested persona/team agents are created inside `createProject`
    before the mutation resolves, so they precede both.) The join itself is
    `useAttachDefaultAi().attachDefaultAi(channelId)`, the only path: it
    resolves the star from the managed-agents **cache at call time** (never
    the render closure, which could attach an un-starred or deleted record as
    a ghost member), reuses `channelAgents.attachManagedAgentToChannel(
    channelId, { agent, role: "bot", ensureRunning: true })` (membership
    write, then start only when the agent is not running/deployed),
    invalidates the same query keys as `applyAgents`, is a no-op without a
    starred agent or when the channel routing mode — fetched at call time,
    since the user may have switched routing Off after opening the form —
    does not join (a failed fetch skips the join with a warning toast), and
    never rejects — a failure surfaces as `toast.warning`,
    never silently. The two steps must never run concurrently (both rewrite
    the replaceable membership event), and because the default AI is already
    a member when the template applies, `applyAgents` passes
    `skipPersonasAlreadyInChannel` to `createChannelManagedAgents`
    (`agentReuse.dropPersonasAlreadyInChannel`) so a template listing the
    default AI's persona reuses it instead of minting a second instance;
    the deploy dialog does not set that flag and keeps its deliberate
    "another instance" behaviour. Do not mint a new agent for the join
    (`applyAgents` is for template personas) and do not auto-join channels the
    user did not create: agent-initiated project channels
    (`projects/useProjectChannelRequests.ts`) intentionally never pass
    `addDefaultAi`.

22. **Start on launch is toggled from the Agents page card menu, nowhere
    else.** `ManagedAgent.startOnAppLaunch` flips only through
    `ui/AgentStartOnLaunchMenuItem.tsx`: a `DropdownMenuCheckboxItem` ("Start
    on launch", `role="menuitemcheckbox"` + `aria-checked`, no second label)
    mounted at the top of `PersonaActionsMenu` when the card's persona has a
    linked instance, and as the only entry of `StandaloneAgentActionsMenu` on
    definition-less instance cards (`UnifiedAgentsSection.tsx`). Visibility is
    the pure `lib/startOnLaunchMenu.ts` `startOnLaunchMenuState(agent)`: no
    instance → hidden (there is no record to persist on; the card's Start
    creates one with `startOnAppLaunch: true`), provider backend → visible but
    disabled with "Managed by provider" (Rust forces the flag off at create),
    local → live. The handler is
    `useManagedAgentActions.handleToggleStartOnAppLaunch` (`AgentsView` adapts
    `(agent, next)` to `(pubkey, next)`), one
    `set_managed_agent_start_on_app_launch` per click, reported through the
    section's feedback toast. The lock is one-directional in both senses: the
    item locks only on `isStartOnLaunchPending` — that mutation's own
    `isPending` — never on the page-wide `isActionPending`, so an unrelated
    start/stop does not grey it out; and that mutation is deliberately left
    **out** of `useManagedAgentActions.isPending`, so flipping the flag never
    greys out the other card actions or sibling menu items. The
    profile Runtime tab reports the flag as a plain read-only "Start on
    launch: Yes/No" row (`UserProfilePanelFields.tsx`, Agent configuration
    group) and carries no switch; `UserProfilePanel` owns no auto-start
    handler, and the header settings menu (`UserProfileAgentActions.tsx`)
    carries no Auto-start item (that slot is `!isBot`-only, unreachable for
    agents — rule 20). The card face shows no start-on-launch indicator.

23. **Channel routing is one Rust-owned enum; per-agent env comes only from
    `routing_role_for`; transitions go through `plan_routing_transition`.**
    `managed_agents/channel_routing/` owns `ChannelRoutingMode`
    (`off | host | lead | desktop-router`, kebab-case) stored alone in
    `<app-data>/agents/channel-routing.json` (`{ "mode": "host" }`; missing or
    unreadable reads as `host`, today's behavior). It is **not** a
    `GlobalAgentConfig` field (saving that restarts every agent). Off, Host,
    Lead, and Smart routing are selectable in this build
    (`ChannelRoutingMode::is_selectable`); `set_channel_routing` still refuses
    a mode that is not ready — Host/Lead without a routing agent, Lead on an
    agent it cannot run, Smart routing without a router model key (rule 24).
    `routing_role_for` is the whole
    role table: Host + star -> Dispatcher (any backend); Lead + star + local +
    owner known -> Lead; everything else, including the star under Off or
    Smart routing, -> None. `routing_env::apply_routing_env` also scrubs
    `BUZZ_ACP_SUBSCRIBE`/`BUZZ_ACP_CONFIG` when the config points into the
    desktop-generated `<app-data>/agents/routing/` directory and the role is
    not Lead, so a Lead-era value cannot outlive the mode; a user's own rules
    file elsewhere is preserved (those keys are deliberately not reserved).
    `set_channel_routing(mode, agentPubkey)` is one user action: star first
    (Host requires an agent), then the mode; each prefix is a valid state
    (Review-Proven rule 5). Off keeps the star so switching back remembers it.
    The card never saves on a radio click alone when no routing agent exists:
    choosing Host opens the picker as a draft and the pick is the save.
    `get_channel_routing` returns the saved mode, the star, and the pure
    `plan_routing_transition` over every keyed record (running role = the
    tracked pair's `SpawnConfigSnapshot.routing_role`; a provider deployment's
    is the role its last successful deploy shipped, stamped in
    `<app-data>/agents/routing/deployed-roles.json` by `deploy_to_provider`
    (a deployment from before stamps reads as Dispatcher iff starred); only a
    process adopted from an earlier desktop run is assumed to run its desired
    role, as the restart badge assumes): `stale` = running with a role other than the
    desired one, `hold` = wants a role while another running agent still
    holds one it is losing, `routerActive` = Smart routing saved and every
    running agent plain, `applied` = what actually runs (`switching` while
    anything is stale). The card's status line shows `applied`, never just
    the saved mode. Losing agents restart first: `decideAutoRestart` holds on
    `routingHold` (from the same plan, re-checked in `restartDriftedAgent`'s
    pre-fire fetch). A routing switch does not wait out the config-drift
    quiescence window: `routingTransition` (`routingTransitionFor` — local,
    stale, not held) makes `decideAutoRestart` fire as soon as every other
    gate is green (idle, connected, unconsumed edge), so after a save each
    idle loser restarts at once and the gainer is promoted the moment its
    hold releases; a mid-turn agent still waits. Ordinary drift keeps the
    window. The card's "Restart <name> now" (the page's ordinary
    `handleRestart`) is disabled with its reason while held or mid-turn, and
    replaced by a redeploy hint for a provider agent. The frontend query key
    nests under `managed-agents` so every agent-list invalidation refreshes
    it. Every launch is gated by the same hold rule
    (`channel_routing::launch_role`, from `spawn_agent_child` with the other
    tracked processes' roles and from the payload `deploy_to_provider`
    invokes): a held agent launches **plain** (no error), is stamped `None`,
    shows stale, and is promoted by the restart after the hold releases, so a
    manual Start, Restart, or redeploy cannot overlap two routing roles. The
    restart badge's prospective snapshot goes through the same `launch_role`
    (`build_managed_agent_summary`), so a held gainer shows **no** badge — a
    restart would only launch it plain again — and the badge lights once the
    hold releases; comparing against the unheld desired role instead badged
    a restart that could never clear it. The
    status line also names a held gainer that is not running yet ("Fizz
    becomes the host after Honey restarts.").
    Until some agent is an instance of `builtin:host`, the Host picker offers
    "Create a Host agent" (under the select, or alone when empty): an
    instance of the built-in `builtin:host` persona (minimal prompt — the
    harness's dispatcher base prompt carries the routing rules; definition
    `effort_level` seeded from `ACP_THOUGHT_LEVEL_FALLBACK_VALUES[0]`, never a
    literal), then `set_channel_routing(host, pk)` as a second write.
    **Lead needs no harness code.** Every lead spawn rewrites
    `<app-data>/agents/routing/lead-<pubkey[..12]>.toml`
    (`channel_routing/lead_rules.rs` `render_lead_rules`): the mention rule
    first with the harness default kinds `[9, 40003, 46010, 40007]` (first
    match wins, so a direct @mention is exactly today's turn), then
    `lead-listen` (kind 9, no mention, `author == "<owner hex>" &&
    !str_contains(content, "@")`), both `channels = "all"`.
    `lead_rules::apply_lead_spawn` runs right after `apply_routing_env` and
    sets `BUZZ_ACP_SUBSCRIBE=config`, `BUZZ_ACP_CONFIG=<that file>`, and
    appends `LEAD_LISTEN_ADDENDUM` to the system prompt the child would
    otherwise get (a prompt file is inlined); a write failure fails the spawn
    rather than silently running Mentions mode. Any other role's spawn deletes
    its own leftover file, and `set_channel_routing` plus app start sweep every
    `lead-*.toml` not used by a tracked Lead process. The rendering is pinned
    byte-for-byte to `crates/buzz-acp/tests/fixtures/lead_rules.toml`, which
    the harness's `lead_rules_fixture_tests` load and match events against —
    change both together. Lead is local-only: `set_channel_routing` refuses a
    remote agent (`lead_unavailable_reason`, mirroring the role table) and the
    Lead picker lists remote agents disabled with "runs remotely — Lead needs
    an agent on this computer"; the remembered star carries over to Lead only
    when it can lead.

24. **Smart routing (`desktop-router`) is a desktop-side, after-send pick
    delivered by editing the sent message; its model is an app task.**
    Only this user's own composer sends in stream/forum channels route; DMs,
    edits, drafts with tray-addressed agents, and
    any applied state other than Smart routing (Off, Host, Lead, or a switch
    in flight — the composer reads the plan's `routerActive`, never the saved
    mode) never call the model (`messages/lib/autoAssignGate.ts`). Typing
    never calls it and Enter never waits for it: the message posts unchanged,
    then `messages/ui/useAutoAssign.ts` queues it in its channel's batch
    (`messages/lib/autoRouteBatcher.ts`: fires 3 s after the last send,
    waits while a composer in that channel holds a draft, at most 20 s, at
    once at 12 messages). One call routes the batch into groups
    `{messageIds, pubkeys, relation: new|continue|amend|cancel, of}`, and
    `useMentionSendFlow`'s `deliverAutoRoute` publishes, per message, one
    same-body kind:40003 edit that newly `p`-tags its group's agents plus
    `["mention", pk, "auto-route"]` (buzz-acp wakes an agent on an edit that
    newly mentions it, so no harness change). A follow-up's edit also
    carries `["buzz:route", relation, of, thread_root, note]`: buzz-acp
    (`queue::route_follow_up`, resolved edits only, so the owner's) anchors
    the reply in `thread_root` and adds a `<follow-up>` line ("supplement or
    fix, don't redo", or "stop"); `note` says the same in the prompt's Tags. `messages/lib/autoRouteLedger.ts`
    keeps this desktop's deliveries for 30 minutes: a message there is never
    routed again, and the channel's entries are the router's PRIOR (with the
    agents mid-turn as WORKING). A follow-up the model gave no agent goes to
    the earlier delivery's agents. Session scope still follows the edited
    message; only the reply anchor moves. The sent row shows "Routing…", then "→ Delivered to
    Name", or "Not delivered" (`messages/lib/autoRouteStatus.ts`); an edit
    or delete first fences the pick out. In a routed channel (Smart
    routing about to route this send, or an applied Host/Lead agent that is
    a member of the channel) the owner's agent `@mentions` are soft: posted
    as `["mention", pk, "soft"]` with no `p` tag (`splitSoftAgentMentions`),
    so the routing decides who acts. Tray chips and agents the send created
    stay hard `p` recipients. Smart routing passes the soft mentions as
    MENTIONED (a mention may be the object, not the assignee); a failed
    call delivers to them instead, and a mentioned agent outside the roster
    is always delivered. The timeline never marks that
    delivery edit as an edit and keeps its `auto-route` tags across later
    edits (`formatTimelineMessages.ts`). Rust owns the rest (`src-tauri/src/message_routing/`):
    the aliased prompt (`a1…aN` agents, `m1…mN` new messages, `p1…pN` prior
    deliveries; pubkeys and event ids never sent; one shared
    `ROUTER_SYSTEM_PROMPT` for every route), caps, the strict
    `{"groups":[…]}` parser (every message in exactly one group, ≤2 agents), the per-route deadline (2 s API key, 12 s
    CLI), the call itself, and the JSONL comparison log
    (`<app-data>/agents/routing-log/desktop.jsonl`, rotated at 5 MB). The
    roster's capability text comes from `messages/lib/routerRoster.ts`:
    persona description → kind:0 `about` → my own agent's system-prompt
    prefix → name only. The router model is the `message-routing` entry in
    Settings › Models › Task models, stored in
    `<app-data>/agents/task-models.json` (never `GlobalAgentConfig`, so a
    change restarts nothing); its row renders provider + model through
    `ModelEffortFields` with `effort: null` (the router never thinks). Its
    Provider select lists every route explicitly, with real availability:
    API keys (Anthropic, OpenAI, OpenRouter — `buzz-agent::complete_once`
    over HTTP, keys from the Providers tab's `GlobalAgentConfig.env_vars`,
    then the process env), "Codex (ChatGPT subscription)" (`codex exec`
    one-shot) and "Claude Code (Claude subscription, slower)" (`claude -p`
    one-shot). Subscription routes go only through the official CLI on its
    own sign-in (`message_routing/cli.rs`): Buzz never reads, stores, or logs
    subscription tokens, and Claude subscription OAuth is never reused over
    HTTP (Anthropic policy). Automatic is fastest first: the global default
    API-key provider, other API keys, Codex, Claude Code. Smart routing is
    selectable when any route is ready; otherwise it is disabled with the
    missing piece, e.g. "Sign in to Codex or Claude Code, or add an API key ·
    Models".

## Channel-only runtime controls

Desktop observer controls identify a channel, not a thread session. The harness
rejects `cancel_turn` and `switch_model` with `ambiguous_target` when that channel
has multiple known session scopes, including retained idle scopes. Do not treat
that result as success or a deferred model switch. Stop feedback waits for the
harness result matching the control type, channel, and request ID; relay delivery
alone does not prove that a turn was signalled. A missing result is unconfirmed,
not success. The activity pane must use its resolved `sessionChannelId` for
both the outgoing control and result correlation, even without a loaded
`Channel` object. Stop is unavailable in an unscoped all-channel pane.

Per-thread observer controls remain a separate protocol/UI change. Do not tell
users to type `!cancel` beside an inline mention: the owner command requires
kind 9, body exactly `!cancel` after trimming, and the agent's separate `p` tag.
The automatic-mention picker also inserts literal `@Name` into the body, so it
does not provide an exact-command workaround. The UI must state this limitation
rather than offer an ineffective command. An authorized owner can instead use
the CLI with the channel and target thread root:

```sh
buzz messages send --channel <channel-id> --reply-to <thread-root-id> \
  --mention <agent-pubkey> --content '!cancel'
```

## The tests that enforce this

- `lib/agentConfigCore.test.mjs` — field model per harness × scope, clearing
  policy. Update when the capability model changes.
- `ui/agentConfigFieldsContract.test.mjs` — canonical behaviors + disclosure
  presets + `shouldShowModelStatusMessage` status-bypass +
  `shouldRenderModelControl` (successful-empty omit vs failure keep). If this
  fails, you probably reintroduced a per-surface flag or conflated empty with
  failed discovery.
- `ui/usePersonaModelDiscovery.test.mjs` — `synthesizeEmptyDiscoveryStatus`,
  `isCacheableDiscoveryResponse`, `deriveModelDiscoveryPending`,
  `isSuccessfulEmptyDiscovery`. If the "reopen to retry" copy becomes inert
  again, these tests will catch it.
- `ui/respondToFieldContract.test.mjs` — plain-language mode labels, the
  persistent warning contract for shared agent access, and its two render
  positions (after the people picker for `allowlist`).
- `lib/agentAccessWarning.test.mjs` — every mode × run-location copy variant
  plus both resolvers, including unknown-reads-as-local and
  blank-`runOn`-is-not-a-provider.
- `lib/personaCatalogRelay.test.mjs` and
  `ui/personaCatalogOwnerLabel.test.mjs` — reject invisible definition text
  and keep Markdown concealment syntax literal in the review surface.
- `../profile/ui/UserProfileRuntimeContent.test.mjs` — profile runtime panels
  cannot reintroduce build-mode previews or synthetic fallback controls, and
  Start on launch stays a read-only row (no switch, handler, or header-menu
  Auto-start item in any profile source).
- Rust `managed_agents::channel_routing` tests: the full `routing_role_for`
  table (mode × star × backend × owner), file fallbacks and round trip, and
  `transition_tests.rs`, an exhaustive exploration of every mode switch ×
  star move × running/remote set proving no reachable state runs two
  routing paths and every switch finishes, plus a hold-ignoring variant that
  must fail. `routing_env_tests.rs` pins role × prior env.
  `lib/channelRouting.test.mjs` pins the card copy, Restart now states, and
  the hold lookup; `ui/routing/ChannelRoutingCard.jsdom-test.mjs` pins one
  label owner per control, picker-only-under-Host, draft-until-pick, one
  `set_channel_routing` per action, and the restart buttons;
  `lib/autoRestartPolicy.test.mjs` has the `routingHold` row.
- Smart routing (rule 24): Rust `message_routing::tests` (prompt text,
  aliasing, caps, strict parser table, provider/model resolution incl.
  subscription-only, deadline timeout, log lines) and
  `managed_agents::task_models` tests; `buzz-agent` `complete_once_tests`;
  `messages/lib/autoAssignGate.test.mjs` (every skip reason),
  `messages/lib/routerRoster.test.mjs` (description priority, caps),
  `messages/ui/useAutoAssign.jsdom-test.mjs` (batch timing and draft hold,
  one call per batch, per-group delivery, exactly once, follow-up route note
  and PRIOR, edit fence, closed gate, soft-mention judging and fallback),
  Rust `events` `edit_carries_a_route_note_for_follow_ups_only`, `messages/ui/useMentionSendFlow.helpers.test.mjs` (soft split),
  `messages/ui/smartRoutingComposerWiring.test.mjs` (send seam),
  `../settings/ui/models/TaskModelRow.jsdom-test.mjs`, and the Smart routing
  cases in `ui/routing/ChannelRoutingCard.jsdom-test.mjs`.
- `lib/startOnLaunchMenu.test.mjs` — `startOnLaunchMenuState` table over
  instance presence × backend × stored flag.
- `ui/AgentStartOnLaunchMenuItem.jsdom-test.mjs` — the card menu item through
  the real `useManagedAgentActions` handler: `role="menuitemcheckbox"` +
  `aria-checked` with no extra label, one
  `set_managed_agent_start_on_app_launch` call carrying the inverted flag,
  provider backend disabled and never writing, no item without an instance.
- `desktop/tests/e2e/profile.spec.ts` — the owned-agent parity flow compares
  every profile tab when opened from Agents and from the agent's DM.
- `ui/AgentConfigPanelPresentation.test.mjs` — shared profile/agent config rows
  show only effective values, with an em dash for unknown values.
- `ui/acpCommandPicker.test.mjs` — stock/discovered/unavailable command mode,
  late discovery, query-failure compatibility, and conventional replacement of
  persisted unknown commands.
- `ui/effortPicker.test.mjs` — `effortPickerState` gating (local + discovered
  `effortConfigId` renders; local + catalog `effortFallbackValues` renders
  before any session; provider backend hides even with a fallback; no
  vocabulary hides), discovered-over-fallback option precedence, the
  out-of-list saved value staying selectable, and
  `effortSelectionToPersistedValue` sentinel → null. This is where the v4
  provider regression is pinned: the write control must never render for a
  provider backend. `ui/agentInstanceEditCancelSafety.test.mjs` pins the edit
  dialog's runtime switch (picker stays visible on the new runtime's fallback
  and shows the stored column — not the old runtime's effective env tier, so a
  pick equal to that stale value still persists; untouched Save writes
  nothing; a post-switch pick is dispatched). `ui/agentCreateEffort.test.mjs`
  mounts the real `RequestedAgentCreateDialogs` → `usePersonaActions` seam and
  pins that a Create pick reaches `create_managed_agent` as
  `input.effortLevel`, an untouched picker sends none, and a pick made before
  saved defaults re-seed a no-effort harness is dropped.
  `lib/instanceInputForDefinition.test.mjs` pins
  that the create-time `effortLevel` override rides the local mapping only.
  Rust: `catalog_exposes_effort_thought_level_only_for_acp_thought_level_harnesses`
  (presets tests) pins which harnesses publish the option id + fallback, and
  `normalize_create_effort_level_*` (agents tests) pins the create boundary.
- `desktop/tests/e2e/onboarding-agent-defaults.spec.ts` — onboarding behavior
  acceptance coverage for readiness, failure states, defaults, session-draft
  restoration, zero-write Skip, Next save failure/retry, navigation, and
  successful-empty vs failed optional-model discovery.
- `desktop/tests/e2e/agents.spec.ts` — community catalog descriptions remain
  visible in the list and full detail before Add agent, including long
  unbroken Unicode text without horizontal overflow.
- `lib/agentDescription.test.mjs` — authored-description resolution: trim,
  blank/missing → null.
- Rust: `catalog_exposes_subscription_provider_only_for_login_runtimes`
  (presets tests) pins `login_hint ⇔ subscription_provider` on every builtin
  and its projection through all four catalog constructors.
  `../settings/ui/models/providersSettingsModel.test.mjs` and
  `ProvidersSettingsTab.jsdom-test.mjs` pin Settings › Models as a
  provider-centric view over the same `GlobalAgentConfig` record: one
  `set_global_agent_config` per action, a harness switch routed through
  `resetConfigForHarnessChange`, one radio group for the single
  `preferred_runtime`/`provider` default, and `model` never written there.
- Rust: `runtime_metadata_env_vars` tests pin spawn-time key application.
- Rust: persona sharing/retention tests pin relay+owner scoping, durable
  enqueue errors, relay rejection/unavailability, and accepted publication.
- Rust: `definition_validation` and inbound persona tests pin the shared
  Unicode/control-character policy at local, import, publish, and sync gates.

## Managed avatar media

Desktop-managed profiles retain the saved persona/instance avatar as the desired
source. Never publish another configured community's authenticated `/media/` URL
verbatim: `relay::profile_avatar::localize_avatar` verifies/copies its bytes into
the caller-pinned target before kind:0 comparison/publication. The shared record
keeps the source URL, not the target projection. Transfer or kind:0 rejection
leaves the previous profile intact so normal reconciliation can retry. The
Agent-managed profiles opt-out still disables automatic reconciliation.

The configured community origin set is refreshed through narrow workspace IPC
before startup restore and when inactive communities change, without resetting
active community state. After source removal, the shared writer may reuse an
already-published target-local picture with the same original content hash;
this is not permission to fetch the removed source. It is not learned from profile URLs. Media transfer
uses the fixed agent signer, origin-scoped Blossom auth, no redirects, byte caps,
and hash/descriptor verification; ordinary public external avatars remain
unauthenticated passthrough. No image-reader proxy or tenant isolation exception.

## Keep this file true

**If you change how agent configuration is modeled, rendered, persisted,
applied, or cleared — update this file in the same PR.** A rule that no longer
matches the code is worse than no rule; a new pattern that isn't written down
here will be broken by the next agent that never learns it existed. Reviewers:
treat a config-behavior diff without a matching AGENTS.md diff (or an explicit
"no rules changed" note) as incomplete.
