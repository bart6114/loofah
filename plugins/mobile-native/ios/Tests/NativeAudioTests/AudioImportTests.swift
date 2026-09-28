import AVFoundation
import Foundation
import UIKit
import UniformTypeIdentifiers
import XCTest

@testable import tauri_plugin_mobile_native

final class AudioImportTests: XCTestCase {
  func testImportsMono16kPCM() throws { try importRecording(rate: 16_000, channels: 1) }
  func testImportsStereo44kPCM() throws { try importRecording(rate: 44_100, channels: 2) }

  @MainActor
  func testCancellationReleasesCompletionAndOnlyCompletesOnce() {
    var probe: NSObject? = NSObject()
    weak var retainedProbe = probe
    var completions = 0
    let picker = AudioImportPicker(
      destination: FileManager.default.temporaryDirectory.appendingPathComponent("unused.wav")
    ) { [captured = probe!] _ in
      withExtendedLifetime(captured) { completions += 1 }
    }
    probe = nil
    XCTAssertNotNil(retainedProbe)

    let controller = UIDocumentPickerViewController(forOpeningContentTypes: [.audio])
    picker.documentPickerWasCancelled(controller)
    withExtendedLifetime(picker) {
      XCTAssertNil(retainedProbe)
      picker.documentPickerWasCancelled(controller)
      picker.documentPicker(controller, didPickDocumentsAt: [])
      XCTAssertEqual(completions, 1)
    }
  }

  private func importRecording(rate: Double, channels: AVAudioChannelCount) throws {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let source = directory.appendingPathComponent("Team catch-up.2026.wav")
    let destination = directory.appendingPathComponent("import.wav")
    let format = try XCTUnwrap(
      AVAudioFormat(
        commonFormat: .pcmFormatFloat32, sampleRate: rate, channels: channels,
        interleaved: false))
    let settings: [String: Any] = [
      AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: rate,
      AVNumberOfChannelsKey: channels, AVLinearPCMBitDepthKey: 16,
      AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false,
    ]
    let count = AVAudioFrameCount(rate * 2)
    let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: count))
    buffer.frameLength = count
    for channel in 0..<Int(channels) {
      for frame in 0..<Int(count) { buffer.floatChannelData![channel][frame] = 0.25 }
    }
    do {
      let file = try AVAudioFile(
        forWriting: source, settings: settings,
        commonFormat: .pcmFormatFloat32, interleaved: false)
      try file.write(from: buffer)
    }
    let imported = try AudioImportPicker.decode(source, to: destination)
    XCTAssertEqual(imported.path, destination.path)
    XCTAssertEqual(imported.title, "Team catch-up.2026")
    XCTAssertEqual(imported.durationSeconds, 2, accuracy: 0.02)
    let output = try AVAudioFile(forReading: destination)
    XCTAssertEqual(output.fileFormat.sampleRate, 16_000)
    XCTAssertEqual(output.fileFormat.channelCount, 1)
    XCTAssertGreaterThan(output.length, 31_000)
  }
}
