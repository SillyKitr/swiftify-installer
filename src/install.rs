use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::mpsc::Sender,
};

use fs2::FileExt;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sysinfo::{ProcessesToUpdate, System};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use zip::ZipArchive;

use crate::release::Release;

const MAX_ARCHIVE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const MANIFEST: &str = ".swiftify-installer.json";
pub(crate) const SHORTCUT_ICON: &str = ".swiftify-installer.ico";

#[derive(Clone, Debug)]
pub struct Installation {
    pub directory: PathBuf,
    pub version: Option<String>,
    pub executable: PathBuf,
    pub problem: Option<String>,
    pub managed: bool,
}

#[derive(Default, Debug)]
pub struct Detection {
    pub installations: Vec<Installation>,
    pub invalid: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct InstallOutcome {
    pub backup: Option<PathBuf>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Progress {
    Downloaded { bytes: u64, total: Option<u64> },
    Message(String),
    Extracted { files: usize, total: usize },
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: String,
    executable: String,
    #[serde(default)]
    files: Vec<InstalledFile>,
}

#[derive(Serialize, Deserialize)]
struct InstalledFile {
    path: String,
    size: u64,
    crc32: u32,
}

pub fn install_path() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not available.")?;
    Ok(PathBuf::from(local).join("Programs").join("Swiftify"))
}

pub fn detect(additional: Option<&Path>) -> Detection {
    let mut result = Detection::default();
    let mut candidates = Vec::new();
    if let Ok(path) = install_path() {
        candidates.push(path);
    } else {
        result.warnings.push(
            "LOCALAPPDATA is unavailable; the default installation location could not be checked."
                .into(),
        );
    }
    let (paths, registry_warnings) = crate::registry::installation_paths();
    candidates.extend(paths);
    result.warnings.extend(registry_warnings);
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(base).join("Swiftify"));
        }
    }
    if let Some(path) = additional {
        candidates.push(path.to_path_buf());
    }
    let mut seen = HashSet::new();
    for path in candidates {
        let normalized = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(normalized.to_string_lossy().to_lowercase()) {
            continue;
        }
        if pending_path(&path).exists() {
            result.warnings.push(format!("An interrupted update is pending at {}. Select this location and retry installation to recover it.", path.display()));
        }
        match detect_at(&path) {
            Ok(Some(installation)) => result.installations.push(installation),
            Ok(None) => {
                if additional.is_some_and(|selected| selected == path) && path.is_dir() {
                    result.invalid.push(format!(
                        "{}: no recognized Swiftify executable or installer metadata.",
                        path.display()
                    ));
                }
            }
            Err(error) => result.invalid.push(format!("{}: {error}", path.display())),
        }
    }
    result
}

pub fn detect_at(path: &Path) -> Result<Option<Installation>, String> {
    if !path.exists() {
        return Ok(None);
    }
    reject_reparse(path)?;
    if path.join(MANIFEST).is_file() {
        let info = read_manifest(path)?;
        let executable = path.join(&info.executable);
        let problem = validate_layout(path, &executable)
            .and_then(|_| validate_inventory(path, &info.files))
            .err();
        return Ok(Some(Installation {
            directory: path.to_path_buf(),
            version: Some(info.version),
            executable,
            problem,
            managed: true,
        }));
    }
    // Only inspect the known release layouts; never recursively search arbitrary folders.
    let Some(executable) = find_executable(path)? else {
        return Ok(None);
    };
    validate_pe(&executable)?;
    let problem = validate_layout(path, &executable).err();
    Ok(Some(Installation {
        directory: path.to_path_buf(),
        version: executable_version(&executable),
        executable,
        problem,
        managed: false,
    }))
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let data = fs::read(path.join(MANIFEST))
        .map_err(|e| format!("Could not read installation metadata: {e}"))?;
    let info: Manifest = serde_json::from_slice(&data).map_err(|e| format!("Installation metadata is damaged: {e}. Restore its backup before replacing this directory."))?;
    if !safe_relative(Path::new(&info.executable))
        || info
            .files
            .iter()
            .any(|file| !safe_relative(Path::new(&file.path)))
    {
        return Err("Installation metadata contains unsafe file paths.".into());
    }
    Ok(info)
}

