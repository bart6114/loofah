import Foundation
import XCTest

@testable import Loofah

final class CapturePackageTests: XCTestCase {
  func testInterruptedWAVRecoversSamplesAndCanBeRetried() throws {
    let audio = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: audio) }
    var header = Data("RIFF".utf8) + Data([36, 0, 0, 0]) + Data("WAVEfmt ".utf8)
    header += Data([16, 0, 0, 0, 1, 0, 1, 0, 192, 93, 0, 0, 128, 187, 0, 0, 2, 0, 16, 0])
    header += Data("data".utf8) + Data([0, 0, 0, 0])
    let samples = Data([0x01, 0x00, 0xff, 0xff])

    XCTAssertFalse(try RecordingWAV.finalize(at: audio))
    try header.write(to: audio)
    XCTAssertFalse(try RecordingWAV.finalize(at: audio))

    try (header + samples + Data([0x7f])).write(to: audio)
    XCTAssertTrue(try RecordingWAV.finalize(at: audio))
    var expected = header + samples
    expected[4] = 40
    expected[40] = 4
    XCTAssertEqual(try Data(contentsOf: audio), expected)
    XCTAssertTrue(try RecordingWAV.finalize(at: audio))
    XCTAssertEqual(try Data(contentsOf: audio), expected)
  }

  func testPackageRoundTripAndRejectsIncompleteAudio() throws {
    let files = FileManager.default
    let root = files.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
    try files.createDirectory(at: root, withIntermediateDirectories: false)
    defer { try? files.removeItem(at: root) }
    let staging = root.appendingPathComponent("staging", isDirectory: true)
    try files.createDirectory(at: staging, withIntermediateDirectories: false)
    let now = Date()
    let capture = Capture(
      id: UUID(), createdAt: now, finishedAt: now.addingTimeInterval(4),
      note: "Studio review\nCheck the mise en page.", recording: true
    )
    let audio = Data(repeating: 7, count: 320_040)
    try audio.write(to: root.appendingPathComponent(CapturePackage.audio))
    let receipt = try CapturePackage.seal(capture, in: root)
    let payload = root.appendingPathComponent(CapturePackage.payload)
    let imported = try CapturePackage.unpack(payload, into: staging, id: capture.id)
    XCTAssertEqual(imported, receipt)
    XCTAssertEqual(
      try Data(contentsOf: staging.appendingPathComponent(CapturePackage.audio)), audio)
    let manifest = try Data(contentsOf: staging.appendingPathComponent(CapturePackage.manifest))
    let stagedCapture = try JSONDecoder().decode(Capture.self, from: manifest)
    XCTAssertEqual(stagedCapture.note, capture.note)
    XCTAssertEqual(stagedCapture.createdAt, capture.createdAt)
    XCTAssertThrowsError(try CapturePackage.unpack(payload, into: staging, id: UUID()))

    var partial = try Data(contentsOf: payload)
    partial.removeLast()
    try partial.write(to: payload)
    XCTAssertThrowsError(try CapturePackage.unpack(payload, into: staging, id: capture.id))

    partial.append(0)
    partial[partial.count - 256] ^= 1
    try partial.write(to: payload)
    XCTAssertThrowsError(try CapturePackage.unpack(payload, into: staging, id: capture.id))
  }
}
