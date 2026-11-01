#[cfg(feature = "qt")]
fn main() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    let module = QmlModule::new("SwiftifyInstaller")
        .qml_files(["qml/Main.qml", "qml/InstallerTitleBar.qml"]);
    unsafe {
        CxxQtBuilder::new_qml_module(module)
            .files(["src/qt_bridge.rs"])
            .cpp_file("src/qt_icon.cpp")
            .qrc_resources(["assets/installer.ico"])
            .cc_builder(|cc| {
                cc.include("src");
            })
            .build();
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winres::WindowsResource::new()
            .set_icon("assets/installer.ico")
            .compile()
            .expect("Could not embed the installer icon in the Windows executable");
    }
}

#[cfg(not(feature = "qt"))]
fn main() {}
