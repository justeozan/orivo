use crate::catalog::Game;
use std::{
    io,
    process::{Child, Command, Stdio},
};

#[derive(Debug)]
pub enum LaunchError {
    MissingExecutable(std::path::PathBuf),
    PermissionDenied(std::path::PathBuf),
    Process(io::Error),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingExecutable(path) => {
                write!(f, "executable does not exist: {}", path.display())
            }
            Self::PermissionDenied(path) => {
                write!(f, "executable is not readable: {}", path.display())
            }
            Self::Process(error) => write!(f, "process could not be started: {error}"),
        }
    }
}

impl std::error::Error for LaunchError {}

pub fn launch(game: &Game) -> Result<Child, LaunchError> {
    let executable = &game.executable_path;
    if !executable.is_file() {
        return Err(LaunchError::MissingExecutable(executable.clone()));
    }
    if !is_executable(executable) {
        return Err(LaunchError::PermissionDenied(executable.clone()));
    }

    let mut command = Command::new(executable);
    command
        .args(&game.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(directory) = game.working_directory.as_ref() {
        command.current_dir(directory);
    }

    command.spawn().map_err(LaunchError::Process)
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.metadata()
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_missing_executable_before_spawning() {
        let game = Game::from_executable("/definitely/not/a/game").unwrap();

        assert!(matches!(
            launch(&game),
            Err(LaunchError::MissingExecutable(_))
        ));
    }
}
