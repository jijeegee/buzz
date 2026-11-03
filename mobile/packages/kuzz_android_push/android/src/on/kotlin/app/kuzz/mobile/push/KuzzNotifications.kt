package app.kuzz.mobile.push

import android.Manifest
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

/** Message notifications the wake engine posts, and their tap payload. */
object KuzzNotifications {
    private const val CHANNEL_ID = "kuzz_messages"
    private const val TAG = "kuzz_message"
    private const val EXTRA_EVENT = "app.kuzz.mobile.push.eventId"
    private const val EXTRA_COMMUNITY = "app.kuzz.mobile.push.communityId"
    private const val EXTRA_CHANNEL = "app.kuzz.mobile.push.channelId"
    private const val EXTRA_THREAD = "app.kuzz.mobile.push.threadRootId"

    /**
     * `args`: id (int), channelName (localized), title, body, subtitle?,
     * eventId?, communityId?, channelId? (all three or none open a room),
     * threadRootId? (with them, opens that thread instead).
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
        val id = (args["id"] as? Number)?.toInt() ?: 0
        val launch = context.packageManager.getLaunchIntentForPackage(context.packageName)
            ?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val eventId = args["eventId"] as? String
        val communityId = args["communityId"] as? String
        val roomId = args["channelId"] as? String
        if (launch != null && eventId != null && communityId != null && roomId != null) {
            launch.putExtra(EXTRA_EVENT, eventId)
            launch.putExtra(EXTRA_COMMUNITY, communityId)
            launch.putExtra(EXTRA_CHANNEL, roomId)
            (args["threadRootId"] as? String)?.let { launch.putExtra(EXTRA_THREAD, it) }
        }
        val builder = NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(context.applicationInfo.icon)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
        (args["subtitle"] as? String)?.let { builder.setSubText(it) }
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
        } catch (_: SecurityException) {
            // Permission withdrawn between the check and the post.
        }
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
    }
}
