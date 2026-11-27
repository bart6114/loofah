import AVFoundation
import Foundation

struct RecordingSession {
  let id: UUID
  private(set) var timerStartedAt: Date
  private(set) var pausedAt: Date?
  private(set) var observationID = UUID()
  private var awaitingInterruptionEnd = false

  init(id: UUID, startedAt: Date) {
    self.id = id
    self.timerStartedAt = startedAt
  }

  mutating func pause(at date: Date, awaitingEnd: Bool) -> Bool {
    if pausedAt != nil {
      if !awaitingEnd { awaitingInterruptionEnd = false }
      return false
    }
    pausedAt = max(date, timerStartedAt)
    awaitingInterruptionEnd = awaitingEnd
    return true
  }

  mutating func interruptionEnded(allowsResume: Bool) -> Bool {
    let shouldResume = awaitingInterruptionEnd && allowsResume
    awaitingInterruptionEnd = false
    return shouldResume
  }

  mutating func renewObservation() {
    observationID = UUID()
    awaitingInterruptionEnd = false
  }

  mutating func resumed(at date: Date) {
    guard let pausedAt else { return }
    timerStartedAt += max(0, date.timeIntervalSince(pausedAt))
    self.pausedAt = nil
  }
}

enum RecordingSessionEvent {
  case interruptionBegan(reason: UInt?, wasSuspended: Bool)
  case interruptionEnded(options: UInt)
  case routeChanged(reason: UInt, previousInputs: [String], previousPorts: [String])
  case mediaServicesChanged(reset: Bool)

  static let notifications: [Notification.Name] = [
    AVAudioSession.interruptionNotification, AVAudioSession.routeChangeNotification,
    AVAudioSession.mediaServicesWereLostNotification,
    AVAudioSession.mediaServicesWereResetNotification,
  ]

  init?(_ notification: Notification) {
    let info = notification.userInfo
    switch notification.name {
    case AVAudioSession.interruptionNotification:
      switch info?[AVAudioSessionInterruptionTypeKey] as? UInt {
      case AVAudioSession.InterruptionType.began.rawValue:
        self = .interruptionBegan(
          reason: info?[AVAudioSessionInterruptionReasonKey] as? UInt,
          wasSuspended: info?[AVAudioSessionInterruptionWasSuspendedKey] as? Bool == true)
      case AVAudioSession.InterruptionType.ended.rawValue:
        self = .interruptionEnded(options: info?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0)
      default:
        return nil
      }
    case AVAudioSession.routeChangeNotification:
      guard let reason = info?[AVAudioSessionRouteChangeReasonKey] as? UInt else { return nil }
      let previous =
        info?[AVAudioSessionRouteChangePreviousRouteKey] as? AVAudioSessionRouteDescription
      self = .routeChanged(
        reason: reason, previousInputs: previous?.inputs.map(\.uid) ?? [],
        previousPorts: previous?.inputs.map { $0.portType.rawValue } ?? [])
    case AVAudioSession.mediaServicesWereLostNotification:
      self = .mediaServicesChanged(reset: false)
    case AVAudioSession.mediaServicesWereResetNotification:
      self = .mediaServicesChanged(reset: true)
    default:
      return nil
    }
  }
}
