# Matriochka

An error wrapper combining per-call context with caller-defined input errors.
Keep the diagnostic chain from an HTTP handler down to a database failure, while
retrieving a typed input error to select the response at the boundary.

```rust
use matriochka::{Error, ResultExt};

#[derive(Debug, thiserror::Error)]
enum InputError {
    #[error("account {0} does not exist")]
    AccountNotFound(u64),
    #[error("service unavailable")]
    Unavailable,
}

fn load_account(id: u64) -> Result<String, Error> {
    std::fs::read_to_string(format!("accounts/{id}"))
        .with_input_error(|err| {
            match err.downcast_ref::<std::io::Error>() {
                Some(error) if error.kind() == std::io::ErrorKind::NotFound =>
                    InputError::AccountNotFound(id),
                _ => InputError::Unavailable,
            }
        })
        .with_context(|| format!("loading account {id}"))
}

fn rpc_account(id: u64) -> Result<String, Error> {
    load_account(id).context("handling account RPC")
}

if let Err(err) = rpc_account(42) {
    // Inspect the user-defined enum to select an RPC response.
    let input = err.input_error::<InputError>();
    // Display every annotated level and the original error sources.
    eprintln!("{err:#}");
}
```

`thiserror` is optional for consumers: any standard error is accepted, and input
errors can be any owned `Send + Sync + 'static` type. Matriochka supplies no fixed
set of input-error variants. Existing enums with `#[from]` and `#[source]` work
through the standard error source chain.

Input errors live in [`runtime-context`](https://github.com/xelis-project/runtime-context)
as `matriochka::InputError<T>`. The runtime context is initialized on first write.
`with_input_error` runs only on failure and only if that type is absent, including
from visible `Error` sources. `set_input_error` explicitly replaces a value at
the current boundary. Different input-error types coexist; the nearest value of
a requested type wins. `data()` and `data_mut()` expose the runtime context for
additional typed metadata. No global or thread-local state is used.

`input_error_mut::<T>()` edits an existing value at the current boundary without
allocating. To insert a value if needed and get a mutable reference, use
`get_or_insert_input_error(value)`, `get_or_insert_input_error_with(|| value)`,
or `get_or_insert_input_error_default::<T>()`. Factories and defaults run only
when a mutable local value is missing.

These mutable helpers operate on the current error's metadata, just like
`set_input_error`. They do not traverse source errors or joined branches, since
standard error sources only expose shared references. Inserting a local value
overrides inherited lookup results without changing the child errors. Immutable
borrowed entries inserted through `data_mut()` are replaced with owned values
by the insertion helpers.

`with_context` builds its context only on failure. Context values retain their
types without eager formatting, and successful results allocate no storage in
Matriochka. Owned errors and context must be `Send + Sync + 'static`.

`Error` holds a single `Box<Inner>`: the value passed between functions is one
pointer wide. Context and input-error updates retain that outer allocation, and
wrapping an existing `Error` reuses its handle. The error chain and runtime
data live behind the pointer. This does not mean one heap allocation in total:
the original erased error, added context layers, and stored metadata can allocate
their own storage. The runtime-context container is stored inline inside `Inner`.

- `{error}` displays the outermost message.
- `{error:#}` displays the complete chain on one line.
- `{error:?}` displays a multiline diagnostic chain.
- `chain()`, `root_cause()`, and `downcast_ref::<E>()` inspect original causes.

The chain contains explicitly added context and sources exposed by each error's
`StdError::source()`. It is not a runtime backtrace and cannot recover unannotated
calls or sources hidden by other wrappers. Input errors are separate metadata;
they do not replace or appear in the diagnostic chain automatically.

## Joining and flattening errors

`Join::new(errors)` groups errors; `Error::join(errors)` wraps the group in a
one-pointer `Error`. Use `Error` elements when the errors have different types.

```rust
use matriochka::Error;
use std::io;

let err = Error::join([
    Error::new(io::Error::other("connection lost")).context("database"),
    Error::join([
        io::Error::other("invalid account"),
        io::Error::other("invalid amount"),
    ]),
]).context("RPC");

let messages: Vec<_> = err.unwrap().map(ToString::to_string).collect();
assert_eq!(messages, [
    "RPC", "database", "connection lost", "invalid account", "invalid amount",
]);
```

`Error::unwrap()` and `Join::unwrap()` return a borrowed `Unwrap` iterator.
It traverses nested joins depth-first in insertion order, yielding context and
source errors and omitting join containers. Duplicate errors remain. Empty joins
yield nothing. `Unwrap::new(&error)` also works with ordinary standard errors.

`Join::errors()` exposes the original children and their runtime metadata.
Input-error lookup uses the current error's value first, then the first matching
branch in depth-first order. Joining does not merge or overwrite child metadata.

The standard `StdError::source()` API supports one source, so a `Join` has no standard
source. `chain()` stops at the join and `root_cause()` returns the join itself;
use `unwrap()` to visit all branches. Full diagnostic formatting includes every
branch, and `downcast_ref::<E>()` can find concrete errors inside branches.
