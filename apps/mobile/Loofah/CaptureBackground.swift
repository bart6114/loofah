import BackgroundTasks
import UIKit

@MainActor
final class CaptureAppDelegate: NSObject, UIApplicationDelegate {
  private static let handoff = "io.loofah.notes.capture-handoff"

  func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    BGTaskScheduler.shared.register(forTaskWithIdentifier: Self.handoff, using: .main) { task in
      Task { @MainActor in Self.runHandoff(task) }
    }
    return true
  }

  static func scheduleHandoff() {
    guard CaptureStore.shared.hasPendingHandoff else {
      BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: handoff)
      return
    }
    let request = BGProcessingTaskRequest(identifier: handoff)
    request.requiresNetworkConnectivity = true
    request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
    try? BGTaskScheduler.shared.submit(request)
  }

  private static func runHandoff(_ task: BGTask) {
    var completed = false
    func finish(_ success: Bool) {
      guard !completed else { return }
      completed = true
      scheduleHandoff()
      task.setTaskCompleted(success: success)
    }

    task.expirationHandler = { Task { @MainActor in finish(false) } }
    CaptureStore.shared.sync { finish(true) }
  }
}