fn reject_reparse(path: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    let mut ancestor = Some(path);
    while let Some(path) = ancestor {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(format!(
                    "Refusing a symbolic link or junction: {}",
                    path.display()
                ));
            }
        }
        ancestor = path.parent();
    }
    Ok(())
}

fn validate_pe(path: &Path) -> Result<(), String> {
    reject_reparse(path)?;
    let check = || -> io::Result<()> {
        let mut file = File::open(path)?;
        let mut dos = [0_u8; 64];
        file.read_exact(&mut dos)?;
        if &dos[..2] != b"MZ" {
            return Err(io::Error::other("not a Windows executable"));
        }
        let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
        if offset > file.metadata()?.len().saturating_sub(6) {
            return Err(io::Error::other("invalid PE header"));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut pe = [0_u8; 6];
        file.read_exact(&mut pe)?;
        if &pe[..4] != b"PE\0\0" || pe[4..6] != [0x64, 0x86] {
            return Err(io::Error::other("not a Windows x86_64 PE file"));
        }
        Ok(())
    };
    check().map_err(|e| {
        format!(
            "Invalid or missing application file {}: {e}",
            path.display()
        )
    })
}

fn executable_version(path: &Path) -> Option<String> {
    use windows::{
        core::{w, HSTRING},
        Win32::Storage::FileSystem::{
            GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
        },
    };
    let name = HSTRING::from(path.as_os_str());
    unsafe {
        let size = GetFileVersionInfoSizeW(&name, None);
        if size == 0 || size > 1024 * 1024 {
            return None;
        }
        let mut data = vec![0_u32; (size as usize).div_ceil(4)];
        GetFileVersionInfoW(&name, None, size, data.as_mut_ptr().cast()).ok()?;
        let mut info = std::ptr::null_mut();
        let mut length = 0;
        if !VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut info, &mut length).as_bool()
            || info.is_null()
            || length < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
        {
            return None;
        }
        let info = std::ptr::read_unaligned(info.cast::<VS_FIXEDFILEINFO>());
        if info.dwSignature != 0xfeef04bd {
            return None;
        }
        Some(format!(
            "v{}.{}.{}",
            info.dwFileVersionMS >> 16,
            info.dwFileVersionMS & 0xffff,
            info.dwFileVersionLS >> 16
        ))
    }
}

