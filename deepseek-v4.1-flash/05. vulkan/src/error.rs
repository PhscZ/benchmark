//! Error type shared by the whole crate.  Every failure is reported explicitly;
//! the renderer never silently falls back to CPU rendering.

use ash::vk;

#[derive(Debug)]
pub enum Error {
    /// A Vulkan call returned a non-success result code.
    Vk(vk::Result),
    /// A capability required by this renderer is missing on the selected device.
    Unsupported(String),
    /// Generic host-side failure.
    Message(String),
    Io(std::io::Error),
    Png(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Vk(r) => write!(f, "Vulkan error: {r:?}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Message(m) => write!(f, "{m}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Png(m) => write!(f, "PNG encoding error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<vk::Result> for Error {
    fn from(r: vk::Result) -> Self {
        Error::Vk(r)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<String> for Error {
    fn from(m: String) -> Self {
        Error::Message(m)
    }
}

impl From<&str> for Error {
    fn from(m: &str) -> Self {
        Error::Message(m.to_string())
    }
}

/// Convenience constructor for [`Error::Message`].
pub fn msg<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Message(m.into()))
}
