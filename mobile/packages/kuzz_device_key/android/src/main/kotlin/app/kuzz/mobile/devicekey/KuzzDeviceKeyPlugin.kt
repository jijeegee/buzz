package app.kuzz.mobile.devicekey

import io.flutter.embedding.engine.plugins.FlutterPlugin

/**
 * Registers the device key channel in every Flutter engine, through the
 * generated plugin registrant. Before this package the app's MainActivity
 * wired it, so the background push engine could not sign. The channel name
 * and Keystore alias are unchanged: existing installs keep their key.
 */
class KuzzDeviceKeyPlugin : FlutterPlugin {
    private var deviceKey: DeviceKeyPlugin? = null

    override fun onAttachedToEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        deviceKey = DeviceKeyPlugin(binding.applicationContext, binding.binaryMessenger)
    }

    override fun onDetachedFromEngine(binding: FlutterPlugin.FlutterPluginBinding) {
        deviceKey?.dispose()
        deviceKey = null
    }
}
