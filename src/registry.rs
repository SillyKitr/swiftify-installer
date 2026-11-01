//! Native per-user installer registration. The MVP deliberately owns one fixed uninstall key:
//! a second location must be unregistered explicitly rather than replacing the first silently.
//! No upstream Swiftify registration is ever modified.

use std::{
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND,
            ERROR_UNSUPPORTED_TYPE, WIN32_ERROR,
        },
        System::Registry::{
            RegCloseKey, RegCreateKeyW, RegDeleteKeyW, RegDeleteValueW, RegEnumKeyExW,
            RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
            HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
            REG_DWORD, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
        },
    },
};

const UNINSTALL_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\SwiftifyCommunity";
const SETTINGS_KEY: &str = r"Software\SwiftifyCommunityInstaller";
const DISPLAY_NAME: &str = "Swiftify (Community)";
const PUBLISHER: &str = "Independent community utility";
const ABOUT_URL: &str = "https://github.com/swiftify-org/swiftify";
const MAX_VALUE_BYTES: u32 = 1024 * 1024;

/// Find registry candidates, including the last successfully registered user-selected location.
/// These are hints, not proof of a valid installation; callers must inspect the files themselves.
/// Duplicate paths (including shared HKCU views) are returned only once.
pub fn installation_paths() -> (Vec<PathBuf>, Vec<String>) {
    let mut paths = Vec::new();
    let mut warnings = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            if let Err(error) = discover_at(hive, UNINSTALL_ROOT, view, &mut paths) {
                warnings.push(error);
            }
        }
    }
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        let saved = Key::open(HKEY_CURRENT_USER, SETTINGS_KEY, KEY_READ | view).and_then(|key| {
            key.map(|key| key.string("InstallLocation"))
                .transpose()
                .map(Option::flatten)
        });
        match saved {
            Ok(Some(location)) => push_location(&mut paths, location),
            Ok(None) => {}
            Err(error) => warnings.push(error),
        }
    }
    (paths, warnings)
}

/// Register without advertising an uninstaller until the installer actually implements one.
pub fn register(target: &Path, version: &str, executable: &Path) -> Result<(), String> {
    register_with_uninstaller(target, version, executable, None)
}

/// An optional uninstaller must already be a real file. Its quoted path is advertised without
/// invented switches; the caller is responsible for supplying a functioning uninstaller.
pub fn register_with_uninstaller(
    target: &Path,
    version: &str,
    executable: &Path,
    uninstaller: Option<&Path>,
) -> Result<(), String> {
    register_at(
        HKEY_CURRENT_USER,
        UNINSTALL_KEY,
        SETTINGS_KEY,
        target,
        version,
        executable,
        uninstaller,
    )
}

/// Remove only our own registration for this location, and clear only a matching saved location.
/// Missing records are a successful no-op; a different installation is left untouched.
#[allow(dead_code)] // Reserved for the deferred clean-uninstall flow.
pub fn unregister(target: &Path) -> Result<(), String> {
    unregister_at(HKEY_CURRENT_USER, UNINSTALL_KEY, SETTINGS_KEY, target)
}

// Owned handles only: predefined hive handles must never be wrapped in Key.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn wide(value: &OsStr) -> Result<Vec<u16>, String> {
    let mut result: Vec<u16> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err("A registry name or value contains an embedded NUL character.".into());
    }
    result.push(0);
    Ok(result)
}

fn missing(status: WIN32_ERROR) -> bool {
    status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND
}

fn checked(status: WIN32_ERROR, action: &str) -> Result<(), String> {
    status.ok().map_err(|e| format!("Could not {action}: {e}"))
}

impl Key {
    fn open(hive: HKEY, path: &str, access: REG_SAM_FLAGS) -> Result<Option<Self>, String> {
        let name = wide(OsStr::new(path))?;
        let mut handle = HKEY::default();
        let status =
            unsafe { RegOpenKeyExW(hive, PCWSTR(name.as_ptr()), None, access, &mut handle) };
        if missing(status) {
            return Ok(None);
        }
        checked(status, &format!("open registry key {path}"))?;
        Ok(Some(Self(handle)))
    }

