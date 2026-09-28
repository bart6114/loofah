import AVFoundation
import Foundation
import XCTest

@testable import tauri_plugin_mobile_native

@_cdecl("loofah_mobile_audio")
func testAudioCallback(_ samples: UnsafePointer<Float>, _ count: Int) -> Bool { true }

@_cdecl("loofah_mobile_event")
func testEventCallback(_ json: UnsafePointer<CChar>) {}

final class AudioWindowTests: XCTestCase {
  private var temporary: [URL] = []

  override func tearDownWithError() throws {
    for url in temporary { try FileManager.default.removeItem(at: url) }
  }

  private func recording(rate: Double, channels: AVAudioChannelCount) throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent(
      "\(UUID().uuidString).wav")
    temporary.append(url)
    let format = try XCTUnwrap(
      AVAudioFormat(
        commonFormat: .pcmFormatFloat32, sampleRate: rate, channels: channels, interleaved: false))
    let file = try AVAudioFile(forWriting: url, settings: format.settings)
    let count = AVAudioFrameCount(rate * 2)
    let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: count))
    buffer.frameLength = count
    for channel in 0..<Int(channels) {
      for frame in 0..<Int(count) {
        buffer.floatChannelData![channel][frame] = frame < Int(rate) ? 0.2 : 0.6
      }
    }
    try file.write(from: buffer)
    return url
  }

  func testStereoResamplingSeeksAndBoundsWindows() throws {
    let source = try recording(rate: 48_000, channels: 2)
    let first = try AudioWindow.read(path: source.path, offset: 0, duration: 0.5)
    let second = try AudioWindow.read(path: source.path, offset: 1.25, duration: 0.5)
    XCTAssertTrue((7900...8000).contains(first.count))
    XCTAssertTrue((7900...8000).contains(second.count))
    let firstMean = first.dropFirst(100).reduce(0, +) / Float(first.count - 100)
    let secondMean = second.dropFirst(100).reduce(0, +) / Float(second.count - 100)
    XCTAssertGreaterThan(secondMean, firstMean * 2)
  }

  func testLowRateAudioUpsamplesAndRejectsOffsetsPastEnd() throws {
    let source = try recording(rate: 8000, channels: 1)
    let samples = try AudioWindow.read(path: source.path, offset: 0.5, duration: 1)
    XCTAssertTrue((15_800...16_000).contains(samples.count))
    XCTAssertTrue(samples.allSatisfy { $0.isFinite })
    XCTAssertThrowsError(try AudioWindow.read(path: source.path, offset: 5, duration: 0.5))
  }
}
