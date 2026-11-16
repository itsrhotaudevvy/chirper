import Foundation
import Security

enum KeychainError: Error {
    /// The Security framework status code. No item data is included.
    case status(OSStatus)
}

/// Keychain items of the app. The item is `ThisDeviceOnly`, so it is never synced or restored to
/// another device, and it is readable after the first unlock so that background work can open the
/// database.
enum Keychain {
    /// Returns the stored value of `service` and `account`, storing `value` first when there is none.
    static func storeIfAbsent(service: String, account: String, value: Data) throws -> Data {
        let item: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        if let stored = try copy(item) {
            return stored
        }
        var add = item
        add[kSecValueData as String] = value
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(add as CFDictionary, nil)
        // Another call may have stored the item in between: read that one.
        if status == errSecDuplicateItem, let stored = try copy(item) {
            return stored
        }
        guard status == errSecSuccess else {
            throw KeychainError.status(status)
        }
        return value
    }

    private static func copy(_ item: [String: Any]) throws -> Data? {
        var query = item
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess, let data = result as? Data else {
            throw KeychainError.status(status)
        }
        return data
    }
}
