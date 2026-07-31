use std::fmt;

/// error returned by a future text parser or document operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub offset: usize,
    pub message: String,
}

impl Error {
    pub fn new(offset: usize, message: impl Into<String>) -> Self {
        Self { offset, message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "JSON error at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for Error {}
 