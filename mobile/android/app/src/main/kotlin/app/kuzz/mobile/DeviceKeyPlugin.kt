package app.kuzz.mobile

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodChannel
import java.math.BigInteger
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.util.concurrent.Executors

/**
 * This install's device key (device-bound keys plan §2.1, PR D): one P-256
 * key pair in the Android Keystore, in StrongBox when the phone has one,
 * otherwise in the TEE. It is created on first use, cannot be exported, and
 * is removed with the app. Flutter only receives the public key and
 * signatures (DER, SHA256withECDSA); signing runs off the main thread
 * because StrongBox is slow.
 */
internal class DeviceKeyPlugin(
    private val context: Context,
    messenger: BinaryMessenger,
) {
    private val channel = MethodChannel(messenger, CHANNEL_NAME)
    private val worker = Executors.newSingleThreadExecutor()
    private val mainHandler = Handler(Looper.getMainLooper())

    init {
        channel.setMethodCallHandler { call, result ->
            when (call.method) {
                "getOrCreate" -> runOffMain(result) { publicKeyPayload() }
                "sign" -> {
                    val message = call.argument<ByteArray>("message")
                    if (message == null) {
                        result.error("bad_args", "message is required", null)
                    } else {
                        runOffMain(result) { sign(message) }
                    }
                }
                else -> result.notImplemented()
            }
        }
    }

    fun dispose() {
        channel.setMethodCallHandler(null)
        worker.shutdown()
    }

    /**
     * Run [work] on the worker thread and always answer [result] exactly once
     * on the main thread: any Throwable (including Errors from a broken
     * Keystore) becomes `device_key_failed`, so Dart never waits forever.
     */
    private fun runOffMain(result: MethodChannel.Result, work: () -> Any?) {
        val task = Runnable {
            val outcome: Result<Any?> = try {
                Result.success(work())
            } catch (error: Throwable) {
                Result.failure(error)
            }
            mainHandler.post { reply(result, outcome) }
        }
        try {
            worker.execute(task)
        } catch (error: Throwable) {
            // Worker already shut down (engine detached): answer anyway.
            reply(result, Result.failure(error))
        }
    }

    private fun reply(result: MethodChannel.Result, outcome: Result<Any?>) {
        try {
            outcome.fold(
                onSuccess = { result.success(it) },
                // Never include key material; the class name is enough.
                onFailure = { result.error("device_key_failed", it.javaClass.simpleName, null) },
            )
        } catch (error: Throwable) {
            // A reply after the engine is gone has nowhere to go.
        }
    }

    private fun keyStore(): KeyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }

    /** The key pair's private half, creating the pair on first use. */
    private fun privateKey(): PrivateKey {
        val store = keyStore()
        (store.getKey(ALIAS, null) as? PrivateKey)?.let { return it }
        generate(strongBox = hasStrongBox())
        return keyStore().getKey(ALIAS, null) as PrivateKey
    }

    private fun hasStrongBox(): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.P &&
            context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)

    private fun generate(strongBox: Boolean) {
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setUserAuthenticationRequired(false)
        if (strongBox && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            spec.setIsStrongBoxBacked(true)
        }
        val generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, KEYSTORE)
        try {
            generator.initialize(spec.build())
            generator.generateKeyPair()
        } catch (error: Exception) {
            // StrongBox listed but unusable for this key: fall back to the TEE.
            if (!strongBox) throw error
            generate(strongBox = false)
        }
    }

    private fun publicKeyPayload(): Map<String, Any> {
        val signingKey = privateKey()
        val publicKey = keyStore().getCertificate(ALIAS).publicKey as ECPublicKey
        return mapOf(
            "publicKey" to uncompressed(publicKey),
            "hardware" to insideSecureHardware(signingKey),
        )
    }

    private fun insideSecureHardware(key: PrivateKey): Boolean = try {
        val info = KeyFactory.getInstance(key.algorithm, KEYSTORE)
            .getKeySpec(key, KeyInfo::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            info.securityLevel == KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT ||
                info.securityLevel == KeyProperties.SECURITY_LEVEL_STRONGBOX ||
                info.securityLevel == KeyProperties.SECURITY_LEVEL_UNKNOWN_SECURE
        } else {
            @Suppress("DEPRECATION")
            info.isInsideSecureHardware
        }
    } catch (error: Exception) {
        false
    }

    private fun sign(message: ByteArray): ByteArray =
        Signature.getInstance("SHA256withECDSA").run {
            initSign(privateKey())
            update(message)
            sign()
        }

    companion object {
        const val CHANNEL_NAME = "kuzz/device_key"
        private const val KEYSTORE = "AndroidKeyStore"
        private const val ALIAS = "kuzz.device-key.v1"

        /** SEC1 uncompressed point `04 || x || y` (65 bytes). */
        internal fun uncompressed(key: ECPublicKey): ByteArray {
            val out = ByteArray(65)
            out[0] = 0x04
            fixed32(key.w.affineX).copyInto(out, 1)
            fixed32(key.w.affineY).copyInto(out, 33)
            return out
        }

        private fun fixed32(value: BigInteger): ByteArray {
            val bytes = value.toByteArray()
            return when {
                bytes.size == 32 -> bytes
                bytes.size > 32 -> bytes.copyOfRange(bytes.size - 32, bytes.size)
                else -> ByteArray(32 - bytes.size) + bytes
            }
        }
    }
}
