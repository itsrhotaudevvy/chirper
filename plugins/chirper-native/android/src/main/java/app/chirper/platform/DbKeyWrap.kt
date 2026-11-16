package app.chirper.platform

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import java.security.ProviderException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Wraps the database key with an AES-256-GCM key that lives in the Android Keystore and cannot be
 * exported. The key needs an unlocked device (API 28+) and no user authentication, so the app can
 * open the database for background work. StrongBox is used when the device has it.
 */
internal object DbKeyWrap {
    private const val ALIAS = "chirper-db-wrap"
    private const val PROVIDER = "AndroidKeyStore"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val TAG_BITS = 128

    private fun key(): SecretKey {
        val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        return try {
            generate(strongBox = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P)
        } catch (e: ProviderException) {
            // StrongBoxUnavailableException extends ProviderException: fall back to the TEE.
            generate(strongBox = false)
        }
    }

    private fun generate(strongBox: Boolean): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        val spec = KeyGenParameterSpec.Builder(
            ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setRandomizedEncryptionRequired(true)
            .setUserAuthenticationRequired(false)
            .apply {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                    setIsStrongBoxBacked(strongBox)
                }
            }
            .build()
        generator.init(spec)
        return generator.generateKey()
    }

    /** Returns the IV and the ciphertext (with its tag) of [plaintext]. */
    fun wrap(plaintext: ByteArray): Pair<ByteArray, ByteArray> {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val ciphertext = cipher.doFinal(plaintext)
        return Pair(cipher.iv, ciphertext)
    }

    fun unwrap(iv: ByteArray, ciphertext: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
        return cipher.doFinal(ciphertext)
    }

    fun deleteKey() {
        try {
            val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
            store.deleteEntry(ALIAS)
        } catch (_: Exception) {}
    }
}