fn validate_layout(root: &Path, executable: &Path) -> Result<(), String> {
    validate_pe(executable)?;
    let bin = executable.parent().ok_or("Invalid executable directory.")?;
    for dll in ["Qt6Core.dll", "Qt6Gui.dll", "Qt6Qml.dll", "Qt6Quick.dll"] {
        validate_pe(&bin.join(dll))?;
    }
    let platform = [
        bin.join("platforms/qwindows.dll"),
        bin.join("plugins/platforms/qwindows.dll"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or("The Windows Qt platform plugin is missing.")?;
    validate_pe(&platform)?;
    reject_reparse(root)
}

fn validate_inventory(root: &Path, files: &[InstalledFile]) -> Result<(), String> {
    for expected in files {
        let path = root.join(&expected.path);
        reject_reparse(&path)?;
        let mut file = File::open(&path)
            .map_err(|e| format!("Missing or unreadable file {}: {e}", expected.path))?;
        if file.metadata().map_err(|e| e.to_string())?.len() != expected.size {
            return Err(format!("File is damaged: {}", expected.path));
        }
        let mut hash = crc32fast::Hasher::new();
        let mut buffer = [0_u8; 65536];
        loop {
            let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        if hash.finalize() != expected.crc32 {
            return Err(format!("File is damaged or modified: {}", expected.path));
        }
    }
    Ok(())
}

fn pending_path(target: &Path) -> PathBuf {
    target.with_file_name(format!(
        ".{}-backup-pending",
        target.file_name().unwrap_or_default().to_string_lossy()
    ))
}

pub async fn install(
    client: &reqwest::Client,
    release: Release,
    target: PathBuf,
    sender: Sender<Progress>,
) -> Result<InstallOutcome, String> {
    validate_destination(&target)?;
    let parent = target.parent().ok_or("Invalid installation directory.")?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| format!("Could not create the Programs directory: {e}"))?;
    let lock = installation_lock(&target)?;
    recover_pending(&target, &pending_path(&target))?;

    let id = Uuid::new_v4();
    let workspace = parent.join(format!(".Swiftify-work-{id}"));
    tokio::fs::create_dir(&workspace)
        .await
        .map_err(|e| format!("Could not create a temporary workspace: {e}"))?;
    let archive = workspace.join("release.zip");
    let stage = workspace.join("stage");
    let backup = parent.join(format!(".Swiftify-backup-{id}"));
    let result = async {
        if release.size > MAX_ARCHIVE {
            return Err("The release archive exceeds the 2 GB safety limit.".into());
        }
        let _ = sender.send(Progress::Message(format!(
            "Downloading {}",
            release.asset_name
        )));
        download(client, &release, &archive, &sender).await?;
        let _ = sender.send(Progress::Message(
            "Validating and extracting the archive…".into(),
        ));
        let version = release.version.clone();
        let path = archive.clone();
        let extracted = stage.clone();
        let extraction_progress = sender.clone();
        tokio::task::spawn_blocking(move || {
            extract(&path, &extracted, &version, Some(&extraction_progress))
        })
        .await
        .map_err(|e| format!("Extraction task failed: {e}"))??;
        let _ = sender.send(Progress::Message("Installing Swiftify…".into()));
        let staged = stage.clone();
        tokio::task::spawn_blocking(move || commit(&target, &staged, &backup))
            .await
            .map_err(|e| format!("Installation task failed: {e}"))?
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&workspace).await;
    let _ = FileExt::unlock(&lock);
    result
}

async fn download(
    client: &reqwest::Client,
    release: &Release,
    path: &Path,
    sender: &Sender<Progress>,
) -> Result<(), String> {
    let url =
        url::Url::parse(&release.download_url).map_err(|e| format!("Invalid release URL: {e}"))?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err("The release download URL is not a GitHub HTTPS URL.".into());
    }
    let response = tokio::time::timeout(std::time::Duration::from_secs(30), client.get(url).send())
        .await
        .map_err(|_| "The download server did not respond within 30 seconds.".to_owned())?
        .map_err(|e| format!("Could not download Swiftify: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Release download failed: {e}"))?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_ARCHIVE)
    {
        return Err("The release archive exceeds the 2 GB safety limit.".into());
    }
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("Could not create the download file: {e}"))?;
    let mut received = 0_u64;
    let total = Some(release.size);
    while let Some(chunk) = tokio::time::timeout(std::time::Duration::from_secs(60), stream.next())
        .await
        .map_err(|_| "The download stalled for over 60 seconds.".to_owned())?
    {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or("Download size overflow.")?;
        if received > MAX_ARCHIVE || received > release.size {
            return Err("The download is larger than the release asset declares.".into());
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Could not write the download: {e}"))?;
        let _ = sender.send(Progress::Downloaded {
            bytes: received,
            total,
        });
    }
    file.sync_all()
        .await
        .map_err(|e| format!("Could not finish the download: {e}"))?;
    if received != release.size {
        return Err(format!(
            "Download incomplete: received {received} of {} bytes.",
            release.size
        ));
    }
    Ok(())
}

