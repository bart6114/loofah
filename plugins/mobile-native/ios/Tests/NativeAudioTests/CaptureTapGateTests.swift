import Foundation
import XCTest

@testable import tauri_plugin_mobile_native

final class CaptureTapGateTests: XCTestCase {
  func testCapturedFramesFreezeAtStopAndExcludeFailedConversion() {
    let gate = CaptureTapGate()
    gate.runCapturing { 16_000 }
    gate.runCapturing { 0 }
    gate.runCapturing { 8_000 }
    XCTAssertEqual(gate.frameCount(), 24_000)
    XCTAssertEqual(gate.close(), 24_000)
    gate.runCapturing {
      XCTFail("Stopped capture advanced its timer")
      return 16_000
    }
    XCTAssertEqual(gate.frameCount(), 24_000)
  }

  func testCaptureFailureClosesGateAndPublishesOnlyOnce() {
    let gate = CaptureTapGate()
    var failures = 0
    gate.runCapturing { 8_000 }
    gate.runCapturing(onFailure: { failures += 1 }) { nil }
    gate.runCapturing(onFailure: { failures += 1 }) {
      XCTFail("Failed capture accepted another callback")
      return nil
    }
    XCTAssertEqual(failures, 1)
    XCTAssertEqual(gate.close(), 8_000)
  }

  func testStopDrainsCurrentCopyAndRejectsLaterCallbacks() {
    let gate = CaptureTapGate()
    let entered = DispatchSemaphore(value: 0)
    let release = DispatchSemaphore(value: 0)
    let closed = DispatchSemaphore(value: 0)
    DispatchQueue(label: "test.capture.copy").async {
      gate.run {
        entered.signal()
        release.wait()
      }
    }
    XCTAssertEqual(entered.wait(timeout: .now() + 1), .success)
    DispatchQueue(label: "test.capture.stop").async {
      gate.close()
      closed.signal()
    }
    XCTAssertEqual(closed.wait(timeout: .now() + 0.05), .timedOut)
    release.signal()
    XCTAssertEqual(closed.wait(timeout: .now() + 1), .success)
    gate.run { XCTFail("Stopped audio reached the PCM callback") }
  }

  func testOverlappingCallbackNeverWaitsForCurrentCopy() {
    let gate = CaptureTapGate()
    let entered = DispatchSemaphore(value: 0)
    let release = DispatchSemaphore(value: 0)
    let returned = DispatchSemaphore(value: 0)
    DispatchQueue(label: "test.capture.copy").async {
      gate.run {
        entered.signal()
        release.wait()
      }
    }
    XCTAssertEqual(entered.wait(timeout: .now() + 1), .success)
    DispatchQueue(label: "test.capture.overlap").async {
      gate.run { XCTFail("Concurrent callback entered the shared converter") }
      returned.signal()
    }
    let result = returned.wait(timeout: .now() + 1)
    release.signal()
    XCTAssertEqual(result, .success)
    gate.close()
  }
}
