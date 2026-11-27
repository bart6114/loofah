import ActivityKit
import Foundation

struct CaptureRecordingAttributes: ActivityAttributes {
  struct ContentState: Codable, Hashable {
    let savingAt: Date?
    var pausedAt: Date? = nil
    var timerStartedAt: Date? = nil

    var status: String {
      if savingAt != nil { return "Saving…" }
      if pausedAt != nil { return "Paused" }
      return "Recording"
    }

    var stoppedAt: Date? { savingAt ?? pausedAt }
  }

  let captureID: UUID
  let startedAt: Date
}
