package app.kuzz.mobile.push

import android.Manifest
import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import androidx.core.app.ActivityCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.messaging.FirebaseMessaging
import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.embedding.engine.plugins.activity.ActivityAware
import io.flutter.embedding.engine.plugins.activity.ActivityPluginBinding
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.PluginRegistry
import java.io.File

/**
 * `kuzz/android_push`, compiled in only with KUZZ_ANDROID_PUSH=true.
 *
 * Every Flutter engine gets an instance: the app's engine (permission,
 * token, notification taps) and the short-lived wake engine that
 * [KuzzPushMessagingService] starts (notifications, `wakeDone`).
 */
class KuzzAndroidPushPluginImpl :
    FlutterPlugin,
    ActivityAware,
    MethodChannel.MethodCallHandler,
    PluginRegistry.NewIntentListener,
    PluginRegistry.RequestPermissionsResultListener {
    private lateinit var context: Context
    private var channel: MethodChannel? = null
    private var activity: ActivityPluginBinding? = null
    private var pendingPermission: MethodChannel.Result? = null

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        context = binding.applicationContext
        channel = MethodChannel(binding.binaryMessenger, CHANNEL).also {
            it.setMethodCallHandler(this)
        }
        synchronized(instances) { instances.add(this) }
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        synchronized(instances) { instances.remove(this) }
        channel?.setMethodCallHandler(null)
        channel = null
    }

    override fun onAttachedToActivity(binding: ActivityPluginBinding) {
        activity = binding
        binding.addOnNewIntentListener(this)
        binding.addRequestPermissionsResultListener(this)
    }

    override fun onDetachedFromActivityForConfigChanges() = onDetachedFromActivity()

    override fun onReattachedToActivityForConfigChanges(binding: ActivityPluginBinding) =
        onAttachedToActivity(binding)

    override fun onDetachedFromActivity() {
        activity?.removeOnNewIntentListener(this)
        activity?.removeRequestPermissionsResultListener(this)
        activity = null
        pendingPermission?.success(permissionStatus())
        pendingPermission = null
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        if (KuzzEnrollmentKeys.handle(call, result)) return
        when (call.method) {
            "configure" -> {
                val options = KuzzFirebase.Options(
                    apiKey = call.argument<String>("apiKey").orEmpty(),
                    appId = call.argument<String>("appId").orEmpty(),
                    senderId = call.argument<String>("senderId").orEmpty(),
                    projectId = call.argument<String>("projectId").orEmpty(),
                )
                result.success(KuzzFirebase.configure(context, options))
            }
            "token" -> token(result)
            "permissionStatus" -> result.success(permissionStatus())
            "requestPermission" -> requestPermission(result)
            "openNotificationSettings" -> result.success(openNotificationSettings())
            "show" -> {
                @Suppress("UNCHECKED_CAST")
                val args = call.arguments as? Map<String, Any?> ?: emptyMap()
                KuzzNotifications.show(context, args)
                result.success(null)
                // The app may have read this message already: let it sweep.
                dispatchToApp("notificationPosted", null)
            }
            "deliveredMessageNotifications" -> result.success(KuzzNotifications.delivered(context))
            "removeDeliveredNotifications" -> {
                val keys = (call.arguments as? List<*>)?.filterIsInstance<String>().orEmpty()
                KuzzNotifications.remove(context, keys)
                result.success(null)
            }
            "cancelAll" -> {
                NotificationManagerCompat.from(context).cancelAll()
                result.success(null)
            }
            "takeLaunchPayload" -> {
                val intent = activity?.activity?.intent
                val payload = intent?.let { KuzzNotifications.payload(it) }
                if (payload != null) KuzzNotifications.clearPayload(intent)
                result.success(payload)
            }
            "pushInboxRoot" -> result.success(File(context.noBackupFilesDir, "push-inbox").path)
            "wakeDone" -> {
                KuzzPushMessagingService.finishWake()
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    private fun token(result: MethodChannel.Result) {
        if (!KuzzFirebase.ensure(context)) {
            result.error("not_configured", "Firebase is not configured", null)
            return
        }
        val messaging = FirebaseMessaging.getInstance()
        // Push is on: let FCM keep the token fresh (onNewToken).
        messaging.isAutoInitEnabled = true
        messaging.token.addOnCompleteListener { task ->
            if (task.isSuccessful) {
                result.success(task.result)
            } else {
                // The message carries the FCM reason, e.g. SERVICE_NOT_AVAILABLE
                // or MISSING_INSTANCEID_SERVICE (no Google Play services).
                val error = task.exception
                result.error("token", error?.javaClass?.simpleName ?: "unknown", error?.message)
            }
        }
    }

    private fun permissionStatus(): String {
        val enabled = NotificationManagerCompat.from(context).areNotificationsEnabled()
        if (Build.VERSION.SDK_INT < 33) return if (enabled) "authorized" else "denied"
        val granted = ContextCompat.checkSelfPermission(
            context, Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED
        if (granted) return if (enabled) "authorized" else "denied"
        return if (prefs(context).getBoolean(KEY_PERMISSION_ASKED, false)) "denied" else "notDetermined"
    }

    private fun requestPermission(result: MethodChannel.Result) {
        val current = permissionStatus()
        val host = activity?.activity
        if (Build.VERSION.SDK_INT < 33 || current == "authorized" || host == null ||
            pendingPermission != null
        ) {
            result.success(current)
            return
        }
        prefs(context).edit().putBoolean(KEY_PERMISSION_ASKED, true).apply()
        pendingPermission = result
        ActivityCompat.requestPermissions(
            host, arrayOf(Manifest.permission.POST_NOTIFICATIONS), PERMISSION_REQUEST,
        )
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ): Boolean {
        if (requestCode != PERMISSION_REQUEST) return false
        pendingPermission?.success(permissionStatus())
        pendingPermission = null
        return true
    }

    private fun openNotificationSettings(): Boolean {
        val host = activity?.activity ?: return false
        val intent = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
            .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
        return try {
            host.startActivity(intent)
            true
        } catch (error: Exception) {
            Log.w(TAG, "notification settings unavailable: ${error.javaClass.simpleName}")
            false
        }
    }

    override fun onNewIntent(intent: Intent): Boolean {
        val payload = KuzzNotifications.payload(intent) ?: return false
        KuzzNotifications.clearPayload(intent)
        channel?.invokeMethod("notificationOpened", payload)
        return true
    }

    companion object {
        const val CHANNEL = "kuzz/android_push"
        private const val TAG = "KuzzAndroidPush"
        private const val PERMISSION_REQUEST = 0x4b50
        private const val KEY_PERMISSION_ASKED = "notification_permission_asked"
        private val instances = mutableSetOf<KuzzAndroidPushPluginImpl>()
        private val main = Handler(Looper.getMainLooper())

        fun prefs(context: Context) =
            context.getSharedPreferences("kuzz_android_push", Context.MODE_PRIVATE)

        /** Tells the app's engine (the one with an activity), if running. */
        fun dispatchToApp(method: String, arguments: Any?) {
            main.post {
                val targets = synchronized(instances) { instances.filter { it.activity != null } }
                for (plugin in targets) plugin.channel?.invokeMethod(method, arguments)
            }
        }
    }
}

/** Firebase started from the build's defines, never from resources. */
object KuzzFirebase {
    data class Options(
        val apiKey: String,
        val appId: String,
        val senderId: String,
        val projectId: String,
    ) {
        val complete get() = listOf(apiKey, appId, senderId, projectId).all { it.isNotBlank() }
    }

    private const val KEY_API = "firebase_api_key"
    private const val KEY_APP = "firebase_app_id"
    private const val KEY_SENDER = "firebase_sender_id"
    private const val KEY_PROJECT = "firebase_project_id"

    /**
     * Starts the default FirebaseApp with [options] and remembers them, so a
     * cold start by an FCM message (no Dart yet) can start it too. These are
     * the app's public Firebase identifiers, not secrets.
     */
    @Synchronized
    fun configure(context: Context, options: Options): Boolean {
        if (!options.complete) return false
        KuzzAndroidPushPluginImpl.prefs(context).edit()
            .putString(KEY_API, options.apiKey)
            .putString(KEY_APP, options.appId)
            .putString(KEY_SENDER, options.senderId)
            .putString(KEY_PROJECT, options.projectId)
            .apply()
        return start(context, options)
    }

    /** True when the default FirebaseApp runs (started from saved options if needed). */
    @Synchronized
    fun ensure(context: Context): Boolean {
        if (FirebaseApp.getApps(context).isNotEmpty()) return true
        val prefs = KuzzAndroidPushPluginImpl.prefs(context)
        val saved = Options(
            apiKey = prefs.getString(KEY_API, null).orEmpty(),
            appId = prefs.getString(KEY_APP, null).orEmpty(),
            senderId = prefs.getString(KEY_SENDER, null).orEmpty(),
            projectId = prefs.getString(KEY_PROJECT, null).orEmpty(),
        )
        return saved.complete && start(context, saved)
    }

    private fun start(context: Context, options: Options): Boolean {
        if (FirebaseApp.getApps(context).isNotEmpty()) return true
        return try {
            FirebaseApp.initializeApp(
                context,
                FirebaseOptions.Builder()
                    .setApiKey(options.apiKey)
                    .setApplicationId(options.appId)
                    .setGcmSenderId(options.senderId)
                    .setProjectId(options.projectId)
                    .build(),
            )
            true
        } catch (error: Exception) {
            Log.w("KuzzAndroidPush", "Firebase start failed: ${error.javaClass.simpleName}")
            false
        }
    }
}

/** Whether the app is on screen (Q5: the wake does nothing then). */
object Foreground {
    fun now(): Boolean {
        val state = ActivityManager.RunningAppProcessInfo()
        ActivityManager.getMyMemoryState(state)
        return state.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
    }
}
