import Foundation
import ObjectiveC
import XCTest

@testable import tauri_plugin_mobile_native

final class PluginCommandTests: XCTestCase {
  func testAllNativeCommandsHaveObjectiveCEntryPoints() {
    let commands = [
      "startRecording", "stopRecording", "beginRecordingActivity", "endRecordingActivity",
      "modelStatus", "downloadModel", "deleteModel",
      "transcribeWindow", "previewFile", "protectRecordingFile", "audioDuration",
      "importAudio", "selectICloudVault", "restoreICloudVault",
      "disconnectICloudVault", "downloadICloudItem", "setApiKey", "readApiKey",
    ]
    for command in commands {
      let selectors = [command + ":", command + ":error:", command + ":completionHandler:"]
      XCTAssertTrue(
        selectors.contains {
          class_getInstanceMethod(MobileNativePlugin.self, NSSelectorFromString($0)) != nil
        },
        "Missing native command: \(command)")
    }
  }
}
