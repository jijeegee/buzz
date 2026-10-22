import { t as translate, useT } from "@/shared/i18n";
import * as React from "react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  useCreateGroupChatMutation,
  useOpenDmMutation,
  useUpsertCachedChannel,
} from "@/features/channels/hooks";
import {
  invitationsSentText,
  MAX_NEW_CHAT_PEOPLE,
} from "@/features/channels/lib/groupChat";
import { useStartChatConfirm } from "@/features/channels/ui/StartChatConfirm";
import type { Channel } from "@/shared/api/types";
import { useSendMessageMutation } from "@/features/messages/hooks";
import { getKeyboardSearchSelection } from "@/features/profile/lib/userCandidateSearch";
import { SelectedRecipientChip } from "@/features/profile/ui/SelectedRecipientChip";
import { useIdentityQuery } from "@/shared/api/hooks";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { Popover, PopoverAnchor, PopoverContent } from "@/shared/ui/popover";
import { Skeleton } from "@/shared/ui/skeleton";

import { MessageComposer } from "./MessageComposer";
import { NewMessageResultRow } from "./NewMessageResultRow";
import {
  formatRecipientName,
  useNewMessageRecipients,
} from "./useNewMessageRecipients";

/** Chips shown before the rest collapse into "+N". */
const VISIBLE_RECIPIENT_CHIPS = 6;

/** Identity of a prepared conversation: the people and the chosen name. */
function preparedKey(pubkeys: readonly string[], name: string) {
  return `${[...pubkeys].sort().join(",")}|${name}`;
}

/**
 * Conversation-shaped compose surface for starting a conversation. One
 * person opens the 1:1; two or more start a new chat (optional name) with
 * one create. The normal chat header becomes an inline "To:" field, while
 * recipient discovery lives in an attached popover.
 */
