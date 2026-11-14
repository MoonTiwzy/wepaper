//! What goes wrong with a script.

use std::fmt;

/// The way a script failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The JavaScript engine itself could not be set up.
    Setup,
    /// The script, or a module it imports, is not valid JavaScript or could not be found.
    Compile,
    /// The script threw.
    Threw,
    /// The script ran past its time budget and was interrupted.
    TimedOut,
}

/// Why a script was not attached, or why it was disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// The way it failed.
    pub kind: ErrorKind,
    /// What the engine or the script said.
    pub message: String,
    /// Where in the script, when the engine knows: `function (module:line:column)`.
    pub at: Option<String>,
}

impl ScriptError {
    pub(super) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), at: None }
    }

    pub(super) fn setup(error: impl fmt::Display) -> Self {
        Self::new(ErrorKind::Setup, error.to_string())
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        match &self.at {
            Some(at) => write!(f, " (at {at})"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for ScriptError {}
