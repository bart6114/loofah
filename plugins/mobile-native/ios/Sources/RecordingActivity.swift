import ActivityKit
import Foundation
import UIKit

@MainActor
final class RecordingActivity {
  private var activity: Activity<LoofahRecordingAttributes>?
  private var pending: Task<Void, Never>?
  private var stopBackgroundTask: UIBackgroundTaskIdentifier = .invalid

  init() {
    enqueue {
      for stale in Activity<LoofahRecordingAttributes>.activities {
        await stale.end(nil, dismissalPolicy: .immediate)
      }
    }
  }

  func begin(sessionID: String, title: String, capture: RecordingCaptureState) async {
    await enqueue { [self] in
      guard activity == nil, UIApplication.shared.applicationState == .active,
        ActivityAuthorizationInfo().areActivitiesEnabled
      else { return }
      do {
        activity = try Activity.request(
          attributes: LoofahRecordingAttributes(sessionID: sessionID, title: title),
          content: ActivityContent(state: content(capture), staleDate: nil), pushType: nil)
      } catch {
        // Live Activities are optional; a failed presentation must not interrupt audio capture.
      }
    }.value
  }

  func update(_ capture: RecordingCaptureState) {
    enqueue { [self] in
      guard let activity else { return }
      await activity.update(ActivityContent(state: content(capture), staleDate: nil))
    }
  }

  func protectStop() {
    guard stopBackgroundTask == .invalid else { return }
    stopBackgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Finish recording") {
      Task { @MainActor [weak self] in self?.releaseStopProtection() }
    }
  }

  private func releaseStopProtection() {
    guard stopBackgroundTask != .invalid else { return }
    let task = stopBackgroundTask
    stopBackgroundTask = .invalid
    UIApplication.shared.endBackgroundTask(task)
  }

  func end(sessionID: String) async {
    await enqueue { [self] in
      if let activity, activity.attributes.sessionID == sessionID {
        self.activity = nil
        await activity.end(nil, dismissalPolicy: .immediate)
      }
      releaseStopProtection()
      NotificationCenter.default.post(
        name: Notification.Name("io.loofah.notes.recording-stopped"), object: nil,
        userInfo: ["sessionID": sessionID])
    }.value
  }

  private func content(_ capture: RecordingCaptureState) -> LoofahRecordingAttributes.ContentState {
    LoofahRecordingAttributes.ContentState(
      capturedSeconds: capture.capturedSeconds,
      timerAnchor: capture.phase == "recording"
        ? Date().addingTimeInterval(-capture.capturedSeconds) : nil,
      phase: capture.phase, message: capture.message)
  }

  @discardableResult
  private func enqueue(_ operation: @escaping @MainActor () async -> Void) -> Task<Void, Never> {
    let previous = pending
    let next = Task { @MainActor in
      await previous?.value
      await operation()
    }
    pending = next
    return next
  }
}
