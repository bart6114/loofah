import ActivityKit
import Foundation

public struct LoofahRecordingAttributes: ActivityAttributes {
  public struct ContentState: Codable, Hashable {
    public var capturedSeconds: Double
    public var timerAnchor: Date?
    public var phase: String
    public var message: String?

    public init(
      capturedSeconds: Double, timerAnchor: Date?, phase: String, message: String? = nil
    ) {
      self.capturedSeconds = capturedSeconds
      self.timerAnchor = timerAnchor
      self.phase = phase
      self.message = message
    }
  }

  public var sessionID: String
  public var title: String

  public init(sessionID: String, title: String) {
    self.sessionID = sessionID
    self.title = title
  }
}
