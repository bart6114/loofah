import Foundation

enum RecordingWAV {
  static func finalize(at url: URL) throws -> Bool {
    let file: FileHandle
    do {
      file = try FileHandle(forUpdating: url)
    } catch let error as CocoaError
      where error.code == .fileNoSuchFile
      || error.code == .fileReadNoSuchFile
    {
      return false
    }
    defer { try? file.close() }

    let end = try file.seekToEnd()
    guard end > 0 else { return false }
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
        guard field(0) == 1, field(2) == 1, field(12) == 2, field(14) == 16 else {
          throw CaptureError.invalid
        }
        foundFormat = true
      case "data":
        guard foundFormat else { throw CaptureError.invalid }
        let available = end - start
        if declaredEnd == end, size <= available, size > 0 || available == 0 {
          guard size.isMultiple(of: 2) else { throw CaptureError.invalid }
          return size > 0
        }

        // AVAudioRecorder can leave the initial sizes in place when the app is killed.
        let audioSize = available - available % 2
        guard audioSize > 0 else { return false }
        let repairedEnd = start + audioSize
        guard let dataSize = UInt32(exactly: audioSize),
          let riffSize = UInt32(exactly: repairedEnd - 8)
        else { throw CaptureError.invalid }
        try write(dataSize, to: file, at: offset + 4)
        if repairedEnd != end { try file.truncate(atOffset: repairedEnd) }
        try write(riffSize, to: file, at: 4)
        try file.synchronize()
        return true
      default:
        break
      }
      offset = start + size + size % 2
    }
    throw CaptureError.invalid
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
    var bytes = value.littleEndian
    try file.write(contentsOf: withUnsafeBytes(of: &bytes) { Data($0) })
  }
}
