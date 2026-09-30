use std::{any::Any, error::Error as StdError, fmt};

use crate::{Chain, InputError, context::Context};

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
    errors: Vec<Box<dyn StdError + Send + Sync>>,
    data: Option<runtime_context::Context<'static, 'static>>,
}

impl Error {
    /// Combine errors into one error, preserving their order and metadata.
    /// Child runtime contexts move into one aggregate context. The first
    /// value of each type wins; duplicate values are dropped.
    /// An empty input produces an empty group. A single child is displayed
    /// transparently and retains its linear source chain.
    pub fn join<E>(errors: impl IntoIterator<Item = E>) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let errors = errors.into_iter();
        let mut joined = Self {
            inner: Box::new(Inner {
                errors: Vec::with_capacity(errors.size_hint().0),
                data: None,
            }),
        };
        for error in errors {
            joined = joined.wrap(error);
        }
        joined
    }

    /// Append a sub-error, retaining this handle and its runtime metadata.
    ///
    /// The sub-error keeps its sources. Its Matriochka runtime context moves
    /// into this error; existing values win on duplicate types.
    /// Existing diagnostic context remains attached to the earlier errors;
    /// call [`Self::context`] afterwards to annotate the whole group.
    ///
    /// ```
    /// use matriochka::Error;
    /// use std::io;
    ///
    /// let error = Error::new(io::Error::other("first"))
    ///     .wrap(io::Error::other("second"));
    /// assert_eq!(error.errors().len(), 2);
    /// assert_eq!(format!("{error:#}"), "2 errors [first; second]");
    /// ```
    pub fn wrap<E>(mut self, sub_error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let mut sub_error: Box<dyn StdError + Send + Sync> = Box::new(sub_error);
        if let Some(error) = sub_error.downcast_mut::<Self>()
            && let Some(mut incoming) = error.inner.data.take()
        {
            // extend overwrites duplicates, so apply existing values last.
            if let Some(existing) = self.inner.data.take() {
                incoming.extend(existing);
            }
            self.inner.data = Some(incoming);
        }
        self.inner.errors.push(sub_error);
        self
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
                    errors: vec![error],
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
        let source = if self.inner.errors.len() == 1 {
            self.inner.errors.pop().expect("one error is present")
        } else {
            Box::new(Self {
                inner: Box::new(Inner {
                    errors: std::mem::take(&mut self.inner.errors),
                    data: None,
                }),
            }) as Box<dyn StdError + Send + Sync>
        };
        self.inner
            .errors
            .push(Box::new(Context { context, source }));
        self
    }

    /// Read an input error from the aggregate context.
    /// Falls back to visible sources inside third-party errors, whose metadata
    /// cannot be moved through the shared `StdError::source()` interface.
    pub fn input_error<I: Send + Sync + 'static>(&self) -> Option<&I> {
        self.inner
            .data
            .as_ref()
            .and_then(|data| data.get::<InputError<I>>())
            .map(|input| &input.0)
            .or_else(|| {
                self.inner
                    .errors
                    .iter()
                    .find_map(|error| input_in_chain::<I>(error.as_ref()))
            })
    }

    /// Set or replace an input error of this type at the current boundary.
    pub fn set_input_error<I: Send + Sync + 'static>(&mut self, input: I) {
        self.data_mut().insert(InputError(input));
    }

    /// Mutably borrow an input error at the current boundary without allocating.
    ///
    /// Includes metadata merged from joined or wrapped Matriochka errors.
    /// Does not search third-party sources, which expose only shared references.
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
    /// when this type has no local entry. Sources and joined branches are unchanged.
    ///
    /// # Panics
    /// Panics if the existing entry is immutably borrowed or has the wrong type.
    pub fn get_or_insert_input_error<I: Send + Sync + 'static>(&mut self, input: I) -> &mut I {
        self.get_or_insert_input_error_with(|| input)
    }

    /// Lazily initialize an input error at the current boundary.
    ///
    /// The factory runs only when this type has no local entry. It does not
    /// copy values from sources or joined branches. The returned value takes
    /// precedence over inherited values in subsequent [`Self::input_error`] lookups.
    ///
    /// # Panics
    /// Panics if the existing entry is immutably borrowed or has the wrong type.
    /// The factory is not called in either case.
    pub fn get_or_insert_input_error_with<I, F>(&mut self, input: F) -> &mut I
    where
        I: Send + Sync + 'static,
        F: FnOnce() -> I,
    {
        &mut self.data_mut().get_or_insert_with(|| InputError(input())).0
    }

    /// Get the current boundary's input error, initializing it with `Default`
    /// only when this type has no local entry.
    ///
    /// # Panics
    /// Panics if the existing entry is immutably borrowed or has the wrong type.
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
        F: FnOnce(&mut Self) -> I,
    {
        if self.input_error::<I>().is_none() {
            let input = input(&mut self);
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

    /// Borrow the outermost error or diagnostic context as a standard error.
    /// Empty and multiple-error groups return this `Error` itself.
    pub fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
        match self.inner.errors.as_slice() {
            [error] => error.as_ref(),
            _ => self,
        }
    }

    /// Borrow immediate wrapped errors. Child metadata has moved into this error.
    pub fn errors(&self) -> &[Box<dyn StdError + Send + Sync>] {
        &self.inner.errors
    }

    /// Iterate over the linear source chain, outermost first.
    /// A group with multiple children terminates this chain;
    /// use [`Self::errors`] to inspect its children.
    pub fn chain(&self) -> Chain<'_> {
        Chain {
            next: Some(self.as_error()),
        }
    }

    /// Return the innermost error in the linear source chain.
    /// For multiple children this is the group, since there is no single root cause.
    pub fn root_cause(&self) -> &(dyn StdError + 'static) {
        self.chain().last().expect("an error is always present")
    }

    /// Find a concrete error type in the chain or any joined branch.
    pub fn downcast_ref<E: StdError + 'static>(&self) -> Option<&E> {
        self.chain()
            .find_map(|error| error.downcast_ref::<E>())
            .or_else(|| {
                self.inner
                    .errors
                    .iter()
                    .find_map(|error| downcast_in_chain::<E>(error.as_ref()))
            })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.inner.errors.len() != 1 {
            write!(f, "{} errors", self.inner.errors.len())?;
            if !self.inner.errors.is_empty() {
                f.write_str(" [")?;
                for (index, error) in self.inner.errors.iter().enumerate() {
                    if index != 0 {
                        f.write_str("; ")?;
                    }
                    // Ordinary errors may not implement alternate chain display.
                    if let Some(error) = error.downcast_ref::<Self>() {
                        write!(f, "{error:#}")?;
                    } else {
                        for (index, cause) in (Chain {
                            next: Some(error.as_ref()),
                        })
                        .enumerate()
                        {
                            if index != 0 {
                                f.write_str(": ")?;
                            }
                            write!(f, "{cause}")?;
                        }
                    }
                }
                f.write_str("]")?;
            }
            return Ok(());
        }
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
        match self.inner.errors.as_slice() {
            [error] => error.source(),
            _ => None,
        }
    }
}

fn input_in_chain<'a, I: Send + Sync + 'static>(
    error: &'a (dyn StdError + 'static),
) -> Option<&'a I> {
    let mut next = Some(error);
    while let Some(error) = next {
        if let Some(error) = error.downcast_ref::<Error>() {
            return error.input_error::<I>();
        }
        next = error.source();
    }
    None
}

fn downcast_in_chain<'a, E: StdError + 'static>(
    error: &'a (dyn StdError + 'static),
) -> Option<&'a E> {
    let mut next = Some(error);
    while let Some(error) = next {
        if let Some(value) = error.downcast_ref::<E>() {
            return Some(value);
        }
        if let Some(error) = error.downcast_ref::<Error>() {
            return error
                .inner
                .errors
                .iter()
                .find_map(|child| downcast_in_chain::<E>(child.as_ref()));
        }
        next = error.source();
    }
    None
}

#[cfg(test)]
mod tests;