    fn create(hive: HKEY, path: &str) -> Result<Self, String> {
        let name = wide(OsStr::new(path))?;
        let mut handle = HKEY::default();
        // RegCreateKeyW uses the process's default view, consistently with registration/removal.
        // Unlike RegCreateKeyExW, it needs no additional Win32_Security crate feature.
        checked(
            unsafe { RegCreateKeyW(hive, PCWSTR(name.as_ptr()), &mut handle) },
            &format!("create registry key {path}"),
        )?;
        Ok(Self(handle))
    }

    fn string(&self, name: &str) -> Result<Option<OsString>, String> {
        let value_name = wide(OsStr::new(name))?;
        let mut bytes = 0;
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        let status = unsafe {
            RegGetValueW(
                self.0,
                PCWSTR::null(),
                PCWSTR(value_name.as_ptr()),
                flags,
                None,
                None,
                Some(&mut bytes),
            )
        };
        if missing(status) || status == ERROR_UNSUPPORTED_TYPE {
            return Ok(None);
        }
        checked(status, &format!("size registry value {name}"))?;
        // Registry writers can change a value between sizing and reading. Retry with a bound,
        // and allocate UTF-16 storage so the native buffer is correctly aligned.
        for _ in 0..8 {
            if bytes > MAX_VALUE_BYTES {
                return Err(format!(
                    "Registry value {name} exceeds the 1 MiB safety limit."
                ));
            }
            let mut buffer = vec![0_u16; (bytes as usize).div_ceil(2).max(1)];
            let mut actual = (buffer.len() * 2) as u32;
            let status = unsafe {
                RegGetValueW(
                    self.0,
                    PCWSTR::null(),
                    PCWSTR(value_name.as_ptr()),
                    flags,
                    None,
                    Some(buffer.as_mut_ptr().cast()),
                    Some(&mut actual),
                )
            };
            if status == ERROR_MORE_DATA {
                bytes = actual;
                continue;
            }
            if missing(status) || status == ERROR_UNSUPPORTED_TYPE {
                return Ok(None);
            }
            checked(status, &format!("read registry value {name}"))?;
            if !actual.is_multiple_of(2) || actual as usize > buffer.len() * 2 {
                return Err(format!(
                    "Registry value {name} is not a valid UTF-16 string."
                ));
            }
            buffer.truncate(actual as usize / 2);
            // RegGetValueW guarantees a terminator. Expansion may leave bytes after it
            // inside the reported capacity, so follow native NUL-terminated semantics.
            let end = buffer
                .iter()
                .position(|&word| word == 0)
                .unwrap_or(buffer.len());
            return Ok(Some(OsString::from_wide(&buffer[..end])));
        }
        Err(format!(
            "Registry value {name} kept changing while being read."
        ))
    }

    fn set_string(&self, name: &str, value: &OsStr) -> Result<(), String> {
        let wide_value = wide(value)?;
        let bytes: Vec<u8> = wide_value
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        self.set(name, REG_SZ, &bytes)
    }

    fn set(&self, name: &str, kind: REG_VALUE_TYPE, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_VALUE_BYTES as usize {
            return Err(format!(
                "Registry value {name} exceeds the 1 MiB safety limit."
            ));
        }
        let name_wide = wide(OsStr::new(name))?;
        checked(
            unsafe { RegSetValueExW(self.0, PCWSTR(name_wide.as_ptr()), None, kind, Some(bytes)) },
            &format!("write registry value {name}"),
        )
    }

    fn delete_value(&self, name: &str) -> Result<(), String> {
        let value_name = wide(OsStr::new(name))?;
        let status = unsafe { RegDeleteValueW(self.0, PCWSTR(value_name.as_ptr())) };
        if missing(status) {
            return Ok(());
        }
        checked(status, &format!("remove registry value {name}"))
    }
}

fn discover_at(
    hive: HKEY,
    root: &str,
    view: REG_SAM_FLAGS,
    paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let Some(key) = Key::open(hive, root, KEY_READ | view)? else {
        return Ok(());
    };
    let mut index = 0;
    loop {
        // A registry subkey name has at most 255 UTF-16 code units (plus terminator).
        let mut name = [0_u16; 256];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        checked(status, &format!("enumerate registry key {root}"))?;
        index += 1;
        let name = String::from_utf16(&name[..length as usize])
            .map_err(|_| format!("Registry key {root} contains an invalid Unicode subkey name."))?;
        let Some(entry) = Key::open(key.0, &name, KEY_READ | view)? else {
            continue; // Removed concurrently.
        };
        let display_name = entry.string("DisplayName")?;
        if !display_name.as_deref().is_some_and(swiftify_display_name) {
            continue;
        }
        if let Some(location) = entry.string("InstallLocation")? {
            push_location(paths, location);
        } else if let Some(icon) = entry.string("DisplayIcon")? {
            if let Some(location) = icon_location(&icon) {
                push_location(paths, location.into_os_string());
            }
        }
    }
    Ok(())
}

