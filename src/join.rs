use std::{error::Error as StdError, fmt, iter::FusedIterator, slice};

use crate::Error;

/// Multiple independent errors represented as one standard error.
///
/// Children retain their context and runtime data. `StdError::source()` is `None`
/// because the standard API cannot represent multiple sources; use [`Self::unwrap`]
/// to traverse every branch, or [`Self::errors`] to inspect individual errors.
/// Empty joins are supported and unwrap to an empty iterator.
///
/// ```
/// use matriochka::{Join, Error};
/// use std::io;
///
/// let joined = Error::new(Join::new([
///     Error::new(io::Error::other("first")).context("database"),
///     Error::join([io::Error::other("second"), io::Error::other("third")]),
/// ])).context("RPC");
/// let messages: Vec<_> = joined.unwrap().map(ToString::to_string).collect();
/// assert_eq!(messages, ["RPC", "database", "first", "second", "third"]);
/// ```
#[derive(Debug)]
pub struct Join {
    errors: Vec<Error>,
}

impl Join {
    /// Collect errors in input order. Use `Error` for heterogeneous errors.
    pub fn new<E>(errors: impl IntoIterator<Item = E>) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self {
            errors: errors.into_iter().map(Error::new).collect(),
        }
    }

    /// Borrow the immediate children with their context and runtime data intact.
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }

    /// Flatten nested joins, context, and sources in input order.
    pub fn unwrap(&self) -> Unwrap<'_> {
        Unwrap::new(self)
    }
}

impl fmt::Display for Join {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} errors", self.errors.len())?;
        if !self.errors.is_empty() {
            f.write_str(" [")?;
            for (index, error) in self.errors.iter().enumerate() {
                if index != 0 {
                    f.write_str("; ")?;
                }
                write!(f, "{error:#}")?;
            }
            f.write_str("]")?;
        }
        Ok(())
    }
}

impl StdError for Join {}

/// Borrowed, depth-first iterator over all errors in a joined error tree.
///
/// Context and source errors are yielded outermost first within each branch.
/// Join containers are expanded rather than yielded. Children are visited in
/// insertion order, including duplicates. No errors or metadata are consumed.
/// The traversal uses a stack of branch iterators rather than recursion, with
/// storage proportional to join nesting depth. Linear chains allocate no stack.
pub struct Unwrap<'a> {
    next: Option<&'a (dyn StdError + 'static)>,
    branches: Vec<slice::Iter<'a, Error>>,
}

impl<'a> Unwrap<'a> {
    /// Traverse an error and all its exposed sources and Matriochka joins.
    pub fn new(error: &'a (dyn StdError + 'static)) -> Self {
        Self {
            next: Some(error),
            branches: Vec::new(),
        }
    }
}

impl<'a> Iterator for Unwrap<'a> {
    type Item = &'a (dyn StdError + 'static);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let Some(error) = self.next.take() else {
                let branch = self.branches.last_mut()?;
                if let Some(err) = branch.next() {
                    self.next = Some(err.as_error());
                } else {
                    self.branches.pop();
                }
                continue;
            };
            // Normalize Error wrappers so their joins are visible and their
            // outer context is yielded exactly once.
            if let Some(err) = error.downcast_ref::<Error>() {
                self.next = Some(err.as_error());
            } else if let Some(join) = error.downcast_ref::<Join>() {
                self.branches.push(join.errors.iter());
            } else {
                self.next = error.source();
                return Some(error);
            }
        }
    }
}

impl FusedIterator for Unwrap<'_> {}
