//! Errors wrapped with caller-defined context at each application boundary.
//!
//! [`Error`] preserves the original error and its sources. Add context with
//! [`ResultExt::context`] or [`ResultExt::with_context`], then use `{error:#}`
//! to display the entire chain, from the outermost context to the root cause.
//! Context is an annotated error chain, not a captured runtime backtrace.
//!
//! ```
//! use matriochka::{Error, ResultExt};
//!
//! fn load_account(id: u64) -> Result<String, Error> {
//!     std::fs::read_to_string(format!("accounts/{id}"))
//!         .with_context(|| format!("loading account {id}"))
//! }
//!
//! fn rpc_account(id: u64) -> Result<String, Error> {
//!     load_account(id).context("handling account RPC")
//! }
//! ```

use std::{any::Any, error::Error as StdError, fmt};

pub use runtime_context;

mod join;
pub use join::{Join, Unwrap};

/// A caller-defined input error stored by type in the runtime context.
///
/// `I` may be a `thiserror` enum, or any owned `Send + Sync` value.
pub struct InputError<I: 'static>(pub I);

runtime_context::tid! { impl<'a, I: 'static> TidAble<'a> for InputError<I> }

/// An owned error with optional, dynamically typed context layers.
///
/// Any error implementing `StdError + Send + Sync + 'static` can be wrapped.
/// Each context can have a different type implementing `Display + Send + Sync
/// + 'static`. Context stays in its original type until it is displayed.
///
/// The handle is one pointer wide. Adding context keeps its outer allocation.
/// Input errors are stored in a runtime context initialized on first insertion.
/// No context formatting or backtrace capture happens implicitly.
#[repr(transparent)]
pub struct Error {
    inner: Box<Inner>,
}

struct Inner {
    // Temporarily empty only while moving the error into a new context layer.
    error: Option<Box<dyn StdError + Send + Sync>>,
    data: Option<runtime_context::Context<'static, 'static>>,
}

impl Error {
    /// Combine errors into one error, preserving their order and metadata.
    /// An empty input produces an empty join.
    pub fn join<E>(errors: impl IntoIterator<Item = E>) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self::new(Join::new(errors))
    }

    /// Wrap a user-defined error, retaining its existing source chain.
    pub fn new<E>(error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        // Recover an existing handle without a temporary heap allocation.
        // Option allows a safe owned downcast through Any::downcast_mut.
        let mut error = Some(error);
        if let Some(err) = (&mut error as &mut dyn Any).downcast_mut::<Option<Self>>() {
            return err.take().expect("error is present");
        }
        Self::from_boxed(Box::new(error.expect("error is present")))
    }

    /// Wrap an already boxed error in a one-pointer handle.
    ///
    /// Allocates the handle's state unless the error is already a `Error`.
    pub fn from_boxed(error: Box<dyn StdError + Send + Sync>) -> Self {
        match error.downcast::<Self>() {
            Ok(err) => *err,
            Err(error) => Self {
                inner: Box::new(Inner {
                    error: Some(error),
                    data: None,
                }),
            },
        }
    }

    /// Add an outer context layer without formatting it eagerly.
    pub fn context<C>(mut self, context: C) -> Self
    where
        C: fmt::Display + Send + Sync + 'static,
    {
        let source = self
            .inner
            .error
            .take()
            .expect("an error is always present");
        self.inner.error = Some(Box::new(Context { context, source }));
        self
    }

    /// Read an input error by type, including sources and joined branches.
    /// The current value wins; otherwise the first depth-first match is used.
    pub fn input_error<I: Send + Sync + 'static>(&self) -> Option<&I> {
        self.inner
            .data
            .as_ref()
            .and_then(|data| data.get::<InputError<I>>())
            .map(|input| &input.0)
            .or_else(|| {
                self.chain().find_map(|error| {
                    if let Some(err) = error.downcast_ref::<Self>() {
                        err.input_error::<I>()
                    } else {
                        error
                            .downcast_ref::<Join>()
                            .and_then(|join| join.errors().iter().find_map(Self::input_error::<I>))
                    }
                })
            })
    }

    /// Set or replace an input error of this type at the current boundary.
    pub fn set_input_error<I: Send + Sync + 'static>(&mut self, input: I) {
        self.data_mut().insert(InputError(input));
    }

    /// Mutably borrow an input error at the current boundary without allocating.
    ///
    /// Unlike [`Self::input_error`], this does not search sources or joined
    /// branches: standard error sources expose only shared references.
    /// Immutable borrowed entries inserted through [`Self::data_mut`] return
    /// `None` as well.
    pub fn input_error_mut<I: Send + Sync + 'static>(&mut self) -> Option<&mut I> {
        self.inner
            .data
            .as_mut()?
            .get_mut::<InputError<I>>()
            .map(|input| &mut input.0)
    }

    /// Return the current boundary's mutable input error, inserting `input`
    /// when no mutable entry exists. Sources and joined branches are unchanged.
    /// An immutable borrowed entry is replaced with the supplied owned value.
    pub fn get_or_insert_input_error<I: Send + Sync + 'static>(&mut self, input: I) -> &mut I {
        self.get_or_insert_input_error_with(|| input)
    }

    /// Lazily initialize an input error at the current boundary.
    ///
    /// The factory runs only when no mutable local entry exists. This includes
    /// replacing an immutable borrowed entry; it does not copy values from
    /// sources or joined branches. The returned value takes precedence over
    /// those inherited values in subsequent [`Self::input_error`] lookups.
    pub fn get_or_insert_input_error_with<I, F>(&mut self, input: F) -> &mut I
    where
        I: Send + Sync + 'static,
        F: FnOnce() -> I,
    {
        if self.input_error_mut::<I>().is_none() {
            self.set_input_error(input());
        }
        self.input_error_mut::<I>()
            .expect("a mutable input error was inserted")
    }

    /// Get the current boundary's input error, initializing it with `Default`
    /// only when no mutable local entry exists.
    pub fn get_or_insert_input_error_default<I>(&mut self) -> &mut I
    where
        I: Default + Send + Sync + 'static,
    {
        self.get_or_insert_input_error_with(I::default)
    }

    /// Compute an input error only when this type has not already been set.
    pub fn with_input_error<I, F>(mut self, input: F) -> Self
    where
        I: Send + Sync + 'static,
        F: FnOnce(&Self) -> I,
    {
        if self.input_error::<I>().is_none() {
            let input = input(&self);
            self.set_input_error(input);
        }
        self
    }

    /// Borrow runtime data without allocating it.
    pub fn data(&self) -> Option<&runtime_context::Context<'static, 'static>> {
        self.inner.data.as_ref()
    }

    /// Initialize runtime data on first access and return it for modification.
    pub fn data_mut(&mut self) -> &mut runtime_context::Context<'static, 'static> {
        self.inner.data.get_or_insert_with(Default::default)
    }

    /// Borrow the outermost error or context as a standard error.
    pub fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
        self.inner
            .error
            .as_deref()
            .expect("an error is always present")
    }

    /// Iterate over the linear source chain, outermost first.
    /// A join terminates this chain; use [`Self::unwrap`] to visit its branches.
    pub fn chain(&self) -> Chain<'_> {
        Chain {
            next: Some(self.as_error()),
        }
    }

    /// Flatten context, sources, and joined branches in depth-first order.
    /// Join containers are omitted; each branch's context and sources remain.
    pub fn unwrap(&self) -> Unwrap<'_> {
        Unwrap::new(self.as_error())
    }

    /// Return the innermost error in the linear source chain.
    /// For joined errors this is the join, since there is no single root cause.
    pub fn root_cause(&self) -> &(dyn StdError + 'static) {
        self.chain().last().expect("an error is always present")
    }

    /// Find a concrete error type in the chain or any joined branch.
    pub fn downcast_ref<E: StdError + 'static>(&self) -> Option<&E> {
        self.chain()
            .find_map(|error| error.downcast_ref::<E>())
            .or_else(|| self.unwrap().find_map(|error| error.downcast_ref::<E>()))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            for (index, error) in self.chain().enumerate() {
                if index != 0 {
                    f.write_str(": ")?;
                }
                write!(f, "{error}")?;
            }
            Ok(())
        } else {
            write!(f, "{}", self.as_error())
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")?;
        for (index, error) in self.chain().skip(1).enumerate() {
            if index == 0 {
                f.write_str("\n\nCaused by:")?;
            }
            write!(f, "\n    {index}: {error}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.as_error().source()
    }
}

struct Context<C> {
    context: C,
    source: Box<dyn StdError + Send + Sync>,
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

/// An iterator over context and source errors, outermost first.
pub struct Chain<'a> {
    next: Option<&'a (dyn StdError + 'static)>,
}