fn extract(
    archive: &Path,
    stage: &Path,
    version: &str,
    progress: Option<&Sender<Progress>>,
) -> Result<(), String> {
    fs::create_dir(stage).map_err(|e| format!("Could not prepare installation: {e}"))?;
    let file =
        File::open(archive).map_err(|e| format!("Could not open the downloaded archive: {e}"))?;
    let mut zip = ZipArchive::new(file).map_err(|e| format!("Invalid ZIP archive: {e}"))?;
    if zip.len() > MAX_FILES {
        return Err("The archive contains too many files.".into());
    }
    let mut paths = Vec::with_capacity(zip.len());
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|e| format!("Could not inspect the archive: {e}"))?;
        let name = entry.name().replace('\\', "/");
        let relative = PathBuf::from(name.trim_end_matches('/'));
        if !safe_relative(&relative) || relative.to_string_lossy().eq_ignore_ascii_case(MANIFEST) {
            return Err(format!("Unsafe path in release archive: {}", entry.name()));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(
                "The archive contains a symbolic link, which cannot be installed safely.".into(),
            );
        }
        total = total
            .checked_add(entry.size())
            .ok_or("Archive size overflow.")?;
        if total > MAX_UNPACKED {
            return Err("The extracted archive exceeds the 4 GB safety limit.".into());
        }
        paths.push((relative, entry.is_dir()));
    }
    let first = paths.first().and_then(|(path, _)| path.components().next());
    let wrapper = if paths.iter().all(|(path, is_dir)| {
        path.components().next() == first && (*is_dir || path.components().count() > 1)
    }) {
        first.map(|c| c.as_os_str().to_os_string())
    } else {
        None
    };
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for (index, (relative, is_dir)) in paths.iter().enumerate() {
        let relative = if wrapper.is_some() {
            relative.components().skip(1).collect::<PathBuf>()
        } else {
            relative.clone()
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        if relative.to_string_lossy().eq_ignore_ascii_case(MANIFEST)
            || relative
                .to_string_lossy()
                .eq_ignore_ascii_case(SHORTCUT_ICON)
        {
            return Err("The release archive conflicts with installer metadata.".into());
        }
        let key = relative.to_string_lossy().to_ascii_lowercase();
        if !seen.insert(key) {
            return Err(format!(
                "Duplicate path in release archive: {}",
                relative.display()
            ));
        }
        let dest = stage.join(&relative);
        if *is_dir {
            fs::create_dir_all(&dest)
                .map_err(|e| format!("Could not create an extracted directory: {e}"))?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Could not create an extracted directory: {e}"))?;
            }
            let mut input = zip
                .by_index(index)
                .map_err(|e| format!("Could not read the archive: {e}"))?;
            let mut output = File::create(&dest)
                .map_err(|e| format!("Could not extract {}: {e}", relative.display()))?;
            let limit = input.size();
            let written = io::copy(&mut (&mut input).take(limit.saturating_add(1)), &mut output)
                .map_err(|e| {
                    format!("Archive validation failed for {}: {e}", relative.display())
                })?;
            if written != input.size() {
                return Err("An extracted file has the wrong size.".into());
            }
            output
                .sync_all()
                .map_err(|e| format!("Could not finish extracting a file: {e}"))?;
            files.push(InstalledFile {
                path: relative.to_string_lossy().into_owned(),
                size: written,
                crc32: input.crc32(),
            });
        }
        if let Some(progress) = progress {
            let _ = progress.send(Progress::Extracted {
                files: index + 1,
                total: paths.len(),
            });
        }
    }
    let executable =
        find_executable(stage)?.ok_or("The archive has no Swiftify.exe executable.")?;
    validate_layout(stage, &executable)?;
    let relative = executable
        .strip_prefix(stage)
        .map_err(|_| "Invalid executable path.")?;
    let icon = include_bytes!("../assets/installer.ico");
    fs::write(stage.join(SHORTCUT_ICON), icon)
        .map_err(|e| format!("Could not stage the shortcut icon: {e}"))?;
    files.push(InstalledFile {
        path: SHORTCUT_ICON.into(),
        size: icon.len() as u64,
        crc32: crc32fast::hash(icon),
    });
    let manifest = Manifest {
        version: version.into(),
        executable: relative.to_string_lossy().into_owned(),
        files,
    };
    let data = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| format!("Could not write installation metadata: {e}"))?;
    fs::write(stage.join(MANIFEST), data)
        .map_err(|e| format!("Could not write installation metadata: {e}"))?;
    Ok(())
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|component| match component {
            Component::Normal(part) => {
                let s = part.to_string_lossy();
                !s.chars()
                    .any(|c| matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
                    && !s.ends_with(['.', ' '])
                    && !s.chars().any(|c| c.is_control())
                    && !matches!(
                        s.split('.')
                            .next()
                            .unwrap_or("")
                            .to_ascii_uppercase()
                            .as_str(),
                        "CON"
                            | "PRN"
                            | "AUX"
                            | "NUL"
                            | "COM1"
                            | "COM2"
                            | "COM3"
                            | "COM4"
                            | "COM5"
                            | "COM6"
                            | "COM7"
                            | "COM8"
                            | "COM9"
                            | "LPT1"
                            | "LPT2"
                            | "LPT3"
                            | "LPT4"
                            | "LPT5"
                            | "LPT6"
                            | "LPT7"
                            | "LPT8"
                            | "LPT9"
                    )
            }
            _ => false,
        })
}

