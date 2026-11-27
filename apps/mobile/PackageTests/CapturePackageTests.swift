import Foundation
import XCTest

@testable import Loofah

final class CapturePackageTests: XCTestCase {
  private var emptyWAV: Data {
    var header = Data("RIFF".utf8) + Data([36, 0, 0, 0]) + Data("WAVEfmt ".utf8)
    header += Data([16, 0, 0, 0, 1, 0, 1, 0, 192, 93, 0, 0, 128, 187, 0, 0, 2, 0, 16, 0])
    header += Data("data".utf8) + Data([0, 0, 0, 0])
    return header
  }

  func testInterruptedWAVRecoversSamplesAndCanBeRetried() throws {
    let audio = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: audio) }
    let header = emptyWAV
    let samples = Data([0x01, 0x00, 0xff, 0xff])

    XCTAssertEqual(try RecordingWAV.finalize(at: audio), 0)
    try header.write(to: audio)
    XCTAssertEqual(try RecordingWAV.finalize(at: audio), 0)

    try (header + samples + Data([0x7f])).write(to: audio)
    XCTAssertEqual(try RecordingWAV.finalize(at: audio), 2)
    var expected = header + samples
    expected[4] = 40
    expected[40] = 4
    XCTAssertEqual(try Data(contentsOf: audio), expected)
    XCTAssertEqual(try RecordingWAV.finalize(at: audio), 2)
    XCTAssertEqual(try Data(contentsOf: audio), expected)
  }

  func testFailedContinuationCanStillKeepTheOriginalRecording() throws {
    let files = FileManager.default
    let root = files.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
    try files.createDirectory(at: root, withIntermediateDirectories: false)
    defer { try? files.removeItem(at: root) }
    let audio = root.appendingPathComponent(CapturePackage.audio)
    let samples = Data([0x01, 0x02, 0x03, 0x04])
    try (emptyWAV + samples).write(to: audio)
    _ = try RecordingWAV.continuationURL(in: root)

    XCTAssertEqual(try RecordingWAV.finalizeCapture(in: root), 2)
    XCTAssertEqual(try Data(contentsOf: audio).suffix(samples.count), samples)
    XCTAssertEqual(try RecordingWAV.finalizeCapture(in: root), 2)
  }

  func testContinuedRecordingKeepsAllSamplesThroughRetryAndHandoff() throws {
    let files = FileManager.default
    let root = files.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
    try files.createDirectory(at: root, withIntermediateDirectories: false)
    defer { try? files.removeItem(at: root) }
    let audio = root.appendingPathComponent(CapturePackage.audio)
    let first = Data(repeating: 0x12, count: 96_000)
    let last = Data(repeating: 0x34, count: 48_000)
    let header = emptyWAV
    try (header + first).write(to: audio)

    let empty = try RecordingWAV.continuationURL(in: root)
    try header.write(to: empty)
    let resumed = try RecordingWAV.continuationURL(in: root)
    try (header + last + Data([0x7f])).write(to: resumed)

    XCTAssertEqual(try RecordingWAV.finalizeCapture(in: root), 72_000)
    let combined = try Data(contentsOf: audio)
    XCTAssertEqual(combined.dropFirst(header.count), first + last)
    XCTAssertEqual(try RecordingWAV.finalizeCapture(in: root), 72_000)
    XCTAssertEqual(try Data(contentsOf: audio), combined)
    RecordingWAV.removeSegments(in: root)
    XCTAssertEqual(try RecordingWAV.finalizeCapture(in: root), 72_000)

    let now = Date()
    let capture = Capture(
      id: UUID(), createdAt: now, finishedAt: now.addingTimeInterval(4), note: "", recording: true)
    let receipt = try CapturePackage.seal(capture, in: root)
    let staging = root.appendingPathComponent("staging", isDirectory: true)
    try files.createDirectory(at: staging, withIntermediateDirectories: false)
    let payload = root.appendingPathComponent(CapturePackage.payload)
    XCTAssertEqual(try CapturePackage.unpack(payload, into: staging, id: capture.id), receipt)
    XCTAssertEqual(
      try Data(contentsOf: staging.appendingPathComponent(CapturePackage.audio)), combined)
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
      title: "Studio review", note: "Check the mise en page.", recording: true
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
    XCTAssertEqual(stagedCapture.title, capture.title)
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

  func testTitleOnlyNoteRoundTrip() throws {
    let files = FileManager.default
    let root = files.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
    try files.createDirectory(at: root, withIntermediateDirectories: false)
    defer { try? files.removeItem(at: root) }
    let now = Date()
    var capture = Capture(id: UUID(), createdAt: now, finishedAt: now, note: "", recording: false)
    XCTAssertThrowsError(try CapturePackage.seal(capture, in: root))

    capture.title = "Invoice"
    let receipt = try CapturePackage.seal(capture, in: root)
    let payload = root.appendingPathComponent(CapturePackage.payload)
    XCTAssertEqual(try CapturePackage.unpack(payload, into: root, id: capture.id), receipt)
    let manifest = try Data(contentsOf: root.appendingPathComponent(CapturePackage.manifest))
    let stagedCapture = try JSONDecoder().decode(Capture.self, from: manifest)
    XCTAssertEqual(stagedCapture.title, "Invoice")
    XCTAssertTrue(stagedCapture.note.isEmpty)
  }
}