export function NewMessageScreen() {
  const t = useT();
  const identityQuery = useIdentityQuery();
  const currentPubkey = identityQuery.data?.pubkey;
  const openDmMutation = useOpenDmMutation();
  const createGroupChatMutation = useCreateGroupChatMutation();
  const startChatConfirm = useStartChatConfirm();
  const [chatName, setChatName] = React.useState("");
  const [showAllRecipients, setShowAllRecipients] = React.useState(false);
  const upsertCachedChannel = useUpsertCachedChannel();
  const sendMessageMutation = useSendMessageMutation(null, identityQuery.data);
  const { goChannel } = useAppNavigation();

  const [isRecipientPickerOpen, setIsRecipientPickerOpen] =
    React.useState(true);
  const [highlightedRecipientPubkey, setHighlightedRecipientPubkey] =
    React.useState<string | null>(null);
  const [inspectedRecipientPubkey, setInspectedRecipientPubkey] =
    React.useState<string | null>(null);
  const [submitErrorMessage, setSubmitErrorMessage] = React.useState<
    string | null
  >(null);
  const [isPreparingMentionSend, setIsPreparingMentionSend] =
    React.useState(false);
  const searchInputRef = React.useRef<HTMLInputElement>(null);
  const toFieldRef = React.useRef<HTMLDivElement>(null);
  const preparedDirectMessageRef = React.useRef<Channel | null>(null);
  const preparedKeyRef = React.useRef<string | null>(null);
  const invitedCountRef = React.useRef({ invited: 0, picked: 0 });
  const isMountedRef = React.useRef(false);
  const isPending =
    isPreparingMentionSend ||
    openDmMutation.isPending ||
    createGroupChatMutation.isPending ||
    sendMessageMutation.isPending;

  const {
    deferredSearchQuery,
    handleDirectoryScroll,
    hasReachedRecipientLimit,
    isDirectoryLoading,
    ownerProfiles,
    removeUser,
    searchError,
    searchQuery,
    searchResults,
    selectUser,
    selectedUsers,
    setSearchQuery,
  } = useNewMessageRecipients({ active: true, currentPubkey });

  const isSearchTransitionPending = searchQuery.trim() !== deferredSearchQuery;
  const visibleSearchResults =
    isSearchTransitionPending || isDirectoryLoading ? [] : searchResults;
  const showRecipientPicker = isRecipientPickerOpen && !isPending;
  const highlightedRecipientIndex = React.useMemo(() => {
    if (!showRecipientPicker || visibleSearchResults.length === 0) {
      return -1;
    }

    if (highlightedRecipientPubkey === null) {
      return 0;
    }

    return visibleSearchResults.findIndex(
      (user) => user.pubkey === highlightedRecipientPubkey,
    );
  }, [highlightedRecipientPubkey, showRecipientPicker, visibleSearchResults]);
  const highlightedRecipient =
    highlightedRecipientIndex < 0
      ? null
      : (visibleSearchResults[highlightedRecipientIndex] ?? null);

  React.useEffect(() => {
    if (
      highlightedRecipientPubkey &&
      !isSearchTransitionPending &&
      highlightedRecipientIndex < 0
    ) {
      setHighlightedRecipientPubkey(null);
    }
  }, [
    highlightedRecipientIndex,
    highlightedRecipientPubkey,
    isSearchTransitionPending,
  ]);

  React.useEffect(() => {
    if (!highlightedRecipient || !showRecipientPicker) {
      return;
    }

    document
      .getElementById(`new-dm-option-${highlightedRecipient.pubkey}`)
      ?.scrollIntoView({ block: "nearest" });
  }, [highlightedRecipient, showRecipientPicker]);

  React.useEffect(() => {
    isMountedRef.current = true;
    searchInputRef.current?.focus({ preventScroll: true });

    return () => {
      isMountedRef.current = false;
    };
  }, []);

  const handleRemoveUser = React.useCallback(
    (pubkey: string) => {
      preparedDirectMessageRef.current = null;
      preparedKeyRef.current = null;
      setInspectedRecipientPubkey((current) =>
        current === pubkey ? null : current,
      );
      removeUser(pubkey);
    },
    [removeUser],
  );

  const handleSelectUser = React.useCallback(
    (user: Parameters<typeof selectUser>[0]) => {
      preparedDirectMessageRef.current = null;
      selectUser(user);
      setHighlightedRecipientPubkey(null);
      setSubmitErrorMessage(null);
      setIsRecipientPickerOpen(true);
      searchInputRef.current?.focus({ preventScroll: true });
    },
    [selectUser],
  );

  const handleResultSelect = React.useCallback(
    (user: Parameters<typeof selectUser>[0]) => {
      const isSelected = selectedUsers.some(
        (selectedUser) => selectedUser.pubkey === user.pubkey,
      );
      if (isSelected) {
        setSearchQuery("");
        setHighlightedRecipientPubkey(null);
        setSubmitErrorMessage(null);
        setIsRecipientPickerOpen(true);
        searchInputRef.current?.focus({ preventScroll: true });
        return;
      }

      handleSelectUser(user);
    },
    [handleSelectUser, selectedUsers, setSearchQuery],
  );

  const openDirectMessage = React.useCallback(
    async (additionalParticipantPubkeys: string[] = []) => {
      const currentNormalizedPubkey = currentPubkey
        ? normalizePubkey(currentPubkey)
        : null;
      const requestedPubkeys = [
        ...new Set(
          [
            ...selectedUsers.map((user) => user.pubkey),
            ...additionalParticipantPubkeys,
          ].map(normalizePubkey),
        ),
      ].filter((pubkey) => pubkey && pubkey !== currentNormalizedPubkey);
      const isChat = requestedPubkeys.length > 1;
      const name = isChat ? chatName.trim() : "";
      const key = preparedKey(requestedPubkeys, name);
      const preparedDirectMessage = preparedDirectMessageRef.current;
      if (preparedDirectMessage && preparedKeyRef.current === key) {
        return preparedDirectMessage;
      }

      if (
        openDmMutation.isPending ||
        createGroupChatMutation.isPending ||
        sendMessageMutation.isPending ||
        requestedPubkeys.length === 0
      ) {
        return null;
      }
      if (requestedPubkeys.length > MAX_NEW_CHAT_PEOPLE) {
        setSubmitErrorMessage(
          translate("messages.newMessage.limit", {
            count: MAX_NEW_CHAT_PEOPLE,
          }),
        );
        return null;
      }

      setSubmitErrorMessage(null);

      try {
        let conversation: Channel;
        if (isChat) {
          const result = await createGroupChatMutation.mutateAsync({
            name,
            pubkeys: requestedPubkeys,
          });
          conversation = result.channel;
          invitedCountRef.current = {
            invited: result.invited.length,
            picked: requestedPubkeys.length,
          };
        } else {
          conversation = await openDmMutation.mutateAsync({
            pubkeys: requestedPubkeys,
          });
          invitedCountRef.current = { invited: 0, picked: 1 };
        }
        preparedDirectMessageRef.current = conversation;
        preparedKeyRef.current = key;
        return conversation;
      } catch (error) {
        setSubmitErrorMessage(
          error instanceof Error
            ? error.message
            : translate("messages.newMessage.startFailed"),
        );
        return null;
      }
    },
    [
      chatName,
      createGroupChatMutation.isPending,
      createGroupChatMutation.mutateAsync,
      currentPubkey,
      openDmMutation.isPending,
      openDmMutation.mutateAsync,
      selectedUsers,
      sendMessageMutation.isPending,
    ],
  );

  // A mention of someone not picked asks first, then joins them to the new
  // chat. Nobody is added silently.
  const prepareSendChannel = React.useCallback(
    async (additionalParticipantPubkeys: string[] = []) => {
      const picked = new Set(
        selectedUsers.map((user) => normalizePubkey(user.pubkey)),
      );
      const me = currentPubkey ? normalizePubkey(currentPubkey) : null;
      const extras = [
        ...new Set(additionalParticipantPubkeys.map(normalizePubkey)),
      ].filter((pubkey) => pubkey && pubkey !== me && !picked.has(pubkey));
      if (extras.length > 0 && !(await startChatConfirm.confirm(extras))) {
        return null;
      }
      const directMessage = await openDirectMessage(extras);
      return directMessage?.id ?? null;
    },
    [currentPubkey, openDirectMessage, selectedUsers, startChatConfirm.confirm],
  );

  const sendFirstMessage = React.useCallback(
    async (
      content: string,
      mentionPubkeys: string[],
      mediaTags?: string[][],
      targetChannelId?: string | null,
    ) => {
      const preparedDirectMessage = preparedDirectMessageRef.current;
      const directMessage =
        targetChannelId && preparedDirectMessage?.id === targetChannelId
          ? preparedDirectMessage
          : await openDirectMessage();
      if (!directMessage) {
        throw new Error(
          submitErrorMessage ?? "Choose at least one recipient first.",
        );
      }

      try {
        await sendMessageMutation.mutateAsync({
          targetChannel: directMessage,
          content,
          mentionPubkeys,
          mediaTags,
          // A newly opened DM is not subscribed yet, so publish its first
          // message through the acknowledged HTTP path. This avoids holding
          // the entire navigation on a WebSocket OK frame that staging may
          // never deliver.
          transport: "http",
        });
      } catch (error) {
        // A created chat is kept so a retry sends there instead of making
        // a second one; a 1:1 reopen is idempotent.
        if (!directMessage.groupChat) preparedDirectMessageRef.current = null;
        const message =
          error instanceof Error
            ? error.message
            : translate("messages.newMessage.sendFailed");
        if (isMountedRef.current) setSubmitErrorMessage(message);
        throw error;
      }

      if (!isMountedRef.current) {
        return;
      }

      await upsertCachedChannel(directMessage);
      if (!isMountedRef.current) {
        return;
      }
      await goChannel(directMessage.id, { replace: true });
      const { invited, picked } = invitedCountRef.current;
      const invitedText = invitationsSentText(invited, picked);
      if (invitedText) toast(invitedText);
    },
    [
      goChannel,
      openDirectMessage,
      sendMessageMutation,
      submitErrorMessage,
      upsertCachedChannel,
    ],
  );

  const composerPlaceholder =
    selectedUsers.length === 0
      ? t("messages.newMessage.chooseRecipient")
      : selectedUsers.length === 1
        ? t("messages.composer.messagePlaceholder", {
            channel: formatRecipientName(selectedUsers[0]),
          })
        : t("messages.newMessage.messagePeople", {
            count: selectedUsers.length,
          });

  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="new-message-page"
    >
      <header
        className="relative z-40 shrink-0 cursor-default select-none border-b border-border/35 bg-background/80 px-5 py-2 backdrop-blur-md supports-backdrop-filter:bg-background/70 dark:bg-background/70 dark:backdrop-blur-xl dark:supports-backdrop-filter:bg-background/55"
        data-testid="new-message-header"
        data-tauri-drag-region
      >
        <div className="flex min-h-9 min-w-0 items-center">
          <Popover
            onOpenChange={(open) => {
              setIsRecipientPickerOpen(
                open || inspectedRecipientPubkey !== null,
              );
            }}
            open={showRecipientPicker}
          >
            <PopoverAnchor asChild>
              {/* biome-ignore lint/a11y/noStaticElementInteractions: clicking anywhere in the recipient field focuses its input */}
              {/* biome-ignore lint/a11y/useKeyWithClickEvents: the nested combobox is the keyboard-accessible focus target */}
              <div
                className="group/to-field flex min-h-9 min-w-0 flex-1 cursor-text flex-wrap items-center gap-1.5 py-1"
                data-testid="new-message-to-field"
                onClick={(event) => {
                  // Portaled popovers (recipient inspection and its nested key
                  // copy) still bubble through React's tree to this handler,
                  // but their event targets are not DOM descendants of the
                  // field. Those clicks belong to the popover's own controls
                  // — they must not steal focus into the search input (which
                  // dismisses the popover via focus-outside) or reopen the
                  // picker. Only clicks physically within the recipient field
                  // focus its input.
                  const { currentTarget, target } = event;
                  if (
                    !(target instanceof Node) ||
                    !currentTarget.contains(target)
                  ) {
                    return;
                  }
                  setIsRecipientPickerOpen(true);
                  searchInputRef.current?.focus({ preventScroll: true });
                }}
                ref={toFieldRef}
              >
                <span className="shrink-0 text-base font-semibold tracking-tight">
                  {t("messages.newMessage.toLabel")}
                </span>
                {(showAllRecipients
                  ? selectedUsers
                  : selectedUsers.slice(0, VISIBLE_RECIPIENT_CHIPS)
                ).map((user) => (
                  <SelectedRecipientChip
                    disabled={isPending}
                    inspectionOpen={inspectedRecipientPubkey === user.pubkey}
                    key={user.pubkey}
                    label={formatRecipientName(user)}
                    onInspectionOpenChange={(inspectionOpen) => {
                      setInspectedRecipientPubkey(
                        inspectionOpen ? user.pubkey : null,
                      );
                    }}
                    onRemove={() => handleRemoveUser(user.pubkey)}
                    testIds={{
                      chip: `new-dm-selected-${user.pubkey}`,
                      keyPopover: `new-dm-selected-key-popover-${user.pubkey}`,
                      name: `new-dm-recipient-name-${user.pubkey}`,
                      pubkey: `new-dm-selected-pubkey-${user.pubkey}`,
                    }}
                    user={user}
                  />
                ))}
                {!showAllRecipients &&
                selectedUsers.length > VISIBLE_RECIPIENT_CHIPS ? (
                  <button
                    className="h-7 shrink-0 rounded-full bg-muted px-2.5 text-sm font-medium text-muted-foreground hover:text-foreground"
                    data-testid="new-dm-more-recipients"
                    disabled={isPending}
                    onClick={(event) => {
                      event.stopPropagation();
                      setShowAllRecipients(true);
                    }}
                    type="button"
                  >
                    +{selectedUsers.length - VISIBLE_RECIPIENT_CHIPS}
                  </button>
                ) : null}
                <input
                  aria-activedescendant={
                    highlightedRecipient
                      ? `new-dm-option-${highlightedRecipient.pubkey}`
                      : undefined
                  }
                  aria-controls="new-dm-results"
                  aria-expanded={showRecipientPicker}
                  aria-label={t("messages.newMessage.toAria")}
                  autoComplete="off"
                  autoCorrect="off"
                  className="h-7 min-w-32 flex-1 bg-transparent text-base outline-hidden placeholder:text-muted-foreground"
                  data-testid="new-dm-search"
                  disabled={isPending}
                  id="new-dm-search"
                  onChange={(event) => {
                    setSearchQuery(event.target.value);
                    setHighlightedRecipientPubkey(null);
                    setSubmitErrorMessage(null);
                    setIsRecipientPickerOpen(true);
                  }}
                  onFocus={() => setIsRecipientPickerOpen(true)}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") {
                      event.preventDefault();
                      if (inspectedRecipientPubkey) {
                        setInspectedRecipientPubkey(null);
                        return;
                      }
                      setHighlightedRecipientPubkey(null);
                      setIsRecipientPickerOpen(false);
                      return;
                    }

                    if (
                      (event.key === "ArrowDown" || event.key === "ArrowUp") &&
                      visibleSearchResults.length > 0
                    ) {
                      event.preventDefault();
                      setIsRecipientPickerOpen(true);
                      setHighlightedRecipientPubkey(() => {
                        const current = highlightedRecipientIndex;
                        if (current < 0) {
                          const initialIndex =
                            event.key === "ArrowDown"
                              ? 0
                              : visibleSearchResults.length - 1;
                          return (
                            visibleSearchResults[initialIndex]?.pubkey ?? null
                          );
                        }

                        const direction = event.key === "ArrowDown" ? 1 : -1;
                        const nextIndex =
                          (current + direction + visibleSearchResults.length) %
                          visibleSearchResults.length;
                        return visibleSearchResults[nextIndex]?.pubkey ?? null;
                      });
                      return;
                    }

                    if (
                      event.key === "Backspace" &&
                      searchQuery.length === 0 &&
                      selectedUsers.length > 0
                    ) {
                      event.preventDefault();
                      const lastRecipient =
                        selectedUsers[selectedUsers.length - 1];
                      if (lastRecipient) {
                        document
                          .querySelector<HTMLElement>(
                            `[data-testid="new-dm-selected-${lastRecipient.pubkey}"]`,
                          )
                          ?.dispatchEvent(
                            new MouseEvent("click", {
                              bubbles: true,
                              detail: 0,
                            }),
                          );
                      }
                      return;
                    }

                    if (event.key !== "Enter") {
                      return;
                    }

                    if (searchQuery.trim().length === 0) {
                      if (highlightedRecipient) {
                        event.preventDefault();
                        handleSelectUser(highlightedRecipient);
                      }
                      return;
                    }

                    const keyboardSelection =
                      highlightedRecipient ??
                      getKeyboardSearchSelection({
                        currentQuery: searchQuery,
                        rankedQuery: deferredSearchQuery,
                        results: visibleSearchResults,
                      });
                    if (!keyboardSelection) {
                      return;
                    }

                    event.preventDefault();
                    handleResultSelect(keyboardSelection);
                  }}
                  ref={searchInputRef}
                  role="combobox"
                  spellCheck={false}
                  type="text"
                  value={searchQuery}
                />
              </div>
            </PopoverAnchor>
            <PopoverContent
              align="start"
              className="w-[min(36rem,calc(100vw-3rem))] overflow-hidden p-0"
              data-testid="new-message-recipient-popover"
              onCloseAutoFocus={(event) => event.preventDefault()}
              onInteractOutside={(event) => {
                const target = event.detail.originalEvent.target;
                if (
                  target instanceof Element &&
                  (toFieldRef.current?.contains(target) ||
                    Boolean(target.closest("[data-recipient-key-popover]")))
                ) {
                  event.preventDefault();
                }
              }}
              onOpenAutoFocus={(event) => event.preventDefault()}
              side="bottom"
              sideOffset={6}
            >
              <div
                className="max-h-80 overflow-y-auto"
                data-testid="new-dm-results"
                id="new-dm-results"
                onScroll={handleDirectoryScroll}
                role="listbox"
              >
                {visibleSearchResults.length > 0 ? (
                  <div>
                    {visibleSearchResults.map((user) => {
                      const isSelected = selectedUsers.some(
                        (selectedUser) => selectedUser.pubkey === user.pubkey,
                      );
                      return (
                        <NewMessageResultRow
                          currentPubkey={currentPubkey}
                          disabled={
                            isPending ||
                            (hasReachedRecipientLimit && !isSelected)
                          }
                          isAlreadySelected={isSelected}
                          isKeyboardHighlighted={
                            highlightedRecipient?.pubkey === user.pubkey
                          }
                          key={user.pubkey}
                          onSelect={handleResultSelect}
                          ownerProfiles={ownerProfiles}
                          user={user}
                        />
                      );
                    })}
                  </div>
                ) : isDirectoryLoading || isSearchTransitionPending ? (
                  <div
                    aria-busy="true"
                    aria-label={t("messages.newMessage.loadingDirectory")}
                    className="space-y-3 px-4 py-3"
                    data-testid="new-dm-loading"
                    role="status"
                  >
                    {["w-40", "w-32", "w-48"].map((nameWidth) => (
                      <div className="flex items-center gap-3" key={nameWidth}>
                        <Skeleton className="h-9 w-9 shrink-0 rounded-full" />
                        <div className="flex min-w-0 flex-1 flex-col gap-2">
                          <Skeleton className={`h-4 ${nameWidth}`} />
                          <Skeleton className="h-3 w-24" />
                        </div>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p
                    className="px-4 py-3 text-sm text-muted-foreground"
                    data-testid="new-dm-empty"
                  >
                    {deferredSearchQuery.length === 0
                      ? t("messages.newMessage.directoryEmpty")
                      : t("messages.newMessage.noMatches")}
                  </p>
                )}
              </div>
            </PopoverContent>
          </Popover>

          {isPending ? (
            <span
              className="shrink-0 pl-2 text-sm text-muted-foreground"
              data-testid="new-dm-opening"
            >
              {t("messages.newMessage.opening")}
            </span>
          ) : null}
        </div>
      </header>

      <div
        className="min-h-0 flex-1 bg-background"
        data-testid="new-message-body"
      />

      {selectedUsers.length > 1 ? (
        <div className="px-5 pb-2" data-testid="new-chat-name-row">
          <input
            aria-label={t("messages.newMessage.chatNameAria")}
            className="h-9 w-full rounded-lg border border-border/60 bg-transparent px-3 text-sm outline-hidden placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
            data-testid="new-chat-name"
            disabled={isPending}
            maxLength={100}
            onChange={(event) => {
              preparedDirectMessageRef.current = null;
              preparedKeyRef.current = null;
              setChatName(event.target.value);
            }}
            placeholder={t("messages.newMessage.chatNamePlaceholder")}
            type="text"
            value={chatName}
          />
        </div>
      ) : null}
      {hasReachedRecipientLimit ? (
        <p
          className="px-5 pb-2 text-sm text-muted-foreground"
          data-testid="new-dm-limit"
        >
          {t("messages.newMessage.limit", { count: MAX_NEW_CHAT_PEOPLE })}
        </p>
      ) : null}
      {searchError ? (
        <p className="px-5 pb-2 text-sm text-destructive">
          {searchError.message}
        </p>
      ) : null}
      {submitErrorMessage ? (
        <p className="px-5 pb-2 text-sm text-destructive">
          {submitErrorMessage}
        </p>
      ) : null}

      <MessageComposer
        channelName={t("messages.newMessage.channelName")}
        channelType="dm"
        containerClassName="px-5"
        disabled={isPending || selectedUsers.length === 0}
        isSending={isPending}
        onPrepareSendChannel={prepareSendChannel}
        onPreparingMentionSendChange={setIsPreparingMentionSend}
        onSend={sendFirstMessage}
        placeholder={composerPlaceholder}
        showBackgroundUploadProgress
      />
      <div aria-hidden="true" className="min-h-8 bg-background px-5 pb-1.5" />
      {startChatConfirm.dialog}
    </div>
  );
}