fn find_executable(path: &Path) -> Result<Option<PathBuf>, String> {
    let candidates = [path.join("bin/swiftify.exe"), path.join("swiftify.exe")];
    let mut found = candidates.into_iter().filter(|path| path.is_file());
    let executable = found.next();
    if found.next().is_some() {
        return Err(
            "Multiple Swiftify executables found; select an unambiguous installation folder."
                .into(),
        );
    }
    Ok(executable)
}

pub fn validate_destination(target: &Path) -> Result<(), String> {
    if !target.is_absolute()
        || target.file_name().is_none()
        || !safe_relative(Path::new(target.file_name().unwrap_or_default()))
    {
        return Err("Choose an absolute installation folder, not a drive root.".into());
    }
    reject_reparse(target)?;
    if target.exists()
        && detect_at(target)?.is_none()
        && fs::read_dir(target)
            .map_err(|e| format!("Could not inspect the destination: {e}"))?
            .next()
            .is_some()
    {
        return Err("This folder contains unrelated files and is not a recognized Swiftify installation. Choose an empty folder.".into());
    }
    Ok(())
}

fn preserve_untracked(target: &Path, stage: &Path) -> Result<(), String> {
    let tracked: HashSet<String> = if target.join(MANIFEST).exists() {
        read_manifest(target)?
            .files
            .into_iter()
            .map(|file| file.path.replace('\\', "/").to_lowercase())
            .collect()
    } else {
        HashSet::new()
    };
    let legacy = tracked.is_empty();
    let mut directories = vec![target.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            reject_reparse(&path)?;
            let relative = path.strip_prefix(target).map_err(|e| e.to_string())?;
            if relative == Path::new(MANIFEST) {
                continue;
            }
            let destination = stage.join(relative);
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                if destination.is_file() {
                    return Err(format!(
                        "A user directory conflicts with a release file: {}",
                        relative.display()
                    ));
                }
                fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
                directories.push(path);
            } else if !tracked
                .contains(&relative.to_string_lossy().replace('\\', "/").to_lowercase())
            {
                if destination.exists() {
                    if legacy {
                        continue;
                    }
                    return Err(format!("An untracked user file conflicts with the release: {}. Move it aside before updating.", relative.display()));
                }
                fs::copy(&path, &destination)
                    .map_err(|e| format!("Could not preserve {}: {e}", relative.display()))?;
            }
        }
    }
    Ok(())
}

