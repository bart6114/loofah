import AppIntents
import Foundation

struct StopRecordingIntent: LiveActivityIntent {
  static var title: LocalizedStringResource = "Stop recording"
  static var description = IntentDescription("Stop and save the current Loofah recording.")
  static var openAppWhenRun = false

  @Parameter(title: "Session")
  var sessionID: String

  init() {}

  init(sessionID: String) {
    self.sessionID = sessionID
  }

  func perform() async throws -> some IntentResult {
    try await withCheckedThrowingContinuation { continuation in
      RecordingStopWaiter(continuation: continuation).request(sessionID: sessionID)
    }
    return .result()
  }
}

private final class RecordingStopWaiter: @unchecked Sendable {
  private let lock = NSLock()
  private var continuation: CheckedContinuation<Void, Error>?
  private var observer: NSObjectProtocol?
  private var timeout: DispatchWorkItem?

  init(continuation: CheckedContinuation<Void, Error>) {
    self.continuation = continuation
  }

  func request(sessionID: String) {
    let timeout = DispatchWorkItem { [self] in
      finish(error: StopRecordingTimeout())
    }
    lock.lock()
    self.timeout = timeout
    observer = NotificationCenter.default.addObserver(
      forName: Notification.Name("io.loofah.mobile.recording-stopped"),
      object: nil,
      queue: nil
    ) { [self] notification in
      guard notification.userInfo?["sessionID"] as? String == sessionID else { return }
      finish()
    }
    lock.unlock()

    DispatchQueue.global().asyncAfter(deadline: .now() + 15, execute: timeout)
    NotificationCenter.default.post(
      name: Notification.Name("io.loofah.mobile.stop-recording"),
      object: nil,
      userInfo: ["sessionID": sessionID]
    )
  }

  private func finish(error: Error? = nil) {
    lock.lock()
    let continuation = self.continuation
    let observer = self.observer
    let timeout = self.timeout
    self.continuation = nil
    self.observer = nil
    self.timeout = nil
    lock.unlock()

    if let observer { NotificationCenter.default.removeObserver(observer) }
    timeout?.cancel()
    if let error {
      continuation?.resume(throwing: error)
    } else {
      continuation?.resume()
    }
  }
}

private struct StopRecordingTimeout: LocalizedError {
  var errorDescription: String? {
    "The recording is taking longer to stop. Open Loofah to check its status."
  }
}
