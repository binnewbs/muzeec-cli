use thiserror::Error;

/// Crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("toml encode error: {0}")]
    TomlEncode(#[from] toml::ser::Error),

    #[error("audio decode error: {0}")]
    Decode(String),

    #[error("audio output error: {0}")]
    Output(String),

    #[error("metadata error: {0}")]
    Metadata(String),

    #[error("image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("config error: {0}")]
    Config(String),

    #[error("ipc error: {0}")]
    Ipc(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("daemon unreachable: {0}")]
    DaemonUnreachable(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Process exit code mapped from the error category.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Invalid(_) => 2,
            Error::DaemonUnreachable(_) => 3,
            Error::NotFound(_) => 4,
            _ => 1,
        }
    }
}

impl From<cpal::Error> for Error {
    fn from(e: cpal::Error) -> Self {
        Error::Output(e.to_string())
    }
}

/// Marker conversion for stringly errors from external crates.
pub fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Other(msg.into()))
}
