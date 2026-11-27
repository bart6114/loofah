import Foundation

enum RecordingWAV {
  static let sampleRate: UInt32 = 24_000

  static func continuationURL(in directory: URL) throws -> URL {
    let files = FileManager.default
    let parts = partsDirectory(in: directory)
    try files.createDirectory(at: parts, withIntermediateDirectories: true)
    let existing = try segments(in: directory)
    if existing.isEmpty {
      let first = directory.appendingPathComponent(CapturePackage.audio)
      if files.fileExists(atPath: first.path) {
        try files.moveItem(at: first, to: part(0, in: parts))
        return part(1, in: parts)
      }
    }
    return part(existing.count, in: parts)
  }

  static func lastWrite(in directory: URL) throws -> Date? {
    let parts = try segments(in: directory)
    let sources = parts.isEmpty ? [directory.appendingPathComponent(CapturePackage.audio)] : parts
    return sources.compactMap {
      try? $0.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
    }.max()
  }

  static func finalizeCapture(in directory: URL) throws -> UInt64 {
    let audio = directory.appendingPathComponent(CapturePackage.audio)
    let parts = try segments(in: directory)
    guard !parts.isEmpty else { return try finalize(at: audio) }
    let files = FileManager.default
    if parts.count == 1, !files.fileExists(atPath: audio.path) {
      // Activation can fail after moving the original WAV. Restore it without a second copy.
      try files.moveItem(at: parts[0], to: audio)
      return try finalize(at: audio)
    }

    // Keep the source segments until the queue checkpoint commits. Retrying always rebuilds
    // from those segments, even if a previous attempt already installed audio.wav.
    let temp = directory.appendingPathComponent(".merged-audio.wav")
    var header = Data("RIFF".utf8)
    header.append(littleEndian(36))
    header.append(Data("WAVEfmt ".utf8))
    header.append(contentsOf: [16, 0, 0, 0, 1, 0, 1, 0])
    header.append(littleEndian(sampleRate))
    header.append(littleEndian(sampleRate * 2))
    header.append(contentsOf: [2, 0, 16, 0])
    header.append(Data("data".utf8))
    header.append(littleEndian(0))
    try header.write(
      to: temp, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    defer { try? files.removeItem(at: temp) }
    let writer = try FileHandle(forWritingTo: temp)
    defer { try? writer.close() }
    try writer.seekToEnd()
    var bytes: UInt64 = 0
    for url in parts {
      let reader = try FileHandle(forUpdating: url)
      defer { try? reader.close() }
      guard let range = try sampleBytes(in: reader, sampleRate: sampleRate) else { continue }
      var remaining = range.upperBound - range.lowerBound
      bytes += remaining
      guard bytes <= UInt64(UInt32.max) - 36 else { throw CaptureError.invalid }
      try reader.seek(toOffset: range.lowerBound)
      while remaining > 0 {
        let block = try read(reader, count: Int(min(remaining, 64 * 1024)))
        try writer.write(contentsOf: block)
        remaining -= UInt64(block.count)
      }
    }
    guard bytes > 0 else { return 0 }
    try write(UInt32(bytes) + 36, to: writer, at: 4)
    try write(UInt32(bytes), to: writer, at: 40)
    try writer.synchronize()
    if files.fileExists(atPath: audio.path) {
      _ = try files.replaceItemAt(audio, withItemAt: temp)
    } else {
      try files.moveItem(at: temp, to: audio)
    }
    return bytes / 2
  }

  static func removeSegments(in directory: URL) {
    try? FileManager.default.removeItem(at: partsDirectory(in: directory))
  }

  static func finalize(at url: URL) throws -> UInt64 {
    let file: FileHandle
    do {
      file = try FileHandle(forUpdating: url)
    } catch let error as CocoaError
      where error.code == .fileNoSuchFile
      || error.code == .fileReadNoSuchFile
    {
      return 0
    }
    defer { try? file.close() }
    guard let range = try sampleBytes(in: file) else { return 0 }
    return (range.upperBound - range.lowerBound) / 2
  }

  private static func sampleBytes(in file: FileHandle, sampleRate: UInt32? = nil)
    throws -> Range<UInt64>?
  {
    let end = try file.seekToEnd()
    guard end > 0 else { return nil }
    try file.seek(toOffset: 0)
    let header = try read(file, count: 12)
    guard header.prefix(4) == Data("RIFF".utf8), header.suffix(4) == Data("WAVE".utf8) else {
      throw CaptureError.invalid
    }
    let declaredEnd = UInt64(uint32(header, at: 4)) + 8
    var foundFormat = false
    var offset: UInt64 = 12
    while offset + 8 <= end {
      try file.seek(toOffset: offset)
      let chunk = try read(file, count: 8)
      let size = UInt64(uint32(chunk, at: 4))
      let start = offset + 8
      switch String(decoding: chunk.prefix(4), as: UTF8.self) {
      case "fmt ":
        guard size >= 16, start + size <= end else { throw CaptureError.invalid }
        let format = try read(file, count: 16)
        func field(_ offset: Int) -> UInt16 {
          format.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset, as: UInt16.self) }
            .littleEndian
        }
        guard field(0) == 1, field(2) == 1, field(12) == 2, field(14) == 16,
          sampleRate == nil || sampleRate == uint32(format, at: 4)
        else {
          throw CaptureError.invalid
        }
        foundFormat = true
      case "data":
        guard foundFormat else { throw CaptureError.invalid }
        let available = end - start
        if declaredEnd == end, size <= available, size > 0 || available == 0 {
          guard size.isMultiple(of: 2) else { throw CaptureError.invalid }
          return size > 0 ? start..<(start + size) : nil
        }

        // AVAudioRecorder can leave the initial sizes in place when the app is killed.
        let audioSize = available - available % 2
        guard audioSize > 0 else { return nil }
        let repairedEnd = start + audioSize
        guard let dataSize = UInt32(exactly: audioSize),
          let riffSize = UInt32(exactly: repairedEnd - 8)
        else { throw CaptureError.invalid }
        try write(dataSize, to: file, at: offset + 4)
        if repairedEnd != end { try file.truncate(atOffset: repairedEnd) }
        try write(riffSize, to: file, at: 4)
        try file.synchronize()
        return start..<repairedEnd
      default:
        break
      }
      offset = start + size + size % 2
    }
    throw CaptureError.invalid
  }

  private static func partsDirectory(in directory: URL) -> URL {
    directory.appendingPathComponent(".recording-segments", isDirectory: true)
  }

  private static func part(_ index: Int, in directory: URL) -> URL {
    directory.appendingPathComponent(String(format: "%010d.wav", index))
  }

  private static func segments(in directory: URL) throws -> [URL] {
    do {
      return try FileManager.default.contentsOfDirectory(
        at: partsDirectory(in: directory), includingPropertiesForKeys: nil,
        options: .skipsHiddenFiles
      ).filter { $0.pathExtension == "wav" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
    } catch let error as CocoaError
      where error.code == .fileNoSuchFile || error.code == .fileReadNoSuchFile
    {
      return []
    }
  }

  private static func read(_ file: FileHandle, count: Int) throws -> Data {
    guard let data = try file.read(upToCount: count), data.count == count else {
      throw CaptureError.invalid
    }
    return data
  }

  private static func uint32(_ data: Data, at offset: Int) -> UInt32 {
    data.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset, as: UInt32.self) }
      .littleEndian
  }

  private static func write(_ value: UInt32, to file: FileHandle, at offset: UInt64) throws {
    try file.seek(toOffset: offset)
    try file.write(contentsOf: littleEndian(value))
  }

  private static func littleEndian(_ value: UInt32) -> Data {
    var bytes = value.littleEndian
    return withUnsafeBytes(of: &bytes) { Data($0) }
  }
}
