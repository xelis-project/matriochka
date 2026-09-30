use std::{error::Error as StdError, fmt};

pub(super) struct Context<C> {
    pub(super) context: C,
    pub(super) source: Box<dyn StdError + Send + Sync>,
}

impl<C: fmt::Display> fmt::Display for Context<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.context)
    }
}

impl<C: fmt::Display> fmt::Debug for Context<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<C: fmt::Display> StdError for Context<C> {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}
