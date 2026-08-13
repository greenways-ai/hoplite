use std::fmt;
use std::num::NonZeroU64;

/// Opaque server-assigned authority scoped by the owning runtime and work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceHandle(NonZeroU64);

impl ResourceHandle {
    pub fn new(value: u64) -> Result<Self, ResourceHandleError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(ResourceHandleError::Zero)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceHandleError {
    Zero,
}

impl fmt::Display for ResourceHandleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("resource handles are server-assigned non-zero integers")
    }
}

impl std::error::Error for ResourceHandleError {}
