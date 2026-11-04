package app.kuzz.mobile.push

import android.Manifest
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.content.ContextCompat

/**
 * Message notifications the wake engine posts, and their tap payload.
 *
 * Like KakaoTalk: one notification per Chats row (a room, or a thread of it),
 * titled like that row, whose expanded view lists its last few messages and
 * whose tap opens the newest one. All share one group with a summary.
 */
object KuzzNotifications {
    private const val CHANNEL_ID = "kuzz_messages"
    private const val TAG = "kuzz_message"
    private const val SUMMARY_TAG = "kuzz_message_summary"
    private const val SUMMARY_ID = 1
    private const val GROUP = "kuzz_messages"
    private const val MAX_LINES = 5
    private const val EXTRA_EVENT = "app.kuzz.mobile.push.eventId"
    private const val EXTRA_COMMUNITY = "app.kuzz.mobile.push.communityId"
    private const val EXTRA_CHANNEL = "app.kuzz.mobile.push.channelId"
    private const val EXTRA_THREAD = "app.kuzz.mobile.push.threadRootId"
    private const val EXTRA_CREATED_AT = "app.kuzz.mobile.push.createdAt"

    /** Every message the room notification shows: "eventId createdAt thread?". */
    private const val EXTRA_LINES = "app.kuzz.mobile.push.lines"

    /** The notification id of a room or its thread; 0 stays the generic notice's. */
    fun roomId(communityId: String, channelId: String, threadHeadId: String? = null): Int {
        val key = "$communityId\n$channelId" + (threadHeadId?.let { "\n$it" } ?: "")
        val id = key.hashCode() and 0x7fffffff
        return if (id == 0 || id == SUMMARY_ID) 2 else id
    }

