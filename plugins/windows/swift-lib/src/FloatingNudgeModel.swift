import Combine
import Foundation

struct FloatingNudgePayload: Codable {
  let id: String
  let recordingId: String
  let text: String
}

final class FloatingNudgeModel: ObservableObject {
  @Published private(set) var payload: FloatingNudgePayload?
  private var timer: Timer?
  private var remaining: TimeInterval = 20
  private var resumedAt: TimeInterval?
  private var hovering = false
  private let now: () -> TimeInterval

  init(now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
    self.now = now
  }

  deinit { timer?.invalidate() }

  func show(_ payload: FloatingNudgePayload) {
    timer?.invalidate()
    self.payload = payload
    remaining = 20
    resumedAt = nil
    if !hovering { resume() }
  }

  func clear(recordingId: String? = nil) {
    if let recordingId, payload?.recordingId != recordingId { return }
    timer?.invalidate()
    timer = nil
    resumedAt = nil
    hovering = false
    payload = nil
  }

  func setHovering(_ hovering: Bool) {
    guard self.hovering != hovering else { return }
    self.hovering = hovering
    if hovering {
      if let resumedAt { remaining = max(0, remaining - (now() - resumedAt)) }
      resumedAt = nil
      timer?.invalidate()
      timer = nil
    } else if payload != nil {
      resume()
    }
  }

  func expire(id: String) {
    guard payload?.id == id, !hovering, let resumedAt,
      now() - resumedAt >= remaining
    else { return }
    clear()
  }

  private func resume() {
    guard let payload else { return }
    resumedAt = now()
    let id = payload.id
    let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
      self?.expire(id: id)
    }
    RunLoop.main.add(timer, forMode: .common)
    self.timer = timer
  }
}
