import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts
import QtQuick.Window
import SwiftifyInstaller 1.0

ApplicationWindow {
    id: window
    readonly property int wizardWidth: 560
    readonly property int wizardHeight: 620
    width: wizardWidth
    height: wizardHeight
    minimumWidth: wizardWidth
    maximumWidth: wizardWidth
    minimumHeight: wizardHeight
    maximumHeight: wizardHeight
    flags: Qt.Window | Qt.FramelessWindowHint | Qt.WindowMinimizeButtonHint | Qt.WindowCloseButtonHint
    visible: true
    title: "Swiftify Installer · Community"
    color: "#141217"
    Material.theme: Material.Dark
    Material.accent: "#ff7aae"

    function centerWindow() {
        const area = window.screen.availableGeometry
        window.x = area.x + Math.round((area.width - window.width) / 2)
        window.y = area.y + Math.round((area.height - window.height) / 2)
    }

    Installer { id: backend }
    Component.onCompleted: {
        backend.poll()
        Qt.callLater(centerWindow)
    }

    InstallerTitleBar {
        id: titleBar
        hostWindow: window
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
    }

    onClosing: function(close) {
        if (backend.installing) {
            close.accepted = false
            backend.warnClose()
        }
    }

    Timer {
        interval: 80
        repeat: true
        running: backend.scanning || backend.checking || backend.installing
        onTriggered: backend.poll()
    }

    component BodyText: Text {
        color: "#d5cbd3"
        wrapMode: Text.Wrap
        font.pixelSize: 14
    }

    component StepButton: Button {
        id: stepButton
        property bool prominent: false
        font.pixelSize: 14
        font.weight: Font.DemiBold
        implicitHeight: 44
        background: Rectangle {
            radius: 15
            color: stepButton.prominent ? (stepButton.down ? "#e96a9d" : "#ff92b8")
                                        : (stepButton.down ? "#393039" : "#302932")
            border.color: stepButton.prominent ? "transparent" : "#514650"
        }
        contentItem: Text {
            text: stepButton.text
            font: stepButton.font
            color: stepButton.prominent ? "#29121e" : "#f8e9f2"
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
    }

    ScrollView {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: titleBar.bottom
        anchors.bottom: parent.bottom
        anchors.leftMargin: 24
        anchors.rightMargin: 24
        anchors.topMargin: 20
        anchors.bottomMargin: 24
        contentWidth: availableWidth
        clip: true

        ColumnLayout {
            width: parent.width
            spacing: 16

            RowLayout {
                Layout.fillWidth: true
                Text {
                    text: "swiftify"
                    color: "#ffe0ec"
                    font.pixelSize: 22
                    font.weight: Font.Bold
                    font.letterSpacing: -0.5
                }
                Text {
                    text: " / installer"
                    color: "#b7a8b4"
                    font.pixelSize: 13
                    Layout.alignment: Qt.AlignBaseline
                }
                Item { Layout.fillWidth: true }
                BodyText { text: "COMMUNITY EDITION"; font.pixelSize: 10; color: "#cdaebe" }
            }

            Rectangle { Layout.fillWidth: true; height: 1; color: "#453a46" }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 6
                BodyText {
                    text: (backend.page + 1) + " / 4  ·  " + ["WELCOME", "LOCATION", "SETUP", "COMPLETE"][backend.page]
                    color: "#ff9cc1"
                    font.pixelSize: 11
                    font.weight: Font.DemiBold
                }
                Text {
                    text: ["Your music, your way.", "Choose your location", "Setting up Swiftify", "Ready to listen"][backend.page]
                    color: "#fff1f7"
                    font.pixelSize: 28
                    font.weight: Font.DemiBold
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }
                BodyText {
                    text: ["Install a new copy or select an existing installation.",
                           "Review the destination before making changes.",
                           "Keep this window open while setup finishes.",
                           "Swiftify is installed. Your configuration stays yours."][backend.page]
                    color: "#bfb2bd"
                    Layout.fillWidth: true
                }
            }

            Rectangle {
                Layout.fillWidth: true
                implicitHeight: cardContent.implicitHeight + 36
                radius: 20
                color: "#242027"
                border.color: "#403741"

                ColumnLayout {
                    id: cardContent
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: 18
                    spacing: 12

                    ColumnLayout {
                        visible: backend.page === 0
                        Layout.fillWidth: true
                        spacing: 10
                        BodyText {
                            text: backend.scanning ? "Looking for installations…"
                                  : backend.installationsJson === "[]" ? "No validated Swiftify installation found in the checked locations."
                                  : "Select a detected installation, or continue to choose a location."
                            Layout.fillWidth: true
                        }
                        Repeater {
                            model: JSON.parse(backend.installationsJson)
                            delegate: Rectangle {
                                required property var modelData
                                Layout.fillWidth: true
                                implicitHeight: choiceContent.implicitHeight + 24
                                radius: 12
                                color: backend.destination.toLowerCase() === modelData.path.toLowerCase()
                                       ? "#48303f" : "#302a32"
                                border.color: backend.destination.toLowerCase() === modelData.path.toLowerCase()
                                              ? "#ff9cc1" : "#514650"
                                ColumnLayout {
                                    id: choiceContent
                                    anchors.left: parent.left
                                    anchors.right: parent.right
                                    anchors.top: parent.top
                                    anchors.margins: 12
                                    spacing: 4
                                    BodyText {
                                        text: (modelData.version || "Version unknown") + " · " + (modelData.problem ? "Repair needed" : "Detected")
                                        color: "#fff1f7"
                                        font.weight: Font.DemiBold
                                        Layout.fillWidth: true
                                    }
                                    BodyText { text: modelData.path; font.pixelSize: 12; Layout.fillWidth: true }
                                    BodyText { visible: !!modelData.problem; text: modelData.problem || ""; font.pixelSize: 12; Layout.fillWidth: true }
                                }
                                MouseArea { anchors.fill: parent; onClicked: backend.destination = modelData.path }
                            }
                        }
                        RowLayout {
                            StepButton { text: "Rescan"; enabled: !backend.scanning; onClicked: backend.scan() }
                            StepButton { text: "Use default"; onClicked: backend.useDefault() }
                        }
                    }

                    ColumnLayout {
                        visible: backend.page === 1
                        Layout.fillWidth: true
                        spacing: 12
                        BodyText { text: "Installation folder"; color: "#fff1f7"; font.weight: Font.DemiBold }
                        TextField {
                            id: destinationField
                            Layout.fillWidth: true
                            text: backend.destination
                            selectByMouse: true
                            onEditingFinished: backend.destination = text
                        }
                        StepButton {
                            text: "Inspect this location"
                            enabled: !backend.scanning
                            onClicked: { backend.destination = destinationField.text; backend.scan() }
                        }
                        BodyText { text: backend.selectedDescription; Layout.fillWidth: true }
                        BodyText {
                            text: "Close Swiftify before continuing. Existing files are backed up; untracked files are preserved, not deleted."
                            Layout.fillWidth: true
                            font.pixelSize: 12
                        }
                    }

                    ColumnLayout {
                        visible: backend.page === 2
                        Layout.fillWidth: true
                        BodyText { text: backend.status; color: "#fff1f7"; Layout.fillWidth: true }
                        ProgressBar {
                            visible: backend.progressText !== ""
                            from: 0
                            to: 1
                            value: backend.progressFraction
                            Layout.fillWidth: true
                        }
                        BodyText { text: backend.progressText || "Working…"; Layout.fillWidth: true }
                    }

                    ColumnLayout {
                        visible: backend.page === 3
                        Layout.fillWidth: true
                        BodyText { text: backend.status; color: "#fff1f7"; Layout.fillWidth: true }
                        BodyText { text: backend.destination; Layout.fillWidth: true }
                        BodyText { visible: backend.backup !== ""; text: "Previous files retained at " + backend.backup; Layout.fillWidth: true; font.pixelSize: 12 }
                        RowLayout {
                            StepButton { text: "Launch Swiftify"; enabled: backend.canLaunch; onClicked: backend.launch() }
                            StepButton { text: "Open folder"; onClicked: backend.openFolder() }
                        }
                    }
                }
            }

            ColumnLayout {
                visible: backend.page < 2
                Layout.fillWidth: true
                spacing: 8
                RowLayout {
                    Layout.fillWidth: true
                    BodyText { text: backend.checking ? "Checking stable release…" : backend.releaseLabel; Layout.fillWidth: true }
                    StepButton { text: "Check again"; enabled: !backend.checking && !backend.retryPending; onClicked: backend.check() }
                }
                BodyText { visible: backend.notice !== ""; text: backend.notice; Layout.fillWidth: true; font.pixelSize: 12 }
                BodyText { visible: backend.diagnostics !== ""; text: backend.diagnostics; Layout.fillWidth: true; font.pixelSize: 12 }
            }

            BodyText {
                visible: backend.error !== ""
                text: backend.error
                color: "#ffb4ab"
                Layout.fillWidth: true
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 10
                StepButton { visible: backend.page === 1; text: "Back"; onClicked: backend.back() }
                Item { Layout.fillWidth: true }
                StepButton {
                    visible: backend.page === 0
                    text: "Continue"
                    prominent: true
                    enabled: !backend.scanning
                    onClicked: backend.advance()
                }
                StepButton {
                    visible: backend.page === 1
                    text: backend.actionLabel
                    prominent: true
                    enabled: backend.canInstall && !backend.checking && !backend.scanning
                    onClicked: { backend.destination = destinationField.text; backend.advance() }
                }
                StepButton {
                    visible: backend.page === 3
                    text: "Done"
                    prominent: true
                    onClicked: window.close()
                }
            }

            BodyText {
                text: "Independent community utility · Not affiliated with Swiftify"
                color: "#9d929d"
                font.pixelSize: 11
                Layout.fillWidth: true
            }
        }
    }
}