    /**
     * `args`: id (int, generic notices), channelName (localized), title, body,
     * subtitle?, eventId?, communityId?, channelId? (all three or none open a
     * room), threadRootId? (with them, opens that thread instead), threadHeadId?
     * (its own notification), conversationTitle? (the room; without it `title`,
     * the sender, titles the notification), createdAt? (seconds, lets the app
     * remove the notification once it is read).
     */
    fun show(context: Context, args: Map<String, Any?>) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val title = args["title"] as? String ?: return
        val body = args["body"] as? String ?: return
        val manager = NotificationManagerCompat.from(context)
        val channelName = (args["channelName"] as? String)?.takeIf { it.isNotBlank() } ?: "Messages"
        manager.createNotificationChannel(
            NotificationChannelCompat.Builder(CHANNEL_ID, NotificationManagerCompat.IMPORTANCE_HIGH)
                .setName(channelName)
                .build(),
        )
        val eventId = args["eventId"] as? String
        val communityId = args["communityId"] as? String
        val roomId = args["channelId"] as? String
        val room = eventId != null && communityId != null && roomId != null
        val id = if (room) {
            roomId(communityId!!, roomId!!, args["threadHeadId"] as? String)
        } else {
            (args["id"] as? Number)?.toInt() ?: 0
        }
        val createdAt = (args["createdAt"] as? Number)?.toLong()
        val launch = context.packageManager.getLaunchIntentForPackage(context.packageName)
            ?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val extras = Bundle()
        if (room) {
            extras.putString(EXTRA_EVENT, eventId)
            extras.putString(EXTRA_COMMUNITY, communityId)
            extras.putString(EXTRA_CHANNEL, roomId)
            (args["threadRootId"] as? String)?.let { extras.putString(EXTRA_THREAD, it) }
            createdAt?.let { extras.putLong(EXTRA_CREATED_AT, it) }
            launch?.putExtras(extras)
        }
        val builder = NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(context.applicationInfo.icon)
            .setContentTitle(title)
            .setContentText(body)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
        (args["subtitle"] as? String)?.let { builder.setSubText(it) }
        if (room) {
            val previous = active(context, TAG, id)
            val previousLines = previous?.extras?.getStringArrayList(EXTRA_LINES).orEmpty()
            if (previousLines.any { it.substringBefore(' ') == eventId }) return
            // A late wake for an older message never replaces the newer one.
            val previousAt = previous?.extras?.getLong(EXTRA_CREATED_AT, 0L) ?: 0L
            if (createdAt != null && createdAt < previousAt) return
            val line = listOfNotNull(
                eventId,
                (createdAt ?: 0L).toString(),
                args["threadRootId"] as? String,
            ).joinToString(" ")
            extras.putStringArrayList(
                EXTRA_LINES,
                ArrayList(previousLines.takeLast(MAX_LINES - 1) + line),
            )
            val style = roomStyle(
                previous,
                args["conversationTitle"] as? String,
                title,
                body,
                createdAt,
            )
            builder.setStyle(style)
                .setContentTitle(style.conversationTitle ?: title)
                .addExtras(extras)
                .setGroup(GROUP)
            createdAt?.let { builder.setWhen(it * 1000) }
        } else {
            builder.setStyle(NotificationCompat.BigTextStyle().bigText(body))
        }
        if (launch != null) {
            builder.setContentIntent(
                PendingIntent.getActivity(
                    context,
                    id,
                    launch,
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                ),
            )
        }
        try {
            manager.notify(TAG, id, builder.build())
            if (room) {
                manager.notify(
                    SUMMARY_TAG,
                    SUMMARY_ID,
                    NotificationCompat.Builder(context, CHANNEL_ID)
                        .setSmallIcon(context.applicationInfo.icon)
                        .setCategory(NotificationCompat.CATEGORY_MESSAGE)
                        .setGroup(GROUP)
                        .setGroupSummary(true)
                        .setGroupAlertBehavior(NotificationCompat.GROUP_ALERT_CHILDREN)
                        .setAutoCancel(true)
                        .build(),
                )
            }
        } catch (_: SecurityException) {
            // Permission withdrawn between the check and the post.
        }
    }

    /** The room's earlier lines (newest last, at most [MAX_LINES]) plus this one. */
    private fun roomStyle(
        previous: android.app.Notification?,
        conversationTitle: String?,
        sender: String,
        body: String,
        createdAt: Long?,
    ): NotificationCompat.MessagingStyle {
        val style = NotificationCompat.MessagingStyle(Person.Builder().setName("You").build())
        val earlierStyle = previous
            ?.let { NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(it) }
        val earlier = earlierStyle?.messages.orEmpty()
        for (message in earlier.takeLast(MAX_LINES - 1)) style.addMessage(message)
        style.addMessage(
            NotificationCompat.MessagingStyle.Message(
                body,
                (createdAt ?: (System.currentTimeMillis() / 1000)) * 1000,
                Person.Builder().setName(sender).build(),
            ),
        )
        // A failed room lookup keeps the title the notification already has.
        style.conversationTitle = conversationTitle ?: earlierStyle?.conversationTitle
        style.isGroupConversation = style.conversationTitle != null
        return style
    }

    private fun active(context: Context, tag: String, id: Int): android.app.Notification? {
        val system = context.getSystemService(NotificationManager::class.java) ?: return null
        return try {
            system.activeNotifications.firstOrNull { it.tag == tag && it.id == id }?.notification
        } catch (_: RuntimeException) {
            null
        }
    }

    /**
     * One row per message a room notification in the tray shows (same key);
     * the app removes a room's notification once all its rows are read.
     */
    fun delivered(context: Context): List<Map<String, Any>> {
        val system = context.getSystemService(NotificationManager::class.java) ?: return emptyList()
        val active = try {
            system.activeNotifications
        } catch (_: RuntimeException) {
            return emptyList()
        }
        return active.filter { it.tag == TAG && it.id != 0 }.flatMap { posted ->
            val extras = posted.notification.extras
            val communityId = extras.getString(EXTRA_COMMUNITY) ?: return@flatMap emptyList()
            val channelId = extras.getString(EXTRA_CHANNEL) ?: return@flatMap emptyList()
            val lines = extras.getStringArrayList(EXTRA_LINES) ?: return@flatMap emptyList()
            lines.map { line ->
                val parts = line.split(' ')
                buildMap<String, Any> {
                    put("key", posted.id.toString())
                    put("eventId", parts[0])
                    put("communityId", communityId)
                    put("channelId", channelId)
                    // A malformed row (0) keeps the notification.
                    put("createdAt", parts.getOrNull(1)?.toLongOrNull() ?: 0L)
                    parts.getOrNull(2)?.let { put("threadRootId", it) }
                }
            }
        }
    }

    /** Removes room notifications by key, and the summary once no room is left. */
    fun remove(context: Context, keys: List<String>) {
        val manager = NotificationManagerCompat.from(context)
        for (key in keys) key.toIntOrNull()?.let { manager.cancel(TAG, it) }
        val system = context.getSystemService(NotificationManager::class.java) ?: return
        val left = try {
            // Cancels apply asynchronously: leave out the rooms just removed.
            system.activeNotifications.any {
                it.tag == TAG && it.id != 0 && it.id.toString() !in keys
            }
        } catch (_: RuntimeException) {
            true
        }
        if (!left) manager.cancel(SUMMARY_TAG, SUMMARY_ID)
    }

    fun payload(intent: Intent): Map<String, String>? {
        val eventId = intent.getStringExtra(EXTRA_EVENT) ?: return null
        val communityId = intent.getStringExtra(EXTRA_COMMUNITY) ?: return null
        val channelId = intent.getStringExtra(EXTRA_CHANNEL) ?: return null
        val payload = mutableMapOf(
            "eventId" to eventId,
            "communityId" to communityId,
            "channelId" to channelId,
        )
        intent.getStringExtra(EXTRA_THREAD)?.let { payload["threadRootId"] = it }
        return payload
    }

    fun clearPayload(intent: Intent) {
        intent.removeExtra(EXTRA_EVENT)
        intent.removeExtra(EXTRA_COMMUNITY)
        intent.removeExtra(EXTRA_CHANNEL)
        intent.removeExtra(EXTRA_THREAD)
        intent.removeExtra(EXTRA_CREATED_AT)
    }
}
