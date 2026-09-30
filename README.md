# Matriochka

An error wrapper combining per-call context with caller-defined input errors.
Keep the diagnostic chain from an HTTP handler down to a database failure, while
retrieving a typed input error to select the response at the boundary.

```rust
use matriochka::{Error, ResultExt};

#[derive(Debug, thiserror::Error)]
enum ErrorKind {
    #[error("account does not exist")]
    NotFound,
    #[error("service unavailable")]
    Unavailable,
}

fn load_account(id: u64) -> Result<String, Error> {
    std::fs::read_to_string(format!("accounts/{id}"))
        .with_input_error(|err| {
            match err.downcast_ref::<std::io::Error>() {
                Some(error) if error.kind() == std::io::ErrorKind::NotFound =>
                    ErrorKind::NotFound,
                _ => ErrorKind::Unavailable,
            }
        })
}

fn rpc_account(id: u64) -> Result<String, Error> {
    load_account(id).with_context(|| format!("loading account {id}"))
}

if let Err(err) = rpc_account(42) {
    // Inspect the user-defined enum to select an RPC response.
    // This can be used to correctly return a 404 or 503 response, for example.
    let _input = err.input_error::<ErrorKind>().expect("input error is always present");
    // Display every annotated level and the original error sources.
    // Output: "loading account 42: account does not exist"
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
the aggregate. Different input-error types coexist. Joining keeps the first value
of each type; wrapping keeps existing values over incoming duplicates. `data()` and `data_mut()` expose the runtime context for
additional typed metadata. No global or thread-local state is used.

`input_error_mut::<T>()` edits an existing value at the current boundary without
allocating. To insert a value if needed and get a mutable reference, use
`get_or_insert_input_error(value)`, `get_or_insert_input_error_with(|| value)`,
or `get_or_insert_input_error_default::<T>()`. Factories and defaults run only
when this type has no local entry.

These mutable helpers operate on the aggregate context, including metadata moved
from joined or wrapped Matriochka errors. Third-party errors expose only shared
sources, so metadata hidden inside those sources cannot be moved into the
aggregate; immutable input-error lookup still searches them as a fallback. Immutable
borrowed entries inserted through `data_mut()` cause the insertion helpers to
panic without calling the factory or default. Use `set_input_error` to explicitly
replace such an entry.

`with_context` builds its context only on failure. Context values retain their
types without eager formatting, and successful results allocate no storage in
Matriochka. Owned errors and context must be `Send + Sync + 'static`.

`Error` holds a single `Box<Inner>`: the value passed between functions is one
pointer wide. Context and input-error updates retain that outer allocation, and
wrapping an existing `Error` reuses its handle. The error chain and runtime
data live behind the pointer. This does not mean one heap allocation in total:
the error vector, original erased errors, added diagnostic context layers, and
stored metadata can allocate their own storage. The runtime-context container is stored inline inside `Inner`.

- `{error}` displays the outermost message.
- `{error:#}` displays the complete chain on one line.
- `{error:?}` displays a multiline diagnostic chain.
- `chain()`, `root_cause()`, and `downcast_ref::<E>()` inspect original causes.

The chain contains explicitly added context and sources exposed by each error's
`StdError::source()`. It is not a runtime backtrace and cannot recover unannotated
calls or sources hidden by other wrappers. Input errors are separate metadata;
they do not replace or appear in the diagnostic chain automatically.

## Grouping errors

`Error::join(errors)` stores wrapped errors directly in its internal vector of
`Box<dyn StdError + Send + Sync>`. `Error::join` and `error.wrap(sub_error)` move
child Matriochka runtime contexts into one aggregate context. All typed metadata
is merged, including entries inserted through `data_mut()`. Existing values win
when wrapping; the first value wins when joining. Duplicate values are dropped.
Child errors keep their diagnostic messages and sources, with their runtime
context removed. Ordinary errors need no Matriochka context of their own. Use `Error` elements when the errors have different types.

Propagation reuses the current handle and its lazily initialized runtime context.
Adding diagnostic messages does not initialize additional runtime contexts.

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

assert!(err.downcast_ref::<io::Error>().is_some());
assert_eq!(format!("{err:#}"),
    "RPC: 2 errors [database: connection lost; 2 errors [invalid account; invalid amount]]");
```

`Error::errors()` exposes the immediate boxed errors. Their merged metadata is
available through the aggregate's `data()`, `data_mut()`, and input-error helpers.
After adding a diagnostic context, that wrapper is the immediate error and the
children remain beneath it. Diagnostic context messages continue to accumulate;
they are separate from the consolidated runtime metadata.

The standard `StdError::source()` API supports one source, so a native group with
multiple children has no standard source. `chain()` stops at the group and
`root_cause()` returns the group itself. Use `errors()` to inspect immediate
children and `downcast_ref::<E>()` to search their errors and sources.
A single-child native group displays transparently and exposes its linear source
chain. Empty native groups display `0 errors`. Full diagnostic formatting includes every
branch, and `downcast_ref::<E>()` can find concrete errors inside branches.
