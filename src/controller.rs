use crate::{install, release};
use std::{
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc,
    },
};

enum Event {
    Detected(install::Detection),
    Checked(Result<release::ReleaseCheck, String>),
    Installed(Result<install::InstallOutcome, String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Location,
    Progress,
    Complete,
}

pub struct Controller {
    runtime: Arc<tokio::runtime::Runtime>,
    client: Option<reqwest::Client>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    progress_rx: Option<Receiver<install::Progress>>,
    pub installations: Vec<install::Installation>,
    pub latest: Option<release::Release>,
    pub release_notice: Option<String>,
    pub release_retry_at: Option<u64>,
    pub destination: String,
    pub page: Page,
    pub checking: bool,
    pub scanning: bool,
    pub installing: bool,
    pub status: String,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub invalid: Vec<String>,
    pub progress: Option<(f32, String)>,
    pub backup: Option<PathBuf>,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Tokio runtime"),
        );
        let client = release::client();
        let mut controller = Self {
            runtime,
            client: client.as_ref().ok().cloned(),
            tx,
            rx,
            progress_rx: None,
            installations: Vec::new(),
            latest: None,
            release_notice: None,
            release_retry_at: None,
            destination: install::install_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            page: Page::Welcome,
            checking: false,
            scanning: false,
            installing: false,
            status: "Looking for Swiftify…".into(),
            error: client.err(),
            warnings: Vec::new(),
            invalid: Vec::new(),
            progress: None,
            backup: None,
        };
        controller.scan();
        controller.check(false);
        controller
    }

    pub fn scan(&mut self) {
        if self.scanning || self.installing {
            return;
        }
        self.scanning = true;
        let additional = PathBuf::from(&self.destination);
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result =
                tokio::task::spawn_blocking(move || install::detect(Some(&additional))).await;
            let detected = result.unwrap_or_else(|error| install::Detection {
                installations: Vec::new(),
                invalid: Vec::new(),
                warnings: vec![format!("Detection failed: {error}")],
            });
            let _ = tx.send(Event::Detected(detected));
        });
    }

    pub fn check(&mut self, force: bool) {
        if self.checking || self.installing {
            return;
        }
        if force && self.release_retry_at.is_some_and(release::retry_pending) {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        self.checking = true;
        self.release_notice = None;
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let _ = tx.send(Event::Checked(release::latest(&client, force).await));
        });
    }

    pub fn selected(&self) -> Option<&install::Installation> {
        let path = PathBuf::from(&self.destination);
        self.installations.iter().find(|i| {
            i.directory
                .to_string_lossy()
                .eq_ignore_ascii_case(&path.to_string_lossy())
        })
    }

    pub fn action(&self) -> &'static str {
        match self.selected() {
            Some(installation) if installation.problem.is_some() => "Repair Swiftify",
            Some(installation)
                if installation.version.as_deref()
                    == self.latest.as_ref().map(|r| r.version.as_str()) =>
            {
                "Repair / reinstall"
            }
            Some(_) => "Update Swiftify",
            None => "Install Swiftify",
        }
    }

    pub fn install(&mut self) {
        let (Some(client), Some(release)) = (self.client.clone(), self.latest.clone()) else {
            return;
        };
        let target = PathBuf::from(self.destination.trim());
        self.destination = target.display().to_string();
        self.installing = true;
        self.error = None;
        self.backup = None;
        self.page = Page::Progress;
        self.status = "Preparing and validating the destination…".into();
        self.progress = None;
        let (progress_tx, progress_rx) = mpsc::channel();
        self.progress_rx = Some(progress_rx);
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let task = tokio::spawn(async move {
                install::install(&client, release, target, progress_tx).await
            });
            let result = task.await.unwrap_or_else(|e| {
                Err(format!(
                    "Installation task stopped: {e}. Retry to recover any pending update."
                ))
            });
            let _ = tx.send(Event::Installed(result));
        });
    }

    pub fn poll(&mut self) {
        if let Some(receiver) = &self.progress_rx {
            while let Ok(progress) = receiver.try_recv() {
                match progress {
                    install::Progress::Downloaded {
                        bytes,
                        total: Some(total),
                    } if total > 0 => {
                        self.progress = Some((
                            (bytes as f64 / total as f64).clamp(0.0, 1.0) as f32,
                            format!(
                                "{:.1} / {:.1} MB",
                                bytes as f64 / 1_000_000.0,
                                total as f64 / 1_000_000.0
                            ),
                        ));
                    }
                    install::Progress::Downloaded { .. } => {}
                    install::Progress::Extracted { files, total } => {
                        self.progress = Some((
                            files as f32 / total.max(1) as f32,
                            format!("{files} / {total} archive entries"),
                        ));
                    }
                    install::Progress::Message(message) => {
                        self.progress = None;
                        self.status = message;
                    }
                }
            }
        }
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Detected(result) => {
                    self.scanning = false;
                    self.installations = result.installations;
                    self.invalid = result.invalid;
                    self.warnings = result.warnings;
                }
                Event::Checked(result) => {
                    self.checking = false;
                    match result {
                        Ok(checked) => {
                            self.latest = Some(checked.release);
                            self.release_notice = checked.notice;
                            self.release_retry_at = checked.retry_at;
                            self.error = None;
                        }
                        Err(error) => {
                            self.latest = None;
                            self.release_notice = Some(error);
                            self.release_retry_at = release::saved_retry_at();
                        }
                    }
                }
                Event::Installed(result) => {
                    self.installing = false;
                    self.progress_rx = None;
                    self.progress = None;
                    match result {
                        Ok(outcome) => {
                            self.backup = outcome.backup;
                            self.error = outcome.warning;
                            self.status = "Swiftify has been installed and verified.".into();
                            self.page = Page::Complete;
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.status =
                                "Installation did not complete. Review the error before retrying."
                                    .into();
                            self.page = Page::Location;
                        }
                    }
                    self.scan();
                }
            }
        }
    }

    pub fn launch(&mut self) {
        if let Some(installed) = self.selected().filter(|i| i.problem.is_none()) {
            if let Err(error) = std::process::Command::new(&installed.executable)
                .current_dir(installed.executable.parent().unwrap())
                .spawn()
            {
                self.error = Some(format!("Could not launch Swiftify: {error}"));
            }
        }
    }

    pub fn open_folder(&mut self) {
        if let Err(error) = open::that(&self.destination) {
            self.error = Some(format!("Could not open folder: {error}"));
        }
    }
}