fn swiftify_display_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.eq_ignore_ascii_case("Swiftify")
        || name.eq_ignore_ascii_case(DISPLAY_NAME)
        || name
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Swiftify "))
            && name[9..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c == 'v')
}

fn icon_location(icon: &OsStr) -> Option<PathBuf> {
    let icon = icon.to_string_lossy();
    let executable = Path::new(
        icon.trim()
            .strip_suffix(",0")
            .unwrap_or(icon.trim())
            .trim_matches('"'),
    );
    if !executable.is_absolute()
        || !executable
            .file_name()?
            .to_string_lossy()
            .eq_ignore_ascii_case("swiftify.exe")
    {
        return None;
    }
    let parent = executable.parent()?;
    Some(
        if parent
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("bin"))
        {
            parent.parent()?.to_path_buf()
        } else {
            parent.to_path_buf()
        },
    )
}

fn same_path(left: &Path, right: &Path) -> bool {
    // Compare components to ignore redundant separators / trailing slashes. ASCII folding covers
    // drive letters and conventional Windows paths without lossily conflating distinct Unicode.
    // Ambiguous aliases are intentionally not treated as ownership of another registration.
    let left: PathBuf = left.components().collect();
    let right: PathBuf = right.components().collect();
    left.as_os_str()
        .as_encoded_bytes()
        .eq_ignore_ascii_case(right.as_os_str().as_encoded_bytes())
}

fn push_location(paths: &mut Vec<PathBuf>, location: OsString) {
    let path = PathBuf::from(location);
    if path.is_absolute() && !paths.iter().any(|previous| same_path(previous, &path)) {
        paths.push(path);
    }
}

fn ensure_owned(key: &Key, target: &Path) -> Result<(), String> {
    if key.string("DisplayName")?.as_deref() != Some(OsStr::new(DISPLAY_NAME))
        || key.string("Publisher")?.as_deref() != Some(OsStr::new(PUBLISHER))
    {
        return Err("The SwiftifyCommunity uninstall key is not an owned community registration; it was left untouched.".into());
    }
    let location = key.string("InstallLocation")?.ok_or(
        "The existing community registration has no valid InstallLocation; it was left untouched.",
    )?;
    if !same_path(Path::new(&location), target) {
        return Err(format!(
            "Swiftify (Community) is already registered at {}. Unregister that location first; registering {} would replace it. Multiple registered locations are not supported yet.",
            Path::new(&location).display(), target.display()
        ));
    }
    Ok(())
}