fn installation_lock(target: &Path) -> Result<File, String> {
    let parent = target.parent().ok_or("Invalid installation directory.")?;
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(format!(
            ".{}-installer.lock",
            target.file_name().unwrap_or_default().to_string_lossy()
        )))
        .map_err(|e| format!("Could not open the installation lock: {e}"))?;
    lock.try_lock_exclusive().map_err(|_| {
        "Another Swiftify installation or update is already in progress.".to_owned()
    })?;
    Ok(lock)
}

fn recover_pending(target: &Path, pending: &Path) -> Result<(), String> {
    if pending.exists() {
        if swiftify_running(target) {
            return Err("Close Swiftify before recovering the interrupted update.".into());
        }
        reject_reparse(pending)?;
        if detect_at(pending)?.is_none() {
            return Err("The interrupted-update backup is not a recognized Swiftify installation; it was left untouched.".into());
        }
        if target.exists() {
            let interrupted =
                target.with_file_name(format!(".Swiftify-interrupted-{}", Uuid::new_v4()));
            fs::rename(target, &interrupted)
                .map_err(|e| format!("Could not recover an interrupted update: {e}"))?;
        }
        fs::rename(pending, target).map_err(|e| {
            format!(
                "Could not restore the previous installation from {}: {e}",
                pending.display()
            )
        })?;
    }
    Ok(())
}

fn swiftify_running(target: &Path) -> bool {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system.processes().values().any(|process| {
        process.name().eq_ignore_ascii_case("Swiftify.exe")
            && process.exe().is_none_or(|executable| {
                let root = fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
                let executable =
                    fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
                executable.to_string_lossy().to_lowercase().starts_with(
                    &(root.to_string_lossy().trim_end_matches('\\').to_lowercase() + "\\"),
                )
            })
    })
}

