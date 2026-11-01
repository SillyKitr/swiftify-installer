#[cfg(not(target_os = "windows"))]
compile_error!("Swiftify Installer only supports Windows.");

pub mod controller;
pub mod install;
pub mod registry;
pub mod release;
pub mod shortcut;
