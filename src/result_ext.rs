use std::{error::Error as StdError, fmt};

use crate::Error;

/// Add context to a failing result while leaving successful values untouched.
pub trait ResultExt<T> {
    fn context<C>(self, context: C) -> Result<T, Error>
    where
        C: fmt::Display + Send + Sync + 'static;

    /// Construct context only if the result is an error.
    fn with_context<C, F>(self, context: F) -> Result<T, Error>
    where
        C: fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C;

    /// On failure, attach a typed input error unless that type is already set.
    /// The closure can inspect the original error to select an enum variant.
    fn with_input_error<I, F>(self, input: F) -> Result<T, Error>
    where
        I: Send + Sync + 'static,
        F: FnOnce(&mut Error) -> I;
}

impl<T, E> ResultExt<T> for Result<T, E>
where
    E: StdError + Send + Sync + 'static,
{
    fn context<C>(self, context: C) -> Result<T, Error>
    where
        C: fmt::Display + Send + Sync + 'static,
    {
        self.map_err(|error| Error::new(error).context(context))
    }

    fn with_context<C, F>(self, context: F) -> Result<T, Error>
    where
        C: fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.map_err(|error| Error::new(error).context(context()))
    }

    fn with_input_error<I, F>(self, input: F) -> Result<T, Error>
    where
        I: Send + Sync + 'static,
        F: FnOnce(&mut Error) -> I,
    {
        self.map_err(|error| Error::new(error).with_input_error(input))
    }
}