fn commit(target: &Path, stage: &Path, backup: &Path) -> Result<InstallOutcome, String> {
    if swiftify_running(target) {
        return Err("Close Swiftify before installing or updating it, then try again.".into());
    }
    let previous = target.exists();
    let pending = pending_path(target);
    let previously_managed = target.join(MANIFEST).exists();
    validate_destination(target)?;
    let staged = detect_at(stage)?.ok_or("Staged installation is missing.")?;
    if let Some(problem) = staged.problem {
        return Err(problem);
    }
    if previous {
        preserve_untracked(target, stage)?;
        if swiftify_running(target) {
            return Err("Swiftify started while preparing the update. Close it and retry.".into());
        }
        fs::rename(target, &pending)
            .map_err(|e| format!("Could not move the existing installation to safety: {e}"))?;
    }
    if let Err(e) = fs::rename(stage, target) {
        if previous {
            if let Err(restore) = fs::rename(&pending, target) {
                return Err(format!("Could not place the new installation ({e}); restoring the previous installation also failed ({restore}). It remains at {}.", pending.display()));
            }
        }
        return Err(format!(
            "Could not place the new installation: {e}. The previous installation is unchanged."
        ));
    }
    // Finalize the backup before touching the shortcut: pending recovery can still
    // restore the old version without having to repair an activated shell link.
    if previous {
        if let Err(e) = fs::rename(&pending, backup) {
            return Err(format!("Swiftify was installed, but its backup could not be moved ({e}). The previous version is preserved at {}. Restart the installer to recover it before updating again.", pending.display()));
        }
    }
    let result = detect_at(target)
        .and_then(|installed| installed.ok_or("Installed executable disappeared.".to_owned()))
        .and_then(|installed| installed.problem.map_or(Ok(()), Err));
    if let Err(e) = result {
        if previous {
            if let Err(move_error) = fs::rename(target, stage) {
                return Err(format!("Could not finish installation ({e}); rollback could not move the new installation ({move_error}). The previous version remains at {}.", backup.display()));
            }
            if let Err(restore) = fs::rename(backup, target) {
                return Err(format!("Could not finish installation ({e}); restoring the previous version failed ({restore}). It remains at {}.", backup.display()));
            }
            let _ = fs::remove_dir_all(stage);
        } else if let Err(remove) = fs::remove_dir_all(target) {
            return Err(format!("Could not finish installation ({e}); removing the incomplete installation also failed ({remove}). Check {}.", target.display()));
        }
        return Err(format!(
            "Could not finish installation: {e}. The installation was rolled back."
        ));
    }
    let installed = detect_at(target)?.ok_or("The installed application could not be verified.")?;
    let shortcut_warning = match crate::shortcut::create(
        &installed.executable,
        &target.join(SHORTCUT_ICON),
        previously_managed,
        previous.then_some(backup),
    ) {
        Ok(notice) => notice,
        Err(error) => Some(format!("Could not create the Start Menu shortcut: {error}")),
    };
    let registration_warning = crate::registry::register(
        target,
        installed.version.as_deref().unwrap_or("Unknown"),
        &installed.executable,
    )
    .err();
    Ok(InstallOutcome {
        backup: previous.then(|| backup.to_path_buf()),
        warning: shortcut_warning
            .into_iter()
            .chain(registration_warning)
            .reduce(|a, b| format!("{a} {b}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn pe_fixture() -> Vec<u8> {
        let mut bytes = vec![0_u8; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64_u32.to_le_bytes());
        bytes[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        bytes
    }

    fn layout_fixture(path: &Path) {
        fs::create_dir_all(path.join("bin/platforms")).unwrap();
        for name in [
            "swiftify.exe",
            "Qt6Core.dll",
            "Qt6Gui.dll",
            "Qt6Qml.dll",
            "Qt6Quick.dll",
            "platforms/qwindows.dll",
        ] {
            fs::write(path.join("bin").join(name), pe_fixture()).unwrap();
        }
    }

    #[test]
    fn rejects_paths_that_escape_or_use_windows_devices() {
        assert!(safe_relative(Path::new("app\\Swiftify.exe")));
        for path in [
            "..\\evil",
            "C:\\evil",
            "app\\CON.txt",
            "app\\name.",
            "app\\bad:name",
        ] {
            assert!(!safe_relative(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn extracts_versioned_windows_release_layout() {
        let root = std::env::temp_dir().join(format!("swiftify-test-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let archive = root.join("release.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        writer
            .add_directory("swiftify-0.25.1-windows-x86_64/", options)
            .unwrap();
        writer
            .start_file("swiftify-0.25.1-windows-x86_64/bin/swiftify.exe", options)
            .unwrap();
        writer.write_all(&pe_fixture()).unwrap();
        for name in [
            "Qt6Core.dll",
            "Qt6Gui.dll",
            "Qt6Qml.dll",
            "Qt6Quick.dll",
            "platforms/qwindows.dll",
        ] {
            writer
                .start_file(
                    format!("swiftify-0.25.1-windows-x86_64/bin/{name}"),
                    options,
                )
                .unwrap();
            writer.write_all(&pe_fixture()).unwrap();
        }
        writer
            .start_file("swiftify-0.25.1-windows-x86_64/share/README.txt", options)
            .unwrap();
        writer.write_all(b"share data").unwrap();
        writer.finish().unwrap();
        let stage = root.join("stage");
        extract(&archive, &stage, "v0.25.1", None).unwrap();
        let installed = detect_at(&stage).unwrap().unwrap();
        assert_eq!(installed.version.as_deref(), Some("v0.25.1"));
        assert_eq!(installed.executable, stage.join("bin/swiftify.exe"));
        assert!(stage.join("share/README.txt").is_file());
        assert_eq!(
            fs::read(stage.join(SHORTCUT_ICON)).unwrap(),
            include_bytes!("../assets/installer.ico")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_installation_with_missing_executable_is_repairable() {
        let root = std::env::temp_dir().join(format!("swiftify-repair-{}", Uuid::new_v4()));
        layout_fixture(&root);
        fs::write(
            root.join(MANIFEST),
            br#"{"version":"v1.0","executable":"bin/swiftify.exe"}"#,
        )
        .unwrap();
        fs::remove_file(root.join("bin/swiftify.exe")).unwrap();
        let detected = detect_at(&root).unwrap().unwrap();
        assert!(detected.managed);
        assert!(detected.problem.is_some());
        assert_eq!(detected.version.as_deref(), Some("v1.0"));
        assert!(validate_destination(&root).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn distinguishes_missing_invalid_and_repairable_unmanaged_installations() {
        let root = std::env::temp_dir().join(format!("swiftify-detection-{}", Uuid::new_v4()));
        assert!(detect_at(&root).unwrap().is_none());
        layout_fixture(&root);
        let found = detect_at(&root).unwrap().unwrap();
        assert!(!found.managed);
        assert!(found.problem.is_none());
        fs::remove_file(root.join("bin/platforms/qwindows.dll")).unwrap();
        assert!(detect_at(&root).unwrap().unwrap().problem.is_some());
        fs::write(root.join("bin/swiftify.exe"), b"not a Windows executable").unwrap();
        assert!(detect_at(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_unrelated_folder_and_fake_executable() {
        let root = std::env::temp_dir().join(format!("swiftify-unrelated-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("notes.txt"), b"user data").unwrap();
        assert!(detect_at(&root).unwrap().is_none());
        assert!(validate_destination(&root).is_err());
        fs::write(root.join("bin/swiftify.exe"), b"not an executable").unwrap();
        assert!(detect_at(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_user_files_and_refuses_release_collisions() {
        let root = std::env::temp_dir().join(format!("swiftify-preserve-{}", Uuid::new_v4()));
        let target = root.join("target");
        let stage = root.join("stage");
        layout_fixture(&target);
        layout_fixture(&stage);
        let tracked = [
            "swiftify.exe",
            "Qt6Core.dll",
            "Qt6Gui.dll",
            "Qt6Qml.dll",
            "Qt6Quick.dll",
            "platforms/qwindows.dll",
        ]
        .into_iter()
        .map(|path| InstalledFile {
            path: format!("bin/{path}"),
            size: 128,
            crc32: crc32fast::hash(&pe_fixture()),
        })
        .collect();
        let manifest = Manifest {
            version: "v1".into(),
            executable: "bin/swiftify.exe".into(),
            files: tracked,
        };
        fs::write(
            target.join(MANIFEST),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(target.join("user.ini"), b"settings").unwrap();
        preserve_untracked(&target, &stage).unwrap();
        assert_eq!(fs::read(stage.join("user.ini")).unwrap(), b"settings");
        assert!(preserve_untracked(&target, &stage).is_err());
        assert_eq!(fs::read(target.join("user.ini")).unwrap(), b"settings");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_damaged_files_even_when_the_size_is_unchanged() {
        let root = std::env::temp_dir().join(format!("swiftify-integrity-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), b"good").unwrap();
        let files = [InstalledFile {
            path: "file".into(),
            size: 4,
            crc32: crc32fast::hash(b"good"),
        }];
        validate_inventory(&root, &files).unwrap();
        fs::write(root.join("file"), b"evil").unwrap();
        assert!(validate_inventory(&root, &files).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restores_pending_update_after_interruption() {
        let root = std::env::temp_dir().join(format!("swiftify-recover-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let target = root.join("Swiftify");
        let pending = root.join(".Swiftify-backup-pending");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&pending).unwrap();
        fs::write(target.join("new"), b"new").unwrap();
        layout_fixture(&pending);
        fs::write(pending.join("old"), b"old").unwrap();
        recover_pending(&target, &pending).unwrap();
        assert!(target.join("old").is_file());
        assert!(!pending.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
