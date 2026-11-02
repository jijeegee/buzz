package app.kuzz.mobile.push

import io.flutter.embedding.engine.plugins.FlutterPlugin
import io.flutter.embedding.engine.plugins.activity.ActivityAware
import io.flutter.embedding.engine.plugins.activity.ActivityPluginBinding

/**
 * The registered plugin class. It holds no Firebase code itself: the
 * implementation comes from src/on (KUZZ_ANDROID_PUSH=true) or the empty
 * one in src/off (see build.gradle).
 */
class KuzzAndroidPushPlugin : FlutterPlugin, ActivityAware {
    private val impl = KuzzAndroidPushPluginImpl()

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) =
        impl.onAttachedToEngine(binding)

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) =
        impl.onDetachedFromEngine(binding)

    override fun onAttachedToActivity(binding: ActivityPluginBinding) =
        impl.onAttachedToActivity(binding)

    override fun onDetachedFromActivityForConfigChanges() =
        impl.onDetachedFromActivityForConfigChanges()

    override fun onReattachedToActivityForConfigChanges(binding: ActivityPluginBinding) =
        impl.onReattachedToActivityForConfigChanges(binding)

    override fun onDetachedFromActivity() = impl.onDetachedFromActivity()
}
