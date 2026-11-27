import AppIntents
import Foundation

struct StopCaptureRecording: LiveActivityIntent {
  static let title: LocalizedStringResource = "Stop recording"
  static let description = IntentDescription("Stop and save a Loofah recording.")
  static let supportedModes: IntentModes = .background
  static let isDiscoverable = false

  @Parameter(title: "Capture")
  var captureID: String

  init() {}

  init(captureID: UUID) {
    self.captureID = captureID.uuidString
  }

  @MainActor
  func perform() async throws -> some IntentResult {
    // LiveActivityIntent executes in the app process, where the recorder lives.
    #if !LOOFAH_WIDGET
      guard let id = UUID(uuidString: captureID),
        CaptureStore.shared.recording?.id == id
      else { return .result() }
      if !(await CaptureStore.shared.finish(id)) {
        throw SaveCaptureError()
      }
    #endif
    return .result()
  }
}

#if !LOOFAH_WIDGET
  private struct SaveCaptureError: LocalizedError {
    var errorDescription: String? { "Open Loofah to finish saving the recording." }
  }
#endif
