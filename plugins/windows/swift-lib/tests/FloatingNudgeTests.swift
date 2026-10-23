import AppKit
import XCTest

@testable import swift_lib

final class FloatingNudgeTests: XCTestCase {
  func testExpiryPausesAndResumesFromRemainingTime() {
    var clock: TimeInterval = 0
    let model = FloatingNudgeModel(now: { clock })
    model.show(.init(id: "one", recordingId: "recording", text: "Ask about timing"))
    clock = 8
    model.setHovering(true)
    clock = 100
    model.expire(id: "one")
    XCTAssertNotNil(model.payload)
    model.setHovering(false)
    clock = 111
    model.expire(id: "one")
    XCTAssertNotNil(model.payload)
    clock = 112
    model.expire(id: "one")
    XCTAssertNil(model.payload)
  }

  func testReplacementIgnoresOldTimerAndOtherRecordingClear() {
    var clock: TimeInterval = 0
    let model = FloatingNudgeModel(now: { clock })
    model.show(.init(id: "one", recordingId: "old", text: "First"))
    clock = 15
    model.show(.init(id: "two", recordingId: "new", text: "Second"))
    clock = 25
    model.expire(id: "one")
    model.clear(recordingId: "old")
    XCTAssertEqual(model.payload?.id, "two")
    model.expire(id: "two")
    XCTAssertNotNil(model.payload)
    clock = 35
    model.expire(id: "two")
    XCTAssertNil(model.payload)
  }

  func testReplacementWhileHoveredGetsFullLifetimeAfterLeaving() {
    var clock: TimeInterval = 0
    let model = FloatingNudgeModel(now: { clock })
    model.show(.init(id: "one", recordingId: "r", text: "First"))
    model.setHovering(true)
    model.show(.init(id: "two", recordingId: "r", text: "Second"))
    clock = 100
    model.setHovering(false)
    clock = 119
    model.expire(id: "two")
    XCTAssertNotNil(model.payload)
    clock = 120
    model.expire(id: "two")
    XCTAssertNil(model.payload)
  }

  func testLongTextGetsEnoughVerticalSpace() {
    let short = FloatingBarLayout.nudgeHeight(text: "Who owns this?")
    let long = FloatingBarLayout.nudgeHeight(text: String(repeating: "問", count: 200))
    XCTAssertGreaterThan(long, short)
    XCTAssertGreaterThan(long, 140)
    XCTAssertLessThan(long, 400)
  }
}

final class FloatingNudgePanelTests: XCTestCase {
  func testNativePanelPreservesFocusAndAnchorAcrossNudgesAndRecordingUpdates() throws {
    _ = NSApplication.shared
    let frontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier
    let manager = FloatingBarManager.shared
    manager.show()
    pump()
    defer {
      manager.hide()
      pump()
    }
    let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? NSPanel }.first { $0.isVisible })
    let original = panel.frame
    let payload = FloatingNudgePayload(
      id: "visible", recordingId: "recording",
      text: "Ask who owns the rollout and when it needs to ship.")
    XCTAssertTrue(manager.showNudge(payload))
    pump()
    XCTAssertEqual(panel.frame.maxX, original.maxX, accuracy: 1)
    XCTAssertEqual(panel.frame.maxY, original.maxY, accuracy: 1)
    XCTAssertGreaterThan(panel.frame.height, original.height)
    let nudgeFrame = panel.frame
    for scheme in [FloatingBarColorScheme.light, .dark] {
      manager.update(
        state: FloatingBarStatePayload(
          amplitude: 0.5, title: "Planning", status: .recording, colorScheme: scheme,
          opacity: 0.78, liveCaptionOpacity: 0.8, liveCaptionWidth: 500,
          liveCaptionLineCount: 3, liveCaptionPosition: .bottomCenter,
          liveCaptionMinimized: false, liveCaptionToggleVisible: false, transcriptBubbles: []))
      pump()
      XCTAssertEqual(panel.frame, nudgeFrame, "Ordinary recording updates must preserve the nudge")
      try render(panel: panel, name: "nudge-\(scheme.rawValue)")
    }
    XCTAssertEqual(NSWorkspace.shared.frontmostApplication?.processIdentifier, frontmost)
    manager.clearNudge(recordingId: "another-recording")
    pump()
    XCTAssertEqual(panel.frame, nudgeFrame)
    manager.clearNudge(recordingId: "recording")
    pump()
    XCTAssertEqual(panel.frame.size, original.size)
    XCTAssertEqual(panel.frame.maxY, original.maxY, accuracy: 1)
    manager.hide()
    pump()
    XCTAssertFalse(manager.showNudge(payload))
  }

  private func pump() {
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
  }

  private func render(panel: NSPanel, name: String) throws {
    guard let directory = ProcessInfo.processInfo.environment["LOOFAH_NUDGE_QA_DIR"],
      let view = panel.contentView,
      let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)
    else { return }
    view.cacheDisplay(in: view.bounds, to: bitmap)
    let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
    try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(name).png"))
  }
}
