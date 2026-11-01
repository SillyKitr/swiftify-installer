use std::path::Path;

use windows::{
    core::{w, Interface, HSTRING},
    Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        UI::Shell::{IShellLinkW, ShellLink},
    },
};

pub fn create(
    executable: &Path,
    icon: &Path,
    previous_managed: bool,
    backup: Option<&Path>,
) -> Result<Option<String>, String> {
    let appdata = std::env::var_os("APPDATA")
        .ok_or("APPDATA is not available; cannot create the Start Menu shortcut.")?;
    let start_menu = std::path::PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    std::fs::create_dir_all(&start_menu)
        .map_err(|e| format!("Could not access the Start Menu: {e}"))?;
    let default_executable = crate::install::install_path()?.join("bin/swiftify.exe");
    let name = if executable
        .to_string_lossy()
        .eq_ignore_ascii_case(&default_executable.to_string_lossy())
    {
        "Swiftify (Community).lnk".to_owned()
    } else {
        let id = crc32fast::hash(executable.to_string_lossy().to_lowercase().as_bytes());
        format!("Swiftify (Community - {id:08x}).lnk")
    };
    let destination = start_menu.join(name);
    create_at(executable, icon, &destination, previous_managed, backup)
}

fn create_at(
    executable: &Path,
    icon: &Path,
    destination: &Path,
    previous_managed: bool,
    backup: Option<&Path>,
) -> Result<Option<String>, String> {
    let existing = destination.symlink_metadata().is_ok();
    if existing {
        if destination.is_symlink() {
            return Ok(Some(format!(
                "The Start Menu shortcut at {} is a symbolic link; it was left untouched.",
                destination.display()
            )));
        }
        let (target, _, _) = read_link(destination)?;
        if !previous_managed || !target.eq_ignore_ascii_case(&executable.to_string_lossy()) {
            return Ok(Some(format!(
                "The existing Start Menu shortcut at {} points to {}. It was left untouched because the installer cannot verify ownership.",
                destination.display(),
                if target.is_empty() { "an unknown location" } else { &target }
            )));
        }
    }
    let working_dir = HSTRING::from(
        executable
            .parent()
            .ok_or("Invalid executable path.")?
            .as_os_str(),
    );
    let executable = HSTRING::from(executable.as_os_str());
    let icon = HSTRING::from(icon.as_os_str());
    let temporary =
        destination.with_file_name(format!(".Swiftify-shortcut-{}.lnk", uuid::Uuid::new_v4()));
    // The icon lives beside the managed installation, not in the removable installer executable.
    let result = unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| format!("Could not initialize Windows shortcuts: {e}"))?;
        let result = (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&executable)?;
            link.SetWorkingDirectory(&working_dir)?;
            link.SetIconLocation(&icon, 0)?;
            link.SetDescription(w!("Swiftify desktop client"))?;
            let file: IPersistFile = link.cast()?;
            file.Save(&HSTRING::from(temporary.as_os_str()), true)?;
            Ok(())
        })();
        CoUninitialize();
        result.map_err(|e| format!("Windows could not save the shortcut: {e}"))
    };
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }
    if existing {
        let Some(backup) = backup else {
            let _ = std::fs::remove_file(&temporary);
            return Err(
                "The existing shortcut cannot be replaced without a backup directory.".into(),
            );
        };
        let saved = backup.join(format!(".Swiftify-shortcut-{}.lnk", uuid::Uuid::new_v4()));
        if let Err(e) = std::fs::copy(destination, &saved) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("Could not preserve the previous shortcut: {e}"));
        }
    }
    let moved = std::fs::rename(&temporary, destination)
        .map_err(|e| format!("Could not place the Start Menu shortcut: {e}"));
    if moved.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    moved.map(|_| None)
}

fn read_link(destination: &Path) -> Result<(String, String, i32), String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| format!("Could not inspect the existing shortcut: {e}"))?;
        let result = (|| -> windows::core::Result<(String, String, i32)> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(
                &HSTRING::from(destination.as_os_str()),
                windows::Win32::System::Com::STGM_READ,
            )?;
            let mut target = vec![0_u16; 32768];
            link.GetPath(&mut target, std::ptr::null_mut(), 0)?;
            let mut icon = vec![0_u16; 32768];
            let mut index = -1;
            link.GetIconLocation(&mut icon, &mut index)?;
            let path = |buffer: &[u16]| {
                let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
                String::from_utf16_lossy(&buffer[..end])
            };
            Ok((path(&target), path(&icon), index))
        })();
        CoUninitialize();
        result.map_err(|e| format!("Could not inspect the existing shortcut: {e}"))
    }
}

#[cfg(test)]
fn matches_link(executable: &Path, icon: &Path, destination: &Path) -> Result<bool, String> {
    let (target, stored_icon, index) = read_link(destination)?;
    Ok(target.eq_ignore_ascii_case(&executable.to_string_lossy())
        && stored_icon.eq_ignore_ascii_case(&icon.to_string_lossy())
        && index == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_unowned_links_and_updates_only_matching_managed_links() {
        let dir =
            std::env::temp_dir().join(format!("swiftify-shortcut-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let destination = dir.join("Swiftify (Community).lnk");
        let executable = Path::new(r"C:\Windows\notepad.exe");
        let other = Path::new(r"C:\Windows\System32\cmd.exe");
        assert!(create_at(executable, executable, &destination, false, None)
            .unwrap()
            .is_none());
        assert!(matches_link(executable, executable, &destination).unwrap());
        let untouched = std::fs::read(&destination).unwrap();
        assert!(create_at(executable, other, &destination, false, None)
            .unwrap()
            .is_some());
        assert_eq!(std::fs::read(&destination).unwrap(), untouched);

        let backup = dir.join("backup");
        std::fs::create_dir(&backup).unwrap();
        assert!(create_at(other, other, &destination, true, Some(&backup))
            .unwrap()
            .is_some());
        assert_eq!(std::fs::read(&destination).unwrap(), untouched);
        assert!(
            create_at(executable, other, &destination, true, Some(&backup))
                .unwrap()
                .is_none()
        );
        assert!(matches_link(executable, other, &destination).unwrap());
        assert_eq!(std::fs::read_dir(&backup).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reports_unreadable_links_without_replacing_them() {
        let dir =
            std::env::temp_dir().join(format!("swiftify-shortcut-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let destination = dir.join("Swiftify (Community).lnk");
        std::fs::write(&destination, b"not a shell link").unwrap();
        let executable = Path::new(r"C:\Windows\notepad.exe");
        assert!(create_at(executable, executable, &destination, true, Some(&dir)).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"not a shell link");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