impl<'a> Iterator for Chain<'a> {
    type Item = &'a (dyn StdError + 'static);

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        self.next = current.source();
        Some(current)
    }
}

impl std::iter::FusedIterator for Chain<'_> {}

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
        F: FnOnce(&Error) -> I;
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
        F: FnOnce(&Error) -> I,
    {
        self.map_err(|error| Error::new(error).with_input_error(input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn preserves_context_across_result_boundaries() {
        let database: Result<(), _> = Err(io::Error::other("database unavailable"));
        let err = database
            .context("reading account")
            .with_context(|| format!("loading account {}", 42))
            .context("handling RPC")
            .unwrap_err();

        assert_eq!(err.to_string(), "handling RPC");
        assert_eq!(
            format!("{err:#}"),
            "handling RPC: loading account 42: reading account: database unavailable"
        );
        assert_eq!(err.chain().count(), 4);
        assert_eq!(err.root_cause().to_string(), "database unavailable");
        assert_eq!(
            err.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::Other
        );
        assert_eq!(
            format!("{err:?}"),
            "handling RPC\n\nCaused by:\n    0: loading account 42\n    1: reading account\n    2: database unavailable"
        );
    }

    #[derive(Debug)]
    struct DatabaseError(io::Error);

    impl fmt::Display for DatabaseError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("database query failed")
        }
    }

    impl StdError for DatabaseError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn preserves_user_error_types_and_existing_sources() {
        let err =
            Error::new(DatabaseError(io::Error::other("connection lost"))).context("handling RPC");
        assert!(err.downcast_ref::<DatabaseError>().is_some());
        assert!(err.downcast_ref::<io::Error>().is_some());
        assert_eq!(
            format!("{err:#}"),
            "handling RPC: database query failed: connection lost"
        );
        assert_eq!(
            err.source().unwrap().to_string(),
            "database query failed"
        );
    }

    #[test]
    fn context_factory_runs_only_on_failure() {
        let success: Result<u8, io::Error> = Ok(7);
        assert_eq!(
            success.with_context(|| panic!("must stay lazy")).unwrap(),
            7
        );
        let mut calls = 0;
        let failure: Result<(), _> = Err(io::Error::other("failure"));
        let err = failure.with_context(|| {
            calls += 1;
            "context"
        });
        assert!(err.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn supports_display_only_context_and_formats_on_demand() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Request(Arc<AtomicUsize>);
        impl fmt::Display for Request {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fetch_add(1, Ordering::Relaxed);
                f.write_str("request 42")
            }
        }
        let formats = Arc::new(AtomicUsize::new(0));
        let err = Error::new(io::Error::other("failure")).context(Request(formats.clone()));
        assert_eq!(formats.load(Ordering::Relaxed), 0);
        assert_eq!(format!("{err:#}"), "request 42: failure");
        assert_eq!(formats.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn boxed_root_has_no_duplicate_layers() {
        let err = Error::from_boxed(Box::new(io::Error::other("failure")));
        assert_eq!(err.chain().count(), 1);
        assert!(err.source().is_none());
        assert_eq!(format!("{err:#}"), "failure");
        assert_eq!(format!("{err:?}"), "failure");
    }

    #[test]
    fn err_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Error>();
    }

    #[test]
    fn err_and_optional_err_are_one_pointer_wide() {
        assert_eq!(size_of::<Error>(), size_of::<*const ()>());
        assert_eq!(size_of::<Option<Error>>(), size_of::<*const ()>());
    }

    #[test]
    fn propagation_preserves_the_outer_allocation() {
        let err = Error::new(io::Error::other("database failure"));
        let address = std::ptr::from_ref(err.inner.as_ref());
        let err = err.context("query").with_input_error(|_| 42_u64);
        let result: Result<(), Error> = Err(err);
        let err = result
            .with_context(|| "service")
            .context("RPC")
            .with_input_error::<u64, _>(|_| panic!("already set"))
            .unwrap_err();
        assert_eq!(std::ptr::from_ref(err.inner.as_ref()), address);
        assert_eq!(err.input_error::<u64>(), Some(&42));
        assert_eq!(
            format!("{err:#}"),
            "RPC: service: query: database failure"
        );
        let err = Error::from_boxed(Box::new(err));
        assert_eq!(std::ptr::from_ref(err.inner.as_ref()), address);
    }
}
