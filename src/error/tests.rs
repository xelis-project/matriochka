use super::*;
use crate::ResultExt;
use std::io;

#[test]
fn wrap_preserves_handle_metadata_and_child_sources() {
    let error = Error::new(io::Error::other("first"))
        .with_input_error(|_| 1_u64)
        .context("original");
    let address = std::ptr::from_ref(error.inner.as_ref());
    let child = Error::new(DatabaseError(io::Error::other("lost")))
        .with_input_error(|_| 2_u64)
        .with_input_error(|_| "child data");
    let error = error.wrap(child).wrap(fmt::Error);

    assert_eq!(std::ptr::from_ref(error.inner.as_ref()), address);
    assert_eq!(error.errors().len(), 3);
    assert_eq!(error.input_error::<u64>(), Some(&1));
    assert_eq!(error.input_error::<&str>(), Some(&"child data"));
    let child = error.errors()[1].downcast_ref::<Error>().unwrap();
    assert!(child.data().is_none());
    assert_eq!(
        child.downcast_ref::<io::Error>().unwrap().to_string(),
        "lost"
    );
    assert!(error.downcast_ref::<DatabaseError>().is_some());
    assert!(error.downcast_ref::<fmt::Error>().is_some());
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().to_string(),
        "first"
    );
    assert!(
        format!("{error:#}")
            .starts_with("3 errors [original: first; database query failed: lost; ")
    );
}

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

#[test]
fn native_groups_consolidate_metadata_with_first_value_winning() {
    let first = Error::new(io::Error::other("first"))
        .with_input_error(|_| 1_u64)
        .context("database");
    let second = Error::new(io::Error::other("second"))
        .with_input_error(|_| 2_u64)
        .with_input_error(|_| "detail");
    let mut err = Error::join([first, Error::join([second])]);
    assert_eq!(err.errors().len(), 2);
    assert_eq!(err.input_error::<u64>(), Some(&1));
    assert_eq!(err.input_error::<&str>(), Some(&"detail"));
    assert_eq!(err.input_error_mut::<u64>().map(|value| *value), Some(1));
    assert_eq!(
        err.errors()[1]
            .downcast_ref::<Error>()
            .unwrap()
            .data()
            .map(|data| data.len()),
        None
    );
    let address = std::ptr::from_ref(err.inner.as_ref());
    err.set_input_error(3_u64);
    let err = err.context("RPC");
    assert_eq!(std::ptr::from_ref(err.inner.as_ref()), address);
    assert_eq!(err.input_error::<u64>(), Some(&3));
    assert_eq!(err.input_error::<&str>(), Some(&"detail"));
    assert_eq!(
        format!("{err:#}"),
        "RPC: 2 errors [database: first; second]"
    );
    assert!(err.downcast_ref::<io::Error>().is_some());
}

#[test]
fn native_groups_handle_empty_and_existing_sources() {
    let empty = Error::join(Vec::<io::Error>::new());
    assert_eq!(empty.to_string(), "0 errors");
    assert!(empty.errors().is_empty());
    assert!(empty.downcast_ref::<io::Error>().is_none());
    assert!(empty.input_error::<u64>().is_none());
    assert!(empty.source().is_none());
    let empty = empty.context("RPC");
    assert_eq!(format!("{empty:#}"), "RPC: 0 errors");
    assert!(empty.downcast_ref::<io::Error>().is_none());

    let group = Error::join([
        DatabaseError(io::Error::other("lost")),
        DatabaseError(io::Error::other("timeout")),
    ]);
    assert!(group.source().is_none());
    assert_eq!(group.chain().count(), 1);
    assert_eq!(group.errors().len(), 2);
    assert_eq!(
        group.downcast_ref::<io::Error>().unwrap().to_string(),
        "lost"
    );
    assert_eq!(
        format!("{group:#}"),
        "2 errors [database query failed: lost; database query failed: timeout]"
    );
}

#[test]
fn downcast_searches_nested_children_and_their_sources() {
    let err = Error::join([
        Error::new(io::Error::other("first")),
        Error::join([
            Error::new(DatabaseError(io::Error::other("nested"))),
            Error::new(fmt::Error),
        ])
        .context("nested group"),
    ])
    .context("RPC");
    assert_eq!(
        err.downcast_ref::<io::Error>().unwrap().to_string(),
        "first"
    );
    assert!(err.downcast_ref::<DatabaseError>().is_some());
    assert!(err.downcast_ref::<fmt::Error>().is_some());
    assert!(err.downcast_ref::<std::num::ParseIntError>().is_none());

    let source = Error::join([DatabaseError(io::Error::other("source"))]);
    assert_eq!(
        source.downcast_ref::<io::Error>().unwrap().to_string(),
        "source"
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
    assert_eq!(err.source().unwrap().to_string(), "database query failed");
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
    assert_eq!(format!("{err:#}"), "RPC: service: query: database failure");
    let err = Error::from_boxed(Box::new(err));
    assert_eq!(std::ptr::from_ref(err.inner.as_ref()), address);
}

#[test]
fn merged_metadata_is_available_through_data_and_mutable_helpers() {
    struct RequestId(u64);
    runtime_context::tid! { impl<'a> TidAble<'a> for RequestId }

    let mut child =
        Error::new(io::Error::other("child")).with_input_error(|_| String::from("child value"));
    child.data_mut().insert(RequestId(42));
    let mut error = Error::new(io::Error::other("parent")).wrap(child);
    assert_eq!(error.data().unwrap().get::<RequestId>().unwrap().0, 42);
    assert_eq!(error.data().unwrap().len(), 2);
    error
        .get_or_insert_input_error_with::<String, _>(|| panic!("already merged"))
        .push_str(" updated");
    assert_eq!(
        error.input_error::<String>().unwrap(),
        "child value updated"
    );
    let error = error.context("RPC");
    assert_eq!(error.data().unwrap().get::<RequestId>().unwrap().0, 42);
}

#[test]
fn aggregation_keeps_context_lazy_and_moves_it_from_empty_groups() {
    let plain = Error::join([io::Error::other("first"), io::Error::other("second")]);
    assert!(plain.data().is_none());
    let child = Error::join(Vec::<io::Error>::new()).with_input_error(|_| 7_u64);
    let error = plain.wrap(child);
    assert_eq!(error.input_error::<u64>(), Some(&7));
    assert!(
        error.errors()[2]
            .downcast_ref::<Error>()
            .unwrap()
            .data()
            .is_none()
    );
}

#[test]
fn input_insertion_is_lazy_and_returns_mutable_value() {
    let mut error = Error::new(io::Error::other("failure"));
    let mut calls = 0;
    *error.get_or_insert_input_error_with(|| {
        calls += 1;
        7_u64
    }) += 1;
    assert_eq!(calls, 1);
    assert_eq!(
        *error.get_or_insert_input_error_with::<u64, _>(|| panic!("already present")),
        8
    );
    assert_eq!(*error.get_or_insert_input_error(99_u64), 8);
    assert_eq!(*error.get_or_insert_input_error_default::<u64>(), 8);
}

#[test]
fn input_insertion_rejects_immutable_entry_without_running_factory() {
    static INPUT: InputError<u64> = InputError(42);
    let mut error = Error::new(io::Error::other("failure"));
    error.data_mut().insert_ref(&INPUT);
    let mut called = false;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        error.get_or_insert_input_error_with(|| {
            called = true;
            99_u64
        });
    }));
    assert!(result.is_err());
    assert!(!called);
    assert_eq!(error.input_error::<u64>(), Some(&42));
    assert!(error.input_error_mut::<u64>().is_none());
}
