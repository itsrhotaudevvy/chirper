import BackgroundTasks
import os
import UIKit

let nativeLog = Logger(subsystem: "app.chirper", category: "native")

/// A short background refresh. iOS decides when, or whether, a refresh runs, and it gives no
/// guarantee about the time. There is no push: Chirper has no server, so the accounts are only
/// current while the app runs. A refresh keeps the app running for about 25 seconds so that the core
/// can finish the sync it has started, and then ends the task. The earliest start is 15 minutes
/// after the app goes to the background.
enum BackgroundRefresh {
    static let identifier = "app.chirper.refresh"
    private static let earliestDelay: TimeInterval = 15 * 60
    private static let windowSeconds: Double = 25

    /// Registers the launch handler. It must run before the app finishes launching, so the plugin
    /// calls it from `load`.
    static func register() {
        let registered = BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: nil) { task in
            guard let refresh = task as? BGAppRefreshTask else {
                task.setTaskCompleted(success: false)
                return
            }
            schedule()
            let window = RefreshWindow(task: refresh)
            refresh.expirationHandler = { window.finish(success: false) }
            DispatchQueue.main.asyncAfter(deadline: .now() + windowSeconds) {
                window.finish(success: true)
            }
        }
        if !registered {
            nativeLog.error("the background refresh handler is not registered")
        }
    }

    /// Asks iOS for the next refresh. Simulators and denied apps refuse; that is only logged.
    static func schedule() {
        let request = BGAppRefreshTaskRequest(identifier: identifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: earliestDelay)
        do {
            try BGTaskScheduler.shared.submit(request)
        } catch {
            nativeLog.notice("the background refresh is not scheduled: \(error.localizedDescription, privacy: .public)")
        }
    }
}

/// Ends a refresh task exactly once, on the main queue.
private final class RefreshWindow {
    private let task: BGAppRefreshTask
    private var finished = false

    init(task: BGAppRefreshTask) {
        self.task = task
    }

    func finish(success: Bool) {
        DispatchQueue.main.async {
            guard !self.finished else {
                return
            }
            self.finished = true
            self.task.setTaskCompleted(success: success)
        }
    }
}
