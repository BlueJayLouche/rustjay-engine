//! Error types for Syphon

use std::fmt;

/// Result type alias for Syphon operations
pub type Result<T> = std::result::Result<T, SyphonError>;

/// Errors that can occur when using Syphon
#[derive(Debug, Clone)]
pub enum SyphonError {
    /// Syphon is not available on this platform
    NotAvailable,
    
    /// The Syphon framework could not be found
    FrameworkNotFound(String),
    
    /// Failed to create a server or client
    CreateFailed(String),
    
    /// Server with the given name was not found
    ServerNotFound(String),

    /// Multiple servers match the given name — use connect_by_info() with a UUID
    AmbiguousServerName(String),
    
    /// Invalid parameter was provided
    InvalidParameter(String),
    
    /// Failed to publish a frame
    PublishFailed(String),
    
    /// Failed to receive a frame
    ReceiveFailed(String),
    
    /// An Objective-C exception was thrown
    ObjcException,
    
    /// Failed to lock/unlock an IOSurface
    LockFailed,
    
    /// The received frame was invalid
    InvalidFrame,
    
    /// IOSurface operation failed
    IOSurfaceError(u32),
    
    /// Texture operation failed
    TextureError(String),
    
    /// Other error
    Other(String),
}

impl fmt::Display for SyphonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyphonError::NotAvailable => {
                write!(f, "Syphon is not available on this platform")
            }
            SyphonError::FrameworkNotFound(msg) => {
                write!(f, "Syphon framework not found: {}", msg)
            }
            SyphonError::CreateFailed(msg) => {
                write!(f, "Failed to create Syphon object: {}", msg)
            }
            SyphonError::ServerNotFound(name) => {
                write!(f, "Syphon server '{}' not found", name)
            }
            SyphonError::AmbiguousServerName(name) => {
                write!(f, "Multiple Syphon servers named '{}' — use connect_by_info() with a UUID for precise selection", name)
            }
            SyphonError::InvalidParameter(msg) => {
                write!(f, "Invalid parameter: {}", msg)
            }
            SyphonError::PublishFailed(msg) => {
                write!(f, "Failed to publish frame: {}", msg)
            }
            SyphonError::ReceiveFailed(msg) => {
                write!(f, "Failed to receive frame: {}", msg)
            }
            SyphonError::ObjcException => {
                write!(f, "Objective-C exception was thrown")
            }
            SyphonError::LockFailed => {
                write!(f, "Failed to lock/unlock IOSurface")
            }
            SyphonError::InvalidFrame => {
                write!(f, "Invalid frame received")
            }
            SyphonError::IOSurfaceError(code) => {
                write!(f, "IOSurface error: {}", code)
            }
            SyphonError::TextureError(msg) => {
                write!(f, "Texture error: {}", msg)
            }
            SyphonError::Other(msg) => {
                write!(f, "{}", msg)
            }
        }
    }
}

impl std::error::Error for SyphonError {}

impl From<std::ffi::NulError> for SyphonError {
    fn from(e: std::ffi::NulError) -> Self {
        SyphonError::InvalidParameter(format!("String contains null byte: {}", e))
    }
}

impl From<std::io::Error> for SyphonError {
    fn from(e: std::io::Error) -> Self {
        SyphonError::Other(format!("IO error: {}", e))
    }
}
