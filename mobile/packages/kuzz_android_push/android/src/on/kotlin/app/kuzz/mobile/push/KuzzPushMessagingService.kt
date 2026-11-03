package app.kuzz.mobile.push

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import io.flutter.FlutterInjector
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.engine.dart.DartExecutor
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Receives the push gateway's data-only `{"k":"wake"}` message.
 *
 * In the background it runs the Dart entry point `kuzzAndroidPushWake` in a
 * new Flutter engine (plugins come from the generated registrant, so the
 * device key and secure storage are there) and holds FCM's thread until Dart
 * calls `wakeDone` or [WAKE_BUDGET_MS] passes. On screen it does nothing:
 * the app's own connection already has the message.
 */
class KuzzPushMessagingService : FirebaseMessagingService() {
    override fun onCreate() {
        // A cold start by FCM has no Dart yet; start Firebase from the
        // options the app saved.
        KuzzFirebase.ensure(applicationContext)
        super.onCreate()
    }

    override fun onNewToken(token: String) {
        KuzzAndroidPushPluginImpl.dispatchToApp("tokenRefreshed", mapOf("token" to token))
    }

    override fun onMessageReceived(message: RemoteMessage) {
        if (message.data["k"] != "wake") return
        if (Foreground.now()) return
        runWake(applicationContext)
    }

    companion object {
        private const val TAG = "KuzzAndroidPush"
        private const val ENTRYPOINT = "kuzzAndroidPushWake"

        /** FCM allows about 10 s; Dart aims to notify by 9 s. */
        private const val WAKE_BUDGET_MS = 9_500L

        @Volatile private var latch: CountDownLatch? = null
        private val serial = Any()

        fun finishWake() {
            latch?.countDown()
        }

        private fun runWake(context: Context) = synchronized(serial) {
            val done = CountDownLatch(1)
            latch = done
            val main = Handler(Looper.getMainLooper())
            var engine: FlutterEngine? = null
            var cancelled = false
            main.post {
                try {
                    val loader = FlutterInjector.instance().flutterLoader()
                    loader.startInitialization(context)
                    loader.ensureInitializationCompleteAsync(context, null, main) {
                        if (cancelled) return@ensureInitializationCompleteAsync
                        try {
                            val started = FlutterEngine(context)
                            engine = started
                            started.dartExecutor.executeDartEntrypoint(
                                DartExecutor.DartEntrypoint(loader.findAppBundlePath(), ENTRYPOINT),
                            )
                        } catch (error: Throwable) {
                            Log.w(TAG, "wake engine failed: ${error.javaClass.simpleName}")
                            done.countDown()
                        }
                    }
                } catch (error: Throwable) {
                    Log.w(TAG, "wake engine failed: ${error.javaClass.simpleName}")
                    done.countDown()
                }
            }
            if (!done.await(WAKE_BUDGET_MS, TimeUnit.MILLISECONDS)) {
                Log.w(TAG, "wake ran out of time")
            }
            latch = null
            main.post {
                cancelled = true
                engine?.destroy()
                engine = null
            }
        }
    }
}
