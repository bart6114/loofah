import Foundation
import XCTest

@testable import tauri_plugin_mobile_native

final class MemoryLifetimeTests: XCTestCase {
  func testCaptureObserversDoNotRetainCapture() {
    for _ in 0..<100 {
      weak var released: NativeCapture?
      autoreleasepool {
        let capture = NativeCapture()
        released = capture
      }
      XCTAssertNil(released, "Notification observers retained the capture owner")
    }
  }

  @MainActor
  func testCompletedActivityOperationsReleaseTheirOwner() async {
    for _ in 0..<100 {
      var activity: RecordingActivity? = RecordingActivity()
      weak var released = activity
      activity?.update(
        RecordingCaptureState(capturedSeconds: 1, phase: "interrupted", message: nil))
      await activity?.end(sessionID: "memory-lifetime-test")
      activity = nil
      XCTAssertNil(released, "Completed activity task retained its owner or earlier tasks")
    }
  }

  @MainActor
  func testPluginReleasesAfterQueuedInitialization() async {
    weak var released: MobileNativePlugin?
    autoreleasepool {
      let plugin = MobileNativePlugin()
      released = plugin
    }
    await withCheckedContinuation { continuation in
      DispatchQueue.main.async { continuation.resume() }
    }
    XCTAssertNil(released, "Plugin initialization or capture callback retained the plugin")
  }
}
