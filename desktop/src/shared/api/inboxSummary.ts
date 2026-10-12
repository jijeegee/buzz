import { invokeTauri } from "@/shared/api/tauri";
import type {
  InboxSummaryCursor,
  InboxSummaryPage,
} from "@/features/home/lib/inboxSummary";

/**
 * Fetch one relay inbox summary page. `read` holds the user's own channel and
 * `thread:<root>` markers; the relay uses them only to count unread rows.
 */
export async function getInboxSummaryPage(
  before: InboxSummaryCursor | null,
  read: Record<string, number>,
  limit: number,
): Promise<InboxSummaryPage> {
  return invokeTauri<InboxSummaryPage>("get_inbox_summary", {
    before,
    limit,
    read,
  });
}
