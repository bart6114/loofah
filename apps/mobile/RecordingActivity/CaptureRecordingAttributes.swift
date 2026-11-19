import ActivityKit
import Foundation

struct CaptureRecordingAttributes: ActivityAttributes {
  struct ContentState: Codable, Hashable {
    let savingAt: Date?
  }

  let captureID: UUID
  let startedAt: Date
}
