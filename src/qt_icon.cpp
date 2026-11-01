#include "qt_icon.h"

#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>

void set_application_icon() {
    QGuiApplication::setWindowIcon(QIcon(QStringLiteral(":/qt/qml/SwiftifyInstaller/assets/installer.ico")));
}
