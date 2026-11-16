import UIKit

/// The secure-screen protection of iOS. There is no `FLAG_SECURE` on iOS, so the window is covered
/// in two cases while the setting is on:
///
/// - the app is inactive or in the background: iOS takes the app-switcher snapshot of the window
///   when it resigns active, so the blur is in place for the snapshot;
/// - the screen is being captured (recording or mirroring): the cover is opaque.
///
/// The cover is removed when the app is active again and the screen is not captured.
final class PrivacyShield {
    private static let coverIdentifier = "app.chirper.privacy-cover"

    private var enabled = false
    private var inactive = false
    private var observers: [NSObjectProtocol] = []

    init() {
        let center = NotificationCenter.default
        let names: [Notification.Name] = [
            UIApplication.willResignActiveNotification,
            UIApplication.didBecomeActiveNotification,
            UIScreen.capturedDidChangeNotification,
        ]
        observers = names.map { name in
            center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                self?.onStateChange(name)
            }
        }
    }

    deinit {
        observers.forEach { NotificationCenter.default.removeObserver($0) }
    }

    /// Turns the protection on or off. The main thread is required, so the call hops there.
    func setEnabled(_ on: Bool) {
        onMain {
            self.enabled = on
            self.refresh()
        }
    }

    private func onStateChange(_ name: Notification.Name) {
        if name == UIApplication.willResignActiveNotification {
            inactive = true
        } else if name == UIApplication.didBecomeActiveNotification {
            inactive = false
        }
        refresh()
    }

    private func refresh() {
        guard let window = keyWindow() else {
            return
        }
        window.subviews
            .filter { $0.accessibilityIdentifier == PrivacyShield.coverIdentifier }
            .forEach { $0.removeFromSuperview() }
        let captured = UIScreen.main.isCaptured
        guard enabled, captured || inactive else {
            return
        }
        let cover = captured ? opaqueCover() : blurCover()
        cover.accessibilityIdentifier = PrivacyShield.coverIdentifier
        cover.frame = window.bounds
        cover.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        window.addSubview(cover)
    }

    private func blurCover() -> UIView {
        UIVisualEffectView(effect: UIBlurEffect(style: .systemThickMaterial))
    }

    private func opaqueCover() -> UIView {
        let view = UIView()
        view.backgroundColor = .systemBackground
        let label = UILabel()
        label.text = "Hidden while the screen is recorded"
        label.textAlignment = .center
        label.numberOfLines = 0
        label.textColor = .secondaryLabel
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            label.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 24),
            label.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -24),
        ])
        return view
    }

    private func keyWindow() -> UIWindow? {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap { $0.windows }
            .first { $0.isKeyWindow }
    }
}

/// Runs `work` on the main thread: inline when already there, so a caller that blocks the main
/// thread for the answer does not deadlock.
func onMain(_ work: @escaping () -> Void) {
    if Thread.isMainThread {
        work()
    } else {
        DispatchQueue.main.async(execute: work)
    }
}
