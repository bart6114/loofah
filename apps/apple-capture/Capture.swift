import CryptoKit
import Foundation

enum CaptureError: LocalizedError {
  case invalid
  case waiting
  case accountChanged

  var errorDescription: String? {
    switch self {
    case .invalid: "The capture could not be verified."
    case .waiting: "The iCloud file is still downloading."
    case .accountChanged: "The iCloud account changed."
    }
  }
}

struct Capture: Codable {
  let id: UUID
  let createdAt: Date
  var finishedAt: Date?
  var title: String? = nil
  var note: String
  let recording: Bool

  var name: String { id.uuidString.lowercased() }

  var customTitle: String? {
    guard let title = title?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty
    else { return nil }
    return title
  }

  var hasText: Bool { customTitle != nil || note.contains { !$0.isWhitespace } }
}

struct CaptureReceipt: Codable, Equatable {
  let id: UUID
  let digest: String
}

private struct PackageHeader: Codable {
  let capture: Capture
  let audioBytes: UInt64
}

enum CapturePackage {
  static let manifest = "capture.json"
  static let audio = "audio.wav"
  static let payload = "capture.loof"
  private static let headerLimit = 8 * 1024 * 1024
  private static let digestLength = 32

  // A length-prefixed manifest, optional WAV, and SHA-256 trailer travel as one
  // file so iCloud never needs to reconcile independently arriving attachments.
  static func seal(_ capture: Capture, in directory: URL) throws -> CaptureReceipt {
    let audioURL = directory.appendingPathComponent(audio)
    let size = capture.recording ? try fileSize(audioURL) : 0
    guard capture.finishedAt != nil, capture.recording ? size > 44 : capture.hasText
    else { throw CaptureError.invalid }
    let data = try JSONEncoder().encode(PackageHeader(capture: capture, audioBytes: size))
    guard data.count <= headerLimit else { throw CaptureError.invalid }

    let destination = directory.appendingPathComponent(payload)
    let temp = directory.appendingPathComponent(".payload")
    try Data().write(to: temp, options: .atomic)
    defer { try? FileManager.default.removeItem(at: temp) }
    let writer = try FileHandle(forWritingTo: temp)
    defer { try? writer.close() }

    var length = UInt64(data.count).bigEndian
    let lengthBytes = withUnsafeBytes(of: &length) { Data($0) }
    try writer.write(contentsOf: lengthBytes)
    try writer.write(contentsOf: data)
    var hash = SHA256()
    hash.update(data: lengthBytes)
    hash.update(data: data)
    if capture.recording {
      let reader = try FileHandle(forReadingFrom: audioURL)
      defer { try? reader.close() }
      try stream(reader, to: writer, size: size, hash: &hash)
    }
    let digest = Data(hash.finalize())
    try writer.write(contentsOf: digest)
    try writer.synchronize()
    if FileManager.default.fileExists(atPath: destination.path) {
      _ = try FileManager.default.replaceItemAt(destination, withItemAt: temp)
    } else {
      try FileManager.default.moveItem(at: temp, to: destination)
    }
    return receipt(id: capture.id, digest: digest)
  }

  static func unpack(_ source: URL, into destination: URL, id: UUID) throws -> CaptureReceipt {
    let total = try fileSize(source)
    let reader = try FileHandle(forReadingFrom: source)
    defer { try? reader.close() }
    guard let lengthData = try reader.read(upToCount: 8), lengthData.count == 8 else {
      throw CaptureError.invalid
    }
    let length = lengthData.withUnsafeBytes { $0.loadUnaligned(as: UInt64.self).bigEndian }
    guard length > 0, length <= headerLimit,
      let data = try reader.read(upToCount: Int(length)), data.count == length
    else {
      throw CaptureError.invalid
    }

    let header = try JSONDecoder().decode(PackageHeader.self, from: data)
    let capture = header.capture
    let overhead = 8 + length + UInt64(digestLength)
    guard capture.id == id, let end = capture.finishedAt, end >= capture.createdAt,
      total >= overhead, total - overhead == header.audioBytes
    else { throw CaptureError.invalid }

    var hash = SHA256()
    hash.update(data: lengthData)
    hash.update(data: data)
    if capture.recording {
      guard header.audioBytes > 44 else { throw CaptureError.invalid }
      let audioURL = destination.appendingPathComponent(audio)
      try Data().write(to: audioURL)
      let writer = try FileHandle(forWritingTo: audioURL)
      defer { try? writer.close() }
      try stream(reader, to: writer, size: header.audioBytes, hash: &hash)
    } else {
      guard header.audioBytes == 0, capture.hasText
      else { throw CaptureError.invalid }
    }

    let digest = Data(hash.finalize())
    guard try reader.read(upToCount: digestLength) == digest else { throw CaptureError.invalid }
    try JSONEncoder().encode(capture).write(
      to: destination.appendingPathComponent(manifest), options: .atomic)
    return receipt(id: id, digest: digest)
  }

  private static func fileSize(_ url: URL) throws -> UInt64 {
    let values = try url.resourceValues(forKeys: [
      .isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey,
    ])
    guard values.isRegularFile == true, values.isSymbolicLink != true, let size = values.fileSize
    else { throw CaptureError.invalid }
    return UInt64(size)
  }

  private static func stream(
    _ reader: FileHandle, to writer: FileHandle, size: UInt64, hash: inout SHA256
  ) throws {
    var remaining = size
    while remaining > 0 {
      guard let data = try reader.read(upToCount: Int(min(remaining, 256 * 1024))), !data.isEmpty
      else {
        throw CaptureError.invalid
      }
      try writer.write(contentsOf: data)
      hash.update(data: data)
      remaining -= UInt64(data.count)
    }
  }

  private static func receipt(id: UUID, digest: Data) -> CaptureReceipt {
    CaptureReceipt(id: id, digest: digest.map { String(format: "%02x", $0) }.joined())
  }
}
