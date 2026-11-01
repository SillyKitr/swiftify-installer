use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use swiftify_installer::controller::{Controller, Page};

#[derive(Default)]
pub struct InstallerRust {
    controller: Controller,
    page: i32,
    destination: QString,
    scanning: bool,
    checking: bool,
    installing: bool,
    installations_json: QString,
    selected_description: QString,
    release_label: QString,
    notice: QString,
    diagnostics: QString,
    error: QString,
    status: QString,
    progress_fraction: f64,
    progress_text: QString,
    backup: QString,
    can_install: bool,
    can_launch: bool,
    retry_pending: bool,
    action_label: QString,
}

#[cxx_qt::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        include!("qt_icon.h");
        type QString = cxx_qt_lib::QString;
        fn set_application_icon();
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(i32, page)]
        #[qproperty(QString, destination)]
        #[qproperty(bool, scanning)]
        #[qproperty(bool, checking)]
        #[qproperty(bool, installing)]
        #[qproperty(QString, installations_json)]
        #[qproperty(QString, selected_description)]
        #[qproperty(QString, release_label)]
        #[qproperty(QString, notice)]
        #[qproperty(QString, diagnostics)]
        #[qproperty(QString, error)]
        #[qproperty(QString, status)]
        #[qproperty(f64, progress_fraction)]
        #[qproperty(QString, progress_text)]
        #[qproperty(QString, backup)]
        #[qproperty(bool, can_install)]
        #[qproperty(bool, can_launch)]
        #[qproperty(bool, retry_pending)]
        #[qproperty(QString, action_label)]
        type Installer = super::InstallerRust;

        #[qinvokable]
        fn poll(self: Pin<&mut Self>);
        #[qinvokable]
        fn scan(self: Pin<&mut Self>);
        #[qinvokable]
        fn check(self: Pin<&mut Self>);
        #[qinvokable]
        fn advance(self: Pin<&mut Self>);
        #[qinvokable]
        fn back(self: Pin<&mut Self>);
        #[qinvokable]
        #[cxx_name = "useDefault"]
        fn use_default(self: Pin<&mut Self>);
        #[qinvokable]
        fn launch(self: Pin<&mut Self>);
        #[qinvokable]
        #[cxx_name = "openFolder"]
        fn open_folder(self: Pin<&mut Self>);
        #[qinvokable]
        #[cxx_name = "warnClose"]
        fn warn_close(self: Pin<&mut Self>);
    }
}

pub fn set_application_icon() {
    ffi::set_application_icon();
}

impl ffi::Installer {
    fn sync_destination(mut self: Pin<&mut Self>) {
        let destination = self.destination().to_string();
        self.as_mut().rust_mut().controller.destination = destination;
    }

    pub fn poll(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().controller.poll();
        let state = &self.rust().controller;
        let page = match state.page {
            Page::Welcome => 0,
            Page::Location => 1,
            Page::Progress => 2,
            Page::Complete => 3,
        };
        let installations = state
            .installations
            .iter()
            .map(|i| {
                serde_json::json!({
                    "path": i.directory.to_string_lossy(),
                    "version": i.version,
                    "problem": i.problem,
                })
            })
            .collect::<Vec<_>>();
        let installations_json =
            serde_json::to_string(&installations).unwrap_or_else(|_| "[]".into());
        let selected_description = state
            .selected()
            .map(|i| {
                format!(
                    "Installed: {}{}{}",
                    i.version.as_deref().unwrap_or("Unknown (unmanaged ZIP)"),
                    i.problem
                        .as_ref()
                        .map(|p| format!("\n{p}"))
                        .unwrap_or_default(),
                    if i.managed {
                        ""
                    } else {
                        "\nAn unmanaged copy will be preserved in full as a backup."
                    }
                )
            })
            .unwrap_or_default();
        let diagnostics = state
            .invalid
            .iter()
            .map(|i| format!("Invalid location: {i}"))
            .chain(state.warnings.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n");
        let release_label = state
            .latest
            .as_ref()
            .map(|r| format!("Latest stable: {}", r.version))
            .unwrap_or_else(|| "Release not available".into());
        let progress_fraction = state
            .progress
            .as_ref()
            .map(|(fraction, _)| *fraction as f64)
            .unwrap_or_default();
        let progress_text = state
            .progress
            .as_ref()
            .map(|(_, text)| text.clone())
            .unwrap_or_default();
        let backup = state
            .backup
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let can_launch = !state.scanning && state.selected().is_some_and(|i| i.problem.is_none());
        let can_install = state.latest.is_some() && !state.destination.trim().is_empty();
        let retry_pending = state
            .release_retry_at
            .is_some_and(swiftify_installer::release::retry_pending);
        let action_label = state.action();
        let destination = state.destination.clone();
        let scanning = state.scanning;
        let checking = state.checking;
        let installing = state.installing;
        let notice = state.release_notice.clone().unwrap_or_default();
        let error = state.error.clone().unwrap_or_default();
        let status = state.status.clone();
        self.as_mut().set_page(page);
        self.as_mut().set_destination(QString::from(&destination));
        self.as_mut().set_scanning(scanning);
        self.as_mut().set_checking(checking);
        self.as_mut().set_installing(installing);
        self.as_mut()
            .set_installations_json(QString::from(&installations_json));
        self.as_mut()
            .set_selected_description(QString::from(&selected_description));
        self.as_mut()
            .set_release_label(QString::from(&release_label));
        self.as_mut().set_notice(QString::from(&notice));
        self.as_mut().set_diagnostics(QString::from(&diagnostics));
        self.as_mut().set_error(QString::from(&error));
        self.as_mut().set_status(QString::from(&status));
        self.as_mut().set_progress_fraction(progress_fraction);
        self.as_mut()
            .set_progress_text(QString::from(&progress_text));
        self.as_mut().set_backup(QString::from(&backup));
        self.as_mut().set_can_install(can_install);
        self.as_mut().set_can_launch(can_launch);
        self.as_mut().set_retry_pending(retry_pending);
        self.as_mut().set_action_label(QString::from(action_label));
    }

    pub fn scan(mut self: Pin<&mut Self>) {
        self.as_mut().sync_destination();
        self.as_mut().rust_mut().controller.scan();
        self.poll();
    }

    pub fn check(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().controller.check(true);
        self.poll();
    }

    pub fn advance(mut self: Pin<&mut Self>) {
        self.as_mut().sync_destination();
        let state = &mut self.as_mut().rust_mut().controller;
        match state.page {
            Page::Welcome if !state.scanning => state.page = Page::Location,
            Page::Location
                if state.latest.is_some()
                    && !state.checking
                    && !state.scanning
                    && !state.destination.trim().is_empty() =>
            {
                state.install()
            }
            _ => {}
        }
        self.poll();
    }

    pub fn back(mut self: Pin<&mut Self>) {
        if self.rust().controller.page == Page::Location {
            self.as_mut().rust_mut().controller.page = Page::Welcome;
        }
        self.poll();
    }

    pub fn use_default(mut self: Pin<&mut Self>) {
        if let Ok(path) = swiftify_installer::install::install_path() {
            self.as_mut().rust_mut().controller.destination = path.display().to_string();
        }
        self.poll();
    }

    pub fn launch(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().controller.launch();
        self.poll();
    }

    pub fn open_folder(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().controller.open_folder();
        self.poll();
    }

    pub fn warn_close(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().controller.error =
            Some("Please wait for installation to finish before closing this window.".into());
        self.poll();
    }
}
