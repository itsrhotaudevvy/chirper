import Tauri
import UIKit
import UserNotifications
import WebKit

private struct KeychainArgs: Decodable {
    let service: String
    let account: String
    /// Standard base64 of the bytes to store.
    let value: String
}

private struct KeychainAnswer: Encodable {
    /// Standard base64 of the stored bytes.
    let value: String
}

private struct EnabledArgs: Decodable {
    let enabled: Bool
}

private struct BadgeArgs: Decodable {
    let count: Int
}

private struct FormFactorAnswer: Encodable {
    let formFactor: String
}

/// The native commands the Rust side calls. Commands answer without waiting for asynchronous work,
/// because the Rust caller blocks the main thread until the answer arrives.
class ChirperNativePlugin: Plugin {
    private let privacy = PrivacyShield()
    private var backgroundObserver: NSObjectProtocol?

    public override func load(webview: WKWebView) {
        BackgroundRefresh.register()
        backgroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main
        ) { _ in
            BackgroundRefresh.schedule()
        }
    }

    @objc public func formFactor(_ invoke: Invoke) {
        let tablet = UIDevice.current.userInterfaceIdiom == .pad
        invoke.resolve(FormFactorAnswer(formFactor: tablet ? "tablet" : "phone"))
    }

    @objc public func setSecureScreen(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(EnabledArgs.self)
            privacy.setEnabled(args.enabled)
            invoke.resolve()
        } catch {
            invoke.reject("invalid arguments")
        }
    }

    @objc public func keychainStoreIfAbsent(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(KeychainArgs.self)
            guard let value = Data(base64Encoded: args.value) else {
                invoke.reject("invalid value")
                return
            }
            let stored = try Keychain.storeIfAbsent(service: args.service, account: args.account, value: value)
            invoke.resolve(KeychainAnswer(value: stored.base64EncodedString()))
        } catch {
            invoke.reject("the keychain refused the database key: \(error)")
        }
    }

    @objc public func setBadge(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(BadgeArgs.self)
            AppBadge.set(max(0, args.count))
            invoke.resolve()
        } catch {
            invoke.reject("invalid arguments")
        }
    }
}

/// The app icon badge. It is only set when the user has allowed badges: iOS asks for notification
/// permission when the first notification is shown, so the badge follows that decision.
enum AppBadge {
    static func set(_ count: Int) {
        if #available(iOS 16.0, *) {
            let center = UNUserNotificationCenter.current()
            center.getNotificationSettings { settings in
                guard settings.badgeSetting == .enabled else {
                    return
                }
                center.setBadgeCount(count) { error in
                    if let error {
                        nativeLog.notice("the badge is not set: \(error.localizedDescription, privacy: .public)")
                    }
                }
            }
        } else {
            onMain {
                UIApplication.shared.applicationIconBadgeNumber = count
            }
        }
    }
}

@_cdecl("init_chirper_native")
public func initChirperNative() -> Plugin {
    return ChirperNativePlugin()
}
