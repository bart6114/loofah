import Foundation

enum FloatingBarStatus: String, Codable {
  case recording
  case processing
  case error
}

enum FloatingBarColorScheme: String, Codable {
  case light
  case dark
}

struct FloatingTranscriptBubblePayload: Codable, Identifiable {
  let id: String
  let speakerLabel: String
  let text: String
  let isSelf: Bool
  let isFinal: Bool
  let startMs: Double
  let endMs: Double
  let overlapsPrevious: Bool
  let overlapsNext: Bool
}

struct DictationActionButton: Codable, Identifiable {
  let id: String
  let label: String
}

struct DictationPresentation: Codable {
  let phase: String
  let attemptId: UInt64
  let detail: String?
  let actions: [DictationActionButton]
}

struct FloatingBarStatePayload: Codable {
  let amplitude: Double
  let title: String
  let status: FloatingBarStatus
  let showStatusText: Bool
  let dictation: DictationPresentation?
  let colorScheme: FloatingBarColorScheme
  let opacity: Double
  let liveCaptionOpacity: Double
  let liveCaptionWidth: Double
  let liveCaptionLineCount: Int
  let liveCaptionPosition: LiveCaptionPosition
  let liveCaptionMinimized: Bool
  let liveCaptionToggleVisible: Bool
  let transcriptBubbles: [FloatingTranscriptBubblePayload]
}
