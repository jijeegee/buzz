package app.kuzz.mobile.push

import android.os.Build
import android.os.Handler
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.ByteArrayOutputStream
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec

/**
 * Keystore keys for push gateway enrollment (NIP-PL `kuzz-android`): one
 * attested EC P-256 key per enrollment. These are separate from the
 * app's device key, which is never touched here.
 */
internal object KuzzEnrollmentKeys {
    private const val KEYSTORE = "AndroidKeyStore"
    private const val PREFIX = "kuzz.push.enroll."
    private val main = Handler(Looper.getMainLooper())

    fun handle(call: MethodCall, result: MethodChannel.Result): Boolean {
        val alias = call.argument<String>("alias")
        when (call.method) {
            "enrollmentKeyCreate" -> {
                val challenge = call.argument<ByteArray>("challenge")
                if (!validAlias(alias) || challenge == null) {
                    result.error("bad_args", "alias and challenge are required", null)
                } else {
                    background(result) { create(alias!!, challenge) }
                }
            }
            "enrollmentKeySign" -> {
                val data = call.argument<ByteArray>("data")
                if (!validAlias(alias) || data == null) {
                    result.error("bad_args", "alias and data are required", null)
                } else {
                    background(result) { sign(alias!!, data) }
                }
            }
            "enrollmentKeyDelete" -> {
                if (!validAlias(alias)) {
                    result.error("bad_args", "alias is required", null)
                } else {
                    background(result) { keyStore().deleteEntry(alias); null }
                }
            }
            else -> return false
        }
        return true
    }

    private fun validAlias(alias: String?) =
        alias != null && alias.startsWith(PREFIX) && alias.length <= 64

    private fun keyStore() = KeyStore.getInstance(KEYSTORE).apply { load(null) }

    private fun create(alias: String, challenge: ByteArray): Map<String, Any> {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
            throw IllegalStateException("Key attestation needs Android 7")
        }
        val generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, KEYSTORE)
        generator.initialize(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
                .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                .setDigests(KeyProperties.DIGEST_SHA256)
                .setAttestationChallenge(challenge)
                .build(),
        )
        val pair = generator.generateKeyPair()
        val chain = keyStore().getCertificateChain(alias)
            ?: throw IllegalStateException("Keystore returned no attestation chain")
        val der = ByteArrayOutputStream()
        for (certificate in chain) der.write(certificate.encoded)
        return mapOf(
            "chain" to Base64.encodeToString(der.toByteArray(), Base64.NO_WRAP),
            "publicKey" to pair.public.encoded,
        )
    }

    private fun sign(alias: String, data: ByteArray): ByteArray {
        val key = keyStore().getKey(alias, null) as? PrivateKey
            ?: throw IllegalStateException("No enrollment key")
        return Signature.getInstance("SHA256withECDSA").run {
            initSign(key)
            update(data)
            sign()
        }
    }

    private fun background(result: MethodChannel.Result, work: () -> Any?) {
        Thread {
            try {
                val value = work()
                main.post { result.success(value) }
            } catch (error: Exception) {
                main.post { result.error("keystore", error.javaClass.simpleName, null) }
            }
        }.start()
    }
}
