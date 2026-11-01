import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: titleBar
    required property var hostWindow
    readonly property url iconSource: "qrc:/qt/qml/SwiftifyInstaller/assets/installer.ico"

    height: 56
    color: "#1d1921"

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 20
        anchors.rightMargin: 10
        spacing: 10

        Image {
            source: titleBar.iconSource
            sourceSize.width: 24
            sourceSize.height: 24
            Layout.preferredWidth: 24
            Layout.preferredHeight: 24
            fillMode: Image.PreserveAspectFit
            smooth: true
            Accessible.name: "Swiftify Installer icon"
        }
        Text {
            text: "Swiftify Installer"
            color: "#fff1f7"
            font.pixelSize: 14
            font.weight: Font.DemiBold
        }
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton
                onPressed: titleBar.hostWindow.startSystemMove()
            }
        }
        ToolButton {
            id: minimizeButton
            text: "−"
            Accessible.name: "Minimize window"
            implicitWidth: 38
            implicitHeight: 38
            onClicked: titleBar.hostWindow.showMinimized()
            background: Rectangle {
                radius: 10
                color: minimizeButton.down ? "#51414e" : minimizeButton.hovered ? "#383039" : "transparent"
            }
            contentItem: Text {
                text: minimizeButton.text
                color: "#e9dbe5"
                font.pixelSize: 20
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }
        ToolButton {
            id: closeButton
            text: "×"
            Accessible.name: "Close window"
            implicitWidth: 38
            implicitHeight: 38
            onClicked: titleBar.hostWindow.close()
            background: Rectangle {
                radius: 10
                color: closeButton.down ? "#a64353" : closeButton.hovered ? "#713443" : "transparent"
            }
            contentItem: Text {
                text: closeButton.text
                color: "#ffe7eb"
                font.pixelSize: 21
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }
    }

    Rectangle {
        anchors.bottom: parent.bottom
        width: parent.width
        height: 1
        color: "#403741"
    }
}