fn validate_file(path: &Path, description: &str) -> Result<(), String> {
    if !path.is_absolute() || !path.is_file() {
        return Err(format!(
            "The {description} must be an existing absolute file path: {}",
            path.display()
        ));
    }
    wide(path.as_os_str())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn register_at(
    hive: HKEY,
    uninstall_key: &str,
    settings_key: &str,
    target: &Path,
    version: &str,
    executable: &Path,
    uninstaller: Option<&Path>,
) -> Result<(), String> {
    if !target.is_absolute() {
        return Err("The installation location must be an absolute path.".into());
    }
    wide(target.as_os_str())?;
    if version.trim().is_empty() {
        return Err("An installed version is required for registry registration.".into());
    }
    wide(OsStr::new(version))?;
    validate_file(executable, "Swiftify executable")?;
    if let Some(uninstaller) = uninstaller {
        validate_file(uninstaller, "uninstaller")?;
    }
    let existing = Key::open(hive, uninstall_key, KEY_READ | KEY_SET_VALUE)?;
    if let Some(key) = existing.as_ref() {
        ensure_owned(key, target)?;
    }
    let new_key = existing.is_none();
    let key = match existing {
        Some(key) => key,
        None => Key::create(hive, uninstall_key)?,
    };
    let result = (|| {
        for (name, value) in [
            ("DisplayName", OsStr::new(DISPLAY_NAME)),
            ("DisplayVersion", OsStr::new(version)),
            ("InstallLocation", target.as_os_str()),
            ("Publisher", OsStr::new(PUBLISHER)),
            ("URLInfoAbout", OsStr::new(ABOUT_URL)),
        ] {
            key.set_string(name, value)?;
        }
        let mut icon = OsString::from("\"");
        icon.push(executable);
        icon.push("\",0");
        key.set_string("DisplayIcon", &icon)?;
        for name in ["NoModify", "NoRepair"] {
            key.set(name, REG_DWORD, &1_u32.to_le_bytes())?;
        }
        if let Some(uninstaller) = uninstaller {
            let mut command = OsString::from("\"");
            command.push(uninstaller);
            command.push("\"");
            key.set_string("UninstallString", &command)?;
        } else {
            key.delete_value("UninstallString")?;
        }
        // Never retain an older quiet-uninstall command when no such command was supplied.
        key.delete_value("QuietUninstallString")?;
        Ok(())
    })();
    drop(key);
    if let Err(error) = result {
        if new_key {
            // This leaf was created by this call; never recursively delete unexpected subkeys.
            let _ = delete_key(hive, uninstall_key);
        }
        return Err(error);
    }
    // The remembered location changes only after every uninstall registration value succeeded.
    Key::create(hive, settings_key)?.set_string("InstallLocation", target.as_os_str())
}

fn delete_key(hive: HKEY, path: &str) -> Result<(), String> {
    let name = wide(OsStr::new(path))?;
    let status = unsafe { RegDeleteKeyW(hive, PCWSTR(name.as_ptr())) };
    if missing(status) {
        return Ok(());
    }
    // Non-recursive deletion deliberately refuses to destroy unexpected nested records.
    checked(status, &format!("remove registry key {path}"))
}

#[allow(dead_code)] // Reserved for the deferred clean-uninstall flow (also exercised by tests).
fn unregister_at(
    hive: HKEY,
    uninstall_key: &str,
    settings_key: &str,
    target: &Path,
) -> Result<(), String> {
    if !target.is_absolute() {
        return Err("The installation location must be an absolute path.".into());
    }
    if let Some(key) = Key::open(hive, uninstall_key, KEY_READ)? {
        ensure_owned(&key, target)?;
        drop(key);
        delete_key(hive, uninstall_key)?;
    }
    if let Some(settings) = Key::open(hive, settings_key, KEY_READ | KEY_SET_VALUE)? {
        if let Some(location) = settings.string("InstallLocation")? {
            if same_path(Path::new(&location), target) {
                settings.delete_value("InstallLocation")?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{RegDeleteTreeW, RegQueryValueExW},
    };

    struct Fixture {
        root: String,
        directory: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let id = uuid::Uuid::new_v4();
            let root = format!(r"Software\SwiftifyCommunityInstallerTest-{id}");
            let directory = std::env::temp_dir().join(format!("swiftify-registry-{id}"));
            std::fs::create_dir(&directory).unwrap();
            Key::create(HKEY_CURRENT_USER, &root).unwrap();
            Self { root, directory }
        }

        fn uninstall(&self) -> String {
            format!(r"{}\Uninstall\SwiftifyCommunity", self.root)
        }

        fn settings(&self) -> String {
            format!(r"{}\Settings", self.root)
        }

        fn executable(&self) -> PathBuf {
            let executable = self.directory.join("Swiftify.exe");
            std::fs::write(&executable, b"test file, never executed").unwrap();
            executable
        }

        fn register(&self, uninstaller: Option<&Path>) -> Result<(), String> {
            register_at(
                HKEY_CURRENT_USER,
                &self.uninstall(),
                &self.settings(),
                &self.directory,
                "1.2.3",
                &self.executable(),
                uninstaller,
            )
        }

        fn unregister(&self, target: &Path) -> Result<(), String> {
            unregister_at(
                HKEY_CURRENT_USER,
                &self.uninstall(),
                &self.settings(),
                target,
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let root = wide(OsStr::new(&self.root)).unwrap();
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(root.as_ptr()));
            }
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn registration_roundtrip_and_matching_cleanup() {
        let fixture = Fixture::new();
        fixture.register(None).unwrap();
        let key = Key::open(HKEY_CURRENT_USER, &fixture.uninstall(), KEY_READ)
            .unwrap()
            .unwrap();
        for (name, expected) in [
            ("DisplayName", OsString::from(DISPLAY_NAME)),
            ("DisplayVersion", OsString::from("1.2.3")),
            ("InstallLocation", fixture.directory.as_os_str().to_owned()),
            ("Publisher", OsString::from(PUBLISHER)),
            ("URLInfoAbout", OsString::from(ABOUT_URL)),
        ] {
            assert_eq!(key.string(name).unwrap(), Some(expected));
        }
        let icon = format!("\"{}\",0", fixture.executable().display());
        assert_eq!(
            key.string("DisplayIcon").unwrap(),
            Some(OsString::from(icon))
        );
        assert_eq!(key.string("UninstallString").unwrap(), None);
        for name in ["NoModify", "NoRepair"] {
            let name = wide(OsStr::new(name)).unwrap();
            let mut kind = REG_VALUE_TYPE::default();
            let mut data = [0_u8; 4];
            let mut bytes = 4;
            let status = unsafe {
                RegQueryValueExW(
                    key.0,
                    PCWSTR(name.as_ptr()),
                    None,
                    Some(&mut kind),
                    Some(data.as_mut_ptr()),
                    Some(&mut bytes),
                )
            };
            assert_eq!(status, ERROR_SUCCESS);
            assert_eq!(kind, REG_DWORD);
            assert_eq!(bytes, 4);
            assert_eq!(u32::from_le_bytes(data), 1);
        }
        let settings = Key::open(HKEY_CURRENT_USER, &fixture.settings(), KEY_READ)
            .unwrap()
            .unwrap();
        assert_eq!(
            settings.string("InstallLocation").unwrap(),
            Some(fixture.directory.as_os_str().to_owned())
        );
        drop(key);
        drop(settings);
        fixture.unregister(&fixture.directory).unwrap();
        assert!(Key::open(HKEY_CURRENT_USER, &fixture.uninstall(), KEY_READ)
            .unwrap()
            .is_none());
        let settings = Key::open(HKEY_CURRENT_USER, &fixture.settings(), KEY_READ)
            .unwrap()
            .unwrap();
        assert_eq!(settings.string("InstallLocation").unwrap(), None);
        fixture.unregister(&fixture.directory).unwrap();
    }

    #[test]
    fn refuses_other_locations_and_unowned_records() {
        let fixture = Fixture::new();
        fixture.register(None).unwrap();
        let other = fixture.directory.join("other");
        let error = register_at(
            HKEY_CURRENT_USER,
            &fixture.uninstall(),
            &fixture.settings(),
            &other,
            "2",
            &fixture.executable(),
            None,
        )
        .unwrap_err();
        assert!(error.contains("already registered"));
        assert!(fixture.unregister(&other).is_err());
        let settings = Key::open(HKEY_CURRENT_USER, &fixture.settings(), KEY_READ)
            .unwrap()
            .unwrap();
        assert_eq!(
            settings.string("InstallLocation").unwrap(),
            Some(fixture.directory.as_os_str().to_owned())
        );
        let key = Key::open(HKEY_CURRENT_USER, &fixture.uninstall(), KEY_SET_VALUE)
            .unwrap()
            .unwrap();
        key.set_string("Publisher", OsStr::new("Some other publisher"))
            .unwrap();
        assert!(fixture.register(None).is_err());
        assert!(fixture.unregister(&fixture.directory).is_err());
    }

    #[test]
    fn advertises_only_existing_explicit_uninstaller() {
        let fixture = Fixture::new();
        let uninstaller = fixture.directory.join("Uninstall Swiftify.exe");
        assert!(fixture.register(Some(&uninstaller)).is_err());
        assert!(Key::open(HKEY_CURRENT_USER, &fixture.settings(), KEY_READ)
            .unwrap()
            .is_none());
        assert!(Key::open(HKEY_CURRENT_USER, &fixture.uninstall(), KEY_READ)
            .unwrap()
            .is_none());
        std::fs::write(&uninstaller, b"not executed").unwrap();
        fixture.register(Some(&uninstaller)).unwrap();
        let key = Key::open(
            HKEY_CURRENT_USER,
            &fixture.uninstall(),
            KEY_READ | KEY_SET_VALUE,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            key.string("UninstallString").unwrap(),
            Some(OsString::from(format!("\"{}\"", uninstaller.display())))
        );
        key.set_string("QuietUninstallString", OsStr::new("stale command"))
            .unwrap();
        fixture.register(None).unwrap();
        assert_eq!(key.string("UninstallString").unwrap(), None);
        assert_eq!(key.string("QuietUninstallString").unwrap(), None);
    }

    #[test]
    fn discovery_filters_exact_names_and_deduplicates_paths() {
        let fixture = Fixture::new();
        let root = format!(r"{}\Uninstall", fixture.root);
        for (name, display, location) in [
            ("Official", "Swiftify", r"C:\Apps\Swiftify"),
            ("Community", DISPLAY_NAME, r"c:\apps\swiftify\"),
            ("Unrelated", "Swiftify Helper", r"C:\Apps\Helper"),
            ("Relative", DISPLAY_NAME, r"relative\path"),
        ] {
            let entry = Key::create(HKEY_CURRENT_USER, &format!(r"{root}\{name}")).unwrap();
            entry
                .set_string("DisplayName", OsStr::new(display))
                .unwrap();
            entry
                .set_string("InstallLocation", OsStr::new(location))
                .unwrap();
        }
        let mut paths = Vec::new();
        discover_at(HKEY_CURRENT_USER, &root, KEY_WOW64_64KEY, &mut paths).unwrap();
        discover_at(HKEY_CURRENT_USER, &root, KEY_WOW64_32KEY, &mut paths).unwrap();
        assert_eq!(paths.len(), 1);
        assert!(same_path(&paths[0], Path::new(r"C:\Apps\Swiftify")));
        discover_at(
            HKEY_CURRENT_USER,
            &format!(r"{}\Missing", fixture.root),
            KEY_WOW64_64KEY,
            &mut paths,
        )
        .unwrap();
    }

    #[test]
    fn discovers_versioned_names_and_display_icon_when_location_is_missing() {
        let fixture = Fixture::new();
        let root = format!(r"{}\Uninstall", fixture.root);
        let entry = Key::create(HKEY_CURRENT_USER, &format!(r"{root}\Versioned")).unwrap();
        entry
            .set_string("DisplayName", OsStr::new("Swiftify v0.25.1"))
            .unwrap();
        let icon = format!(
            "\"{}\",0",
            fixture.directory.join("bin/swiftify.exe").display()
        );
        entry.set_string("DisplayIcon", OsStr::new(&icon)).unwrap();
        let mut paths = Vec::new();
        discover_at(HKEY_CURRENT_USER, &root, KEY_WOW64_64KEY, &mut paths).unwrap();
        assert_eq!(paths, vec![fixture.directory.clone()]);
        assert!(!swiftify_display_name(OsStr::new("Swiftify Helper")));
    }

    #[test]
    fn reads_unicode_and_expanded_strings_and_preserves_other_saved_location() {
        let fixture = Fixture::new();
        let settings = Key::create(HKEY_CURRENT_USER, &fixture.settings()).unwrap();
        let unicode = OsStr::new(r"C:\Apps\日本語\café");
        settings.set_string("Unicode", unicode).unwrap();
        assert_eq!(
            settings.string("Unicode").unwrap().as_deref(),
            Some(unicode)
        );
        let text = wide(OsStr::new(r"%SystemRoot%\Swiftify")).unwrap();
        let bytes: Vec<u8> = text.iter().flat_map(|word| word.to_le_bytes()).collect();
        settings
            .set(
                "Expanded",
                windows::Win32::System::Registry::REG_EXPAND_SZ,
                &bytes,
            )
            .unwrap();
        let expanded = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("Swiftify");
        assert_eq!(
            settings.string("Expanded").unwrap(),
            Some(expanded.into_os_string())
        );
        settings
            .set_string("InstallLocation", OsStr::new(r"C:\Other\Swiftify"))
            .unwrap();
        fixture.unregister(&fixture.directory).unwrap();
        assert_eq!(
            settings.string("InstallLocation").unwrap(),
            Some(OsString::from(r"C:\Other\Swiftify"))
        );
        assert!(wide(OsStr::new("bad\0value")).is_err());
    }
}
