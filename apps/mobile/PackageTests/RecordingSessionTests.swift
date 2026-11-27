import Foundation
import XCTest

@testable import Loofah

final class RecordingSessionTests: XCTestCase {
  private let start = Date(timeIntervalSinceReferenceDate: 100)

  func testDuplicateNotificationsResumeOnlyOnceAndExcludePauseFromTimer() {
    var session = RecordingSession(id: UUID(), startedAt: start)
    let originalObservation = session.observationID
    let pause = start.addingTimeInterval(4)
    XCTAssertTrue(session.pause(at: pause, awaitingEnd: true))
    XCTAssertFalse(session.pause(at: pause.addingTimeInterval(1), awaitingEnd: true))
    XCTAssertEqual(session.pausedAt, pause)
    XCTAssertTrue(session.interruptionEnded(allowsResume: true))
    XCTAssertFalse(session.interruptionEnded(allowsResume: true))

    session.renewObservation()
    session.resumed(at: pause.addingTimeInterval(3))
    XCTAssertNotEqual(session.observationID, originalObservation)
    XCTAssertNil(session.pausedAt)
    XCTAssertEqual(session.timerStartedAt, start.addingTimeInterval(3))
  }

  func testMissingEndOrFailedResumeLeavesTheRecordingPaused() {
    var session = RecordingSession(id: UUID(), startedAt: start)
    let pause = start.addingTimeInterval(4)
    XCTAssertTrue(session.pause(at: pause, awaitingEnd: true))
    XCTAssertEqual(session.pausedAt, pause)

    XCTAssertTrue(session.interruptionEnded(allowsResume: true))
    session.renewObservation()
    XCTAssertEqual(session.pausedAt, pause)
    XCTAssertFalse(session.interruptionEnded(allowsResume: true))
  }

  func testDeniedResumeOrMediaLossRequiresExplicitResume() {
    var session = RecordingSession(id: UUID(), startedAt: start)
    XCTAssertTrue(session.pause(at: start, awaitingEnd: true))
    XCTAssertFalse(session.interruptionEnded(allowsResume: false))
    XCTAssertFalse(session.interruptionEnded(allowsResume: true))

    session.resumed(at: start)
    XCTAssertTrue(session.pause(at: start, awaitingEnd: true))
    XCTAssertFalse(session.pause(at: start, awaitingEnd: false))
    XCTAssertFalse(session.interruptionEnded(allowsResume: true))

    XCTAssertFalse(session.pause(at: start, awaitingEnd: true))
    XCTAssertFalse(session.interruptionEnded(allowsResume: true))
  }
}
