import Cocoa
import XCTest

@testable import swift_lib

final class NotificationLifetimeTests: XCTestCase {
  private var notifications: [NotificationInstance] = []

  override func setUp() {
    super.setUp()
    _ = NSApplication.shared
    timedOutNotificationKeys.removeAll()
  }

  override func tearDown() {
    for notification in notifications {
      notification.pauseDismissTimer()
      notification.panel.close()
    }
    NotificationManager.shared.activeNotifications.removeAll()
    notifications.removeAll()
    super.tearDown()
  }

  private func makeNotification(source: NotificationSource? = nil) -> NotificationInstance {
    let payload = NotificationPayload(
      key: UUID().uuidString, title: "Reminder", message: "Microphone in use",
      timeoutSeconds: 30, source: source, startTime: nil, participants: nil, eventDetails: nil,
      actionLabel: "Record", actionVariant: nil, options: nil, footer: nil, icon: nil
    )
    let panel = NotificationManager.shared.createPanel(yPosition: 100)
    let view = ClickableView(frame: panel.contentView!.bounds)
    panel.contentView = view
    let notification = NotificationInstance(
      payload: payload, panel: panel, clickableView: view, creationIndex: notifications.count)
    view.notification = notification
    view.onHover = { [weak notification] hovering in
      if hovering {
        notification?.pauseDismissTimer()
      } else {
        notification?.resumeDismissTimer()
      }
    }
    notifications.append(notification)
    return notification
  }

  func testThirtySecondTimerStartsPausedWhenPointerAlreadyOverReminder() {
    let notification = makeNotification()
    notification.clickableView.setHovering(true)
    notification.startDismissTimer(timeoutSeconds: 30)

    XCTAssertNil(notification.dismissTimer)
    notification.clickableView.setHovering(false)
    XCTAssertEqual(notification.dismissTimer!.fireDate.timeIntervalSinceNow, 30, accuracy: 0.1)
  }

  func testExpandedReminderDoesNotStartTimerUntilCollapsed() {
    let notification = makeNotification()
    notification.isExpanded = true
    notification.startDismissTimer(timeoutSeconds: 30)
    notification.resumeDismissTimer()
    XCTAssertNil(notification.dismissTimer)

    notification.isExpanded = false
    notification.resumeDismissTimer()
    XCTAssertEqual(notification.dismissTimer!.fireDate.timeIntervalSinceNow, 30, accuracy: 0.1)
  }

  func testRepeatedResumeDoesNotResetRunningTimer() {
    let notification = makeNotification()
    notification.startDismissTimer(timeoutSeconds: 30)
    let originalTimer = notification.dismissTimer
    notification.resumeDismissTimer()
    XCTAssertTrue(notification.dismissTimer === originalTimer)
  }

  func testPauseResumeKeepsRemainingTime() {
    let notification = makeNotification()
    notification.startDismissTimer(timeoutSeconds: 30)
    let originalDeadline = notification.dismissTimer!.fireDate
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    notification.pauseDismissTimer()
    let pauseStart = Date()
    XCTAssertNil(notification.dismissTimer)
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    let pausedDuration = Date().timeIntervalSince(pauseStart)
    notification.resumeDismissTimer()
    XCTAssertEqual(
      notification.dismissTimer!.fireDate.timeIntervalSince(originalDeadline), pausedDuration,
      accuracy: 0.02)
  }

  func testGlobalHoverUpdatesViewStateAndPausesTimer() {
    let notification = makeNotification()
    notification.startDismissTimer(timeoutSeconds: 30)
    NotificationManager.shared.activeNotifications[notification.key] = notification
    let frame = notification.panel.frame
    NotificationManager.shared.updateHoverForAll(
      atScreenPoint: NSPoint(x: frame.midX, y: frame.midY))
    XCTAssertTrue(notification.clickableView.isHovering)
    XCTAssertNil(notification.dismissTimer)

    NotificationManager.shared.updateHoverForAll(atScreenPoint: NSPoint(x: -1000, y: -1000))
    XCTAssertFalse(notification.clickableView.isHovering)
    XCTAssertNotNil(notification.dismissTimer)
  }

  func testFocusCleanupPreservesMicrophoneReminderDeadline() {
    let reminder = makeNotification(
      source: .micDetected(appNames: ["Zoom"], appIds: ["us.zoom.xos"], eventIds: []))
    let other = makeNotification(source: .session(sessionId: "session"))
    for notification in [reminder, other] {
      notification.startDismissTimer(timeoutSeconds: 30)
      NotificationManager.shared.activeNotifications[notification.key] = notification
    }
    let timer = reminder.dismissTimer

    NotificationManager.shared.dismissOnFocus()
    NotificationManager.shared.dismissOnFocus()
    XCTAssertTrue(reminder.dismissTimer === timer)
    XCTAssertTrue(timer!.isValid)
    XCTAssertNil(other.dismissTimer)
    XCTAssertTrue(reminder.panel.collectionBehavior.contains(.canJoinAllSpaces))
    XCTAssertTrue(reminder.panel.collectionBehavior.contains(.fullScreenAuxiliary))
    XCTAssertFalse(reminder.panel.hidesOnDeactivate)
  }

  func testUnpausedTimerExpiresAndEmitsTimeout() {
    let notification = makeNotification()
    notification.startDismissTimer(timeoutSeconds: 0.03)
    let timer = notification.dismissTimer!
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))

    XCTAssertNil(notification.dismissTimer)
    XCTAssertFalse(timer.isValid)
    XCTAssertEqual(timedOutNotificationKeys, [notification.key])
  }

  func testExplicitDismissStillCancelsMicrophoneReminderImmediately() {
    let reminder = makeNotification(
      source: .micDetected(appNames: ["Zoom"], appIds: ["us.zoom.xos"], eventIds: []))
    reminder.startDismissTimer(timeoutSeconds: 30)
    let timer = reminder.dismissTimer!
    reminder.dismissWithUserAction()
    XCTAssertNil(reminder.dismissTimer)
    XCTAssertFalse(timer.isValid)
  }
}
