package app.kuzz.mobile

import android.app.Activity
import android.os.CancellationSignal
import androidx.core.content.ContextCompat
import androidx.credentials.CredentialManager
import androidx.credentials.CredentialManagerCallback
import androidx.credentials.CustomCredential
import androidx.credentials.GetCredentialRequest
import androidx.credentials.GetCredentialResponse
import androidx.credentials.exceptions.GetCredentialCancellationException
import androidx.credentials.exceptions.GetCredentialException
import androidx.credentials.exceptions.NoCredentialException
import com.google.android.libraries.identity.googleid.GetSignInWithGoogleOption
import com.google.android.libraries.identity.googleid.GoogleIdTokenCredential
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodChannel

/**
 * `kuzz/google_account`: sign in with a Google account already on the phone
 * through Credential Manager, without a browser. `idToken {clientId, nonce}`
 * answers the id_token; `CANCELED` when the user closes the picker, any
 * other error code when the phone cannot show it (Dart then opens the
 * browser sign-in).
 */
internal object GoogleAccountSignIn {
    fun register(activity: Activity, messenger: BinaryMessenger) {
        MethodChannel(messenger, "kuzz/google_account").setMethodCallHandler { call, result ->
            if (call.method != "idToken") {
                result.notImplemented()
                return@setMethodCallHandler
            }
            val clientId = call.argument<String>("clientId")
            val nonce = call.argument<String>("nonce")
            if (clientId.isNullOrEmpty() || nonce.isNullOrEmpty()) {
                result.error("bad_arguments", "clientId and nonce are required.", null)
                return@setMethodCallHandler
            }
            requestIdToken(activity, clientId, nonce, result)
        }
    }

    private fun requestIdToken(
        activity: Activity,
        clientId: String,
        nonce: String,
        result: MethodChannel.Result,
    ) {
        // The "Sign in with Google" sheet lists every Google account on the
        // phone and lets the user add one; it never picks one silently.
        val option = GetSignInWithGoogleOption.Builder(clientId)
            .setNonce(nonce)
            .build()
        val request = GetCredentialRequest.Builder()
            .addCredentialOption(option)
            .build()
        try {
            CredentialManager.create(activity).getCredentialAsync(
                activity,
                request,
                CancellationSignal(),
                ContextCompat.getMainExecutor(activity),
                object : CredentialManagerCallback<GetCredentialResponse, GetCredentialException> {
                    override fun onResult(response: GetCredentialResponse) {
                        val credential = response.credential
                        if (credential is CustomCredential &&
                            credential.type == GoogleIdTokenCredential.TYPE_GOOGLE_ID_TOKEN_CREDENTIAL
                        ) {
                            try {
                                result.success(GoogleIdTokenCredential.createFrom(credential.data).idToken)
                            } catch (error: Exception) {
                                result.error("bad_credential", "Unreadable Google credential.", null)
                            }
                        } else {
                            result.error("bad_credential", "Unexpected credential type.", null)
                        }
                    }

                    override fun onError(error: GetCredentialException) {
                        when (error) {
                            is GetCredentialCancellationException ->
                                result.error("CANCELED", "Sign-in closed.", null)
                            is NoCredentialException ->
                                result.error("no_account", "No Google account available.", null)
                            else ->
                                result.error("unavailable", error.type, null)
                        }
                    }
                },
            )
        } catch (error: Exception) {
            result.error("unavailable", error.javaClass.simpleName, null)
        }
    }
}
